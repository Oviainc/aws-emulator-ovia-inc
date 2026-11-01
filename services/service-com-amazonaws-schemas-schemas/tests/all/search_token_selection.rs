// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the selection a paginated listing binds into its
//! `NextToken`.
//!
//! RULE under test: a listing's `NextToken` names a position in the selection its
//! handler RESOLVED, so two requests whose selecting members resolve to the same
//! selection walk one listing and each honours the other's token, while a request that
//! resolves to another selection is refused. A member the handler normalises before it
//! selects anything — a search's `Keywords`, a registry listing's `Scope` — is therefore
//! bound in its canonical form, not as the bytes the caller typed.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The
//! schemas a search pages over are created through `CreateSchema`, so a handler that
//! answered from its input instead of from the store would have to make the walk agree.
//!
//! The harness (`Emulator`, `Answer`, `walk`, …) is the one `registry_schema_lifecycle`
//! already owns — `tests/all/*.rs` are modules of ONE test crate.

use serde_json::{json, Value};

use super::registry_schema_lifecycle::{
    create_registry, registry_names, schema_names, schema_uri, wait_for_keyword_index, walk,
    Answer, Emulator,
};

/// The registry the search cases below page over.
const REGISTRY: &str = "token-selection";

/// The schemas the search cases page over: the event each one declares, under the name
/// it is addressed by. Three declare the same source and two of those the same detail
/// type, so each selection below has more than one page at a page size of one.
const STORED: &[(&str, &str, &str)] = &[
    ("orders.cancelled", "my.orders", "Order Cancelled v1"),
    ("orders.placed", "my.orders", "Order Placed v1"),
    ("orders.placed.v2", "my.orders", "Order Placed v2"),
    ("orders.shipped", "my.orders", "Order Shipped v1"),
    ("widgets.packed", "my.widgets", "Widget Packed v1"),
];

/// One selection of the search, with the spellings of `Keywords` that all resolve to it.
#[derive(Debug)]
struct Selection {
    case: &'static str,
    /// Values of `Keywords` that select the same schemas: they differ in case, in which
    /// separator divides the keywords, in how many separators there are, in the order
    /// the keywords arrive in, and in whether a keyword is repeated — none of which the
    /// selection looks at.
    spellings: &'static [&'static str],
    /// Every schema the selection reports, in the order a search serves them (the
    /// codepoint order of their names).
    expected: &'static [&'static str],
}

/// The selections, each with its spellings. Rows differ along how many keywords the
/// selection holds — one, two, and none at all.
const SELECTIONS: &[Selection] = &[
    Selection {
        case: "one keyword",
        spellings: &[
            "orders",
            "ORDERS",
            "Orders",
            "orders,",
            ",orders",
            " orders ",
            "orders,orders",
            "ORDERS orders",
        ],
        expected: &[
            "orders.cancelled",
            "orders.placed",
            "orders.placed.v2",
            "orders.shipped",
        ],
    },
    Selection {
        case: "two keywords",
        spellings: &[
            "order placed",
            "placed order",
            "placed,order",
            "ORDER,PLACED",
            " placed  order ",
            "order placed order",
        ],
        expected: &["orders.placed", "orders.placed.v2"],
    },
    Selection {
        case: "no keyword at all",
        spellings: &["", " ", ",", ",,  ,"],
        expected: &[
            "orders.cancelled",
            "orders.placed",
            "orders.placed.v2",
            "orders.shipped",
            "widgets.packed",
        ],
    },
];

/// `GET /v1/registries/name/{registry}/schemas/search?keywords=…`, with the one
/// character these cases use that a query string cannot carry raw.
fn search_uri(registry: &str, keywords: &str) -> String {
    format!(
        "/v1/registries/name/{registry}/schemas/search?keywords={}",
        keywords.replace(' ', "%20")
    )
}

/// Create `REGISTRY` and every row of [`STORED`] in it, and wait until the registry's
/// keyword index has caught up with them: these cases are about the TOKENS a search
/// mints, so they start from a search that has a selection to page.
async fn seed(emulator: &Emulator) {
    create_registry(emulator, REGISTRY).await;
    for (name, source, detail_type) in STORED {
        let content = json!({
            "openapi": "3.0.0",
            "info": {"version": "1.0.0", "title": name},
            "paths": {},
            "components": {"schemas": {"AWSEvent": {
                "type": "object",
                "x-amazon-events-source": source,
                "x-amazon-events-detail-type": detail_type,
                "properties": {},
            }}},
        })
        .to_string();
        let created = emulator
            .call(
                "POST",
                &schema_uri(REGISTRY, name),
                Some(json!({"Content": content, "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );
    }
    wait_for_keyword_index();
}

/// The `NextToken` a first page of one item hands back.
async fn token_of_first_page(emulator: &Emulator, uri: &str, case: &str) -> String {
    let first = emulator.call("GET", &with_query(uri, "limit=1"), None).await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: expected 200 for the first page, got {} {:?}",
        first.status, first.body
    );
    first
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a selection of more than one item pages, got {:?}", first.body)
        })
        .to_string()
}

/// `uri` with one more query member appended, whatever query string it already carries.
fn with_query(uri: &str, member: &str) -> String {
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{member}")
}

/// RULE: every spelling of ONE search selection walks the same listing, so a
/// `NextToken` minted under any of them resumes under all of them — the spelling of
/// `Keywords` is normalised before it selects anything, and what the token is bound to
/// is the selection, not the bytes.
#[tokio::test]
async fn every_spelling_of_one_search_selection_shares_its_tokens() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for selection in SELECTIONS {
        // Each spelling on its own walks the whole selection, in one order.
        for spelling in selection.spellings {
            let case = format!("{} / {spelling:?}", selection.case);
            for limit in [None, Some(1), Some(2)] {
                let walked = walk(&emulator, &search_uri(REGISTRY, spelling), limit, schema_names).await;
                assert_eq!(
                    walked, selection.expected,
                    "case {case} limit={limit:?}: expected {:?}, got {walked:?}",
                    selection.expected
                );
            }
        }

        // And a token minted under the FIRST spelling resumes under every other one, at
        // the same position: the second item of the selection.
        let minted_under = selection.spellings[0];
        let token = token_of_first_page(
            &emulator,
            &search_uri(REGISTRY, minted_under),
            selection.case,
        )
        .await;
        for spelling in selection.spellings {
            let case = format!("{} / {minted_under:?} -> {spelling:?}", selection.case);
            let resumed = emulator
                .call(
                    "GET",
                    &with_query(
                        &search_uri(REGISTRY, spelling),
                        &format!("limit=1&nextToken={token}"),
                    ),
                    None,
                )
                .await;
            assert_eq!(
                resumed.status, 200,
                "case {case}: the same selection must resume, got {} {:?}",
                resumed.status, resumed.body
            );
            let served = schema_names(&resumed);
            assert_eq!(
                served,
                vec![selection.expected[1].to_string()],
                "case {case}: a resumed page serves the item after the one already \
                 served, expected {:?}, got {served:?}",
                selection.expected[1]
            );
        }
    }
}

/// RULE (the other side of it): a token still names a position in ONE selection, so a
/// search that resolves to ANOTHER selection is refused rather than served a page of a
/// listing it is not walking.
#[tokio::test]
async fn a_token_does_not_resume_a_differently_selected_search() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for selection in SELECTIONS {
        let token = token_of_first_page(
            &emulator,
            &search_uri(REGISTRY, selection.spellings[0]),
            selection.case,
        )
        .await;
        // Rows: every OTHER selection of this table, under each of its spellings — a
        // selection that is not this one however it is written.
        for other in SELECTIONS {
            if other.case == selection.case {
                continue;
            }
            for spelling in other.spellings {
                let case = format!("{} -> {} / {spelling:?}", selection.case, other.case);
                let answer = emulator
                    .call(
                        "GET",
                        &with_query(
                            &search_uri(REGISTRY, spelling),
                            &format!("limit=1&nextToken={token}"),
                        ),
                        None,
                    )
                    .await;
                assert_eq!(
                    answer.status, 400,
                    "case {case}: a token of another selection must be refused, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "BadRequestException",
                    "case {case}: expected BadRequestException, got {:?}",
                    answer.body
                );
            }
        }
    }

    // And the registry a search names selects it too: the same keywords in another
    // registry are another listing.
    create_registry(&emulator, "token-selection-other").await;
    let token = token_of_first_page(&emulator, &search_uri(REGISTRY, "orders"), "other registry").await;
    let answer = emulator
        .call(
            "GET",
            &with_query(
                &search_uri("token-selection-other", "orders"),
                &format!("limit=1&nextToken={token}"),
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 400,
        "case other registry: a token of another registry's search must be refused, got {} {:?}",
        answer.status, answer.body
    );
}

/// RULE, on the sibling listing that resolves a selecting member the same way: the
/// spellings of `Scope` a registry listing accepts all resolve to one scope, so they
/// share one listing and its tokens, while the other scope is another listing.
#[tokio::test]
async fn every_spelling_of_one_registry_scope_shares_its_tokens() {
    let emulator = Emulator::new();
    for name in ["scoped-one", "scoped-two", "scoped-three"] {
        create_registry(&emulator, name).await;
    }

    // The spellings the handler accepts for the caller's own registries, and the ones it
    // accepts for the service-owned ones.
    for (case, spellings) in [
        ("local", ["local", "Local", "LOCAL"]),
        ("aws", ["aws", "AWS", "Aws"]),
    ] {
        let listing = |scope: &str| format!("/v1/registries?scope={scope}");
        let expected = walk(&emulator, &listing(spellings[0]), None, registry_names).await;
        assert!(
            !expected.is_empty(),
            "case {case:?}: the scope must select a registry to walk, got {expected:?}"
        );
        // A position to resume after exists only where the selection holds more than one
        // registry: this service publishes ONE AWS-owned registry, so only the caller's
        // own scope pages here.
        let token = if expected.len() > 1 {
            Some(token_of_first_page(&emulator, &listing(spellings[0]), case).await)
        } else {
            None
        };
        for spelling in spellings {
            let case = format!("{case} / {spelling:?}");
            let walked = walk(&emulator, &listing(spelling), Some(1), registry_names).await;
            assert_eq!(
                walked, expected,
                "case {case}: every spelling walks one listing, expected {expected:?}, \
                 got {walked:?}"
            );
            let Some(token) = &token else { continue };
            let resumed = emulator
                .call(
                    "GET",
                    &with_query(&listing(spelling), &format!("limit=1&nextToken={token}")),
                    None,
                )
                .await;
            assert_eq!(
                resumed.status, 200,
                "case {case}: the same selection must resume, got {} {:?}",
                resumed.status, resumed.body
            );
            let served = registry_names(&resumed);
            assert_eq!(
                served,
                vec![expected[1].clone()],
                "case {case}: a resumed page serves the item after the one already served, \
                 expected {:?}, got {served:?}",
                expected[1]
            );
        }
    }

    // The two scopes are two listings, so neither resumes the other's token.
    let local_token = token_of_first_page(&emulator, "/v1/registries?scope=local", "local").await;
    for other in ["/v1/registries?scope=AWS", "/v1/registries"] {
        let answer = emulator
            .call("GET", &with_query(other, &format!("limit=1&nextToken={local_token}")), None)
            .await;
        assert_eq!(
            answer.status, 400,
            "case {other:?}: a token of another scope's listing must be refused, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {other:?}: expected BadRequestException, got {:?}",
            answer.body
        );
    }
}

/// A resumed page is still a page of the selection, so following the token from any
/// spelling to exhaustion visits each item exactly once.
#[tokio::test]
async fn resuming_under_another_spelling_walks_the_selection_once() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for selection in SELECTIONS {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // Each page is requested under the NEXT spelling in the row, so the walk crosses
        // every spelling of the selection and still sees it exactly once.
        for (index, spelling) in selection
            .spellings
            .iter()
            .cycle()
            .take(selection.expected.len())
            .enumerate()
        {
            let case = format!("{} / page {index} as {spelling:?}", selection.case);
            let mut uri = with_query(&search_uri(REGISTRY, spelling), "limit=1");
            if let Some(token) = &token {
                uri = with_query(&uri, &format!("nextToken={token}"));
            }
            let answer: Answer = emulator.call("GET", &uri, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            seen.extend(schema_names(&answer));
            token = answer
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        assert_eq!(
            seen, selection.expected,
            "case {}: a walk crossing the spellings sees the selection once, expected {:?}, \
             got {seen:?}",
            selection.case, selection.expected
        );
    }
}

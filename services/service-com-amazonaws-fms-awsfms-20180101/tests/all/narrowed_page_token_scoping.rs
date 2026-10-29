// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the token rule the paginated listings whose page set is
//! SELECTED by a request member share.
//!
//! The rule: such a listing's `NextToken` names a page boundary only under the
//! selection the request that minted it named, so a token minted under one
//! selection is refused under another rather than resumed from as an arbitrary
//! key — and a BARE key, the unstamped form the listings of the same page class
//! mint for their own un-narrowed pages, is no boundary of a narrowed page
//! either. A refused call reports nothing and leaves the page as it was, and the
//! token a selection does mint still walks that selection's whole page.
//!
//! Asserted over a TABLE whose rows are the narrowed listings of this service:
//! `ListComplianceStatus` (the page is the accounts ONE policy applies to),
//! `ListResourceSetResources` (the resources ONE resource set holds),
//! `ListAppsLists` and `ListProtocolsLists` (the lists of ONE of the two
//! namespaces `DefaultLists` selects). The rows therefore differ in the page's
//! key class — an `AWSAccountId`, a resource ARN, a minted `ListId` — in the
//! type of the selecting member (a string identifier, a boolean) and in whether
//! the operation requires `MaxResults`.
//!
//! Each row carries at least two selections, which differ in how many items they
//! hold (one, two, three) and INTERLEAVE in the page's own key order, so a
//! boundary of one selection's page sorts inside, before or after another's
//! items — resuming from a cross-selection token would therefore truncate one
//! target's page and empty another's. The refusals differ in the token's origin
//! (another selection of the same listing, the bare key of the target's own
//! item, a token minted by a sibling listing of the same page class) and the
//! accepted walks in page size.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`] — so the assertions
//! are about the WIRE response.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{
    associate_admin, seed_apps_list, seed_protocols_list, with_store,
};
use crate::compliance_status::{associate, put_policy, ADMIN};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};
use crate::resource_set_resources::{create_set_with_members, instance, seeded_emulator};

/// The accounts the three policies of the `ListComplianceStatus` row apply to,
/// named by each policy's `IncludeMap`. They differ in how many accounts a
/// policy applies to and in `AWSAccountId` length (the model constrains that
/// shape only to `^[0-9]+$` with length 1..1024), and they interleave in
/// ascending account-id order, which is the order the page is served in.
const SCOPES: [&[&str]; 3] = [
    &["111111111111", "999988887777"],
    &["222222222222"],
    &["333333333333", "444444444444", "7"],
];

/// The EC2 instances the three resource sets of the `ListResourceSetResources`
/// row hold, as indexes into [`instance`]'s ARN form. The ARNs differ only in
/// that index, which is zero-padded hex, so index order IS the ascending URI
/// order the page is served in — and these sets interleave in it.
const MEMBER_INDEXES: [&[usize]; 3] = [&[1, 9], &[5], &[2, 4, 6]];

/// The list ids the two namespaces of the `ListAppsLists` / `ListProtocolsLists`
/// rows hold. Both namespaces share ONE id space, and these ids interleave in
/// it: the managed list sorts BETWEEN the two customer ones, so a customer
/// token resumed from in the managed namespace would serve nothing.
///
/// Each is a `com.amazonaws.fms#ListId` as the model declares it (36 characters
/// matching `^[a-z0-9A-Z-]{36}$`), written here rather than minted so the order
/// the page walks is known.
const CUSTOMER_LIST_IDS: [&str; 2] = [
    "11111111-1111-4a26-9d0e-6b4f8d2a7c31",
    "33333333-3333-4a26-9d0e-6b4f8d2a7c31",
];
const MANAGED_LIST_IDS: [&str; 1] = ["22222222-2222-4a26-9d0e-6b4f8d2a7c31"];

/// One narrowed listing: the operation, the request member that SELECTS its
/// page, the output member carrying the items, the member of an item that is the
/// page's own key, the error code the operation refuses a token naming no
/// boundary of the selected page with, and whether its `MaxResults` is required.
struct NarrowedPage {
    operation: &'static str,
    selector: &'static str,
    items: &'static str,
    key_member: &'static str,
    refusal: &'static str,
    max_results_required: bool,
}

/// One selection of a narrowed listing: the value the request names it by, and
/// the keys its page holds, in the page's own order.
struct Selection {
    value: Value,
    keys: Vec<String>,
}

impl Selection {
    /// How the selection reads in an assertion message.
    fn label(&self) -> String {
        self.value.to_string()
    }
}

/// The narrowed listings the rule is asserted over.
fn pages() -> Vec<NarrowedPage> {
    vec![
        NarrowedPage {
            operation: "ListComplianceStatus",
            selector: "PolicyId",
            items: "PolicyComplianceStatusList",
            key_member: "MemberAccount",
            // The model declares no client-input error for this operation, so a
            // request it refuses is a `ValidationException`.
            refusal: "ValidationException",
            max_results_required: false,
        },
        NarrowedPage {
            operation: "ListResourceSetResources",
            selector: "Identifier",
            items: "Items",
            key_member: "URI",
            refusal: "InvalidInputException",
            max_results_required: false,
        },
        NarrowedPage {
            operation: "ListAppsLists",
            selector: "DefaultLists",
            items: "AppsLists",
            key_member: "ListId",
            refusal: "ValidationException",
            max_results_required: true,
        },
        NarrowedPage {
            operation: "ListProtocolsLists",
            selector: "DefaultLists",
            items: "ProtocolsLists",
            key_member: "ListId",
            refusal: "ValidationException",
            max_results_required: true,
        },
    ]
}

/// A fresh emulator holding one row's selections, with the keys each one's page
/// holds, plus tokens minted ELSEWHERE that name no boundary of any of them (as
/// `(origin, token)`).
async fn prepare(
    page: &NarrowedPage,
) -> (MultiServiceRouter, Vec<Selection>, Vec<(String, String)>) {
    match page.operation {
        "ListComplianceStatus" => compliance_fixture().await,
        "ListResourceSetResources" => resource_set_fixture().await,
        "ListAppsLists" | "ListProtocolsLists" => list_namespace_fixture(page.operation).await,
        other => panic!("case {other}: this suite has no setup for that narrowed page"),
    }
}

/// Three stored policies, each applying to the accounts of one row of
/// [`SCOPES`] — and the token the organisation roster mints, which is keyed by
/// account id just as these pages are.
async fn compliance_fixture() -> (MultiServiceRouter, Vec<Selection>, Vec<(String, String)>) {
    let (router, _engine) = emulator_with_engine();
    associate(&router, ADMIN).await;

    let mut selections = Vec::new();
    for (index, scope) in SCOPES.iter().enumerate() {
        let policy_id = put_policy(
            &router,
            &format!("narrowed-page-{index}"),
            Some(json!({ "ACCOUNT": scope })),
            None,
        )
        .await;
        let mut keys: Vec<String> = scope.iter().map(|account| (*account).to_string()).collect();
        keys.sort();
        keys.dedup();
        selections.push(Selection {
            value: json!(policy_id),
            keys,
        });
    }

    // A token minted by a sibling listing of the SAME page class: the
    // organisation roster `ListMemberAccounts` serves is keyed by account id
    // too, and its token names a boundary of THAT roster — of no policy's page.
    let (status, body) = call(&router, "ListMemberAccounts", &json!({ "MaxResults": 1 })).await;
    assert_eq!(
        status, 200,
        "ListMemberAccounts must answer 200 so this suite has its token, got {status} with {body}"
    );
    let roster_token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("this suite needs a roster of more than one account to mint a token, got {body}")
        })
        .to_string();

    let foreign = vec![(
        "a token minted by ListMemberAccounts".to_string(),
        roster_token,
    )];
    (router, selections, foreign)
}

/// Three resource sets, each holding the instances of one row of
/// [`MEMBER_INDEXES`].
async fn resource_set_fixture() -> (MultiServiceRouter, Vec<Selection>, Vec<(String, String)>) {
    let (router, _engine) = seeded_emulator().await;

    let mut selections = Vec::new();
    for (index, members) in MEMBER_INDEXES.iter().enumerate() {
        let uris: Vec<String> = members
            .iter()
            .map(|member| instance(ACCOUNT, *member))
            .collect();
        let (id, _arn) = create_set_with_members(
            &router,
            &format!("narrowed-page-{index}"),
            &[state::RESOURCE_TYPE_EC2_INSTANCE],
            &uris,
        )
        .await;
        let mut keys = uris.clone();
        keys.sort();
        selections.push(Selection {
            value: json!(id),
            keys,
        });
    }

    (router, selections, Vec::new())
}

/// Both namespaces of one kind of managed list, each holding the ids of one row
/// of [`CUSTOMER_LIST_IDS`] / [`MANAGED_LIST_IDS`].
///
/// Seeded through the store seam the sibling suites seed these records with: a
/// Firewall Manager MANAGED list is created by no API, and a customer list with
/// a CHOSEN id by none either — and the ids are what this suite needs to
/// interleave.
async fn list_namespace_fixture(
    operation: &str,
) -> (MultiServiceRouter, Vec<Selection>, Vec<(String, String)>) {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    with_store(&engine, |store| {
        for (namespace, ids) in [
            (false, CUSTOMER_LIST_IDS.as_slice()),
            (true, MANAGED_LIST_IDS.as_slice()),
        ] {
            for id in ids {
                match operation {
                    "ListAppsLists" => {
                        seed_apps_list(store, REGION, id, &format!("apps-{id}"), &[], namespace);
                    }
                    "ListProtocolsLists" => {
                        seed_protocols_list(store, id, &format!("protocols-{id}"), &["tcp"], namespace);
                    }
                    other => panic!("case {other}: this fixture seeds no list of that kind"),
                }
            }
        }
    });

    let selections = vec![
        Selection {
            value: json!(false),
            keys: CUSTOMER_LIST_IDS.iter().map(|id| id.to_string()).collect(),
        },
        Selection {
            value: json!(true),
            keys: MANAGED_LIST_IDS.iter().map(|id| id.to_string()).collect(),
        },
    ];
    (router, selections, Vec::new())
}

/// One request of a narrowed listing: the selection, and optionally a page size
/// and a token. An operation whose `MaxResults` is REQUIRED is asked for the
/// largest page the service serves when the caller names no size, so one builder
/// serves both kinds of row.
fn request(
    page: &NarrowedPage,
    selection: &Value,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Value {
    let mut request = serde_json::Map::new();
    request.insert(page.selector.to_string(), selection.clone());
    let max_results = max_results.or_else(|| {
        page.max_results_required
            .then_some(state::MAX_PAGE_SIZE)
    });
    if let Some(max_results) = max_results {
        request.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(token) = token {
        request.insert("NextToken".to_string(), json!(token));
    }
    Value::Object(request)
}

/// The page keys a response carries, in the order it served them. Empty when the
/// response carries no items at all, which is what a refusal looks like.
fn served(page: &NarrowedPage, case: &str, body: &Value) -> Vec<String> {
    body.get(page.items)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get(page.key_member)
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            panic!(
                                "case {case}: every item must carry its {} key, got {item}",
                                page.key_member
                            )
                        })
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The whole page of one selection, asked for with no page size of its own:
/// served, holding that selection's keys and carrying no token.
async fn whole_page(
    router: &MultiServiceRouter,
    page: &NarrowedPage,
    selection: &Selection,
) -> Vec<String> {
    let operation = page.operation;
    let case = format!("{operation} / selection {}", selection.label());
    let (status, body) = call(router, operation, &request(page, &selection.value, None, None)).await;
    assert_eq!(
        status, 200,
        "case {case}: the whole page must be served, got {status} with {body}"
    );
    assert!(
        body.get("NextToken").is_none_or(Value::is_null),
        "case {case}: a page holding the whole selection carries no NextToken, got {body}"
    );
    served(page, &case, &body)
}

/// The token the first page of size one mints for a selection, asserting that
/// page is that selection's first key — `None` for a selection whose page fits
/// in one item, which mints no token at all.
async fn first_token(
    router: &MultiServiceRouter,
    page: &NarrowedPage,
    selection: &Selection,
) -> Option<String> {
    let operation = page.operation;
    let case = format!(
        "{operation} / selection {} / first page",
        selection.label()
    );
    let (status, body) = call(
        router,
        operation,
        &request(page, &selection.value, Some(1), None),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: the first page must be served, got {status} with {body}"
    );
    assert_eq!(
        served(page, &case, &body),
        selection.keys[..1].to_vec(),
        "case {case}: expected the selection's first key, got {body}"
    );
    let token = body
        .get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string);
    assert_eq!(
        token.is_some(),
        selection.keys.len() > 1,
        "case {case}: a token must be minted exactly while keys remain after the page, got \
         {token:?} for {:?}",
        selection.keys
    );
    token
}

/// A token that names no boundary of `target`'s page is refused with the
/// operation's own error code, reports nothing, and leaves that page as it was.
async fn refuse(
    router: &MultiServiceRouter,
    page: &NarrowedPage,
    target: &Selection,
    token: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        page.operation,
        &request(page, &target.value, None, Some(token)),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, page.refusal),
        "case {case}: a token naming no boundary of the selected page must be refused, got \
         {status} with {body}"
    );
    assert!(
        served(page, case, &body).is_empty() && body.get("NextToken").is_none_or(Value::is_null),
        "case {case}: a refused call must report nothing, got {body}"
    );
    let after = whole_page(router, page, target).await;
    assert_eq!(
        after, target.keys,
        "case {case}: a refused token must leave the selected page as it was, got {after:?}"
    );
}

/// A `NextToken` of a narrowed listing names a boundary only under the selection
/// that minted it: replayed under another selection — or sent as the bare key a
/// listing of the same page class mints for its own un-narrowed page — it is
/// refused, while under its own selection it resumes the page.
#[tokio::test]
async fn a_narrowed_pages_token_is_a_boundary_only_under_the_selection_that_minted_it() {
    for page in pages() {
        let operation = page.operation;
        let (router, selections, foreign) = prepare(&page).await;
        assert!(
            selections.len() >= 2,
            "case {operation}: this suite needs at least two selections, got {}",
            selections.len()
        );

        // The precondition every row below reads against: each selection's whole
        // page is its own keys, in the page's order.
        for selection in &selections {
            let served = whole_page(&router, &page, selection).await;
            assert_eq!(
                served,
                selection.keys,
                "case {operation} / selection {}: expected that selection's own keys, got \
                 {served:?}",
                selection.label()
            );
        }

        for minter in &selections {
            let Some(token) = first_token(&router, &page, minter).await else {
                continue;
            };

            // Under ANOTHER selection the same token names no boundary.
            for target in &selections {
                if target.value == minter.value {
                    continue;
                }
                let case = format!(
                    "{operation} / token minted under {} replayed under {}",
                    minter.label(),
                    target.label()
                );
                refuse(&router, &page, target, &token, &case).await;
            }

            // Under the selection that minted it, it resumes exactly after the
            // page it was minted for — so the refusals above are not refusing
            // every token.
            let case = format!(
                "{operation} / token minted under {} replayed under itself",
                minter.label()
            );
            let (status, body) = call(
                &router,
                operation,
                &request(&page, &minter.value, None, Some(&token)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the minting selection's own token must resume its page, got {status} \
                 with {body}"
            );
            assert_eq!(
                served(&page, &case, &body),
                minter.keys[1..].to_vec(),
                "case {case}: expected the keys after the first, got {body}"
            );
        }

        // The BARE key — no selection stamped on it, which is what a listing of
        // the same page class mints for its own un-narrowed page — names no
        // boundary of a narrowed page, not even when it is a key the target
        // selection itself holds.
        for target in &selections {
            for key in &target.keys {
                let case = format!(
                    "{operation} / bare key {key:?} sent under {}",
                    target.label()
                );
                refuse(&router, &page, target, &hex::encode(key), &case).await;
            }
        }

        // And neither does a token another operation of the same page class
        // really minted.
        for (origin, token) in &foreign {
            for target in &selections {
                let case = format!("{operation} / {origin} sent under {}", target.label());
                refuse(&router, &page, target, token, &case).await;
            }
        }
    }
}

/// The token a selection mints decodes through the same stamp, so walking the
/// chain serves every key of THAT selection exactly once, in the page's own
/// order, at every page size.
#[tokio::test]
async fn the_token_a_selection_mints_walks_that_selections_whole_page() {
    for page in pages() {
        let operation = page.operation;
        let (router, selections, _foreign) = prepare(&page).await;

        for selection in &selections {
            for max_results in [1usize, 2] {
                let case = format!(
                    "{operation} / selection {} / MaxResults {max_results}",
                    selection.label()
                );
                let mut walked: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                let mut ended = false;
                // One round more than the chain can need, so a token that never
                // clears fails here instead of looping forever.
                for round in 0..selection.keys.len() + 3 {
                    let (status, body) = call(
                        &router,
                        operation,
                        &request(&page, &selection.value, Some(max_results), token.as_deref()),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case} / page {round}: expected 200, got {status} with {body}"
                    );
                    let keys = served(&page, &case, &body);
                    assert!(
                        keys.len() <= max_results,
                        "case {case} / page {round}: a page must not exceed the requested size, \
                         got {keys:?}"
                    );
                    walked.extend(keys);
                    match body
                        .get("NextToken")
                        .filter(|token| !token.is_null())
                        .and_then(Value::as_str)
                    {
                        Some(next) => token = Some(next.to_string()),
                        None => {
                            ended = true;
                            break;
                        }
                    }
                }
                assert!(
                    ended,
                    "case {case}: the NextToken chain did not end, walked {walked:?}"
                );
                assert_eq!(
                    walked, selection.keys,
                    "case {case}: walking the minted tokens must serve every key of that \
                     selection exactly once, in the page's order, got {walked:?}"
                );
            }
        }
    }
}

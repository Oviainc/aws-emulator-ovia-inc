// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule every paginated read of this service follows
//! about the `NextToken` it accepts.
//!
//! The rule: a paginated read resumes from a `NextToken` only when the boundary
//! the token decodes to is a key of the collection THAT read pages — which takes
//! both the collection the boundary belongs to (the stamp the token carries, or
//! the prefix the collection's composite keys carry) and that collection's own
//! key form. A boundary of another collection, or a bare key carrying no
//! collection at all, names no page here however well its shape fits, so the
//! request is refused as a bad request rather than resumed from: resuming would
//! silently drop every item sorting below the foreign boundary and report the
//! remainder — or nothing at all — as a complete page.
//!
//! Asserted over the MATRIX of this service's paginated reads: each read's own
//! page, and the token every OTHER read mints, so no pair of them can come to
//! agree about a boundary. The rows differ in the collection's key class (a
//! minted `PolicyId`, a minted `ListId`, an `AWSAccountId`, a composite attack
//! key), in whether a request member selects the page, in the type of that
//! member (a string identifier, a boolean), in whether `MaxResults` is required
//! and in the modelled error each read refuses with. Two of the rows —
//! `ListComplianceStatus` and `GetProtectionStatus` — page DIFFERENT collections
//! narrowed by the SAME policy, so the policy alone cannot tell their boundaries
//! apart.
//!
//! Beside the minted tokens, each read is offered the BARE key of every row's
//! page — the unstamped form, which names no collection — including keys of its
//! own page and keys that sort below and above everything it holds, so a bare
//! boundary that would truncate a page in either direction is refused rather
//! than resumed from.
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
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, REGION};
use crate::protection_status::{put_scoped_policy, seed_attack, Attack, SCOPED_ACCOUNTS};

/// The policies stored for the `ListPolicies` row. Three, so that page has
/// boundaries of its own, and Shield Advanced ones because the
/// `GetProtectionStatus` row reports only that security service's policies.
const POLICY_NAMES: &[&str] = &["scoped-alpha", "scoped-beta", "scoped-gamma"];

/// The ids of the applications lists seeded for the `ListAppsLists` row, and of
/// the protocols lists seeded for the `ListProtocolsLists` row.
///
/// Each is a `com.amazonaws.fms#ListId` as the model declares it (36 characters
/// matching `^[a-z0-9A-Z-]{36}$`), written here rather than minted so where each
/// one sorts is KNOWN: a minted identifier of this service is 36 hex characters,
/// so the first id of each row sorts below every stored `PolicyId`, the last
/// sorts above every one of them, and the middle id falls inside the span they
/// occupy. A boundary of these collections resumed from in the policy page would
/// therefore truncate it from either end.
const APPS_LIST_IDS: &[&str] = &[
    "00000000-0000-4a26-9d0e-6b4f8d2a7c31",
    "6a000000-0000-4a26-9d0e-6b4f8d2a7c31",
    "zzzzzzzz-zzzz-4a26-9d0e-6b4f8d2a7c31",
];
const PROTOCOLS_LIST_IDS: &[&str] = &[
    "11111111-1111-4a26-9d0e-6b4f8d2a7c31",
    "aaaaaaaa-aaaa-4a26-9d0e-6b4f8d2a7c31",
    "yyyyyyyy-yyyy-4a26-9d0e-6b4f8d2a7c31",
];

/// The attacks seeded against the Shield Advanced policy the
/// `GetProtectionStatus` row pages, so that page has boundaries of its own. They
/// differ in account (each one in the policy's scope), in start instant and in
/// whether they have ended.
fn seeded_attacks() -> Vec<Attack> {
    vec![
        Attack {
            attack_id: "attack-earliest",
            account: SCOPED_ACCOUNTS[0],
            start_secs: 0,
            start_millis: 0,
            duration_secs: Some(60),
            vectors: &["UDP_TRAFFIC"],
        },
        Attack {
            attack_id: "attack-middle",
            account: SCOPED_ACCOUNTS[1],
            start_secs: 1_800,
            start_millis: 500,
            duration_secs: Some(120),
            vectors: &["REQUEST_FLOOD"],
        },
        Attack {
            attack_id: "attack-ongoing",
            account: SCOPED_ACCOUNTS[2],
            start_secs: 3_600,
            start_millis: 0,
            duration_secs: None,
            vectors: &["HTTP_REFLECTION", "UDP_TRAFFIC"],
        },
    ]
}

/// Where a read's response carries the keys of the page it served.
#[derive(Clone, Copy)]
enum Items {
    /// A modelled list member: `list` is the output member, and each item
    /// carries the page's key in `key` — or IS that key when `key` is `None`.
    Member {
        list: &'static str,
        key: Option<&'static str>,
    },
    /// The `Data` document `GetProtectionStatus` reports, whose entries carry
    /// the attack id the page is keyed by.
    ProtectionData,
}

/// One paginated read of this service: the operation, the request members that
/// SELECT its page (none for a read of a whole collection), where its items are,
/// the modelled error code it refuses a token naming no boundary of its page
/// with, and whether its `MaxResults` is required.
struct Paged {
    operation: &'static str,
    selection: Value,
    items: Items,
    refusal: &'static str,
    max_results_required: bool,
}

/// A fresh emulator holding every row's collection, and the rows themselves.
async fn fixture() -> (MultiServiceRouter, Vec<Paged>) {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let mut policy_ids = Vec::new();
    for name in POLICY_NAMES {
        policy_ids.push(
            put_scoped_policy(
                &router,
                name,
                "SHIELD_ADVANCED",
                Some(json!({ "ACCOUNT": SCOPED_ACCOUNTS })),
                None,
            )
            .await,
        );
    }
    // The policy the two narrowed rows are read under — the SAME one for both,
    // so neither can tell its page from the other's by the policy alone.
    let narrowing_policy = policy_ids
        .first()
        .expect("this suite stores at least one policy")
        .clone();

    // The managed-list records and the attack inventory are seeded through the
    // store the way the sibling suites seed them: a Firewall Manager MANAGED
    // list is created by no API, a customer list with a CHOSEN id by none
    // either, and Shield Advanced detects the attacks this emulator stands in
    // for no detector of.
    with_store(&engine, |store| {
        for (index, id) in APPS_LIST_IDS.iter().enumerate() {
            seed_apps_list(store, REGION, id, &format!("apps-{index}"), &[], false);
        }
        for (index, id) in PROTOCOLS_LIST_IDS.iter().enumerate() {
            seed_protocols_list(store, id, &format!("protocols-{index}"), &["tcp"], false);
        }
        for attack in &seeded_attacks() {
            seed_attack(store, &narrowing_policy, attack);
        }
    });

    let rows = vec![
        Paged {
            operation: "ListPolicies",
            selection: json!({}),
            items: Items::Member {
                list: "PolicyList",
                key: Some("PolicyId"),
            },
            // The model declares no client-input error for this operation, so a
            // request it refuses is a `ValidationException`.
            refusal: "ValidationException",
            max_results_required: false,
        },
        Paged {
            operation: "ListAppsLists",
            selection: json!({ "DefaultLists": false }),
            items: Items::Member {
                list: "AppsLists",
                key: Some("ListId"),
            },
            refusal: "ValidationException",
            max_results_required: true,
        },
        Paged {
            operation: "ListProtocolsLists",
            selection: json!({ "DefaultLists": false }),
            items: Items::Member {
                list: "ProtocolsLists",
                key: Some("ListId"),
            },
            refusal: "ValidationException",
            max_results_required: true,
        },
        Paged {
            operation: "ListMemberAccounts",
            selection: json!({}),
            items: Items::Member {
                list: "MemberAccounts",
                key: None,
            },
            refusal: "ValidationException",
            max_results_required: false,
        },
        Paged {
            operation: "ListComplianceStatus",
            selection: json!({ "PolicyId": narrowing_policy }),
            items: Items::Member {
                list: "PolicyComplianceStatusList",
                key: Some("MemberAccount"),
            },
            refusal: "ValidationException",
            max_results_required: false,
        },
        Paged {
            operation: "GetProtectionStatus",
            selection: json!({ "PolicyId": narrowing_policy }),
            items: Items::ProtectionData,
            refusal: "InvalidInputException",
            max_results_required: false,
        },
    ];
    (router, rows)
}

/// One request of a read: its selection, optionally a page size and a token. A
/// read whose `MaxResults` is REQUIRED is asked for the largest page the service
/// serves when no size is named, so one builder serves every row.
fn request(row: &Paged, max_results: Option<usize>, token: Option<&str>) -> Value {
    let mut request = match &row.selection {
        Value::Object(members) => members.clone(),
        other => panic!(
            "case {}: a selection must be an object, got {other}",
            row.operation
        ),
    };
    let max_results = max_results.or_else(|| {
        row.max_results_required
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

/// The page keys a response carries, in the order it served them — empty when it
/// carries no items at all, which is what a refusal looks like.
fn served(row: &Paged, case: &str, body: &Value) -> Vec<String> {
    let items: Vec<Value> = match row.items {
        Items::Member { list, .. } => body
            .get(list)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        Items::ProtectionData => match body.get("Data").and_then(Value::as_str) {
            Some(data) => match serde_json::from_str::<Value>(data) {
                Ok(Value::Array(entries)) => entries,
                other => panic!("case {case}: Data must be a JSON array, got {data:?} ({other:?})"),
            },
            None => Vec::new(),
        },
    };
    let key_member = match row.items {
        Items::Member { key, .. } => key,
        Items::ProtectionData => Some("AttackId"),
    };
    items
        .iter()
        .map(|item| {
            let key = match key_member {
                Some(member) => item.get(member).unwrap_or(&Value::Null),
                None => item,
            };
            key.as_str()
                .unwrap_or_else(|| panic!("case {case}: a page item must carry its key, got {item}"))
                .to_string()
        })
        .collect()
}

/// The `NextToken` a response carries, if any.
fn token_of(body: &Value) -> Option<String> {
    body.get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The whole page of one read, asked for with no token: served, and holding at
/// least two keys with no token of its own — the precondition every row below
/// reads against.
async fn whole_page(router: &MultiServiceRouter, row: &Paged) -> Vec<String> {
    let operation = row.operation;
    let (status, body) = call(router, operation, &request(row, None, None)).await;
    assert_eq!(
        status, 200,
        "case {operation}: the whole page must be served, got {status} with {body}"
    );
    let keys = served(row, operation, &body);
    assert!(
        keys.len() >= 2 && token_of(&body).is_none(),
        "case {operation}: this suite needs one whole page of at least two keys, got {keys:?} \
         with NextToken {:?}",
        token_of(&body)
    );
    keys
}

/// The token a read mints for a page of ONE item, asserting that page is the
/// first key of its collection.
async fn first_token(router: &MultiServiceRouter, row: &Paged, keys: &[String]) -> String {
    let operation = row.operation;
    let (status, body) = call(router, operation, &request(row, Some(1), None)).await;
    assert_eq!(
        status, 200,
        "case {operation}: the first page must be served, got {status} with {body}"
    );
    assert_eq!(
        served(row, operation, &body),
        keys[..1].to_vec(),
        "case {operation}: expected the collection's first key, got {body}"
    );
    token_of(&body).unwrap_or_else(|| {
        panic!("case {operation}: a page leaving keys behind must mint a token, got {body}")
    })
}

/// A token naming no boundary of `row`'s page is refused with that read's own
/// modelled error, reports nothing, and leaves the page as it was.
async fn refuse(
    router: &MultiServiceRouter,
    row: &Paged,
    keys: &[String],
    token: &str,
    case: &str,
) {
    let (status, body) = call(router, row.operation, &request(row, None, Some(token))).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, row.refusal),
        "case {case}: a token naming no boundary of this page must be refused, got {status} with \
         {body}"
    );
    assert!(
        served(row, case, &body).is_empty() && token_of(&body).is_none(),
        "case {case}: a refused call must report nothing, got {body}"
    );
    let after = whole_page(router, row).await;
    assert_eq!(
        after,
        keys.to_vec(),
        "case {case}: a refused token must leave the page as it was, got {after:?}"
    );
}

/// A `NextToken` names a boundary only in the collection that minted it: every
/// read refuses the token each other read minted, and refuses a BARE key of any
/// collection — its own included — while resuming its own page from its own
/// token.
#[tokio::test]
async fn a_page_token_names_a_boundary_only_in_the_collection_that_minted_it() {
    let (router, rows) = fixture().await;

    // Every row's own page, and the token it mints for a boundary inside it.
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut minted: Vec<String> = Vec::new();
    for row in &rows {
        let keys = whole_page(&router, row).await;
        minted.push(first_token(&router, row, &keys).await);
        pages.push(keys);
    }

    // The bare, unstamped form of a key of each collection: the first and the
    // last key of every row's page, so each read is offered a bare boundary
    // sorting below and above its own items as well as ones it holds itself.
    let mut bare: Vec<(String, String)> = Vec::new();
    for (row, keys) in rows.iter().zip(&pages) {
        for (where_in_page, key) in [("first", keys.first()), ("last", keys.last())] {
            let key = key.expect("a page of at least two keys");
            bare.push((
                format!(
                    "the bare {where_in_page} key of {}'s page ({key:?})",
                    row.operation
                ),
                hex::encode(key),
            ));
        }
    }

    for (index, row) in rows.iter().enumerate() {
        let operation = row.operation;
        let keys = &pages[index];

        // A token another read really minted names no boundary here.
        for (other, token) in rows.iter().zip(&minted) {
            if other.operation == operation {
                continue;
            }
            let case = format!("{operation} / a token minted by {}", other.operation);
            refuse(&router, row, keys, token, &case).await;
        }

        // Neither does a bare key, which carries no collection at all.
        for (origin, token) in &bare {
            let case = format!("{operation} / {origin}");
            refuse(&router, row, keys, token, &case).await;
        }

        // Its OWN token resumes its page exactly after the item it was minted
        // for — so the refusals above are not refusing every token.
        let case = format!("{operation} / its own token");
        let (status, body) = call(
            &router,
            operation,
            &request(row, None, Some(&minted[index])),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a read's own token must resume its page, got {status} with {body}"
        );
        assert_eq!(
            served(row, &case, &body),
            keys[1..].to_vec(),
            "case {case}: expected the keys after the first, got {body}"
        );
    }
}

/// The token a read mints decodes through its own collection's stamp, so walking
/// the chain reports every key of THAT collection exactly once, in the page's
/// own order, at every page size.
#[tokio::test]
async fn the_token_a_collection_mints_walks_its_whole_page() {
    let (router, rows) = fixture().await;

    for row in &rows {
        let operation = row.operation;
        let keys = whole_page(&router, row).await;

        for max_results in [1usize, 2] {
            let case = format!("{operation} / MaxResults {max_results}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut ended = false;
            // One round more than the chain can need, so a token that never
            // clears fails here instead of looping forever.
            for round in 0..keys.len() + 3 {
                let (status, body) = call(
                    &router,
                    operation,
                    &request(row, Some(max_results), token.as_deref()),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case} / page {round}: expected 200, got {status} with {body}"
                );
                let page = served(row, &case, &body);
                assert!(
                    page.len() <= max_results,
                    "case {case} / page {round}: a page must not exceed the requested size, got \
                     {page:?}"
                );
                walked.extend(page);
                match token_of(&body) {
                    Some(next) => token = Some(next),
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
                walked, keys,
                "case {case}: walking the minted tokens must report every key of this collection \
                 exactly once, in the page's order, got {walked:?}"
            );
        }
    }
}

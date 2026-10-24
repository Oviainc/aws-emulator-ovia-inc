// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListEventDataStores`.
//!
//! Every case goes over the wire through the shared [`Emulator`] harness, so what is
//! asserted here is what a client sees — including the HTTP status and the error
//! code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * a listing reports every event data store the account holds in the Region the
//!   request is served in ("Returns information about all event data stores in the
//!   account, in the current Region"), as that store's ARN and name — the model
//!   deprecates every other member of the `EventDataStore` shape with "<member> is no
//!   longer returned by ListEventDataStores";
//! * a page carries at most `MaxResults` stores in the registry's own store-id order
//!   and hands back a `NextToken` exactly when stores remain, which resumes the walk
//!   at the first store the page did not carry — so every store is reported exactly
//!   once across the pages whatever the page size;
//! * a `NextToken` this service did not issue for THIS walk is refused with
//!   `InvalidNextTokenException` ("a token that is not valid, or a token that was
//!   previously used in a request with different parameters");
//! * `MaxResults` is bounded at 1..1000 by the model's `@range` on
//!   `ListEventDataStoresMaxResultsCount`, and a limit outside it is the modelled
//!   `InvalidMaxResultsException` rather than a clamped page.
//!
//! What a store CARRIES is `CreateEventDataStore`'s and `GetEventDataStore`'s rule,
//! asserted in `event_data_store_lifecycle.rs`; these cases assert only what the
//! listing adds.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that pin a listing to the Region it is served in.
const OTHER_REGION: &str = "us-west-2";

/// The names the pagination cases create. A store's listing position follows its
/// server-minted id, not its name or its creation order, so these are deliberately
/// not in sorted order either.
const NAMES: [&str; 9] = [
    "store.e", "store-i", "store_a", "store-c", "store.g", "store-b", "store_h", "store-d",
    "store.f",
];

/// The string member `member` of `body`, for an assertion that names the case.
fn string_member(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a string {member} in {body:?}"))
        .to_string()
}

/// The id suffix of an event data store ARN — the other form every operation accepts
/// in place of the whole ARN.
fn id_of(case: &str, arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("case {case}: {arn:?} carries no id suffix"))
        .to_string()
}

/// Create an event data store called `name` in `region`.
async fn create_in(emulator: &Emulator, name: &str, region: &str) -> Reply {
    emulator
        .call_in("CreateEventDataStore", region, &json!({"Name": name}))
        .await
}

/// Create an event data store called `name` in [`HOME_REGION`].
async fn create(emulator: &Emulator, name: &str) -> Reply {
    create_in(emulator, name, HOME_REGION).await
}

/// Walk every `ListEventDataStores` page in `region`, asserting along the way that no
/// page carries more than `page_size` stores and that a token appears only on a page
/// that was full. Hands back the `(name, ARN)` pairs in the order the pages reported
/// them.
async fn list_every_page(
    emulator: &Emulator,
    case: &str,
    region: &str,
    max_results: Option<usize>,
    page_size: usize,
) -> Vec<(String, String)> {
    let mut collected: Vec<(String, String)> = Vec::new();
    let mut next_token: Option<String> = None;
    // A page carrying a token carries at least one store, so the walk cannot take
    // more turns than there are stores; this bound only keeps a broken handler from
    // hanging the suite.
    for _ in 0..64 {
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let reply = emulator
            .call_in("ListEventDataStores", region, &request)
            .await;
        let body = reply.expect_ok(case);
        let page = body
            .get("EventDataStores")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected EventDataStores in {body:?}"))
            .clone();
        assert!(
            page.len() <= page_size,
            "case {case}: a page carried {} stores, more than the page size {page_size}",
            page.len()
        );
        for entry in &page {
            // A listed store reports the two members the model does not deprecate,
            // and nothing else.
            let reported = entry
                .as_object()
                .unwrap_or_else(|| panic!("case {case}: expected an object, got {entry:?}"))
                .len();
            assert_eq!(
                reported, 2,
                "case {case}: a listed store reports only EventDataStoreArn and Name, \
                 got {entry:?}"
            );
            collected.push((
                string_member(case, entry, "Name"),
                string_member(case, entry, "EventDataStoreArn"),
            ));
        }
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match next_token.as_deref() {
            None => return collected,
            Some(token) => assert_eq!(
                page.len(),
                page_size,
                "case {case}: NextToken {token:?} appeared on a page of {} stores, which is not \
                 a full page of {page_size}",
                page.len()
            ),
        }
    }
    panic!("case {case}: ListEventDataStores never stopped paging");
}

/// RULE: a listing reports every store the Region holds exactly once across its
/// pages, in the registry's store-id order, whatever the page size — and a token
/// appears only while stores remain.
#[tokio::test]
async fn every_store_in_the_region_is_listed_once_across_every_page_size() {
    // (case, how many stores exist, the MaxResults the listing asks for)
    let rows: [(&str, usize, Option<usize>); 12] = [
        ("no stores at all", 0, None),
        ("no stores, one per page", 0, Some(1)),
        ("one store, no MaxResults", 1, None),
        ("one store, one per page", 1, Some(1)),
        ("two stores, one per page", 2, Some(1)),
        ("two stores, two per page", 2, Some(2)),
        ("five stores, two per page", 5, Some(2)),
        ("five stores, three per page", 5, Some(3)),
        ("five stores, five per page", 5, Some(5)),
        ("nine stores, no MaxResults", 9, None),
        ("nine stores, four per page", 9, Some(4)),
        ("nine stores, the modelled maximum per page", 9, Some(1000)),
    ];

    for (case, count, max_results) in rows {
        let emulator = Emulator::new();
        // (the store's id, its name, its ARN), as the create response reports them.
        let mut created: Vec<(String, String, String)> = Vec::new();
        for name in NAMES.iter().take(count) {
            let reply = create(&emulator, name).await;
            let arn = string_member(case, reply.expect_ok(case), "EventDataStoreArn");
            created.push((id_of(case, &arn), (*name).to_string(), arn));
        }
        let page_size = max_results.unwrap_or(state::LIST_EVENT_DATA_STORES_PAGE_SIZE);

        let listed = list_every_page(&emulator, case, HOME_REGION, max_results, page_size).await;

        // The registry is keyed by store id, so that is the order the pages walk.
        created.sort();
        let expected: Vec<(String, String)> = created
            .iter()
            .map(|(_, name, arn)| (name.clone(), arn.clone()))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected every store once in store-id order, got {listed:?}"
        );
        // Each listed store carries the ARN its own read reports, so a client can
        // address it from the listing alone.
        for (name, arn) in &listed {
            let read = emulator
                .call("GetEventDataStore", &json!({"EventDataStore": arn}))
                .await;
            let body = read.expect_ok(case);
            assert_eq!(
                &string_member(case, body, "Name"),
                name,
                "case {case:?}: the ARN {arn:?} listed for {name:?} reads back as another store"
            );
        }
    }
}

/// RULE: a listing reports the stores of the Region the request is served in, and no
/// other Region's.
#[tokio::test]
async fn a_listing_reports_only_the_region_the_request_is_served_in() {
    let emulator = Emulator::new();
    // (case, name, the Region the store is created in)
    let rows: [(&str, &str, &str); 3] = [
        ("a home-Region store", "home-one", HOME_REGION),
        ("a second home-Region store", "home-two", HOME_REGION),
        ("another Region's store", "away-one", OTHER_REGION),
    ];
    for (case, name, region) in rows {
        create_in(&emulator, name, region).await.expect_ok(case);
    }

    for (case, region, expected) in [
        ("the home Region", HOME_REGION, vec!["home-one", "home-two"]),
        ("the other Region", OTHER_REGION, vec!["away-one"]),
        ("a Region holding none", "eu-west-1", vec![]),
    ] {
        let listed = list_every_page(
            &emulator,
            case,
            region,
            None,
            state::LIST_EVENT_DATA_STORES_PAGE_SIZE,
        )
        .await;
        let mut actual: Vec<String> = listed.into_iter().map(|(name, _)| name).collect();
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?} in {region}, got {actual:?}"
        );
    }
}

/// RULE: a store still serving its `PENDING_DELETION` wait period is still a store
/// the Region holds, so a listing reports it — "the event data store enters a
/// PENDING_DELETION state, and is automatically deleted after a wait period of seven
/// days" (model doc for `DeleteEventDataStore`), and the wait has not elapsed.
#[tokio::test]
async fn a_store_awaiting_deletion_is_still_listed() {
    let emulator = Emulator::new();
    let case = "three stores, one of them deleted";
    let mut arns: Vec<String> = Vec::new();
    for name in NAMES.iter().take(3) {
        let reply = create(&emulator, name).await;
        arns.push(string_member(case, reply.expect_ok(case), "EventDataStoreArn"));
    }
    emulator
        .call("DeleteEventDataStore", &json!({"EventDataStore": arns[1]}))
        .await
        .expect_ok(case);

    let listed = list_every_page(
        &emulator,
        case,
        HOME_REGION,
        None,
        state::LIST_EVENT_DATA_STORES_PAGE_SIZE,
    )
    .await;
    let mut listed_arns: Vec<String> = listed.into_iter().map(|(_, arn)| arn).collect();
    listed_arns.sort();
    let mut expected = arns.clone();
    expected.sort();
    assert_eq!(
        listed_arns, expected,
        "case {case:?}: expected every store, including the one awaiting deletion, \
         got {listed_arns:?}"
    );
}

/// The page token this listing hands out, as the service encodes it: the hex of
/// `<page size>|<region>|<store id>`. Spelled out here — rather than read off the
/// service's own helper — so a case can hand back a token that is well formed but
/// belongs to ANOTHER walk.
fn page_token(page_size: usize, region: &str, resume_at: &str) -> String {
    hex::encode(format!("{page_size}|{region}|{resume_at}"))
}

/// RULE: a `NextToken` this listing did not issue — unreadable, resuming at a value
/// no store id this service mints can take, or issued for another Region or another
/// `MaxResults` — is refused with `InvalidNextTokenException`; the one it DID issue
/// resumes the walk exactly where the page stopped.
#[tokio::test]
async fn a_token_the_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for name in NAMES.iter().take(3) {
        let reply = create(&emulator, name).await;
        let arn = string_member(name, reply.expect_ok(name), "EventDataStoreArn");
        created.push(id_of(name, &arn));
    }
    created.sort();
    // A store id this service could have minted, for the rows about a token that is
    // well formed but issued for another walk.
    let minted = created[1].clone();

    // Every row is at least 4 characters, the modelled `PaginationToken` minimum, so
    // the handler — not the boundary validator — is what refuses it. Each listing
    // below asks for `MaxResults` 1 in the home Region, so a token naming any other
    // page size or Region belongs to a different walk.
    let rows: [(&str, String); 10] = [
        ("a token this service never issued", "not-a-token".to_string()),
        ("hex that is not a token at all", hex::encode("nonsense")),
        (
            "a token carrying no resume position",
            page_token(1, HOME_REGION, ""),
        ),
        (
            "a resume position that is no store id",
            page_token(1, HOME_REGION, "zzzz"),
        ),
        (
            "a store id one group short",
            page_token(1, HOME_REGION, "3f2504e0-4f89-11d3-9a0c"),
        ),
        (
            "a store id spelled in upper case",
            page_token(1, HOME_REGION, "3F2504E0-4F89-11D3-9A0C-0305E82C3301"),
        ),
        (
            "a store id spelled without its hyphens",
            page_token(1, HOME_REGION, "3f2504e04f8911d39a0c0305e82c3301"),
        ),
        (
            "a token issued for another page size",
            page_token(2, HOME_REGION, &minted),
        ),
        (
            "a token issued in another Region",
            page_token(1, OTHER_REGION, &minted),
        ),
        (
            "a token with no separators at all",
            hex::encode(minted.clone()),
        ),
    ];
    for (case, token) in rows {
        emulator
            .call(
                "ListEventDataStores",
                &json!({"NextToken": token, "MaxResults": 1}),
            )
            .await
            .expect_error(case, 400, "InvalidNextTokenException");
    }

    // The token the service DID mint resumes at the store the first page stopped
    // before — the second in store-id order.
    let case = "a minted token";
    let first = emulator
        .call("ListEventDataStores", &json!({"MaxResults": 1}))
        .await;
    let token = string_member(case, first.expect_ok(case), "NextToken");
    let second = emulator
        .call(
            "ListEventDataStores",
            &json!({"MaxResults": 1, "NextToken": token}),
        )
        .await;
    let body = second.expect_ok(case);
    let resumed = body
        .get("EventDataStores")
        .and_then(Value::as_array)
        .and_then(|page| page.first())
        .map(|entry| string_member(case, entry, "EventDataStoreArn"))
        .unwrap_or_default();
    assert_eq!(
        id_of(case, &resumed),
        created[1],
        "case {case:?}: expected the page to resume at {:?}, got {resumed:?}",
        created[1]
    );
}

/// RULE: `MaxResults` is bounded at 1..1000 — "the maximum number of event data
/// stores to display on a single page", the `@range` the model puts on
/// `ListEventDataStoresMaxResultsCount` — so each published bound is served and a
/// value past either one is refused with the error this operation's model declares
/// for a limit that is not valid (`InvalidMaxResultsException`, HTTP 400), rather
/// than clamped.
#[tokio::test]
async fn a_max_results_outside_the_modelled_bound_is_refused() {
    let emulator = Emulator::new();
    for name in NAMES.iter().take(2) {
        create(&emulator, name).await.expect_ok(name);
    }

    // (case, the MaxResults asked for, how many stores the page may carry — None
    //  when the request is refused)
    let rows: [(&str, i64, Option<usize>); 6] = [
        ("the lowest page size the model admits", 1, Some(1)),
        ("the largest page size the model admits", 1000, Some(2)),
        ("one below the lowest", 0, None),
        ("one above the largest", 1001, None),
        ("far below the lowest", -1, None),
        ("far above the largest", 100_000, None),
    ];

    for (case, max_results, expected) in rows {
        let reply = emulator
            .call("ListEventDataStores", &json!({"MaxResults": max_results}))
            .await;
        match expected {
            Some(carried) => {
                let body = reply.expect_ok(case);
                let page = body
                    .get("EventDataStores")
                    .and_then(Value::as_array)
                    .map(Vec::len);
                assert_eq!(
                    page,
                    Some(carried),
                    "case {case:?}: expected a page of {carried} stores, got {body:?}"
                );
            }
            None => {
                reply.expect_error(case, 400, "InvalidMaxResultsException");
                assert_eq!(
                    reply.body.get("EventDataStores"),
                    None,
                    "case {case:?}: a refused request must report no page, got {reply:?}"
                );
            }
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListScopes`, driven over the service's own `restJson1`
//! wire protocol through the one harness this crate shares.
//!
//! The rule under test: `ListScopes` reports every scope the store holds, one
//! `ScopeSummary` per record carrying that record's own `scopeId`, `scopeArn` and
//! current `status`, in a stable `scopeId` order, cut into pages of at most
//! `maxResults` items with a `nextToken` that resumes after the last one — and
//! refuses a `nextToken` this listing did not issue. Each test asserts that over a
//! SET of inputs: the store's size, the page size, and the token's shape.

use serde_json::{json, Value};

use crate::harness::{
    extra_region, is_uuid_shaped, targets, Emulator, ACCOUNT_ID, PAST_PROVISIONING, SIGNING_NAME,
};

/// One `ScopeSummary` as the test compares it: the three `@required` members of
/// `ScopeSummary` (`smithy_query(query="shape",shape="ScopeSummary")`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    scope_id: String,
    scope_arn: String,
    status: String,
}

fn summaries(answer: &crate::harness::Answer) -> Vec<Summary> {
    answer
        .member("scopes")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| Summary {
            scope_id: text(item, "scopeId"),
            scope_arn: text(item, "scopeArn"),
            status: text(item, "status"),
        })
        .collect()
}

fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create `count` scopes, each with a distinct target so no two are the same
/// resource, and answer their `scopeId`s with the Region each was created in.
///
/// The account holds one scope per Region (`state::SCOPE_QUOTA`), so each scope
/// is signed for its own Region and the listing walks a store holding all of
/// them.
async fn create_scopes(emulator: &Emulator, count: usize) -> Vec<(String, &'static str)> {
    let mut created = Vec::new();
    for index in 0..count {
        let account = format!("{:012}", 100_000_000_000u64 + index as u64);
        let signed = extra_region(index);
        let answer = emulator
            .create_scope_in(
                signed,
                json!({ "targets": targets(&[(account.as_str(), signed)]) }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case scope #{index} in {signed}: the scope must be created, got {} with body {}",
            answer.status, answer.body
        );
        created.push((answer.text("scopeId"), signed));
    }
    created
}

/// Every scope `ListScopes` reports, walked page by page with `max_results`: the
/// summaries in page order, and how many pages it took.
async fn page_through(
    emulator: &Emulator,
    max_results: Option<i32>,
) -> (Vec<Summary>, usize) {
    let mut listed: Vec<Summary> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = String::new();
        if let Some(limit) = max_results {
            query.push_str(&format!("?maxResults={limit}"));
        }
        if let Some(cursor) = &token {
            let separator = if query.is_empty() { '?' } else { '&' };
            query.push_str(&format!("{separator}nextToken={cursor}"));
        }
        let answer = emulator.list_scopes(&query).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults {max_results:?}, page {pages}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let page = summaries(&answer);
        // An absent `maxResults` is still a bounded page: `MaxResults`' own
        // `@range` maximum (1..25) is the ceiling (`state::MAX_PAGE_SIZE`).
        let limit = max_results.unwrap_or(25) as usize;
        assert!(
            page.len() <= limit,
            "case maxResults {max_results:?}, page {pages}: a page holds at most {limit} items, got {}",
            page.len()
        );
        listed.extend(page);
        pages += 1;
        token = answer.member("nextToken").as_str().map(ToString::to_string);
        if token.is_none() {
            return (listed, pages);
        }
        assert!(
            pages < 200,
            "case maxResults {max_results:?}: paging did not terminate after {pages} pages"
        );
    }
}

#[tokio::test]
async fn list_scopes_reports_every_created_scope_once_per_page_size() {
    // The store-size dimension: empty, one scope, a page's worth, and more than
    // one page's worth of the largest page `maxResults` allows.
    for store_size in [0usize, 1, 3, 25, 27] {
        let emulator = Emulator::new();
        let created = create_scopes(&emulator, store_size).await;
        // A scope's key in the store is its `scopeId`, so the listing order is
        // id order whatever order the creates came in.
        let mut expected: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();
        expected.sort();
        let regions: std::collections::HashMap<String, &str> = created
            .iter()
            .map(|(id, region)| (id.clone(), *region))
            .collect();

        // The page-size dimension: both published bounds of `maxResults` (1..25),
        // a size in between, and an absent one.
        for max_results in [Some(1), Some(2), Some(25), None] {
            let (listed, pages) = page_through(&emulator, max_results).await;
            let ids: Vec<String> = listed.iter().map(|item| item.scope_id.clone()).collect();
            assert_eq!(
                ids, expected,
                "case store {store_size}, maxResults {max_results:?}: expected every scope exactly once in id order, got {ids:?}"
            );
            let want_pages = store_size
                .div_ceil(max_results.unwrap_or(25) as usize)
                .max(1);
            assert_eq!(
                pages, want_pages,
                "case store {store_size}, maxResults {max_results:?}: expected {want_pages} pages, got {pages}"
            );
            for item in &listed {
                assert!(
                    is_uuid_shaped(&item.scope_id),
                    "case store {store_size}, maxResults {max_results:?}: expected a UUID-shaped scopeId, got {:?}",
                    item.scope_id
                );
                let region = regions
                    .get(&item.scope_id)
                    .expect("every listed scope is one this test created");
                assert_eq!(
                    item.scope_arn,
                    format!(
                        "arn:aws:{SIGNING_NAME}:{region}:{ACCOUNT_ID}:scope/{}",
                        item.scope_id
                    ),
                    "case store {store_size}, maxResults {max_results:?}: expected the ARN of {}, got {:?}",
                    item.scope_id,
                    item.scope_arn
                );
            }
        }
    }
}

#[tokio::test]
async fn every_summary_matches_what_get_scope_reports() {
    let emulator = Emulator::new();
    let created = create_scopes(&emulator, 4).await;
    // Before the provisioning span every scope is still IN_PROGRESS; after it the
    // clock owes each one SUCCEEDED. Both are read through the listing and
    // through `GetScope`, so a summary reports the status a reader sees at that
    // instant rather than the status the create answered with.
    for (phase, want_status) in [("fresh", "IN_PROGRESS"), ("settled", "SUCCEEDED")] {
        if phase == "settled" {
            tokio::time::sleep(PAST_PROVISIONING).await;
        }
        let (listed, _) = page_through(&emulator, Some(25)).await;
        assert_eq!(
            listed.len(),
            created.len(),
            "case {phase:?}: expected {} summaries, got {}",
            created.len(),
            listed.len()
        );
        for item in &listed {
            assert_eq!(
                item.status, want_status,
                "case {phase:?} scope {}: expected status {want_status}, got {:?}",
                item.scope_id, item.status
            );
            let read = emulator.get_scope(&item.scope_id).await;
            assert_eq!(
                read.status, 200,
                "case {phase:?} scope {}: the listed scope must be readable, got {} with body {}",
                item.scope_id, read.status, read.body
            );
            assert_eq!(
                Summary {
                    scope_id: read.text("scopeId"),
                    scope_arn: read.text("scopeArn"),
                    status: read.text("status"),
                },
                *item,
                "case {phase:?} scope {}: the summary must be the record GetScope reports, got {:?}",
                item.scope_id,
                item
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_scope_leaves_the_listing() {
    let emulator = Emulator::new();
    let mut remaining: Vec<String> = create_scopes(&emulator, 5)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    remaining.sort();

    while let Some(gone) = remaining.first().cloned() {
        let deleted = emulator.delete_scope(&gone).await;
        assert_eq!(
            deleted.status, 200,
            "case {gone:?}: expected 200 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );
        remaining.retain(|held| held != &gone);

        let (listed, _) = page_through(&emulator, Some(2)).await;
        let ids: Vec<String> = listed.iter().map(|item| item.scope_id.clone()).collect();
        assert_eq!(
            ids, remaining,
            "case {gone:?}: expected {remaining:?} still listed, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn list_scopes_refuses_a_next_token_it_never_issued() {
    let emulator = Emulator::new();
    let ids: Vec<String> = create_scopes(&emulator, 3)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();

    // A token THIS listing issued resumes the walk.
    let issued = emulator.list_scopes("?maxResults=1").await;
    let good = issued.text("nextToken");
    assert!(
        !good.is_empty(),
        "the first page of three scopes must carry a nextToken, got body {}",
        issued.body
    );
    let resumed = emulator
        .list_scopes(&format!("?maxResults=1&nextToken={good}"))
        .await;
    assert_eq!(
        resumed.status, 200,
        "the issued token must be accepted, got {} with body {}",
        resumed.status, resumed.body
    );
    assert_eq!(
        summaries(&resumed).len(),
        1,
        "the resumed page holds one scope, got body {}",
        resumed.body
    );

    // A token this listing did NOT issue is refused with the modelled 400 rather
    // than served as "start from the beginning" — including hex that decodes
    // cleanly but carries another listing's tag or no cursor at all.
    let rows = [
        ("not hex at all", "not-a-token".to_string()),
        ("two non-hex characters", "zz".to_string()),
        ("an odd-length hex string", "abc".to_string()),
        ("a single hex digit", "0".to_string()),
        ("percent-escaped braces", "%7B%7D".to_string()),
        ("hex of a bare scopeId", hex::encode(&ids[0])),
        ("hex of arbitrary text", hex::encode("zzzz")),
        ("hex of the tag with no key", hex::encode("scopes/1:")),
        (
            "hex of the monitors listing's tag",
            hex::encode(format!("monitors/1:{}", ids[0])),
        ),
    ];

    for (case, token) in rows {
        let answer = emulator.list_scopes(&format!("?nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({token:?}): expected 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException")
                || answer.text("__type").contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} / body {}",
            answer.error_type,
            answer.body
        );
    }
}

#[tokio::test]
async fn list_scopes_refuses_a_max_results_outside_the_modelled_range() {
    let emulator = Emulator::new();
    create_scopes(&emulator, 2).await;

    // `MaxResults`' own `@range` is 1..25
    // (`smithy_query(query="operation",operation="ListScopes")`): each bound is
    // accepted and one past it on either side is refused at the boundary.
    for accepted in [1, 25] {
        let answer = emulator
            .list_scopes(&format!("?maxResults={accepted}"))
            .await;
        assert_eq!(
            answer.status, 200,
            "case maxResults {accepted}: a published bound is accepted, got {} with body {}",
            answer.status, answer.body
        );
    }
    for refused in [0, 26] {
        let answer = emulator
            .list_scopes(&format!("?maxResults={refused}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {refused}: outside 1..25 is refused, got {} with body {}",
            answer.status, answer.body
        );
    }
}

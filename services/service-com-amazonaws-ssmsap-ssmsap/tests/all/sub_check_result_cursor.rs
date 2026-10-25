// SPDX-License-Identifier: CC0-1.0

//! What a `ListSubCheckResults` page token MEANS, and what the listing leaves behind,
//! driven over the wire through the same router harness the rest of this target uses.
//!
//! RULE under test: a sub-check result listing is a pure REPORT over one total order —
//! its `NextToken` names a POSITION in that order rather than a fixed page, so a token
//! resumed at any `MaxResults` continues at the result straight after the last one
//! reported; a walk that changes its page size between calls still reports every stored
//! result exactly once in `SubCheckResultId` order; and repeating a request reports the
//! same page while leaving the configuration check run it reports on exactly as it was.
//!
//! `tests/all/sub_check_results.rs` pins WHAT the reader reports (the children of the run
//! its `OperationId` names, in id order, paged, and the refusals); this suite pins the two
//! properties a reader can satisfy row-by-row and still get wrong: that the cursor is
//! independent of the page size asked for, and that reading changes nothing.
//!
//! The rows span what the rule generalises over: stored result counts (one, two, several,
//! one past the largest page the model admits) crossed with page-size SEQUENCES that
//! differ in the published `MaxResults` bounds they use, in whether the member is absent,
//! and in which direction the size changes mid-walk.
//!
//! As in `tests/all/sub_check_results.rs`, the sub-check results a run produced are
//! PLACED in the store (a configuration check of this emulator evaluates no rule, so no
//! API of this service writes one); every call under test goes over the wire.

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::CONFIGURATION_CHECK_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_PAGE_SIZE;

use crate::application_lifecycle::Emulator;
use crate::configuration_checks::{read_record, settle};
use crate::sub_check_results::{member, place_results, sub_check_results};

/// How many pages a walk follows before it gives up: a reader that handed out a token
/// that does not advance must fail its case rather than loop forever.
const PAGE_LIMIT: usize = 200;

/// The stored result counts every row is walked at: one, two, several, and one past the
/// largest page the model's `MaxResults` range admits (so at least two pages at any size).
fn counts() -> Vec<u32> {
    vec![1, 2, 5, MAX_PAGE_SIZE as u32 + 1]
}

/// The page-size sequences a walk cycles through, one entry per page. `None` is an absent
/// `MaxResults`; `1` and `MAX_PAGE_SIZE` are the published bounds of the member's range
/// (`smithy_query(query="shape", shape="MaxResults")`, range 1..50).
fn size_sequences() -> Vec<(&'static str, Vec<Option<i32>>)> {
    vec![
        ("growing: 1 then 2", vec![Some(1), Some(2)]),
        ("shrinking: 2 then 1", vec![Some(2), Some(1)]),
        ("a page size then none", vec![Some(1), None]),
        ("none then the smallest", vec![None, Some(1)]),
        (
            "the largest bound then the smallest",
            vec![Some(MAX_PAGE_SIZE), Some(1)],
        ),
        (
            "three sizes in turn",
            vec![Some(3), Some(1), Some(MAX_PAGE_SIZE)],
        ),
    ]
}

/// One request to the reader: `MaxResults` and `NextToken` present only when given.
fn request(operation_id: &str, max_results: Option<i32>, next_token: Option<&str>) -> Value {
    let mut body = json!({"OperationId": operation_id});
    let object = body
        .as_object_mut()
        .expect("the request body is a JSON object");
    if let Some(max_results) = max_results {
        object.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(next_token) = next_token {
        object.insert("NextToken".to_string(), json!(next_token));
    }
    body
}

/// The `SubCheckResultId`s one reply reported, in the order it reported them.
fn reported_ids(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            member(item, "Id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Follow a listing to exhaustion, asking for `sizes[page % sizes.len()]` items on each
/// page, and return every `SubCheckResultId` it reported, in page order.
async fn walk_changing_page_size(
    emulator: &Emulator,
    operation_id: &str,
    sizes: &[Option<i32>],
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..PAGE_LIMIT {
        let max_results = sizes[page % sizes.len()];
        let reply = emulator
            .list_sub_check_results(request(operation_id, max_results, token.as_deref()))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} page {page}: expected 200, got {}: {:?}",
            reply.status, reply.body
        );
        let items = reply
            .member("SubCheckResults")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?} page {page}: expected a SubCheckResults list, got {reply:?}")
            });
        let limit = max_results.unwrap_or(MAX_PAGE_SIZE) as usize;
        assert!(
            items.len() <= limit,
            "case {case:?} page {page}: expected at most {limit} items, got {}",
            items.len()
        );
        seen.extend(reported_ids(&items));
        token = reply
            .member("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match token.as_deref() {
            None => return seen,
            Some(handed_out) => assert_eq!(
                items.len(),
                limit,
                "case {case:?} page {page}: a token must only resume after a FULL page, got {} \
                 items with token {handed_out:?}",
                items.len()
            ),
        }
    }
    panic!("case {case:?}: the listing handed out tokens for more than {PAGE_LIMIT} pages");
}

/// RULE: the token names a POSITION in the `SubCheckResultId` order, not a page — so a
/// walk that asks for a different `MaxResults` on every page still reports each stored
/// result exactly once, in id order, with none skipped and none repeated.
#[tokio::test]
async fn a_token_resumes_at_a_position_whatever_page_size_follows_it() {
    for count in counts() {
        // One emulator per row: the rows are independent runs, not a sequence.
        let emulator = Emulator::new();
        let application_id = format!("HDB-CURSOR-{count}");
        emulator.with_application(&application_id).await;
        let operation_id = emulator
            .one_check(&application_id, CONFIGURATION_CHECK_TYPE_VALUES[0])
            .await;
        let stored = sub_check_results(0xC0, count, 1);
        place_results(&emulator, &operation_id, stored.clone());

        let mut expected: Vec<String> = stored.iter().map(|result| result.id.clone()).collect();
        expected.sort();

        for (label, sizes) in size_sequences() {
            let case = format!("{count} sub-check results, {label}");
            let reported =
                walk_changing_page_size(&emulator, &operation_id, &sizes, &case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected every stored sub-check result once, in id order, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE: the listing reports rather than consumes — the same request answered twice
/// reports the identical page (items AND the token that resumes after them), whether it
/// is the first page or one resumed from a token, and the run it reports on reads back
/// unchanged afterwards.
#[tokio::test]
async fn repeating_a_listing_reports_the_same_page_and_changes_nothing() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-CURSOR-REPEAT").await;
    let operation_id = emulator
        .one_check("HDB-CURSOR-REPEAT", CONFIGURATION_CHECK_TYPE_VALUES[0])
        .await;
    place_results(&emulator, &operation_id, sub_check_results(0xC1, 5, 2));
    // Wait out the terminal status the clock owes the run, so nothing further is due and
    // the two reads of the record must be identical.
    settle().await;
    let before = read_record(&emulator.get_check(&operation_id).await);

    for max_results in [None, Some(1), Some(2), Some(MAX_PAGE_SIZE)] {
        let case = format!("MaxResults {max_results:?}");
        let first = emulator
            .list_sub_check_results(request(&operation_id, max_results, None))
            .await;
        let again = emulator
            .list_sub_check_results(request(&operation_id, max_results, None))
            .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200, got {}: {:?}",
            first.status, first.body
        );
        assert_eq!(
            again.body, first.body,
            "case {case:?}: the same request must report the same page, got {:?} then {:?}",
            first.body, again.body
        );

        // And the page a token resumes is just as repeatable.
        if let Some(token) = first.member("NextToken").and_then(Value::as_str) {
            let resumed = emulator
                .list_sub_check_results(request(&operation_id, max_results, Some(token)))
                .await;
            let resumed_again = emulator
                .list_sub_check_results(request(&operation_id, max_results, Some(token)))
                .await;
            assert_eq!(
                resumed.status, 200,
                "case {case:?} resumed: expected 200, got {}: {:?}",
                resumed.status, resumed.body
            );
            assert_eq!(
                resumed_again.body, resumed.body,
                "case {case:?} resumed: the same token must report the same page, got {:?} then \
                 {:?}",
                resumed.body, resumed_again.body
            );
            let resumed_ids = reported_ids(
                resumed
                    .member("SubCheckResults")
                    .and_then(Value::as_array)
                    .unwrap_or(&Vec::new()),
            );
            let first_ids = reported_ids(
                first
                    .member("SubCheckResults")
                    .and_then(Value::as_array)
                    .unwrap_or(&Vec::new()),
            );
            assert!(
                resumed_ids
                    .iter()
                    .all(|resumed_id| !first_ids.contains(resumed_id)),
                "case {case:?}: a resumed page must report what the page before it did not, got \
                 {first_ids:?} then {resumed_ids:?}"
            );
        }
    }

    let after = read_record(&emulator.get_check(&operation_id).await);
    assert_eq!(
        after, before,
        "listing a run's sub-check results must leave the run as it was, got {after:?}"
    );
}

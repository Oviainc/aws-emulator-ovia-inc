// SPDX-License-Identifier: CC0-1.0

//! The selection step both paginated operations reach through one statement in
//! `crate::state` (`select_rows`, beside the page cut it feeds), driven over the
//! wire from EACH of its call sites.
//!
//! One rule: a page begins at the stored row its `nextToken` names, reports the
//! rows the request's narrowing selects in the stored order, and the token it
//! issues is the position of the last reported row in the STORED collection plus
//! one. Driven through `ListAccountActivities` (narrowed by
//! `filterActivityStatuses`) AND `GetFreeTierUsage` (narrowed by `filter`),
//! because a token that was an offset into the FILTERED view would still page a
//! narrowed list consistently — it would only go wrong once the narrowing
//! changes between pages, which is what every row below replays.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::free_tier::{harness, ok, text, ACTIVITIES_PAGE_SIZE_CEILING};

/// The largest page `ListAccountActivities` serves
/// ([`ACTIVITIES_PAGE_SIZE_CEILING`]), which both paginated operations accept,
/// so this asks for every remaining row at once.
const MAX_RESULTS_CEILING: usize = ACTIVITIES_PAGE_SIZE_CEILING as usize;

/// This service's paginated operations, each with the output member carrying its
/// page — the two call sites of the selection step.
const PAGINATED: [(&str, &str); 2] = [
    ("ListAccountActivities", "activities"),
    ("GetFreeTierUsage", "freeTierUsages"),
];

/// One call of a paginated operation: its page and the `nextToken` it issued.
async fn page(
    router: &MultiServiceRouter,
    operation: &str,
    items_member: &str,
    narrowing: &Value,
    page_size: usize,
    token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut body = narrowing.clone();
    body["maxResults"] = json!(page_size);
    if let Some(token) = token {
        body["nextToken"] = json!(token);
    }
    let payload = ok(router, operation, &body).await;
    let items = payload[items_member]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {operation}: {items_member} must be a list, got {payload}")
        })
        .clone();
    (items, payload["nextToken"].as_str().map(str::to_string))
}

/// Every row one paginated operation reports with no narrowing at all, which IS
/// the stored order the tokens below have to be offsets into.
async fn stored_rows(
    router: &MultiServiceRouter,
    operation: &str,
    items_member: &str,
) -> Vec<Value> {
    let (rows, token) = page(
        router,
        operation,
        items_member,
        &json!({}),
        MAX_RESULTS_CEILING,
        None,
    )
    .await;
    assert!(
        token.is_none(),
        "case {operation} at the maxResults ceiling: a page holding every row must carry no \
         nextToken, got {token:?}"
    );
    rows
}

/// The distinct values one reported member takes, in stored order, so a
/// narrowing below is built from what the service itself reports instead of
/// naming a stored value this test hard-codes.
fn distinct(rows: &[Value], member: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for row in rows {
        let value = text(row, member);
        if !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen
}

/// The narrowings to run against one operation: request members that select a
/// subset of the stored rows, plus the empty narrowing, which selects them all.
///
/// The rows differ in how much of the collection they select and in where the
/// selected rows sit in the stored order — one value of a member, two values,
/// every row — which is the class the rule generalises over. Each operation's
/// narrowing is the one its own model gives it: a status list for the activity
/// catalog, a `Dimensions` expression for the usage records.
fn narrowings(operation: &str, rows: &[Value]) -> Vec<(String, Value)> {
    let mut cases: Vec<(String, Value)> = vec![("no narrowing".to_string(), json!({}))];
    if operation == "ListAccountActivities" {
        let statuses = distinct(rows, "status");
        for status in &statuses {
            cases.push((
                format!("status {status}"),
                json!({"filterActivityStatuses": [status]}),
            ));
        }
        if let [first, second, ..] = statuses.as_slice() {
            cases.push((
                format!("statuses {first} and {second}"),
                json!({"filterActivityStatuses": [first, second]}),
            ));
        }
    } else {
        // The reported member each `Dimension` reads, from the model's
        // `Dimension` enum (`shared_types::DIMENSION_VALUES`) and the
        // `FreeTierUsage` members it compares against.
        for (member, key) in [
            ("service", "SERVICE"),
            ("region", "REGION"),
            ("freeTierType", "FREE_TIER_TYPE"),
        ] {
            for value in distinct(rows, member) {
                cases.push((
                    format!("{key} equals {value}"),
                    json!({"filter": {
                        "Dimensions": {
                            "Key": key,
                            "Values": [value],
                            "MatchOptions": ["EQUALS"],
                        }
                    }}),
                ));
            }
        }
    }
    cases
}

/// The stored position of each selected row, asserting on the way that the
/// selection is a sub-sequence of the stored collection: every selected row is a
/// stored row, and the selection keeps the stored order.
fn stored_positions(case: &str, stored: &[Value], selected: &[Value]) -> Vec<usize> {
    let mut positions: Vec<usize> = Vec::new();
    let mut cursor = 0usize;
    for row in selected {
        let found = stored
            .get(cursor..)
            .and_then(|tail| tail.iter().position(|stored| stored == row));
        let Some(found) = found else {
            panic!(
                "case {case}: every selected row must be a stored row, in the stored order; \
                 {row} is not among the stored rows at or after {cursor}"
            );
        };
        positions.push(cursor + found);
        cursor += found + 1;
    }
    positions
}

/// Both paginated operations select their rows and issue their tokens by ONE
/// rule: the token names a position in the STORED collection, so replaying it
/// with the narrowing REMOVED resumes at the stored row just after the last one
/// reported — and the narrowed walk itself reassembles the selected rows in the
/// stored order exactly once, at every page size.
///
/// A token that had been an offset into the filtered view would pass a narrowed
/// walk and fail here, which is exactly the drift the shared selection step
/// rules out for both call sites at once.
#[tokio::test]
async fn both_paginated_operations_select_rows_by_one_stored_position_rule() {
    let router = harness().0;

    for (operation, items_member) in PAGINATED {
        let stored = stored_rows(&router, operation, items_member).await;
        assert!(
            stored.len() >= 2,
            "case {operation}: the account must hold at least two rows for paging to be \
             observable, got {}",
            stored.len()
        );

        for (narrowing_name, narrowing) in narrowings(operation, &stored) {
            let case = format!("{operation} {narrowing_name}");

            // What the narrowing selects, asked for in one page: the sequence
            // every page size below must reassemble.
            let (selected, token) = page(
                &router,
                operation,
                items_member,
                &narrowing,
                MAX_RESULTS_CEILING,
                None,
            )
            .await;
            assert!(
                token.is_none(),
                "case {case} at the maxResults ceiling: a page holding every selected row must \
                 carry no nextToken, got {token:?}"
            );
            assert!(
                !selected.is_empty() && selected.len() <= stored.len(),
                "case {case}: the narrowing must select between one row and the whole \
                 collection of {}, got {}",
                stored.len(),
                selected.len()
            );
            let positions = stored_positions(&case, &stored, &selected);

            for page_size in 1..=selected.len() + 1 {
                let mut reported: Vec<Value> = Vec::new();
                let mut token: Option<String> = None;
                let mut terminated = false;

                // A walk over `selected.len()` rows takes at most that many
                // pages plus the empty tail; anything beyond is a
                // non-terminating walk.
                for page_index in 0..selected.len() + 2 {
                    let (items, issued) = page(
                        &router,
                        operation,
                        items_member,
                        &narrowing,
                        page_size,
                        token.as_deref(),
                    )
                    .await;

                    let already = reported.len();
                    let want = &selected[already..(already + page_size).min(selected.len())];
                    assert_eq!(
                        items.as_slice(),
                        want,
                        "case {case} size {page_size} page {page_index}: expected the selected \
                         rows {want:?} after {already} reported, got {items:?}"
                    );
                    reported.extend(items);

                    let Some(issued) = issued else {
                        assert_eq!(
                            reported.len(),
                            selected.len(),
                            "case {case} size {page_size} page {page_index}: the walk ended with \
                             {} of {} selected rows reported",
                            reported.len(),
                            selected.len()
                        );
                        terminated = true;
                        break;
                    };

                    // The token is an offset into the STORED collection: with
                    // the narrowing dropped, it resumes at the stored row just
                    // after the last row this page reported.
                    let resumes_at = positions[reported.len() - 1] + 1;
                    assert!(
                        resumes_at < stored.len(),
                        "case {case} size {page_size} page {page_index}: a token is issued only \
                         while a selected row is unreported, so it must name an existing stored \
                         position, got {resumes_at} of {}",
                        stored.len()
                    );
                    let (tail, tail_token) = page(
                        &router,
                        operation,
                        items_member,
                        &json!({}),
                        MAX_RESULTS_CEILING,
                        Some(&issued),
                    )
                    .await;
                    assert!(
                        tail_token.is_none(),
                        "case {case} size {page_size} page {page_index}: the unnarrowed tail asked \
                         for at the ceiling must carry no nextToken, got {tail_token:?}"
                    );
                    assert_eq!(
                        tail.as_slice(),
                        &stored[resumes_at..],
                        "case {case} size {page_size} page {page_index}: the token must resume at \
                         stored row {resumes_at}, got {tail:?}"
                    );

                    token = Some(issued);
                }

                assert!(
                    terminated,
                    "case {case} size {page_size}: the pagination walk did not terminate after {} \
                     rows",
                    reported.len()
                );
                assert_eq!(
                    reported, selected,
                    "case {case} size {page_size}: the pages must reassemble the selected rows in \
                     the stored order exactly once, got {reported:?}"
                );
            }
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `ListImportFileTask`, end to end over HTTP.
//!
//! The rule under test: the listing is a page of the import file tasks THIS STORE holds —
//! every task `StartImportFileTask` persisted, each reported exactly as the single-task
//! read reports it, in one stable total order, bounded by the page size the caller asked
//! for and resumed exactly once per task by an opaque cursor; a page size outside the
//! documented bound, or a cursor this service did not mint, is refused with the modelled
//! `ValidationException` rather than answered with a page.
//!
//! So nothing below asserts a canned list: every expectation is derived from the tasks
//! the test started, and every table spans what the rule generalises over — how many
//! tasks exist, which optional members each one carries, what page size is asked for
//! (including each published bound and one past it), and which ways a cursor can fail to
//! resolve.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::COMPLETION_SPAN;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{
    at, call, emulator, get_import, minimal_import, start_import, Answer,
};

/// The largest page `ListImportFileTask` serves, from the documentation of
/// `ListImportFileTaskRequest$maxResults`: "The total number of items to return. The
/// maximum value is 100." Spelled out here rather than imported from the crate, so a
/// wrong bound in `src/state.rs` fails this suite instead of agreeing with itself.
const DOCUMENTED_MAX_RESULTS: i64 = 100;

/// `ImportFileTaskStatus`'s non-terminal import status, and the terminal one the clock
/// owes a task — spelled out for the same reason.
const IMPORT_IN_PROGRESS: &str = "ImportInProgress";
const IMPORT_SUCCESS: &str = "ImportSuccess";

/// This service's one sentence for a value that fails a rule it enforces.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// This service's one sentence for a parameter it cannot resolve.
const INVALID_OR_MISSING_PARAMETERS: &str = "Looks like you provided invalid or missing parameters.";

/// `GET /list-import-file-task`, with `query` appended when it is not empty.
async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/list-import-file-task".to_string()
    } else {
        format!("/list-import-file-task?{query}")
    };
    call(router, "GET", &uri, None).await
}

/// The `taskInfos` member of a listing, which is present on every success — empty
/// included.
fn task_infos(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the listing must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    at(&answer.body, "/taskInfos")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: taskInfos is always reported as a list, got {:?}",
                answer.body
            )
        })
}

/// The ids of a page's entries, in the order the page reported them.
fn entry_ids(case: &str, entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            at(entry, "/id")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every listed task reports its id, got {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// The `nextToken` of a listing, or `None` when the page is the last one.
fn next_token(answer: &Answer) -> Option<String> {
    at(&answer.body, "/nextToken")
        .as_str()
        .map(str::to_string)
}

/// `count` `StartImportFileTask` requests that differ along what the rule generalises
/// over: which optional members are present at all, how many import groups, which
/// `dataSourceType`, and the shape of the key — so a listing is never asserted over one
/// kind of task.
fn start_requests(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| match index % 4 {
            0 => minimal_import(&format!("import-{index}")),
            1 => json!({
                "name": format!("import_{index}"),
                "S3Bucket": "migrationhub-strategy-my-bucket",
                "s3key": format!("inventory/{index}/servers.csv"),
                "s3bucketForReportData": "migrationhub-strategy-reports",
            }),
            2 => json!({
                "name": format!("IMPORT-{index}_mixed-case"),
                "S3Bucket": "migrationhub-strategy-bucket.with.dots",
                "s3key": format!("{index}"),
                // A `DataSourceType` entry beyond the `Import` one a start case usually
                // carries, from `smithy_query(query="shape",shape="DataSourceType")`.
                "dataSourceType": "StrategyRecommendationsApplicationDataCollector",
                "groupId": [{"name": "ExternalId", "value": format!("wave-{index}")}],
            }),
            _ => json!({
                "name": format!("i{index}"),
                "S3Bucket": "migrationhub-strategy-ab",
                "s3key": format!("a/{}servers.csv", "b/".repeat(index)),
                "s3bucketForReportData": "migrationhub-strategy-r",
                "dataSourceType": "Import",
                "groupId": [
                    {"name": "ExternalId", "value": format!("wave-{index}")},
                    {"name": "ExternalSourceType", "value": "MPA"},
                ],
            }),
        })
        .collect()
}

/// Start `count` varied imports on `router` and answer their ids, in start order.
async fn start_all(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for request in start_requests(count) {
        let started = start_import(router, request.clone()).await;
        assert_eq!(
            started.status, 200,
            "the import {request:?} must start, got {} with {:?}",
            started.status, started.body
        );
        ids.push(started.string("/id"));
    }
    ids
}

/// The listing reports exactly the tasks the store holds: one entry per started task, in
/// one stable total order, with no cursor while the whole store fits in the page. The
/// rows differ in how many tasks exist, the empty account included.
#[tokio::test]
async fn the_listing_is_exactly_the_tasks_that_were_started() {
    for total in [0usize, 1, 2, 5] {
        let case = format!("{total} started task(s)");
        let router = emulator();
        let started = start_all(&router, total).await;

        let answer = list(&router, "").await;
        let entries = task_infos(&case, &answer);
        assert_eq!(
            entry_ids(&case, &entries),
            started,
            "case {case:?}: the listing is every started task, in the store's order, got {:?}",
            answer.body
        );
        assert_eq!(
            next_token(&answer),
            None,
            "case {case:?}: a page holding the whole store carries no cursor, got {:?}",
            next_token(&answer)
        );

        // A read must not change what a later caller sees: the same call answers the
        // same page.
        let again = list(&router, "").await;
        assert_eq!(
            entry_ids(&case, &task_infos(&case, &again)),
            started,
            "case {case:?}: listing twice must report the same tasks, got {:?}",
            again.body
        );
    }
}

/// A listed task is the SAME record the single-task read reports: while it is running it
/// is reported running with no completion members, and once the clock owes it a
/// completion every member of the entry equals `GetImportFileTask`'s own answer.
#[tokio::test]
async fn a_listed_task_is_the_record_the_single_task_read_reports() {
    let router = emulator();
    let started = start_all(&router, 4).await;

    // Inside the completion span nothing has finished importing yet.
    let running = list(&router, "").await;
    for (index, entry) in task_infos("inside the completion span", &running)
        .iter()
        .enumerate()
    {
        let case = format!("task {index} inside the completion span");
        assert_eq!(
            at(entry, "/status"),
            json!(IMPORT_IN_PROGRESS),
            "case {case:?}: a listed task inside the span is still {IMPORT_IN_PROGRESS}, got {entry:?}"
        );
        for member in [
            "/completionTime",
            "/numberOfRecordsSuccess",
            "/numberOfRecordsFailed",
        ] {
            assert_eq!(
                at(entry, member),
                Value::Null,
                "case {case:?}: a running task reports no {member}, got {entry:?}"
            );
        }
    }

    // Nothing is done here but wait: no call drives the transition.
    std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));

    let settled = list(&router, "").await;
    let entries = task_infos("after the completion span", &settled);
    for (index, id) in started.iter().enumerate() {
        let case = format!("task {index} ({id})");
        let entry = &entries[index];
        assert_eq!(
            at(entry, "/status"),
            json!(IMPORT_SUCCESS),
            "case {case:?}: once the span has passed the listed task is {IMPORT_SUCCESS}, got {entry:?}"
        );
        let reported = get_import(&router, id).await;
        assert_eq!(
            entry, &reported.body,
            "case {case:?}: the listed entry must be the record GetImportFileTask reports, \
             got {entry:?} against {:?}",
            reported.body
        );
    }
}

/// Following the cursor walks every task exactly once, whatever page size is asked for:
/// no page is longer than the size, no page but the last is short, each page carries a
/// cursor only while another task follows, and the concatenated pages are the whole
/// store in its one order. The rows are the cross product of how many tasks exist and
/// the page size, so a page size both smaller and larger than the store is covered.
#[tokio::test]
async fn following_the_cursor_walks_every_task_exactly_once() {
    for total in [0usize, 1, 2, 5, 7] {
        let router = emulator();
        let started = start_all(&router, total).await;

        for size in [1i64, 2, 3, DOCUMENTED_MAX_RESULTS] {
            let case = format!("{total} task(s), maxResults={size}");
            let page_size = usize::try_from(size).expect("the page size is positive");

            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut calls = 0usize;
            loop {
                let mut query = format!("maxResults={size}");
                if let Some(cursor) = token.as_deref() {
                    query.push_str(&format!("&nextToken={cursor}"));
                }
                let answer = list(&router, &query).await;
                let entries = task_infos(&case, &answer);
                calls += 1;
                assert!(
                    entries.len() <= page_size,
                    "case {case:?}: a page must not exceed maxResults, got {} entries",
                    entries.len()
                );
                walked.extend(entry_ids(&case, &entries));
                token = next_token(&answer);
                match &token {
                    // A cursor is handed out only when another task follows, so a
                    // caller that follows cursors never makes an empty call.
                    Some(cursor) => {
                        assert_eq!(
                            entries.len(),
                            page_size,
                            "case {case:?}: only the last page may be short, got {} entries \
                             with cursor {cursor:?}",
                            entries.len()
                        );
                        assert!(
                            walked.iter().all(|id| !cursor.contains(id)),
                            "case {case:?}: the cursor is opaque, got {cursor:?}"
                        );
                    }
                    None => break,
                }
                assert!(
                    calls <= total + 1,
                    "case {case:?}: the walk must terminate, made {calls} calls"
                );
            }

            assert_eq!(
                walked, started,
                "case {case:?}: the pages concatenated are the whole store, once each"
            );
            let expected_calls = total.div_ceil(page_size).max(1);
            assert_eq!(
                calls, expected_calls,
                "case {case:?}: {total} task(s) in pages of {page_size} take {expected_calls} \
                 call(s), made {calls}"
            );
        }

        // The walk consumed nothing: the unpaged listing still holds every task.
        let case = format!("{total} task(s) after the paged walks");
        let answer = list(&router, "").await;
        assert_eq!(
            entry_ids(&case, &task_infos(&case, &answer)),
            started,
            "case {case:?}: paging is a read, so the store is unchanged by it"
        );
    }
}

/// `maxResults` is accepted AT each published bound and refused one past it, with the
/// modelled `ValidationException` (HTTP 400) and this service's one sentence for a value
/// it rejects — never a page and never a 501.
#[tokio::test]
async fn max_results_is_held_to_its_documented_bound() {
    let router = emulator();
    let started = start_all(&router, 3).await;

    // Accepted: the documented bounds themselves, and a size inside them.
    for size in [1i64, 2, DOCUMENTED_MAX_RESULTS] {
        let case = format!("maxResults={size} (inside the documented bound)");
        let answer = list(&router, &format!("maxResults={size}")).await;
        let entries = task_infos(&case, &answer);
        let expected = usize::try_from(size)
            .expect("the page size is positive")
            .min(started.len());
        assert_eq!(
            entries.len(),
            expected,
            "case {case:?}: a page of {size} over {} task(s) holds {expected} entries, got {:?}",
            started.len(),
            answer.body
        );
    }

    // Refused: one past each end of the documented range.
    for size in [0i64, -1, DOCUMENTED_MAX_RESULTS + 1, i64::from(i32::MAX)] {
        let case = format!("maxResults={size} (outside the documented bound)");
        let answer = list(&router, &format!("maxResults={size}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a page size outside the documented bound is refused, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: the refusal is the modelled ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            INVALID_REQUEST_BODY,
            "case {case:?}: the refusal carries this service's one sentence, got {:?}",
            answer.body
        );
    }

    // And the store is untouched by the refusals.
    let case = "after the refusals";
    assert_eq!(
        entry_ids(case, &task_infos(case, &list(&router, "").await)),
        started,
        "case {case:?}: a refused listing must not change the store"
    );
}

/// A `nextToken` this store cannot resolve is refused with the modelled
/// `ValidationException` (HTTP 400) and the service's one sentence for an unresolvable
/// parameter — never silently treated as "start from the beginning". The rows span the
/// ways a cursor can fail: not this service's encoding, its encoding without its marker,
/// a cursor mangled after it was handed out, and a well-formed cursor naming a task this
/// store does not hold.
#[tokio::test]
async fn a_cursor_this_store_cannot_resolve_is_refused() {
    let router = emulator();
    let started = start_all(&router, 3).await;

    // A cursor minted over a store that holds MORE tasks: well formed, and naming a task
    // the small store below never held.
    let crowded = emulator();
    start_all(&crowded, 6).await;
    let foreign = list(&crowded, "maxResults=5")
        .await
        .string("/nextToken");
    assert!(
        !foreign.is_empty(),
        "the crowded store must hand out a cursor to borrow"
    );

    // A cursor this store DID hand out, then mangled: the bytes it carries no longer
    // name a task, so it resolves to nothing. (Hex is case-insensitive, so changing a
    // cursor's case names the same task and is NOT a mangled cursor.)
    let mine = list(&router, "maxResults=1").await.string("/nextToken");
    assert!(!mine.is_empty(), "a short page must hand out a cursor");
    let truncated = mine[..mine.len() - 2].to_string();

    let cases: Vec<(&str, String)> = vec![
        ("not this service's encoding", "not-a-cursor".to_string()),
        (
            "well-formed hex carrying no marker",
            hex::encode("import-file-task-000000000001"),
        ),
        ("a cursor truncated after it was issued", truncated),
        ("a cursor naming a task this store never held", foreign),
    ];

    for (case, cursor) in cases {
        let answer = list(&router, &format!("nextToken={cursor}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unresolvable cursor is refused, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: the refusal is the modelled ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            INVALID_OR_MISSING_PARAMETERS,
            "case {case:?}: the refusal carries this service's one sentence for an \
             unresolvable parameter, got {:?}",
            answer.body
        );
    }

    // The cursor this store issued still resolves after the refusals, and resumes after
    // exactly the task the first page ended on.
    let case = "the store's own cursor after the refusals";
    let resumed = list(&router, &format!("maxResults=1&nextToken={mine}")).await;
    assert_eq!(
        entry_ids(case, &task_infos(case, &resumed)),
        vec![started[1].clone()],
        "case {case:?}: the cursor resumes at the task after the first page, got {:?}",
        resumed.body
    );
}

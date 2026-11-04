// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListBuildBatches`, the account-wide batch build id list,
//! driven over the wire the router serves (`POST /` with
//! `X-Amz-Target: CodeBuild_20161006.ListBuildBatches`).
//!
//! RULE under test: `ListBuildBatches` reports the ids of EVERY batch build the account
//! holds, across all of its projects, ordered by the instant each one STARTED — most
//! recently started first unless `sortOrder` asks for `ASCENDING` — keeping only the
//! batch builds whose current status is `filter.status` when the request carries one, cut
//! into pages of at most `maxResults` entries (the service's own ceiling when none is
//! asked for) that `nextToken` resumes after; a token naming no listed batch build is
//! refused with the modelled `InvalidInputException`, and the call reports stored state
//! without changing what a later caller sees.
//!
//! The harness (`emulator`, `call`, `create_project`, `error_type`, `listed_ids`,
//! `PAST_THE_SETTLE_SPAN`) is the one `project_build_lifecycle` already drives this
//! service with.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::project_build_lifecycle::{
    batch_enabled, call, create_project, emulator, error_type, listed_ids, PAST_THE_SETTLE_SPAN,
};

/// The bounds `ListBuildBatchesInput$maxResults` is held to, from the model: its target
/// `PageSize` is `@range(min: 1, max: 100)`
/// (`smithy_query(query="shape",shape="PageSize")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// Every value `StatusType` — the enum `BuildBatchFilter$status` targets — accepts on
/// the wire (`smithy_query(query="shape",shape="StatusType")`).
const STATUS_TYPE_VALUES: [&str; 6] = [
    "FAILED",
    "FAULT",
    "IN_PROGRESS",
    "STOPPED",
    "SUCCEEDED",
    "TIMED_OUT",
];

/// Long enough for two consecutive `StartBuildBatch` calls to fall in DIFFERENT
/// milliseconds.
const BETWEEN_STARTS: Duration = Duration::from_millis(2);

/// Start one batch build of `project` and hand back its id.
async fn start_build_batch(
    router: &smithy_server_runtime::MultiServiceRouter,
    project: &str,
) -> String {
    let (status, body) = call(router, "StartBuildBatch", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartBuildBatch {project:?}: expected 200, got {status} with {body}"
    );
    let id = body["buildBatch"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartBuildBatch {project:?} must report an id, got {body}"))
        .to_string();
    id
}

/// Create each project named in `per_project` (a project name and how many batch builds
/// to start for it) and hand back every id started, OLDEST FIRST — which is the order
/// `ListBuildBatches` reports them in when asked for `ASCENDING`.
///
/// Each start is separated by [`BETWEEN_STARTS`], so every batch build falls in a
/// different millisecond and the order expected back is the start order itself rather
/// than a tie-break between two that share a start instant.
async fn start_batches(
    router: &smithy_server_runtime::MultiServiceRouter,
    per_project: &[(&str, usize)],
) -> Vec<String> {
    let mut started = Vec::new();
    let mut created: Vec<&str> = Vec::new();
    for (project, count) in per_project {
        if !created.contains(project) {
            create_project(router, project, batch_enabled()).await;
            created.push(project);
        }
        for _ in 0..*count {
            started.push(start_build_batch(router, project).await);
            tokio::time::sleep(BETWEEN_STARTS).await;
        }
    }
    started
}

/// `started` in the order the listing reports it when the request names no `sortOrder`:
/// most recently started first.
fn newest_first(started: &[String]) -> Vec<String> {
    started.iter().rev().cloned().collect()
}

/// `ListBuildBatches` with `request`, asserting it succeeded: its ids and its nextToken.
async fn list_batches(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: Value,
) -> (Vec<String>, Option<String>) {
    let (status, body) = call(router, "ListBuildBatches", request.clone()).await;
    assert_eq!(
        status, 200,
        "ListBuildBatches {request}: expected 200, got {status} with {body}"
    );
    let token = body["nextToken"].as_str().map(str::to_string);
    (listed_ids(&body), token)
}

/// The status `BatchGetBuildBatches` reports for each id, which is what a status filter
/// is judged against.
async fn stored_statuses(
    router: &smithy_server_runtime::MultiServiceRouter,
    ids: &[String],
) -> BTreeMap<String, String> {
    let (status, body) = call(router, "BatchGetBuildBatches", json!({"ids": ids})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuildBatches {ids:?}: expected 200, got {status} with {body}"
    );
    let mut statuses = BTreeMap::new();
    for batch in body["buildBatches"].as_array().into_iter().flatten() {
        let id = batch["id"]
            .as_str()
            .unwrap_or_else(|| panic!("a stored batch build must carry an id, got {batch}"));
        let batch_status = batch["buildBatchStatus"].as_str().unwrap_or_else(|| {
            panic!("a stored batch build must carry a buildBatchStatus, got {batch}")
        });
        statuses.insert(id.to_string(), batch_status.to_string());
    }
    assert_eq!(
        statuses.len(),
        ids.len(),
        "BatchGetBuildBatches must resolve every id {ids:?}, got {statuses:?}"
    );
    statuses
}

/// The `startTime` `BatchGetBuildBatches` reports for each id, as epoch seconds, which
/// is the criterion the listing is ordered by.
async fn stored_start_times(
    router: &smithy_server_runtime::MultiServiceRouter,
    ids: &[String],
) -> BTreeMap<String, f64> {
    let (status, body) = call(router, "BatchGetBuildBatches", json!({"ids": ids})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuildBatches {ids:?}: expected 200, got {status} with {body}"
    );
    let mut start_times = BTreeMap::new();
    for batch in body["buildBatches"].as_array().into_iter().flatten() {
        let id = batch["id"]
            .as_str()
            .unwrap_or_else(|| panic!("a stored batch build must carry an id, got {batch}"));
        let start = batch["startTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("a stored batch build must carry a startTime, got {batch}"));
        start_times.insert(id.to_string(), start);
    }
    assert_eq!(
        start_times.len(),
        ids.len(),
        "BatchGetBuildBatches must resolve every id {ids:?}, got {start_times:?}"
    );
    start_times
}

/// RULE: the listing is every batch build of every project, ordered by the instant each
/// one started, reversed unless the request asks for `ASCENDING`.
///
/// The table varies the number of projects and how many batch builds each one holds —
/// so a handler listing only one project's batches, or listing per project rather than
/// account-wide, shows up — and spans `sortOrder` over the enum's published values
/// (`SortOrderType`: ASCENDING | DESCENDING) plus the absent case. The last row starts
/// batch builds of two projects alternately, so one project's batch builds are not
/// contiguous in the expected order: a handler ordering per project, or by the id's
/// `<projectName>:` prefix, answers that row differently.
#[tokio::test]
async fn list_build_batches_reports_every_stored_batch_build_in_start_order() {
    // (case, the batch builds to start: project and count, in start order)
    let cases: [(&str, &[(&str, usize)]); 5] = [
        ("one project, one batch build", &[("solo", 1)]),
        ("one project, several batch builds", &[("many", 4)]),
        (
            "several projects, uneven counts",
            &[("alpha", 1), ("beta", 3), ("gamma", 2)],
        ),
        (
            "a project holding no batch build at all",
            &[("holds-two", 2), ("holds-none", 0)],
        ),
        (
            "projects interleaved, so one project's batch builds are not contiguous",
            &[("zulu", 1), ("alpha", 1), ("zulu", 1), ("alpha", 1)],
        ),
    ];

    for (case, per_project) in cases {
        let router = emulator();
        let ascending = start_batches(&router, per_project).await;
        let descending = newest_first(&ascending);

        // (what the request carries for sortOrder, the order expected back)
        let orders: [(Value, &Vec<String>); 3] = [
            (Value::Null, &descending),
            (json!("ASCENDING"), &ascending),
            (json!("DESCENDING"), &descending),
        ];
        for (sort_order, expected) in orders {
            let mut request = json!({});
            if !sort_order.is_null() {
                request["sortOrder"] = sort_order.clone();
            }
            let (ids, next_token) = list_batches(&router, request).await;
            assert_eq!(
                &ids, expected,
                "case {case:?} / sortOrder {sort_order}: expected {expected:?}, got {ids:?}"
            );
            assert_eq!(
                next_token, None,
                "case {case:?} / sortOrder {sort_order}: one page holds every id, got \
                 {next_token:?}"
            );
        }

        // Every id listed names a batch build an implemented reader hands back — the
        // ids are the stored batch builds', not strings minted for the list — and the
        // `startTime` that reader reports is what the order follows.
        if !ascending.is_empty() {
            let (status, fetched) =
                call(&router, "BatchGetBuildBatches", json!({"ids": ascending.clone()})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchGetBuildBatches of the listed ids: expected 200, got \
                 {status} with {fetched}"
            );
            let found = fetched["buildBatches"].as_array().map_or(0, Vec::len);
            assert_eq!(
                found,
                ascending.len(),
                "case {case:?}: every listed id must resolve to a stored batch build, got \
                 {fetched}"
            );
            assert_eq!(
                fetched["buildBatchesNotFound"],
                json!([]),
                "case {case:?}: no listed id may be unknown to BatchGetBuildBatches, got \
                 {fetched}"
            );

            // The criterion itself: the ids the default listing reports come back in
            // non-increasing `startTime`, and the `ASCENDING` listing in non-decreasing
            // `startTime`.
            let start_times = stored_start_times(&router, &ascending).await;
            for (sort_order, listed) in [(json!("ASCENDING"), &ascending), (Value::Null, &descending)]
            {
                let times: Vec<f64> = listed
                    .iter()
                    .map(|id| start_times[id.as_str()])
                    .collect();
                let ordered = if sort_order.is_null() {
                    times.windows(2).all(|pair| pair[0] >= pair[1])
                } else {
                    times.windows(2).all(|pair| pair[0] <= pair[1])
                };
                assert!(
                    ordered,
                    "case {case:?} / sortOrder {sort_order}: the listed ids must run in \
                     startTime order, got the startTimes {times:?}"
                );
            }
        }
    }
}

/// An account with no batch build still states its (empty) listing: `BuildBatchIds` is
/// `@length(min: 0)`, so `ids` is `[]` rather than absent.
#[tokio::test]
async fn list_build_batches_on_an_account_with_no_batch_builds_reports_an_empty_ids_member() {
    let router = emulator();

    for case in ["no projects at all", "a project with no batch build"] {
        if case == "a project with no batch build" {
            create_project(&router, "empty-proj", json!({})).await;
        }
        let (status, body) = call(&router, "ListBuildBatches", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["ids"],
            json!([]),
            "case {case:?}: the listing is empty, so ids must be [], got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: an empty listing carries no nextToken, got {body}"
        );
    }
}

/// RULE: `filter.status` keeps exactly the batch builds whose CURRENT status is that
/// value — the status an implemented reader reports for the same batch build — and a
/// filter naming no status keeps every one of them.
///
/// The table spans every value `StatusType` publishes, and runs twice: once while the
/// started batch builds are still in their initial status, and once past the span the
/// clock owes their transition out of it, so the filter is shown to follow the stored
/// status rather than a fixed answer.
#[tokio::test]
async fn list_build_batches_filters_on_the_stored_status() {
    let router = emulator();
    let started = start_batches(&router, &[("filter-a", 2), ("filter-b", 1)]).await;
    let listed_order = newest_first(&started);

    for phase in ["before the clock-owed transition", "after it"] {
        if phase == "after it" {
            tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
        }
        let statuses = stored_statuses(&router, &started).await;

        for wanted in STATUS_TYPE_VALUES {
            let expected: Vec<String> = listed_order
                .iter()
                .filter(|id| statuses.get(*id).map(String::as_str) == Some(wanted))
                .cloned()
                .collect();
            let (ids, _) = list_batches(&router, json!({"filter": {"status": wanted}})).await;
            assert_eq!(
                ids, expected,
                "case {phase:?} / filter.status {wanted:?}: expected {expected:?} from the \
                 stored statuses {statuses:?}, got {ids:?}"
            );
        }

        // A filter that names no status selects as widely as no filter at all.
        for (case, request) in [
            ("no filter", json!({})),
            ("a filter with no status", json!({"filter": {}})),
        ] {
            let (ids, _) = list_batches(&router, request).await;
            assert_eq!(
                ids, listed_order,
                "case {phase:?} / {case}: expected every stored id {listed_order:?}, got {ids:?}"
            );
        }
    }
}

/// RULE: a page carries at most `maxResults` ids and `nextToken` opens the page starting
/// at the next one, so walking the tokens reassembles the whole listing exactly once,
/// whatever the page size.
///
/// The table spans page sizes below, at and above the number of batch builds held,
/// including both bounds the model publishes for `maxResults`, and the absent case (the
/// service's own ceiling).
#[tokio::test]
async fn list_build_batches_pages_at_max_results() {
    const HELD: usize = 5;

    let page_sizes: [Option<i64>; 6] = [
        None,
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(HELD as i64),
        Some(MAX_RESULTS_MAX),
    ];

    for max_results in page_sizes {
        let case = format!("maxResults {max_results:?} over {HELD} batch builds");
        let router = emulator();
        let started = start_batches(&router, &[("page-a", 3), ("page-b", HELD - 3)]).await;
        let expected = newest_first(&started);
        let page_size = max_results.unwrap_or(MAX_RESULTS_MAX) as usize;

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(size) = max_results {
                request["maxResults"] = json!(size);
            }
            if let Some(ref resume) = token {
                request["nextToken"] = json!(resume);
            }
            let (ids, next_token) = list_batches(&router, request).await;
            pages += 1;
            assert!(
                ids.len() <= page_size,
                "case {case:?}: page {pages} must hold at most {page_size} ids, got {ids:?}"
            );
            if let Some(ref next) = next_token {
                assert_eq!(
                    ids.len(),
                    page_size,
                    "case {case:?}: page {pages} is not the last, so it must be full, got {ids:?}"
                );
                assert_eq!(
                    next,
                    &expected[walked.len() + ids.len()],
                    "case {case:?}: page {pages}'s nextToken must name the next id, got {next:?}"
                );
            }
            walked.extend(ids);
            match next_token {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= HELD + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: walking the pages must yield every id once, in order"
        );
        let expected_pages = HELD.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s), got {pages}"
        );
    }
}

/// RULE: a `maxResults` inside the range the model publishes is accepted — at each
/// bound — and one outside it is refused.
#[tokio::test]
async fn list_build_batches_holds_max_results_to_the_modelled_range() {
    let router = emulator();
    let started = start_batches(&router, &[("range-a", 2), ("range-b", 1)]).await;

    // (case, the maxResults asked for, how many ids the answer may carry)
    let accepted: [(&str, i64, usize); 2] = [
        ("the published minimum", MAX_RESULTS_MIN, 1),
        ("the published maximum", MAX_RESULTS_MAX, started.len()),
    ];
    for (case, max_results, expected_len) in accepted {
        let (ids, _) = list_batches(&router, json!({"maxResults": max_results})).await;
        assert_eq!(
            ids.len(),
            expected_len,
            "case {case:?} (maxResults {max_results}): expected {expected_len} id(s), got {ids:?}"
        );
    }

    let refused: [(&str, i64); 2] = [
        ("one below the published minimum", MAX_RESULTS_MIN - 1),
        ("one past the published maximum", MAX_RESULTS_MAX + 1),
    ];
    for (case, max_results) in refused {
        let (status, body) =
            call(&router, "ListBuildBatches", json!({"maxResults": max_results})).await;
        assert_eq!(
            status, 400,
            "case {case:?} (maxResults {max_results}): expected 400, got {status} with {body}"
        );
    }
}

/// RULE: a `nextToken` that names no batch build of the listing is a refusable request,
/// not an empty page: `ListBuildBatches` models `InvalidInputException` (HTTP 400) and
/// nothing else.
#[tokio::test]
async fn list_build_batches_refuses_a_next_token_naming_no_stored_batch_build() {
    let router = emulator();
    let started = start_batches(&router, &[("token-proj", 2)]).await;

    // (case, the token sent)
    let cases: [(&str, String); 5] = [
        ("empty", String::new()),
        ("not a token at all", "not-a-token".to_string()),
        ("a batch build id with a changed suffix", format!("{}x", started[0])),
        ("a project name rather than a batch build id", "token-proj".to_string()),
        (
            "a batch build id of a project that holds none",
            "other-proj:00000000-0000-8000-8000-000000000000".to_string(),
        ),
    ];
    for (case, token) in cases {
        let (status, body) = call(&router, "ListBuildBatches", json!({"nextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
    }

    // The refusals changed nothing: the stored ids are still all of them.
    let (ids, _) = list_batches(&router, json!({"sortOrder": "ASCENDING"})).await;
    assert_eq!(
        ids, started,
        "after the refusals every stored id must still be listed, got {ids:?}"
    );
}

/// RULE: `ListBuildBatches` reports; it does not change what a later caller sees.
/// Listing twice — and listing between two reads of a batch build — leaves both the
/// listing and the batch build as they were.
#[tokio::test]
async fn list_build_batches_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let started = start_batches(&router, &[("read-proj", 2), ("read-other", 1)]).await;

    // Past the span the clock owes each batch build's transition out of `IN_PROGRESS`,
    // so the two reads below differ only if a call changed something — not because the
    // clock settled a batch build between them.
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

    let (_, before) = call(&router, "BatchGetBuildBatches", json!({"ids": started.clone()})).await;
    let (_, first) = call(&router, "ListBuildBatches", json!({})).await;
    let (_, again) = call(&router, "ListBuildBatches", json!({})).await;
    assert_eq!(
        first, again,
        "two ListBuildBatches calls must answer identically, got {first} then {again}"
    );
    let (_, after) = call(&router, "BatchGetBuildBatches", json!({"ids": started.clone()})).await;
    assert_eq!(
        before["buildBatches"], after["buildBatches"],
        "ListBuildBatches must not change a stored batch build: {} then {}",
        before["buildBatches"], after["buildBatches"]
    );
    assert_eq!(
        listed_ids(&first).len(),
        started.len(),
        "ListBuildBatches must list every started batch build, got {first}"
    );
}

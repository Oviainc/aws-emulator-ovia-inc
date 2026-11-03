// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListBuildBatchesForProject`, the per-project batch build id
//! list, driven over the wire the router serves (`POST /` with
//! `X-Amz-Target: CodeBuild_20161006.ListBuildBatchesForProject`).
//!
//! RULE under test: `ListBuildBatchesForProject` reports the ids of the batch builds of
//! ONE named project and of no other, ordered by the instant each one started — most
//! recently started first unless `sortOrder` asks for `ASCENDING` — keeping only the
//! batch builds whose current status is `filter.status` when the request carries one, cut
//! into pages of at most `maxResults` entries that `nextToken` resumes after; a project
//! the account does not hold is refused with the modelled `ResourceNotFoundException`, a
//! token naming no entry of that project's listing with `InvalidInputException`, and the
//! call reports stored state without changing what a later caller sees.
//!
//! The harness (`emulator`, `call`, `create_project`, `error_type`, `expected_arn`,
//! `listed_ids`, `PAST_THE_SETTLE_SPAN`) is the one `project_build_lifecycle` already
//! drives this service with.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::project_build_lifecycle::{
    call, create_project, emulator, error_type, expected_arn, listed_ids, PAST_THE_SETTLE_SPAN,
};

/// The bounds `ListBuildBatchesForProjectInput$maxResults` is held to, from the model:
/// its target `PageSize` is `@range(min: 1, max: 100)`
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
/// milliseconds, so the order expected back is the start order itself rather than a
/// tie-break between two batch builds sharing a start instant.
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
    body["buildBatch"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartBuildBatch {project:?} must report an id, got {body}"))
        .to_string()
}

/// Create each project named in `per_project` (a project name and how many batch builds
/// to start for it, in start order) and hand back, per project, the ids started for it
/// OLDEST FIRST — the order `ListBuildBatchesForProject` reports when asked for
/// `ASCENDING`.
async fn start_batches(
    router: &smithy_server_runtime::MultiServiceRouter,
    per_project: &[(&str, usize)],
) -> BTreeMap<String, Vec<String>> {
    let mut started: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (project, count) in per_project {
        if !started.contains_key(*project) {
            create_project(router, project, json!({})).await;
            started.insert((*project).to_string(), Vec::new());
        }
        for _ in 0..*count {
            let id = start_build_batch(router, project).await;
            started
                .get_mut(*project)
                .expect("the project was just recorded")
                .push(id);
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

/// `ListBuildBatchesForProject` with `request`, asserting it succeeded: its ids and its
/// nextToken.
async fn list_for_project(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: Value,
) -> (Vec<String>, Option<String>) {
    let (status, body) = call(router, "ListBuildBatchesForProject", request.clone()).await;
    assert_eq!(
        status, 200,
        "ListBuildBatchesForProject {request}: expected 200, got {status} with {body}"
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

/// The `startTime` `BatchGetBuildBatches` reports for each id, as epoch seconds — the
/// criterion the listing is ordered by.
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
    start_times
}

/// RULE: the listing is the named project's batch builds and nobody else's, ordered by
/// the instant each one started, reversed unless the request asks for `ASCENDING`.
///
/// The table varies how many projects the account holds and how many batch builds each
/// one holds — so a handler listing the whole account, or one project's batches under
/// another project's name, shows up — and spans `sortOrder` over the enum's published
/// values (`SortOrderType`: ASCENDING | DESCENDING) plus the absent case. The last row
/// starts two projects' batch builds alternately, so neither project's batch builds are
/// contiguous account-wide: a handler paging over the account-wide order answers that
/// row differently.
#[tokio::test]
async fn list_build_batches_for_project_reports_only_that_project_in_start_order() {
    // (case, the batch builds to start: project and count, in start order)
    let cases: [(&str, &[(&str, usize)]); 5] = [
        ("one project, one batch build", &[("solo", 1)]),
        ("one project, several batch builds", &[("many", 4)]),
        (
            "several projects, uneven counts",
            &[("alpha", 1), ("beta", 3), ("gamma", 2)],
        ),
        (
            "a project holding no batch build beside one that does",
            &[("holds-two", 2), ("holds-none", 0)],
        ),
        (
            "two projects interleaved, so neither one's batches are contiguous",
            &[("zulu", 1), ("alpha", 1), ("zulu", 1), ("alpha", 1)],
        ),
    ];

    for (case, per_project) in cases {
        let router = emulator();
        let started = start_batches(&router, per_project).await;

        for (project, ascending) in &started {
            let descending = newest_first(ascending);

            // (what the request carries for sortOrder, the order expected back)
            let orders: [(Value, &Vec<String>); 3] = [
                (Value::Null, &descending),
                (json!("ASCENDING"), ascending),
                (json!("DESCENDING"), &descending),
            ];
            for (sort_order, expected) in orders {
                let mut request = json!({"projectName": project});
                if !sort_order.is_null() {
                    request["sortOrder"] = sort_order.clone();
                }
                let (ids, next_token) = list_for_project(&router, request).await;
                assert_eq!(
                    &ids, expected,
                    "case {case:?} / project {project:?} / sortOrder {sort_order}: expected \
                     {expected:?}, got {ids:?}"
                );
                assert_eq!(
                    next_token, None,
                    "case {case:?} / project {project:?} / sortOrder {sort_order}: one page \
                     holds every id, got {next_token:?}"
                );
            }

            // No other project's batch build may appear under this project's name, and
            // every id listed names a stored batch build an implemented reader hands
            // back whose `startTime` is what the order follows.
            for id in ascending {
                assert!(
                    id.starts_with(&format!("{project}:")),
                    "case {case:?} / project {project:?}: every listed id belongs to that \
                     project, got {id:?}"
                );
            }
            if !ascending.is_empty() {
                let (status, fetched) =
                    call(&router, "BatchGetBuildBatches", json!({"ids": ascending})).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} / project {project:?}: BatchGetBuildBatches of the listed \
                     ids: expected 200, got {status} with {fetched}"
                );
                assert_eq!(
                    fetched["buildBatchesNotFound"],
                    json!([]),
                    "case {case:?} / project {project:?}: no listed id may be unknown to \
                     BatchGetBuildBatches, got {fetched}"
                );
                let start_times = stored_start_times(&router, ascending).await;
                let times: Vec<f64> = ascending
                    .iter()
                    .map(|id| start_times[id.as_str()])
                    .collect();
                assert!(
                    times.windows(2).all(|pair| pair[0] <= pair[1]),
                    "case {case:?} / project {project:?}: the ASCENDING listing must run in \
                     startTime order, got the startTimes {times:?}"
                );
            }
        }

        // The per-project listings partition the account-wide one: every id the account
        // holds is reported under exactly one project.
        let (account_wide, _) = {
            let (status, body) = call(&router, "ListBuildBatches", json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListBuildBatches: expected 200, got {status} with {body}"
            );
            (listed_ids(&body), ())
        };
        let mut per_project_total = 0;
        for (project, ids) in &started {
            let (listed, _) = list_for_project(&router, json!({"projectName": project})).await;
            per_project_total += listed.len();
            for id in &listed {
                assert!(
                    account_wide.contains(id),
                    "case {case:?} / project {project:?}: a listed id must be one the account \
                     holds, got {id:?} outside {account_wide:?}"
                );
            }
            assert_eq!(
                listed.len(),
                ids.len(),
                "case {case:?} / project {project:?}: expected {} id(s), got {listed:?}",
                ids.len()
            );
        }
        assert_eq!(
            per_project_total,
            account_wide.len(),
            "case {case:?}: the per-project listings must cover the account-wide listing \
             {account_wide:?} exactly once"
        );
    }
}

/// A project holding no batch build still states its (empty) listing: `BuildBatchIds` is
/// `@length(min: 0)`, so `ids` is `[]` rather than absent.
#[tokio::test]
async fn list_build_batches_for_project_with_no_batch_builds_reports_an_empty_ids_member() {
    let router = emulator();
    create_project(&router, "quiet-proj", json!({})).await;

    // (case, what else the account holds when the quiet project is listed)
    for case in [
        "nothing else in the account",
        "another project that does hold batch builds",
    ] {
        if case == "another project that does hold batch builds" {
            start_batches(&router, &[("busy-proj", 2)]).await;
        }
        let (status, body) = call(
            &router,
            "ListBuildBatchesForProject",
            json!({"projectName": "quiet-proj"}),
        )
        .await;
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

/// RULE: `filter.status` keeps exactly the project's batch builds whose CURRENT status is
/// that value — the status an implemented reader reports for the same batch build — and a
/// filter naming no status keeps every one of them.
///
/// The table spans every value `StatusType` publishes, and runs twice: once while the
/// started batch builds are still in their initial status, and once past the span the
/// clock owes their transition out of it, so the filter is shown to follow the stored
/// status rather than a fixed answer.
#[tokio::test]
async fn list_build_batches_for_project_filters_on_the_stored_status() {
    let router = emulator();
    let started = start_batches(&router, &[("filter-a", 3), ("filter-b", 1)]).await;

    for phase in ["before the clock-owed transition", "after it"] {
        if phase == "after it" {
            tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
        }
        for (project, ascending) in &started {
            let listed_order = newest_first(ascending);
            let statuses = stored_statuses(&router, ascending).await;

            for wanted in STATUS_TYPE_VALUES {
                let expected: Vec<String> = listed_order
                    .iter()
                    .filter(|id| statuses.get(*id).map(String::as_str) == Some(wanted))
                    .cloned()
                    .collect();
                let (ids, _) = list_for_project(
                    &router,
                    json!({"projectName": project, "filter": {"status": wanted}}),
                )
                .await;
                assert_eq!(
                    ids, expected,
                    "case {phase:?} / project {project:?} / filter.status {wanted:?}: expected \
                     {expected:?} from the stored statuses {statuses:?}, got {ids:?}"
                );
            }

            // A filter that names no status selects as widely as no filter at all.
            for (case, filter) in [("no filter", Value::Null), ("a filter with no status", json!({}))]
            {
                let mut request = json!({"projectName": project});
                if !filter.is_null() {
                    request["filter"] = filter.clone();
                }
                let (ids, _) = list_for_project(&router, request).await;
                assert_eq!(
                    ids, listed_order,
                    "case {phase:?} / project {project:?} / {case}: expected every stored id \
                     {listed_order:?}, got {ids:?}"
                );
            }
        }
    }
}

/// RULE: a page carries at most `maxResults` ids and `nextToken` opens the page starting
/// at the next one, so walking the tokens reassembles that project's whole listing
/// exactly once, whatever the page size.
///
/// The table spans page sizes below, at and above the number of batch builds the project
/// holds, including both bounds the model publishes for `maxResults`, and the absent case
/// (the service's own ceiling). A second project holds batch builds throughout, so a
/// handler paging over the account-wide listing overruns the project's pages.
#[tokio::test]
async fn list_build_batches_for_project_pages_at_max_results() {
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
        let started = start_batches(&router, &[("page-proj", HELD), ("page-other", 2)]).await;
        let expected = newest_first(&started["page-proj"]);
        let page_size = max_results.unwrap_or(MAX_RESULTS_MAX) as usize;

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"projectName": "page-proj"});
            if let Some(size) = max_results {
                request["maxResults"] = json!(size);
            }
            if let Some(ref resume) = token {
                request["nextToken"] = json!(resume);
            }
            let (ids, next_token) = list_for_project(&router, request).await;
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
            "case {case:?}: walking the pages must yield every id of the project once, in order"
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
async fn list_build_batches_for_project_holds_max_results_to_the_modelled_range() {
    let router = emulator();
    let started = start_batches(&router, &[("range-proj", 3)]).await;
    let held = started["range-proj"].len();

    // (case, the maxResults asked for, how many ids the answer may carry)
    let accepted: [(&str, i64, usize); 2] = [
        ("the published minimum", MAX_RESULTS_MIN, 1),
        ("the published maximum", MAX_RESULTS_MAX, held),
    ];
    for (case, max_results, expected_len) in accepted {
        let (ids, _) = list_for_project(
            &router,
            json!({"projectName": "range-proj", "maxResults": max_results}),
        )
        .await;
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
        let (status, body) = call(
            &router,
            "ListBuildBatchesForProject",
            json!({"projectName": "range-proj", "maxResults": max_results}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} (maxResults {max_results}): expected 400, got {status} with {body}"
        );
    }
}

/// RULE: a `nextToken` that names no entry of THIS project's listing is a refusable
/// request, not an empty page: the modelled `InvalidInputException` (HTTP 400). Another
/// project's batch build id is such a token, which is what distinguishes the per-project
/// listing's paging from the account-wide one's.
#[tokio::test]
async fn list_build_batches_for_project_refuses_a_next_token_outside_the_projects_listing() {
    let router = emulator();
    let started = start_batches(&router, &[("token-proj", 2), ("token-other", 1)]).await;
    let own = started["token-proj"].clone();
    let other = started["token-other"][0].clone();

    // (case, the token sent)
    let cases: [(&str, String); 5] = [
        ("empty", String::new()),
        ("not a token at all", "not-a-token".to_string()),
        ("a batch build id with a changed suffix", format!("{}x", own[0])),
        ("a project name rather than a batch build id", "token-proj".to_string()),
        ("another project's batch build id", other),
    ];
    for (case, token) in cases {
        let (status, body) = call(
            &router,
            "ListBuildBatchesForProject",
            json!({"projectName": "token-proj", "nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {body}"
        );
    }

    // The refusals changed nothing: the project's stored ids are still all of them.
    let (ids, _) = list_for_project(
        &router,
        json!({"projectName": "token-proj", "sortOrder": "ASCENDING"}),
    )
    .await;
    assert_eq!(
        ids, own,
        "after the refusals every stored id of the project must still be listed, got {ids:?}"
    );
}

/// RULE: a project the account does not hold is the modelled `ResourceNotFoundException`
/// (HTTP 400) naming the project ARN the request resolves to, and a request naming no
/// project at all is the modelled `InvalidInputException` — never an empty listing.
#[tokio::test]
async fn list_build_batches_for_project_refuses_a_project_it_does_not_hold() {
    let router = emulator();
    start_batches(&router, &[("present-proj", 1)]).await;

    // (case, the project name sent) — including a name whose batch builds exist under a
    // DIFFERENT project, and one that is a prefix of a project that does exist.
    let missing: [(&str, &str); 3] = [
        ("a name nothing was ever created for", "absent-proj"),
        ("a project name with a changed suffix", "present-projx"),
        ("a prefix of an existing project's name", "present"),
    ];
    for (case, project) in missing {
        let (status, body) = call(
            &router,
            "ListBuildBatchesForProject",
            json!({"projectName": project}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({project:?}): expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {case:?} ({project:?}): expected ResourceNotFoundException, got {body}"
        );
        let expected = format!(
            "The provided project {} does not exist.",
            expected_arn(&format!("project/{project}"))
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?} ({project:?}): expected the message {expected:?}, got {body}"
        );
    }

    // `projectName` carries no `@required`, so an absent one reaches the handler: there
    // is no project to list, which is a refusable request.
    let (status, body) = call(&router, "ListBuildBatchesForProject", json!({})).await;
    assert_eq!(
        status, 400,
        "no projectName: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("InvalidInputException"),
        "no projectName: expected InvalidInputException, got {body}"
    );
}

/// RULE: `ListBuildBatchesForProject` reports; it does not change what a later caller
/// sees. Listing twice — and listing between two reads of a batch build — leaves both the
/// listing and the batch build as they were.
#[tokio::test]
async fn list_build_batches_for_project_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let started = start_batches(&router, &[("read-proj", 2), ("read-other", 1)]).await;
    let ids = started["read-proj"].clone();

    // Past the span the clock owes each batch build's transition out of `IN_PROGRESS`,
    // so the two reads below differ only if a call changed something — not because the
    // clock settled a batch build between them.
    tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

    let (_, before) = call(&router, "BatchGetBuildBatches", json!({"ids": ids.clone()})).await;
    let request = json!({"projectName": "read-proj"});
    let (_, first) = call(&router, "ListBuildBatchesForProject", request.clone()).await;
    let (_, again) = call(&router, "ListBuildBatchesForProject", request).await;
    assert_eq!(
        first, again,
        "two ListBuildBatchesForProject calls must answer identically, got {first} then {again}"
    );
    let (_, after) = call(&router, "BatchGetBuildBatches", json!({"ids": ids.clone()})).await;
    assert_eq!(
        before["buildBatches"], after["buildBatches"],
        "ListBuildBatchesForProject must not change a stored batch build: {} then {}",
        before["buildBatches"], after["buildBatches"]
    );
    assert_eq!(
        listed_ids(&first).len(),
        ids.len(),
        "ListBuildBatchesForProject must list every batch build the project started, got {first}"
    );
}

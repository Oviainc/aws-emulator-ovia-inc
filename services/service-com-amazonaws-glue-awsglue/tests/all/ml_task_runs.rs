// SPDX-License-Identifier: CC0-1.0

//! `GetMLTaskRuns`, and the `StartMLLabelingSetGenerationTaskRun` whose runs it reports:
//! the historic task runs of ONE machine learning transform.
//!
//! RULE: `GetMLTaskRuns` "gets a list of runs for a machine learning transform… a
//! sortable, filterable list of machine learning task runs… with their parent transform's
//! TransformID" — it reports every task run started of the transform the request
//! addresses and no other transform's, narrowed to the runs satisfying EVERY criterion the
//! `Filter` names, ordered by the property and direction the `Sort` names and, absent a
//! `Sort` of the request's own, newest-started first, one
//! `MaxResults`-sized page at a time with a `NextToken` that resumes that same order. Each
//! element is the whole stored run: the minted `TaskRunId`, the transform it belongs to,
//! the properties of the task its start described, the status it stands in and the
//! instants it moved between statuses. A `TransformId` this account holds no transform
//! under addresses no run history, so it earns the operation's modelled
//! `EntityNotFoundException` rather than an empty listing, and so does a start of a
//! transform that does not exist. Listing is reporting only: the
//! only state it writes is the status the CLOCK already owed a run.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The transform every
//! run below is started of is registered through the create helper
//! [`crate::ml_transforms::given_transform`], so this suite and the transform suites
//! register one the same way.
//!
//! The rows differ along what the rule generalises over: how many runs are stored, the
//! page size asked for (including the published bounds 1 and 1000 and one past each),
//! which criterion the filter names and with which value (every modelled task type and
//! every modelled status, each instant bound on both sides of a run's own instant, and two
//! criteria at once), which column and direction the sort names (every modelled value of
//! both), and the form of a `TransformId` that addresses no transform.
//!
//! The due instant a run's owed status turns on is moved THROUGH THE STORE rather than
//! slept for, so the transient window is deterministic; one test sleeps, to pin that the
//! transition is owed by the wall clock and not by a reader. One run of the sort table is
//! settled on purpose, so the `STATUS` column discriminates rather than tying; the
//! `TASK_RUN_TYPE` column ties for now, because the one start operation implemented today
//! records one kind of task, and a tie falls back on the ordering key's own tie-break.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, ML_TASK_RUN_SETTLES_AFTER, ML_TASK_RUN_STATUS_STARTING,
    ML_TASK_RUN_STATUS_TIMEOUT, ML_TASK_RUN_TIMED_OUT_MESSAGE,
    ML_TASK_TYPE_LABELING_SET_GENERATION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, emulator_with_engine, far_future};
use crate::ml_transforms::given_transform;
use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// Every `TaskType` the model publishes. Source: the generated `TASK_TYPE_VALUES`
/// (`smithy_query shape=TaskType`), so a filter row exists for each kind of task and not
/// only for the kind these runs are.
const TASK_TYPES: [&str; 5] = [
    "EVALUATION",
    "EXPORT_LABELS",
    "FIND_MATCHES",
    "IMPORT_LABELS",
    "LABELING_SET_GENERATION",
];

/// Every `TaskStatusType` the model publishes. Source: the generated
/// `TASK_STATUS_TYPE_VALUES` (`smithy_query shape=TaskStatusType`).
const TASK_STATUSES: [&str; 7] = [
    "FAILED",
    "RUNNING",
    "STARTING",
    "STOPPED",
    "STOPPING",
    "SUCCEEDED",
    "TIMEOUT",
];

/// Every `TaskRunSortColumnType` the model publishes. Source: the generated
/// `TASK_RUN_SORT_COLUMN_TYPE_VALUES` (`smithy_query shape=TaskRunSortColumnType`).
const SORT_COLUMNS: [&str; 3] = ["STARTED", "STATUS", "TASK_RUN_TYPE"];

/// Every `SortDirectionType` the model publishes. Source: the generated
/// `SORT_DIRECTION_TYPE_VALUES` (`smithy_query shape=SortDirectionType`).
const SORT_DIRECTIONS: [&str; 2] = ["ASCENDING", "DESCENDING"];

/// One row of the paging table: the case, how many runs are started and the page size
/// asked for.
type PageRow<'a> = (&'a str, usize, Option<i32>);

/// One row of a filter table: the case, the `Filter` sent and whether the stored runs are
/// admitted by it.
type FilterRow<'a> = (&'a str, Value, bool);

/// One row of the refusal table: the case and the `TransformId` the listing addresses.
type MissRow<'a> = (&'a str, String);

/// How many pages a paged walk below may read before it is treated as making no
/// progress: more than the runs any row of these tables stores, plus the page that
/// reports the listing is exhausted.
const MAX_PAGES_PER_WALK: usize = 64;

/// Start one labelling set generation task run of `transform_id` and answer the
/// `TaskRunId` it reports.
pub(crate) async fn start_labeling_run(
    router: &MultiServiceRouter,
    transform_id: &str,
    output_s3_path: &str,
) -> String {
    let started = ok(
        router,
        "StartMLLabelingSetGenerationTaskRun",
        &json!({ "TransformId": transform_id, "OutputS3Path": output_s3_path }),
    )
    .await;
    started
        .get("TaskRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("StartMLLabelingSetGenerationTaskRun must answer a TaskRunId, got {started}")
        })
        .to_string()
}

/// Start `count` runs of `transform_id`, each writing its labelling set to its own path,
/// and answer the ids in the order they were started.
async fn start_runs(router: &MultiServiceRouter, transform_id: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        ids.push(
            start_labeling_run(router, transform_id, &format!("s3://labels/set-{index}/")).await,
        );
    }
    ids
}

/// The ids `started` holds, newest-started first: the order a listing that names no `Sort`
/// of its own reports a transform's run history in.
pub(crate) fn newest_first(started: &[String]) -> Vec<String> {
    let mut order = started.to_vec();
    order.reverse();
    order
}

/// List the task runs of `transform_id`, with whatever `extra` adds to the request.
async fn list_runs(router: &MultiServiceRouter, transform_id: &str, extra: Value) -> Value {
    let mut body = json!({ "TransformId": transform_id });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    ok(router, "GetMLTaskRuns", &body).await
}

/// The `TaskRuns` a listing reports, in the order it reported them.
fn listed(page: &Value) -> Vec<Value> {
    page.get("TaskRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("GetMLTaskRuns must report a TaskRuns list, got {page}"))
}

/// The `TaskRunId` of each reported run, in the order they were reported.
fn ids(runs: &[Value]) -> Vec<String> {
    runs.iter()
        .map(|run| {
            run["TaskRunId"]
                .as_str()
                .unwrap_or_else(|| panic!("a reported run must carry a TaskRunId, got {run}"))
                .to_string()
        })
        .collect()
}

/// Walk the whole listing one `page_size`-sized page at a time, following the `NextToken`,
/// and answer the ids in the order the pages reported them. `extra` carries the filter and
/// sort criteria every page is asked with.
async fn walk_pages(
    router: &MultiServiceRouter,
    case: &str,
    transform_id: &str,
    page_size: Option<i32>,
    extra: Value,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES_PER_WALK {
        let mut body = extra.clone();
        if let Some(size) = page_size {
            body["MaxResults"] = json!(size);
        }
        if let Some(cursor) = token.as_deref() {
            body["NextToken"] = json!(cursor);
        }
        let page = list_runs(router, transform_id, body).await;
        let entries = listed(&page);
        if let Some(size) = page_size {
            assert!(
                entries.len() <= size as usize,
                "case {case:?}: a page must hold at most the {size} runs asked for, got {page}"
            );
        }
        collected.extend(ids(&entries));
        match page.get("NextToken").and_then(Value::as_str) {
            Some(cursor) => token = Some(cursor.to_string()),
            None => return collected,
        }
    }
    panic!("case {case:?}: the paged walk never reached the end of the listing")
}

/// Move the due instant of every unsettled task run to `at`, so a test can hold the
/// transient window open or make the owed transition due without sleeping. The instants
/// themselves come from [`crate::job_runs`], which every run suite of this crate shares.
pub(crate) fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    for transform in store.ml_transforms.values_mut() {
        for run in &mut transform.task_runs {
            if run.settles_at.is_some() {
                run.settles_at = Some(at);
            }
        }
    }
}

/// The same for ONE run, so a test can hold two runs of a transform in different statuses
/// without waiting for either.
fn set_run_due_instant(engine: &Arc<engine::InMemoryEngine>, run_id: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    for transform in store.ml_transforms.values_mut() {
        for run in &mut transform.task_runs {
            if run.id == run_id {
                run.settles_at = Some(at);
            }
        }
    }
}

#[tokio::test]
async fn every_started_run_reads_back_in_its_transforms_listing() {
    // (case, how many runs are started, the page size asked for)
    let rows: &[PageRow<'_>] = &[
        ("no run started yet", 0, None),
        ("one run, no page size", 1, None),
        ("three runs, no page size", 3, None),
        // 1 and 1000 are the published bounds of `MaxResults` (`PageSize`, range 1..1000).
        ("three runs, one per page", 3, Some(1)),
        ("five runs, two per page", 5, Some(2)),
        ("three runs at the largest page the model accepts", 3, Some(1_000)),
    ];

    for (case, count, page_size) in rows {
        let router = emulator();
        let transform = given_transform(&router, "listed", Value::Null).await;
        let started = start_runs(&router, &transform, *count).await;

        let walked = walk_pages(&router, case, &transform, *page_size, json!({})).await;
        assert_eq!(
            walked,
            newest_first(&started),
            "case {case:?}: the listing must report every started run, newest started first, \
             got {walked:?} for {started:?}"
        );

        // Each element is the whole stored run: the properties its own start described,
        // the transform it belongs to and a status of the modelled enum.
        let single = list_runs(&router, &transform, json!({})).await;
        let entries = listed(&single);
        for (position, run) in entries.iter().enumerate() {
            // The listing runs newest first, so the element at `position` is the run
            // started that many places from the END of the start order.
            let index = count.saturating_sub(position + 1);
            assert_eq!(
                run["TransformId"].as_str(),
                Some(transform.as_str()),
                "case {case:?}: run {index} must name the transform it was started of, got {run}"
            );
            assert_eq!(
                run["Properties"]["TaskType"].as_str(),
                Some(ML_TASK_TYPE_LABELING_SET_GENERATION),
                "case {case:?}: run {index} must report the type of task it is, got {run}"
            );
            assert_eq!(
                run["Properties"]["LabelingSetGenerationTaskRunProperties"]["OutputS3Path"].as_str(),
                Some(format!("s3://labels/set-{index}/").as_str()),
                "case {case:?}: run {index} must report the path its start named, got {run}"
            );
            let status = run["Status"].as_str().unwrap_or("<no Status>");
            assert!(
                TASK_STATUSES.contains(&status),
                "case {case:?}: run {index} must stand in a modelled TaskStatusType, got {status}"
            );
            assert!(
                run["StartedOn"].as_f64().unwrap_or_default() > 0.0,
                "case {case:?}: run {index} must report the instant it started, got {run}"
            );
        }
        assert_eq!(
            entries.len(),
            *count,
            "case {case:?}: expected {count} runs in one page, got {single}"
        );
    }
}

/// A request that names no `Sort` of its own gets the run history newest-started first, so
/// the head of the first page is the run started most recently however many runs the
/// transform holds and however small the page is; a `Sort` the request does carry decides
/// the order instead, and the continuation token follows whichever order is in force.
#[tokio::test]
async fn the_run_history_reports_the_most_recently_started_run_first() {
    // (case, how many runs are started, the page size asked for)
    let rows: &[PageRow<'_>] = &[
        ("one run, no page size", 1, None),
        ("two runs, no page size", 2, None),
        ("two runs, one per page", 2, Some(1)),
        ("three runs, no page size", 3, None),
        ("three runs, two per page", 3, Some(2)),
        ("five runs, one per page", 5, Some(1)),
    ];

    for (case, count, page_size) in rows {
        let router = emulator();
        let transform = given_transform(&router, "recent-first", Value::Null).await;
        let started = start_runs(&router, &transform, *count).await;
        let expected = newest_first(&started);

        // The head of the FIRST page is the run started last, whatever page size is asked
        // for — which is what a client polling the head of the listing for the run it just
        // started reads.
        let mut head_body = json!({});
        if let Some(size) = page_size {
            head_body["MaxResults"] = json!(size);
        }
        let head = ids(&listed(&list_runs(&router, &transform, head_body).await));
        assert_eq!(
            head.first().map(String::as_str),
            started.last().map(String::as_str),
            "case {case:?}: the first element must be the run started most recently, got \
             {head:?} for {started:?}"
        );

        // And the whole paged walk follows that same order, page after page.
        let walked = walk_pages(&router, case, &transform, *page_size, json!({})).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: an unsorted listing must report the runs newest started first, got \
             {walked:?} for {started:?}"
        );

        // A `Sort` the request carries decides the order instead: ascending over the
        // instant a run started is the start order, and descending is the order a request
        // naming no criteria already gets.
        for (direction, wanted) in [
            ("ASCENDING", started.clone()),
            ("DESCENDING", expected.clone()),
        ] {
            let sorted = walk_pages(
                &router,
                case,
                &transform,
                *page_size,
                json!({ "Sort": { "Column": "STARTED", "SortDirection": direction } }),
            )
            .await;
            assert_eq!(
                sorted, wanted,
                "case {case:?}: sorted by STARTED {direction}, expected {wanted:?}, got \
                 {sorted:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_filter_narrows_the_listing_to_the_runs_it_admits() {
    let (router, engine) = emulator_with_engine();
    let transform = given_transform(&router, "filtered", Value::Null).await;
    let started = start_runs(&router, &transform, 3).await;
    // The runs are held in flight, so the status every row below filters on is the one
    // the starts recorded rather than one the clock moved mid-table.
    set_due_instant(&engine, far_future());

    let first = listed(&list_runs(&router, &transform, json!({})).await)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the started runs must be listed"));
    let started_on = first["StartedOn"]
        .as_f64()
        .unwrap_or_else(|| panic!("StartedOn must be a timestamp, got {first}"));
    // One second each side of the runs' own instants, so each row sits firmly inside or
    // outside the window the criterion describes rather than on its edge.
    let (before, after) = (started_on - 1.0, started_on + 1.0);

    let mut rows: Vec<FilterRow<'_>> = vec![
        (
            "the task type the runs were started as",
            json!({ "TaskRunType": ML_TASK_TYPE_LABELING_SET_GENERATION }),
            true,
        ),
        (
            "the status the runs stand in",
            json!({ "Status": ML_TASK_RUN_STATUS_STARTING }),
            true,
        ),
        (
            "started after an earlier instant",
            json!({ "StartedAfter": before }),
            true,
        ),
        (
            "started before a later instant",
            json!({ "StartedBefore": after }),
            true,
        ),
        (
            "started after a later instant",
            json!({ "StartedAfter": after }),
            false,
        ),
        (
            "started before an earlier instant",
            json!({ "StartedBefore": before }),
            false,
        ),
        (
            "a window around the instants",
            json!({ "StartedAfter": before, "StartedBefore": after }),
            true,
        ),
        (
            "a window that closes before them",
            json!({ "StartedAfter": before - 1.0, "StartedBefore": before }),
            false,
        ),
        (
            "the task type and the status together",
            json!({
                "TaskRunType": ML_TASK_TYPE_LABELING_SET_GENERATION,
                "Status": ML_TASK_RUN_STATUS_STARTING,
            }),
            true,
        ),
        (
            "the task type with a status no run stands in",
            json!({
                "TaskRunType": ML_TASK_TYPE_LABELING_SET_GENERATION,
                "Status": "SUCCEEDED",
            }),
            false,
        ),
    ];
    // Every other kind of task the model publishes admits none of these runs, and every
    // other status the model publishes admits none either.
    for task_type in TASK_TYPES {
        if task_type == ML_TASK_TYPE_LABELING_SET_GENERATION {
            continue;
        }
        rows.push((
            "another task type the model publishes",
            json!({ "TaskRunType": task_type }),
            false,
        ));
    }
    for status in TASK_STATUSES {
        if status == ML_TASK_RUN_STATUS_STARTING {
            continue;
        }
        rows.push((
            "another status the model publishes",
            json!({ "Status": status }),
            false,
        ));
    }

    for (case, filter, admitted) in &rows {
        let page = list_runs(&router, &transform, json!({ "Filter": filter })).await;
        let got = ids(&listed(&page));
        let expected: Vec<String> = if *admitted {
            newest_first(&started)
        } else {
            Vec::new()
        };
        assert_eq!(
            got, expected,
            "case {case:?}: expected the runs {filter} admits, got {page}"
        );

        // A filter narrows the pages a walk reports exactly as it narrows one page.
        let walked = walk_pages(
            &router,
            case,
            &transform,
            Some(1),
            json!({ "Filter": filter }),
        )
        .await;
        assert_eq!(
            walked, expected,
            "case {case:?}: a one-per-page walk must report the same runs {filter} admits"
        );
    }
}

#[tokio::test]
async fn a_sort_criteria_orders_the_listing_and_its_pages() {
    let (router, engine) = emulator_with_engine();
    let transform = given_transform(&router, "sorted", Value::Null).await;
    let started = start_runs(&router, &transform, 3).await;
    // Every run is held in flight, so no status moves part-way through the table below.
    set_due_instant(&engine, far_future());
    // The owed status of ONE run is then made due, so the `STATUS` column really
    // discriminates: that run comes to rest in the failure status while the other two are
    // still starting. The listing that follows is what materialises it.
    set_run_due_instant(&engine, &started[1], already_past());

    // Absent a sort, the listing reports the run history newest started first.
    let unsorted = ids(&listed(&list_runs(&router, &transform, json!({})).await));
    assert_eq!(
        unsorted,
        newest_first(&started),
        "case {:?}: a listing with no sort criteria reports the runs newest started first, got \
         {unsorted:?}",
        "no sort criteria"
    );

    for column in SORT_COLUMNS {
        let mut orders: Vec<(&str, Vec<String>)> = Vec::new();
        for direction in SORT_DIRECTIONS {
            let case = format!("sorted by {column} {direction}");
            let sort = json!({ "Column": column, "SortDirection": direction });
            let order = ids(&listed(
                &list_runs(&router, &transform, json!({ "Sort": sort })).await,
            ));
            let mut sorted_ids = order.clone();
            sorted_ids.sort();
            let mut all = started.clone();
            all.sort();
            assert_eq!(
                sorted_ids, all,
                "case {case:?}: a sorted listing must still report every run, got {order:?}"
            );

            // Paging over the sorted order reports that same order.
            let walked = walk_pages(
                &router,
                &case,
                &transform,
                Some(1),
                json!({ "Sort": sort }),
            )
            .await;
            assert_eq!(
                walked, order,
                "case {case:?}: a one-per-page walk must report the sorted order, got {walked:?}"
            );
            orders.push((direction, order));
        }

        // The direction reverses the order the column puts the runs in, whichever column
        // it is — the ordering key is total, so the two directions are each other's
        // reverse.
        let ascending = &orders[0].1;
        let descending = &orders[1].1;
        let mut reversed = descending.clone();
        reversed.reverse();
        assert_eq!(
            ascending, &reversed,
            "case {:?}: DESCENDING must be the reverse of ASCENDING, got {ascending:?} and \
             {descending:?}",
            format!("sorted by {column}")
        );
        if column == "STARTED" {
            assert_eq!(
                ascending, &started,
                "case {:?}: ordering by the instant a run started is the start order, got \
                 {ascending:?}",
                "sorted by STARTED ASCENDING"
            );
        }
        if column == "STATUS" {
            // The one run the clock has terminated stands in `TIMEOUT`, which orders after
            // the `STARTING` the other two stand in, so ordering by status puts it last
            // ascending and first descending. The two runs sharing a status are separated
            // by the key's own tie-break, which is what makes the order total.
            assert_eq!(
                ascending.last().map(String::as_str),
                Some(started[1].as_str()),
                "case {:?}: the run standing in {ML_TASK_RUN_STATUS_TIMEOUT} orders after \
                 the ones standing in {ML_TASK_RUN_STATUS_STARTING}, got {ascending:?}",
                "sorted by STATUS ASCENDING"
            );
            assert_eq!(
                descending.first().map(String::as_str),
                Some(started[1].as_str()),
                "case {:?}: the run standing in {ML_TASK_RUN_STATUS_TIMEOUT} orders first \
                 descending, got {descending:?}",
                "sorted by STATUS DESCENDING"
            );
        }
    }
}

#[tokio::test]
async fn the_status_the_listing_reports_is_the_one_the_clock_owes() {
    let (router, engine) = emulator_with_engine();
    let transform = given_transform(&router, "settling", Value::Null).await;
    let run_id = start_labeling_run(&router, &transform, "s3://labels/settling/").await;

    // In flight: the status the start recorded, nothing completed and no failure reason.
    set_due_instant(&engine, far_future());
    let in_flight = listed(&list_runs(&router, &transform, json!({})).await)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the started run must be listed"));
    assert_eq!(
        in_flight["TaskRunId"].as_str(),
        Some(run_id.as_str()),
        "case {:?}: the listing must report the run the start minted, got {in_flight}",
        "in flight"
    );
    assert_eq!(
        in_flight["Status"].as_str(),
        Some(ML_TASK_RUN_STATUS_STARTING),
        "case {:?}: a run whose owed status is not due yet is still starting, got {in_flight}",
        "in flight"
    );
    assert!(
        in_flight.get("CompletedOn").is_none(),
        "case {:?}: a run that has not finished reports no CompletedOn, got {in_flight}",
        "in flight"
    );
    assert!(
        in_flight.get("ErrorString").is_none(),
        "case {:?}: a run that has not failed reports no ErrorString, got {in_flight}",
        "in flight"
    );
    // Listing twice changes nothing a later caller sees.
    let again = listed(&list_runs(&router, &transform, json!({})).await)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the started run must still be listed"));
    assert_eq!(
        again, in_flight,
        "case {:?}: listed twice, a listing must change nothing",
        "in flight"
    );

    // Due: the status the clock owed it, carrying the reason nothing ran it to completion,
    // and the instant it completed.
    set_due_instant(&engine, already_past());
    let settled = listed(&list_runs(&router, &transform, json!({})).await)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the settled run must be listed"));
    assert_eq!(
        settled["Status"].as_str(),
        Some(ML_TASK_RUN_STATUS_TIMEOUT),
        "case {:?}: the status the clock owed must be reported, got {settled}",
        "owed status due"
    );
    assert_eq!(
        settled["ErrorString"].as_str(),
        Some(ML_TASK_RUN_TIMED_OUT_MESSAGE),
        "case {:?}: a run that ran no task says so, got {settled}",
        "owed status due"
    );
    assert!(
        settled["CompletedOn"].as_f64().unwrap_or_default() > 0.0,
        "case {:?}: a finished run reports the instant it completed, got {settled}",
        "owed status due"
    );
    let settled_again = listed(&list_runs(&router, &transform, json!({})).await)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the settled run must still be listed"));
    assert_eq!(
        settled_again, settled,
        "case {:?}: a settled run reports the same instants on every later listing",
        "owed status due"
    );

    // The filter selects on the status the run really stands in, not the one it was
    // started in.
    for (case, status, admitted) in [
        ("the status the clock moved it to", ML_TASK_RUN_STATUS_TIMEOUT, true),
        ("the status it was started in", ML_TASK_RUN_STATUS_STARTING, false),
    ] {
        let page = list_runs(&router, &transform, json!({ "Filter": { "Status": status } })).await;
        let got = ids(&listed(&page));
        assert_eq!(
            got.len(),
            usize::from(admitted),
            "case {case:?}: expected the runs standing in {status}, got {page}"
        );
    }
}

/// The clock owes the transition, not the reader: a run left alone for longer than the
/// span is finished on the first listing afterwards.
#[tokio::test]
async fn the_wall_clock_settles_a_run_nobody_listed() {
    let router = emulator();
    let transform = given_transform(&router, "unwatched", Value::Null).await;
    let run_id = start_labeling_run(&router, &transform, "s3://labels/unwatched/").await;

    tokio::time::sleep(ML_TASK_RUN_SETTLES_AFTER + Duration::from_millis(150)).await;

    let run = listed(&list_runs(&router, &transform, json!({})).await)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("run {run_id} must be listed"));
    assert_eq!(
        run["Status"].as_str(),
        Some(ML_TASK_RUN_STATUS_TIMEOUT),
        "case {:?}: the wall clock alone makes the owed status due, got {run}",
        "slept past the span"
    );
}

#[tokio::test]
async fn a_start_for_a_transform_this_account_does_not_hold_is_refused() {
    let router = emulator();
    let stored = given_transform(&router, "startable", Value::Null).await;
    let run = start_labeling_run(&router, &stored, "s3://labels/startable/").await;

    // The start records the run ON the transform it names, so an id addressing no
    // transform starts nothing. (case, the TransformId the start addresses)
    let rows: &[MissRow<'_>] = &[
        (
            "an id nothing has minted",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        (
            // "Names are not guaranteed unique and can be changed at any time"
            // (`MLTransform$Name`), so a name addresses no transform.
            "the stored transform's name used as an id",
            "startable".to_string(),
        ),
        ("the stored id in upper case", stored.to_uppercase()),
        (
            "the stored transform's own task run id",
            run.clone(),
        ),
    ];

    for (case, transform_id) in rows {
        let (status, body) = call(
            &router,
            "StartMLLabelingSetGenerationTaskRun",
            &json!({ "TransformId": transform_id, "OutputS3Path": "s3://labels/refused/" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled EntityNotFoundException's 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            format!("Cannot find MLTransform in account {ACCOUNT_ID} with handle {transform_id}"),
            "case {case:?}: expected the refusal to name the requested id, got {body}"
        );
        assert!(
            body.get("TaskRunId").is_none(),
            "case {case:?}: a refused start must mint no TaskRunId, got {body}"
        );
    }

    // A refused start recorded nothing: the transform this account holds still reports the
    // one run it really has.
    assert_eq!(
        ids(&listed(&list_runs(&router, &stored, json!({})).await)),
        vec![run],
        "case a refused start: a refused start must register no task run"
    );
}

#[tokio::test]
async fn each_transform_reports_only_its_own_runs() {
    let router = emulator();
    let first = given_transform(&router, "first-transform", Value::Null).await;
    let second = given_transform(&router, "second-transform", Value::Null).await;
    let first_runs = start_runs(&router, &first, 2).await;
    let second_runs = start_runs(&router, &second, 1).await;

    for (case, transform, expected) in [
        ("the transform with two runs", &first, newest_first(&first_runs)),
        ("the transform with one run", &second, newest_first(&second_runs)),
    ] {
        let page = list_runs(&router, transform, json!({})).await;
        assert_eq!(
            ids(&listed(&page)),
            expected,
            "case {case:?}: a transform reports its own runs and no sibling's, got {page}"
        );
    }
    assert!(
        first_runs
            .iter()
            .all(|id| !second_runs.contains(id)),
        "case {:?}: every start must mint its own TaskRunId, got {first_runs:?} and \
         {second_runs:?}",
        "two transforms"
    );
}

#[tokio::test]
async fn an_id_this_account_holds_no_transform_under_is_refused() {
    let router = emulator();
    let stored = given_transform(&router, "stored", Value::Null).await;
    let run = start_labeling_run(&router, &stored, "s3://labels/stored/").await;

    // (case, the TransformId the listing addresses)
    let rows: &[MissRow<'_>] = &[
        (
            "an id nothing has minted",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        (
            // "Names are not guaranteed unique and can be changed at any time"
            // (`MLTransform$Name`), so a name addresses no transform.
            "the stored transform's name used as an id",
            "stored".to_string(),
        ),
        (
            "the stored id with one character changed",
            format!("{}x", &stored[..stored.len() - 1]),
        ),
        ("the stored id in upper case", stored.to_uppercase()),
        (
            // A run's own id addresses a run, never the transform that holds it.
            "the task run's id used as a transform id",
            run.clone(),
        ),
        (
            // The longest and shortest `TransformId` the model accepts (`HashString`,
            // length 1..255), so the refusal is the answer at each published bound too.
            "the longest id the model accepts",
            "z".repeat(255),
        ),
        ("the shortest id the model accepts", "z".to_string()),
    ];

    for (case, transform_id) in rows {
        let (status, body) = call(
            &router,
            "GetMLTaskRuns",
            &json!({ "TransformId": transform_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled EntityNotFoundException's 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        // The sentence is the crate's one wording for a transform this account holds none
        // of, and its variable parts are the account and the id the request itself sent.
        assert_eq!(
            error_message(&body),
            format!("Cannot find MLTransform in account {ACCOUNT_ID} with handle {transform_id}"),
            "case {case:?}: expected the refusal to name the requested id, got {body}"
        );
    }

    // A refused listing stored nothing: the transform this account holds still reports its
    // one run.
    assert_eq!(
        ids(&listed(&list_runs(&router, &stored, json!({})).await)),
        vec![run],
        "case a refused listing: the stored transform's run history must be unchanged"
    );
}

#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused_at_the_boundary() {
    let router = emulator();
    let stored = given_transform(&router, "bounded", Value::Null).await;
    let run = start_labeling_run(&router, &stored, "s3://labels/bounded/").await;

    // `TransformId` is `HashString`, `length 1..255` (and `@required`), and `MaxResults`
    // is `PageSize`, `range 1..1000`, so a request outside either is refused by the
    // modelled constraint rather than served. One step past each published bound, and the
    // required member absent.
    // (case, the body the listing sends)
    let rows: &[(&str, Value)] = &[
        (
            "an empty id, one short of the published minimum length",
            json!({ "TransformId": "" }),
        ),
        (
            "an id one character past the published maximum length",
            json!({ "TransformId": "z".repeat(256) }),
        ),
        ("no id at all, where the member is required", json!({})),
        (
            "a page size one short of the published minimum",
            json!({ "TransformId": stored, "MaxResults": 0 }),
        ),
        (
            "a page size one past the published maximum",
            json!({ "TransformId": stored, "MaxResults": 1_001 }),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = call(&router, "GetMLTaskRuns", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the modelled constraints is refused at the \
             boundary with 400, got {status} with {answered}"
        );
        assert!(
            error_code(&answered).ends_with("ValidationException"),
            "case {case:?}: expected the boundary's ValidationException, got {} with {}",
            error_code(&answered),
            error_message(&answered)
        );
    }

    // Each published bound is SERVED: the page sizes one step inside the range answer.
    for (case, max_results) in [
        ("the published minimum page size", 1),
        ("the published maximum page size", 1_000),
    ] {
        let page = list_runs(&router, &stored, json!({ "MaxResults": max_results })).await;
        assert_eq!(
            ids(&listed(&page)),
            vec![run.clone()],
            "case {case:?}: a page size at the published bound must be served, got {page}"
        );
    }
}

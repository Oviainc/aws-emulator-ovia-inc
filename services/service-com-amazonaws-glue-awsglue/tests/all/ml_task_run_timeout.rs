// SPDX-License-Identifier: CC0-1.0

//! The end a machine learning task run comes to: the `Timeout` its transform carries.
//!
//! RULE: `Timeout` is "the maximum time that a task run for this transform can consume
//! resources before it is terminated and enters `TIMEOUT` status. The default is 2,880
//! minutes (48 hours)" (`CreateMLTransform$Timeout`), so a task run that spends that budget
//! without finishing its task is terminated in the modelled `TIMEOUT` status — the terminal
//! status of a run whose resource budget ran out, distinct from the `FAILED` a run Glue
//! rejects on its own input comes to rest in. Nothing here performs the machine learning
//! task a run names, so no run ever finishes work of its own: every run reaches that end,
//! and a run is never reported in flight once its own budget has elapsed.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The transform each run
//! below belongs to is registered through [`crate::ml_transforms::given_transform`] and each
//! run started through [`crate::ml_task_runs::start_labeling_run`], so this suite stocks the
//! store exactly as the listing and read suites beside it do.
//!
//! The rows differ along what the rule generalises over: the `Timeout` the create names
//! (absent, so the documented default applies; the published minimum of `Timeout`; a value
//! in between; the ceiling `Job$Timeout` publishes; and one so large that adding it to an
//! instant must not overflow), and how much of that budget the run has spent by the time it
//! is read (well inside it, and well past it).
//!
//! The instants a run turns on are moved THROUGH THE STORE rather than slept for, so every
//! row is deterministic; that the wall clock alone settles a run nobody read is pinned by
//! the listing suite ([`crate::ml_task_runs`]).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES,
    ML_TASK_RUN_STATUS_STARTING, ML_TASK_RUN_STATUS_TIMEOUT, ML_TASK_RUN_TIMED_OUT_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, emulator_with_engine, far_future};
use crate::ml_task_runs::{set_due_instant, start_labeling_run};
use crate::ml_transforms::given_transform;
use crate::wire::ok;

/// Every `Timeout` a row below creates its transform with, as `(case, the member the create
/// sends, the minutes the transform then carries)`. `None` is the create that names none, so
/// the documented default applies.
///
/// Source: `CreateMLTransform$Timeout` targets `Timeout`, whose `@range` has `min: 1` and no
/// maximum (`smithy_query shape=Timeout`), and the default is the 2,880 minutes the member's
/// own documentation publishes.
type TimeoutRow<'a> = (&'a str, Option<i32>, i32);

/// One row of the budget table: the case, the `Timeout` the create names, how long ago the
/// run started, the status the read must then report and the `ExecutionTime` it must report
/// once the run has been terminated.
type BudgetRow<'a> = (&'a str, Option<i32>, Duration, &'a str, Option<i64>);

/// The timeouts every status row is exercised over.
fn timeout_rows() -> Vec<TimeoutRow<'static>> {
    vec![
        (
            "no Timeout at all, so the documented default applies",
            None,
            DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES,
        ),
        ("the published minimum of one minute", Some(1), 1),
        ("an hour", Some(60), 60),
        // The ceiling `Job$Timeout` publishes (seven days), as a value well inside the
        // range this member admits.
        ("seven days", Some(10_080), 10_080),
        // `Timeout` publishes no maximum, so the largest value the integer admits must
        // bound a run rather than overflow the instant it is added to.
        ("the largest timeout the type admits", Some(i32::MAX), i32::MAX),
    ]
}

/// Read one task run of one transform through `GetMLTaskRun`.
async fn read_run(router: &MultiServiceRouter, transform_id: &str, task_run_id: &str) -> Value {
    ok(
        router,
        "GetMLTaskRun",
        &json!({ "TransformId": transform_id, "TaskRunId": task_run_id }),
    )
    .await
}

/// The one run `GetMLTaskRuns` reports for `transform_id`.
async fn listed_run(router: &MultiServiceRouter, transform_id: &str) -> Value {
    let page = ok(
        router,
        "GetMLTaskRuns",
        &json!({ "TransformId": transform_id }),
    )
    .await;
    page["TaskRuns"]
        .as_array()
        .and_then(|runs| runs.first())
        .cloned()
        .unwrap_or_else(|| panic!("GetMLTaskRuns must report the started run, got {page}"))
}

/// Move the instant every unsettled task run of the store started to `at`, so a row can put
/// a run's elapsed time either side of its transform's budget without sleeping.
fn set_started_instant(engine: &Arc<engine::InMemoryEngine>, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    for transform in store.ml_transforms.values_mut() {
        for run in &mut transform.task_runs {
            if run.settles_at.is_some() {
                run.started_on = at;
                run.last_modified_on = at;
            }
        }
    }
}

#[tokio::test]
async fn a_task_run_comes_to_rest_in_the_timeout_status() {
    for (case, sent, minutes) in timeout_rows() {
        let (router, engine) = emulator_with_engine();
        let extra = sent.map_or_else(|| Value::Null, |timeout| json!({ "Timeout": timeout }));
        let transform = given_transform(&router, "timed-out", extra).await;
        let run_id = start_labeling_run(&router, &transform, "s3://labels/timed-out/").await;

        // The transform really carries the timeout the row names, so what follows is the
        // end of a run whose budget is that one.
        let read_transform = ok(
            &router,
            "GetMLTransform",
            &json!({ "TransformId": &transform }),
        )
        .await;
        assert_eq!(
            read_transform["Timeout"].as_i64(),
            Some(i64::from(minutes)),
            "case {case:?}: the transform must carry {minutes} minutes of timeout, got \
             {read_transform}"
        );

        // In flight, the run stands in the status its start recorded.
        set_due_instant(&engine, far_future());
        let in_flight = read_run(&router, &transform, &run_id).await;
        assert_eq!(
            in_flight["Status"].as_str(),
            Some(ML_TASK_RUN_STATUS_STARTING),
            "case {case:?}: a run whose owed transition is not due yet is still starting, got \
             {in_flight}"
        );

        // Once the owed transition is due, the run is terminated in the status its budget
        // running out puts it in — never in the status a rejected input earns.
        set_due_instant(&engine, already_past());
        let settled = read_run(&router, &transform, &run_id).await;
        assert_eq!(
            settled["Status"].as_str(),
            Some(ML_TASK_RUN_STATUS_TIMEOUT),
            "case {case:?}: expected the terminated run to report {ML_TASK_RUN_STATUS_TIMEOUT}, \
             got {settled}"
        );
        assert_eq!(
            settled["ErrorString"].as_str(),
            Some(ML_TASK_RUN_TIMED_OUT_MESSAGE),
            "case {case:?}: a terminated run must report why it ended, got {settled}"
        );
        assert!(
            settled["CompletedOn"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: a terminated run must report when it ended, got {settled}"
        );
        // The transform's own listing reports that same end, element for element.
        assert_eq!(
            listed_run(&router, &transform).await,
            settled,
            "case {case:?}: the listing must report the run as the single read does"
        );
        // Read twice, the terminated run is the same run: the transition was owed by the
        // clock, so no reader moved it.
        assert_eq!(
            read_run(&router, &transform, &run_id).await,
            settled,
            "case {case:?}: a second read must report the same terminated run"
        );
    }
}

#[tokio::test]
async fn the_transforms_timeout_terminates_a_run_still_consuming_resources() {
    // (case, the Timeout the create names, how long ago the run started, the status the
    //  read must report, the ExecutionTime a terminated run must report)
    let rows: &[BudgetRow<'_>] = &[
        (
            "the published minimum, half a minute spent",
            Some(1),
            Duration::from_secs(30),
            ML_TASK_RUN_STATUS_STARTING,
            None,
        ),
        (
            "the published minimum, two minutes spent",
            Some(1),
            Duration::from_secs(120),
            ML_TASK_RUN_STATUS_TIMEOUT,
            Some(60),
        ),
        (
            "an hour's budget, two minutes spent",
            Some(60),
            Duration::from_secs(120),
            ML_TASK_RUN_STATUS_STARTING,
            None,
        ),
        (
            "an hour's budget, two hours spent",
            Some(60),
            Duration::from_secs(7_200),
            ML_TASK_RUN_STATUS_TIMEOUT,
            Some(3_600),
        ),
        (
            "the documented default, an hour spent",
            None,
            Duration::from_secs(3_600),
            ML_TASK_RUN_STATUS_STARTING,
            None,
        ),
        (
            "the documented default, three days spent",
            None,
            Duration::from_secs(3 * 86_400),
            ML_TASK_RUN_STATUS_TIMEOUT,
            Some(i64::from(DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES) * 60),
        ),
    ];

    for (case, sent, spent, expected_status, execution_time) in rows {
        let (router, engine) = emulator_with_engine();
        let extra = sent.map_or_else(|| Value::Null, |timeout| json!({ "Timeout": timeout }));
        let transform = given_transform(&router, "budgeted", extra).await;
        let run_id = start_labeling_run(&router, &transform, "s3://labels/budgeted/").await;

        // The owed transition is held well out, so what decides the run's end below is the
        // budget its transform grants it and nothing else.
        set_due_instant(&engine, far_future());
        set_started_instant(&engine, SystemTime::now() - *spent);

        let read = read_run(&router, &transform, &run_id).await;
        assert_eq!(
            read["Status"].as_str(),
            Some(*expected_status),
            "case {case:?}: expected {expected_status} after {spent:?} of the transform's \
             budget, got {read}"
        );
        match execution_time {
            // A terminated run consumed resources for exactly the budget it was granted,
            // and the instant it ended is that budget's end rather than the instant it was
            // read.
            Some(seconds) => {
                assert_eq!(
                    read["ExecutionTime"].as_i64(),
                    Some(*seconds),
                    "case {case:?}: a run terminated by its timeout consumed {seconds} \
                     seconds, got {read}"
                );
                assert_eq!(
                    read["ErrorString"].as_str(),
                    Some(ML_TASK_RUN_TIMED_OUT_MESSAGE),
                    "case {case:?}: a terminated run must report why it ended, got {read}"
                );
            }
            None => assert!(
                read.get("CompletedOn").is_none() && read.get("ErrorString").is_none(),
                "case {case:?}: a run still inside its budget has not ended, got {read}"
            ),
        }

        // The listing agrees with the single read about the run's end.
        assert_eq!(
            listed_run(&router, &transform).await,
            read,
            "case {case:?}: the listing must report the run as the single read does"
        );
    }
}

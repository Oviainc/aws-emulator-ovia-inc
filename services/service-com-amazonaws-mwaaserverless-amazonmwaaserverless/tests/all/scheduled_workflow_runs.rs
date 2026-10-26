// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the automated execution a workflow's trigger mode asks for.
//!
//! The rule under test: a workflow whose configuration asks the SERVICE to execute it
//! is executed by the service itself, so once its trigger's firing instant has passed
//! its run history reports a run no caller started, carrying the `RunType` that names
//! automated execution (`smithy_query(query="shape", shape="RunType")` declares
//! `ON_DEMAND | SCHEDULED`); a workflow configured any other way produces no run a
//! caller did not start, and a run a caller DID start always reports the caller's
//! `RunType` whatever the workflow's trigger mode is.
//!
//! Every test drives the real wire path (`MultiServiceRouter::dispatch` over an
//! `awsJson1_0` request). A firing instant is a CLOCK field of the stored workflow, so
//! the tables drive it from the store — exactly the way the run-lifecycle suite drives a
//! run's deadlines — rather than racing the wall clock; one suite then watches a real
//! workflow fire on the real clock, and one follows the trigger across an update and a
//! delete.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    Workflow, RUN_STATUS_FAILED, RUN_STATUS_QUEUED, RUN_STATUS_STARTING, RUN_TYPE_ON_DEMAND,
    RUN_TYPE_SCHEDULED, SCHEDULED_RUN_EVERY, TRIGGER_MODE_SCHEDULED,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ComAmazonawsMwaaserverlessAmazonmwaaserverlessState;

use crate::task_instance_reads::epoch_seconds;
use crate::workflow_lifecycle::{role_arn, Harness};

/// The `WorkflowRunStatus` values a run of this emulator can be reported in, in the
/// order the clock owes them: a run nothing executed is admitted, waits for an executor
/// and then reaches the modelled failure state.
const LIFECYCLE: &[&str] = &[RUN_STATUS_STARTING, RUN_STATUS_QUEUED, RUN_STATUS_FAILED];

/// How closely two timestamps must agree to be the same instant on the wire, in seconds.
const TIMESTAMP_TOLERANCE: f64 = 0.002;

/// Edit the stored workflow in place. The instant its trigger next fires is a clock
/// field, so setting it is how a row puts a firing in the past without waiting for it.
fn with_workflow<R>(harness: &Harness, arn: &str, edit: impl FnOnce(&mut Workflow) -> R) -> R {
    let state = harness
        .engine()
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    let mut store = state.lock();
    let workflow = store
        .workflows
        .get_mut(arn)
        .expect("setup: the created workflow is stored");
    edit(workflow)
}

/// The instant the stored workflow's trigger fires next, and `None` for a workflow the
/// service does not execute on its own.
fn trigger(harness: &Harness, arn: &str) -> Option<SystemTime> {
    with_workflow(harness, arn, |workflow| workflow.next_scheduled_run_at)
}

/// Put the workflow's next firing `firings` firings plus half a period in the past, so
/// exactly `firings` of them are owed at the moment of the call that follows. Returns
/// the instant of the OLDEST owed firing.
fn owe_firings(harness: &Harness, arn: &str, firings: u32) -> SystemTime {
    let behind = SCHEDULED_RUN_EVERY * firings - SCHEDULED_RUN_EVERY / 2;
    let oldest = SystemTime::now() - behind;
    with_workflow(harness, arn, |workflow| {
        workflow.next_scheduled_run_at = Some(oldest);
    });
    oldest
}

/// Create a workflow with `trigger_mode` (omitting the member entirely when it is
/// `None`) and return its ARN.
async fn create(harness: &Harness, name: &str, trigger_mode: Option<&str>) -> String {
    let mut body = serde_json::json!({
        "Name": name,
        "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/scheduled.yaml"},
        "RoleArn": role_arn(),
    });
    if let Some(mode) = trigger_mode {
        body["TriggerMode"] = serde_json::Value::String(mode.to_string());
    }
    let (status, created) = harness.call("CreateWorkflow", body).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string()
}

/// The `WorkflowRuns` a listing reports for a workflow, over the wire.
async fn listed_runs(harness: &Harness, arn: &str, case: &str) -> Vec<serde_json::Value> {
    let (status, listed) = harness
        .call("ListWorkflowRuns", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListWorkflowRuns to succeed, got {status} {listed:?}"
    );
    listed["WorkflowRuns"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One row of the trigger-mode table: the `TriggerMode` the workflow is configured with
/// and whether the service executes that workflow on its own.
struct ModeRow {
    case: &'static str,
    trigger_mode: Option<&'static str>,
    automated: bool,
}

/// The rows. `TriggerMode` is a free string (`GenericString`, length 1..255, pattern
/// `^.*$` — `smithy_query(query="shape", shape="CreateWorkflowRequest")`), so they differ
/// along the value supplied: the mode naming automated execution, the mode naming
/// caller-started execution, no mode at all, a mode the service does not define, the
/// shortest value the member allows, and the automated mode's word in another case
/// (which is NOT that value).
fn mode_rows() -> Vec<ModeRow> {
    vec![
        ModeRow {
            case: "the mode that asks the service to execute the workflow",
            trigger_mode: Some(TRIGGER_MODE_SCHEDULED),
            automated: true,
        },
        ModeRow {
            case: "the mode that leaves execution to the caller",
            trigger_mode: Some(RUN_TYPE_ON_DEMAND),
            automated: false,
        },
        ModeRow {
            case: "no trigger mode at all",
            trigger_mode: None,
            automated: false,
        },
        ModeRow {
            case: "a trigger mode the service does not define",
            trigger_mode: Some("EVENT"),
            automated: false,
        },
        ModeRow {
            case: "a one-character trigger mode, the shortest the member allows",
            trigger_mode: Some("M"),
            automated: false,
        },
        ModeRow {
            case: "the automated mode's word in lower case, which is another value",
            trigger_mode: Some("scheduled"),
            automated: false,
        },
    ]
}

/// Assert `member` of `value` is `expected` epoch seconds.
fn assert_instant(value: &serde_json::Value, member: &str, expected: SystemTime, case: &str) {
    let want = expected
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let got = epoch_seconds(&value[member]);
    assert!(
        got.is_some_and(|seconds| (seconds - want).abs() < TIMESTAMP_TOLERANCE),
        "case {case:?}: expected {member} at {want} epoch seconds, got {:?}",
        value[member]
    );
}

#[tokio::test]
async fn only_the_automated_trigger_mode_makes_the_service_run_a_workflow_itself() {
    for row in mode_rows() {
        // A fresh emulator per row, so one row's workflow cannot answer another's read.
        let harness = Harness::new("us-west-2");
        let arn = create(&harness, "triggerMode", row.trigger_mode).await;
        let case = row.case;

        // Whatever the mode, no firing is instantaneous: the history a create leaves
        // behind is empty, so a caller reading straight back sees the workflow it just
        // made rather than a run it never asked for.
        let runs = listed_runs(&harness, &arn, case).await;
        assert!(
            runs.is_empty(),
            "case {case:?}: a workflow's history is empty until its trigger fires, got \
             {runs:?}"
        );
        assert_eq!(
            trigger(&harness, &arn).is_some(),
            row.automated,
            "case {case:?}: expected the service to {} this workflow on its own",
            if row.automated { "execute" } else { "not execute" }
        );

        if row.automated {
            // One firing is owed, so one run the caller never started is reported.
            let fired_at = owe_firings(&harness, &arn, 1);
            let runs = listed_runs(&harness, &arn, case).await;
            assert_eq!(
                runs.len(),
                1,
                "case {case:?}: one owed firing produces exactly one run, got {runs:?}"
            );
            let run = &runs[0];
            assert_eq!(
                run["RunType"].as_str(),
                Some(RUN_TYPE_SCHEDULED),
                "case {case:?}: a run the workflow's own trigger started reports \
                 {RUN_TYPE_SCHEDULED}, got {:?}",
                run["RunType"]
            );
            assert_instant(&run["RunDetailSummary"], "CreatedOn", fired_at, case);
            assert_eq!(
                trigger(&harness, &arn),
                Some(fired_at + SCHEDULED_RUN_EVERY),
                "case {case:?}: a fired trigger moves on to its next firing"
            );

            // ROUND TRIP: the run the trigger produced is readable through the reader
            // every other run is read through, reporting the same type.
            let run_id = run["RunId"].as_str().unwrap_or_default();
            let (status, fetched) = harness
                .call(
                    "GetWorkflowRun",
                    serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetWorkflowRun to succeed, got {status} {fetched:?}"
            );
            assert_eq!(
                fetched["RunType"].as_str(),
                Some(RUN_TYPE_SCHEDULED),
                "case {case:?}: GetWorkflowRun reports the type the run carries, got {:?}",
                fetched["RunType"]
            );
            assert_eq!(
                fetched["RunDetail"]["RunType"].as_str(),
                Some(RUN_TYPE_SCHEDULED),
                "case {case:?}: the run detail reports the same type, got {:?}",
                fetched["RunDetail"]["RunType"]
            );
        }

        // A run the CALLER started is the caller's own under every trigger mode, so the
        // automated mode does not relabel an explicit start.
        let (status, started) = harness
            .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
        );
        let run_id = started["RunId"].as_str().unwrap_or_default();
        let (status, fetched) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflowRun to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["RunType"].as_str(),
            Some(RUN_TYPE_ON_DEMAND),
            "case {case:?}: a run the caller started reports {RUN_TYPE_ON_DEMAND}, got {:?}",
            fetched["RunType"]
        );
    }
}

#[tokio::test]
async fn a_trigger_left_behind_reports_one_run_per_firing_it_owed() {
    // The rows differ along how many firings the clock owes at once: one, a handful, and
    // a trigger that has been behind for several periods.
    for owed in [1_u32, 2, 3, 7] {
        let harness = Harness::new("eu-west-1");
        let arn = create(&harness, "owedFirings", Some(TRIGGER_MODE_SCHEDULED)).await;
        let case = format!("{owed} owed firing(s)");

        let oldest = owe_firings(&harness, &arn, owed);
        let runs = listed_runs(&harness, &arn, &case).await;
        assert_eq!(
            runs.len(),
            owed as usize,
            "case {case:?}: every owed firing produces its own run, got {runs:?}"
        );

        let mut seen_ids: Vec<&str> = Vec::new();
        for (index, run) in runs.iter().enumerate() {
            let expected_at = oldest + SCHEDULED_RUN_EVERY * u32::try_from(index).unwrap_or(0);
            assert_eq!(
                run["RunType"].as_str(),
                Some(RUN_TYPE_SCHEDULED),
                "case {case:?}: run {index} was started by the trigger, got {:?}",
                run["RunType"]
            );
            // The listing is chronological, so the i-th run is the i-th firing and
            // carries that firing's instant rather than the instant of the read.
            assert_instant(
                &run["RunDetailSummary"],
                "CreatedOn",
                expected_at,
                &format!("{case} / run {index}"),
            );
            let status = run["RunDetailSummary"]["Status"].as_str().unwrap_or_default();
            assert!(
                LIFECYCLE.contains(&status),
                "case {case:?}: run {index} reports a status of the lifecycle \
                 {LIFECYCLE:?}, got {status:?}"
            );
            let id = run["RunId"].as_str().unwrap_or_default();
            assert!(
                !seen_ids.contains(&id),
                "case {case:?}: every firing mints its own RunId, got {id:?} twice"
            );
            seen_ids.push(id);
        }

        // A read changes nothing a later caller sees: with the trigger parked beyond the
        // horizon of this test, the second listing answers exactly what the first did.
        with_workflow(&harness, &arn, |workflow| {
            workflow.next_scheduled_run_at = Some(SystemTime::now() + Duration::from_secs(3_600));
        });
        let again = listed_runs(&harness, &arn, &case).await;
        assert_eq!(
            again, runs,
            "case {case:?}: a read must not change what a later caller sees, got {again:?} \
             after {runs:?}"
        );
    }
}

#[tokio::test]
async fn a_scheduled_workflow_really_fires_on_the_clock_and_an_on_demand_one_never_does() {
    let harness = Harness::new("ap-south-1");
    let scheduled = create(&harness, "realSchedule", Some(TRIGGER_MODE_SCHEDULED)).await;
    let on_demand = create(&harness, "realOnDemand", Some(RUN_TYPE_ON_DEMAND)).await;

    // Poll the way a client waiting for the schedule does. Nothing but the clock drives
    // the trigger, so the poll neither hurries it nor holds it back.
    let deadline = SystemTime::now() + Duration::from_secs(10);
    let mut produced: Vec<serde_json::Value> = Vec::new();
    while SystemTime::now() < deadline {
        produced = listed_runs(&harness, &scheduled, "real clock").await;
        if !produced.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(
        !produced.is_empty(),
        "case real clock: a workflow the service executes must report a run within \
         {:?} of being created, got none",
        Duration::from_secs(10)
    );
    for run in &produced {
        assert_eq!(
            run["RunType"].as_str(),
            Some(RUN_TYPE_SCHEDULED),
            "case real clock: nobody started this run, so it reports \
             {RUN_TYPE_SCHEDULED}, got {:?}",
            run["RunType"]
        );
        assert_eq!(
            run["WorkflowArn"].as_str(),
            Some(scheduled.as_str()),
            "case real clock: the run belongs to the workflow whose trigger fired, got {:?}",
            run["WorkflowArn"]
        );
    }

    let idle = listed_runs(&harness, &on_demand, "real clock / on demand").await;
    assert!(
        idle.is_empty(),
        "case real clock: a workflow left to its caller produces no run of its own, got \
         {idle:?}"
    );
}

#[tokio::test]
async fn the_trigger_in_force_is_the_active_version_s_and_a_delete_removes_it() {
    let harness = Harness::new("us-east-1");
    let arn = create(&harness, "triggerFollowsVersion", Some(RUN_TYPE_ON_DEMAND)).await;
    assert!(
        trigger(&harness, &arn).is_none(),
        "case created on demand: the service executes no workflow the caller owns"
    );

    /// The update body, with the trigger mode a row switches to.
    fn update(arn: &str, trigger_mode: &str) -> serde_json::Value {
        serde_json::json!({
            "WorkflowArn": arn,
            "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v2.yaml"},
            "RoleArn": role_arn(),
            "TriggerMode": trigger_mode,
        })
    }

    // An update into automated execution configures the trigger, and the run it later
    // produces executes the version that asked for it.
    let (status, updated) = harness
        .call("UpdateWorkflow", update(&arn, TRIGGER_MODE_SCHEDULED))
        .await;
    assert_eq!(
        status, 200,
        "case updated to automated: expected UpdateWorkflow to succeed, got {status} \
         {updated:?}"
    );
    let active_version = updated["WorkflowVersion"]
        .as_str()
        .expect("an update reports the version it appended")
        .to_string();
    assert!(
        trigger(&harness, &arn).is_some(),
        "case updated to automated: the new version's trigger is the one in force"
    );

    let fired_at = owe_firings(&harness, &arn, 1);
    let runs = listed_runs(&harness, &arn, "updated to automated").await;
    assert_eq!(
        runs.len(),
        1,
        "case updated to automated: the owed firing produces one run, got {runs:?}"
    );
    assert_eq!(
        runs[0]["WorkflowVersion"].as_str(),
        Some(active_version.as_str()),
        "case updated to automated: the firing runs the version whose configuration \
         asked for it, got {:?}",
        runs[0]["WorkflowVersion"]
    );
    assert_instant(
        &runs[0]["RunDetailSummary"],
        "CreatedOn",
        fired_at,
        "updated to automated",
    );

    // An update back out of automated execution disables it: the runs already produced
    // stay, and no further one appears however long the clock runs.
    let (status, updated) = harness
        .call("UpdateWorkflow", update(&arn, RUN_TYPE_ON_DEMAND))
        .await;
    assert_eq!(
        status, 200,
        "case updated back to on demand: expected UpdateWorkflow to succeed, got {status} \
         {updated:?}"
    );
    assert!(
        trigger(&harness, &arn).is_none(),
        "case updated back to on demand: scheduling is disabled with the version that \
         asked for it"
    );
    tokio::time::sleep(SCHEDULED_RUN_EVERY * 2).await;
    let after = listed_runs(&harness, &arn, "updated back to on demand").await;
    assert_eq!(
        after.len(),
        1,
        "case updated back to on demand: the history keeps the run the trigger produced \
         and gains no other, got {after:?}"
    );

    // A delete's cleanup removes the scheduled triggers, so a workflow on its way out
    // fires no further run.
    let scheduled_again = create(&harness, "deletedSchedule", Some(TRIGGER_MODE_SCHEDULED)).await;
    assert!(
        trigger(&harness, &scheduled_again).is_some(),
        "setup: the created workflow is one the service executes"
    );
    let (status, deleted) = harness
        .call(
            "DeleteWorkflow",
            serde_json::json!({"WorkflowArn": scheduled_again}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case deleted: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );
    assert!(
        trigger(&harness, &scheduled_again).is_none(),
        "case deleted: the teardown removes the workflow's scheduled triggers"
    );
    let during_teardown = listed_runs(&harness, &scheduled_again, "deleted").await;
    assert!(
        during_teardown.is_empty(),
        "case deleted: a workflow tearing down produces no new run, got {during_teardown:?}"
    );
}

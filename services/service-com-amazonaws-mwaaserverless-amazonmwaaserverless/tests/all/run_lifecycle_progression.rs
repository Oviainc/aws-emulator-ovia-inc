// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the status lifecycle of a workflow run, and for which optional
//! members `GetWorkflowRun` reports.
//!
//! Two rules are under test, both over the real wire path (`MultiServiceRouter::dispatch`
//! over an `awsJson1_0` request).
//!
//! 1. A run walks the modelled `WorkflowRunStatus` lifecycle on the CLOCK, one state at a
//!    time, and never reports a status for work nothing ran. `StartWorkflowRun` accepts the
//!    run into `STARTING` — "This is typically STARTING when you first create the run"
//!    (`smithy_query(query="operation", operation="StartWorkflowRun")`, `Status`) — the
//!    control plane's admission of the run makes `STARTING` -> `QUEUED` due
//!    `RUN_ADMITTED_AFTER` later, and because this emulator has no Apache Airflow engine to
//!    execute a workflow definition with, nothing dequeues the run: `RUN_SETTLE_AFTER`
//!    after that it reaches the modelled failure state `FAILED` carrying the reason
//!    (`NO_AIRFLOW_ENGINE`), with `CompletedOn` and `Duration` set and no task instance.
//!    `RUNNING` and `SUCCESS` are never reported, because nothing ran. Every transition is
//!    owed by the clock and dated from the instant it fell DUE, so a read never changes
//!    what a later caller sees and a terminal status is never left.
//!
//! 2. An optional member is reported only when the stored run HOLDS a value for it: a run
//!    started with no (or an empty) `OverrideParameters` map, and a run holding no task
//!    instance, omit those members rather than answering an empty map or an empty list —
//!    the same rule `GetTaskInstance` applies to the `Xcom` of an instance that exchanged
//!    no cross-task data.
//!
//! The first suite drives the schedule from SEEDED clock fields, so every state of the
//! lifecycle (including the intermediate `QUEUED`) is asserted deterministically rather
//! than raced against the wall clock; the second suite then walks one real run with the
//! real clock and checks the order of what a polling client observes.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    WorkflowRun, NO_AIRFLOW_ENGINE, RUN_ADMITTED_AFTER, RUN_SETTLE_AFTER, RUN_STATUS_FAILED,
    RUN_STATUS_QUEUED, RUN_STATUS_STARTING,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ComAmazonawsMwaaserverlessAmazonmwaaserverlessState;

use crate::task_instance_reads::{epoch_seconds, instance, seed};
use crate::workflow_lifecycle::{role_arn, Harness};

/// The `WorkflowRunStatus` values a run of this emulator passes through, in the order the
/// clock owes them (`smithy_query(query="shape", shape="WorkflowRunStatus")` declares
/// `STARTING | QUEUED | RUNNING | SUCCESS | FAILED | TIMEOUT | STOPPING | STOPPED`; these
/// three are the ones a run nothing executed reaches).
const LIFECYCLE: &[&str] = &[RUN_STATUS_STARTING, RUN_STATUS_QUEUED, RUN_STATUS_FAILED];

/// Create a workflow and start a run under it: `(WorkflowArn, RunId)`.
async fn started_run(harness: &Harness, name: &str) -> (String, String) {
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/lifecycle.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string();
    let (status, started) = harness
        .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    assert_eq!(
        started["Status"], RUN_STATUS_STARTING,
        "setup: a just-started run carries the documented initial status, got {:?}",
        started["Status"]
    );
    let run_id = started["RunId"]
        .as_str()
        .expect("setup: a started run has a RunId")
        .to_string();
    (arn, run_id)
}

/// Edit the stored run in place. The clock fields are what the schedule is driven from, so
/// setting them is how a row puts a deadline in the past without sleeping for it.
fn with_run<R>(
    harness: &Harness,
    workflow_arn: &str,
    run_id: &str,
    edit: impl FnOnce(&mut WorkflowRun) -> R,
) -> R {
    let state = harness
        .engine()
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    let mut store = state.lock();
    let run = store
        .workflows
        .get_mut(workflow_arn)
        .expect("setup: the created workflow is stored")
        .runs
        .get_mut(run_id)
        .expect("setup: the started run is stored");
    edit(run)
}

/// Assert a timestamp member reports `expected`, or is absent when it is `None`.
fn assert_timestamp(
    detail: &serde_json::Value,
    member: &str,
    expected: Option<SystemTime>,
    case: &str,
) {
    match expected {
        Some(instant) => {
            let want = instant
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("an instant after the epoch")
                .as_secs_f64();
            let got = epoch_seconds(&detail[member]);
            assert!(
                got.is_some_and(|seconds| (seconds - want).abs() < 0.002),
                "case {case:?}: expected {member} at {want} epoch seconds, got {:?}",
                detail[member]
            );
        }
        None => assert!(
            detail[member].is_null(),
            "case {case:?}: expected {member} absent, got {:?}",
            detail[member]
        ),
    }
}

/// One row of the schedule table: the state a run is in, how far back it started, when its
/// next change falls due, and what a read must then report.
struct Row {
    case: &'static str,
    /// The `WorkflowRunStatus` the stored run is in.
    status: &'static str,
    /// `StartedOn`, this many milliseconds before the row's reference instant.
    started_millis_ago: i64,
    /// `next_status_at`, this many milliseconds after the reference instant (negative puts
    /// the deadline in the past, so the change is already owed).
    due_in_millis: i64,
    /// The `RunState` the read must report.
    expected_state: &'static str,
    /// `Duration`, in whole seconds rounded up, for a row that reaches a terminal state.
    expected_duration: Option<i32>,
    /// How long after the reference instant `CompletedOn` falls, for a terminal row.
    completed_in_millis: Option<i64>,
    /// The `ErrorMessage` the read must report.
    expected_error: Option<&'static str>,
}

/// The rows. They differ along the state the run is in, whether the next change is due,
/// how far overdue it is (so that one change or BOTH are owed at once), and the length of
/// the span `Duration` is computed over (zero, sub-second, seconds, an hour) — not merely
/// along the run's identity.
fn rows() -> Vec<Row> {
    let settle = i64::try_from(RUN_SETTLE_AFTER.as_millis()).expect("a span of a few hundred ms");
    vec![
        Row {
            case: "starting/no change due yet",
            status: RUN_STATUS_STARTING,
            started_millis_ago: 0,
            due_in_millis: 10_000,
            expected_state: RUN_STATUS_STARTING,
            expected_duration: None,
            completed_in_millis: None,
            expected_error: None,
        },
        Row {
            // ONE change is owed: admission is done, so the run is waiting for an
            // executor. The intermediate state IS observable.
            case: "starting/admission due, terminal change not",
            status: RUN_STATUS_STARTING,
            started_millis_ago: 1_000,
            due_in_millis: 0,
            expected_state: RUN_STATUS_QUEUED,
            expected_duration: None,
            completed_in_millis: None,
            expected_error: None,
        },
        Row {
            // BOTH changes are owed, so one read applies both and lands on the terminal
            // state, dated from when each fell due: 5s + RUN_SETTLE_AFTER after the start.
            case: "starting/whole schedule overdue",
            status: RUN_STATUS_STARTING,
            started_millis_ago: 10_000,
            due_in_millis: -5_000,
            expected_state: RUN_STATUS_FAILED,
            expected_duration: Some(6),
            completed_in_millis: Some(-5_000 + settle),
            expected_error: Some(NO_AIRFLOW_ENGINE),
        },
        Row {
            case: "queued/terminal change not due yet",
            status: RUN_STATUS_QUEUED,
            started_millis_ago: 0,
            due_in_millis: 10_000,
            expected_state: RUN_STATUS_QUEUED,
            expected_duration: None,
            completed_in_millis: None,
            expected_error: None,
        },
        Row {
            // A sub-second run still reports the second it occupied.
            case: "queued/terminal change due, sub-second span",
            status: RUN_STATUS_QUEUED,
            started_millis_ago: 1,
            due_in_millis: 0,
            expected_state: RUN_STATUS_FAILED,
            expected_duration: Some(1),
            completed_in_millis: Some(0),
            expected_error: Some(NO_AIRFLOW_ENGINE),
        },
        Row {
            case: "queued/terminal change due at the instant it started",
            status: RUN_STATUS_QUEUED,
            started_millis_ago: 0,
            due_in_millis: 0,
            expected_state: RUN_STATUS_FAILED,
            expected_duration: Some(0),
            completed_in_millis: Some(0),
            expected_error: Some(NO_AIRFLOW_ENGINE),
        },
        Row {
            case: "queued/terminal change due, hour-long span",
            status: RUN_STATUS_QUEUED,
            started_millis_ago: 3_600_000,
            due_in_millis: 0,
            expected_state: RUN_STATUS_FAILED,
            expected_duration: Some(3_600),
            completed_in_millis: Some(0),
            expected_error: Some(NO_AIRFLOW_ENGINE),
        },
        Row {
            // A terminal state is never left, and never rewritten: the deadline is long
            // past, and the run keeps the completion it already had.
            case: "failed/terminal state is never left",
            status: RUN_STATUS_FAILED,
            started_millis_ago: 9_000,
            due_in_millis: -10_000,
            expected_state: RUN_STATUS_FAILED,
            expected_duration: Some(7),
            completed_in_millis: Some(-1_000),
            expected_error: Some("a failure this run already recorded"),
        },
    ]
}

/// The instant `offset_millis` after `reference`.
fn shifted(reference: SystemTime, offset_millis: i64) -> SystemTime {
    if offset_millis >= 0 {
        reference + Duration::from_millis(offset_millis.unsigned_abs())
    } else {
        reference - Duration::from_millis(offset_millis.unsigned_abs())
    }
}

#[tokio::test]
async fn a_run_reports_the_state_the_clock_owes_it_and_nothing_further() {
    let harness = Harness::new("us-east-1");
    let (arn, run_id) = started_run(&harness, "runSchedule").await;

    for row in rows() {
        // The reference instant every offset in the row is measured from. `settle` dates
        // what it writes from the DUE instants, never from the instant of the read, so
        // every expectation below is an exact value rather than a window.
        let reference = SystemTime::now();
        let started_on = shifted(reference, -row.started_millis_ago);
        with_run(&harness, &arn, &run_id, |run| {
            run.status = row.status.to_string();
            run.started_on = started_on;
            run.created_at = started_on;
            run.modified_at = started_on;
            run.next_status_at = shifted(reference, row.due_in_millis);
            // A terminal row keeps the completion it already recorded; every other row
            // starts from a run that has not completed.
            if row.status == RUN_STATUS_FAILED {
                run.completed_on = row.completed_in_millis.map(|at| shifted(reference, at));
                run.duration = row.expected_duration;
                run.error_message = row.expected_error.map(str::to_string);
            } else {
                run.completed_on = None;
                run.duration = None;
                run.error_message = None;
            }
        });

        let request = serde_json::json!({"WorkflowArn": arn, "RunId": run_id});
        let (status, answered) = harness.call("GetWorkflowRun", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetWorkflowRun to succeed, got {status} {answered:?}",
            row.case
        );
        let detail = &answered["RunDetail"];
        assert_eq!(
            detail["RunState"].as_str(),
            Some(row.expected_state),
            "case {:?}: expected RunState {:?}, got {:?}",
            row.case,
            row.expected_state,
            detail["RunState"]
        );
        assert_eq!(
            detail["Duration"].as_i64(),
            row.expected_duration.map(i64::from),
            "case {:?}: expected Duration {:?}, got {:?}",
            row.case,
            row.expected_duration,
            detail["Duration"]
        );
        assert_eq!(
            detail["ErrorMessage"].as_str(),
            row.expected_error,
            "case {:?}: expected ErrorMessage {:?}, got {:?}",
            row.case,
            row.expected_error,
            detail["ErrorMessage"]
        );
        assert_timestamp(
            detail,
            "CompletedOn",
            row.completed_in_millis.map(|at| shifted(reference, at)),
            row.case,
        );
        assert!(
            detail["TaskInstances"].is_null(),
            "case {:?}: a run nothing executed holds no task instance, so the member is \
             absent, got {:?}",
            row.case,
            detail["TaskInstances"]
        );

        // A reporting operation changes nothing a later caller sees: the second read of
        // the same run answers identically.
        let (status, again) = harness.call("GetWorkflowRun", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the repeated read to succeed, got {status} {again:?}",
            row.case
        );
        assert_eq!(
            again, answered,
            "case {:?}: a read must not change what a later caller sees, got {again:?} \
             after {answered:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_real_run_is_observed_walking_the_lifecycle_in_order() {
    let harness = Harness::new("eu-west-1");
    let (arn, run_id) = started_run(&harness, "runProgression").await;
    let request = serde_json::json!({"WorkflowArn": arn, "RunId": run_id});

    // Poll the way a client waiting for completion does, and record each state as it
    // CHANGES. Nothing but the clock drives the run, so the poll neither hurries it nor
    // holds it back.
    let mut observed: Vec<String> = Vec::new();
    let mut last = serde_json::Value::Null;
    let deadline = SystemTime::now() + Duration::from_secs(10);
    while SystemTime::now() < deadline {
        let (status, answered) = harness.call("GetWorkflowRun", request.clone()).await;
        assert_eq!(
            status, 200,
            "case poll: expected GetWorkflowRun to succeed, got {status} {answered:?}"
        );
        let state = answered["RunDetail"]["RunState"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if observed.last() != Some(&state) {
            observed.push(state.clone());
        }
        last = answered;
        if state == RUN_STATUS_FAILED {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        observed.first().map(String::as_str),
        Some(RUN_STATUS_STARTING),
        "case poll: the first state a client sees is the one the start reported, got \
         {observed:?}"
    );
    assert_eq!(
        observed.last().map(String::as_str),
        Some(RUN_STATUS_FAILED),
        "case poll: a run nothing executed must reach the modelled failure state within \
         {:?}, got {observed:?}",
        RUN_ADMITTED_AFTER + RUN_SETTLE_AFTER
    );
    let positions: Vec<Option<usize>> = observed
        .iter()
        .map(|state| LIFECYCLE.iter().position(|known| known == state))
        .collect();
    assert!(
        positions.iter().all(Option::is_some),
        "case poll: every observed state belongs to the lifecycle {LIFECYCLE:?} — nothing \
         reports RUNNING or SUCCESS for work nothing ran, got {observed:?}"
    );
    let positions: Vec<usize> = positions.into_iter().flatten().collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "case poll: the states must be observed in lifecycle order and never revisited, \
         got {observed:?}"
    );

    let detail = &last["RunDetail"];
    assert_eq!(
        detail["ErrorMessage"].as_str(),
        Some(NO_AIRFLOW_ENGINE),
        "case poll: a failed run says why it failed, got {:?}",
        detail["ErrorMessage"]
    );
    assert!(
        epoch_seconds(&detail["CompletedOn"]).is_some(),
        "case poll: a complete run carries CompletedOn, got {:?}",
        detail["CompletedOn"]
    );
    assert!(
        detail["Duration"].as_i64().is_some_and(|seconds| seconds >= 0),
        "case poll: a complete run carries a Duration, got {:?}",
        detail["Duration"]
    );
    assert!(
        detail["TaskInstances"].is_null(),
        "case poll: nothing executed the definition, so the run holds no task instance, \
         got {:?}",
        detail["TaskInstances"]
    );

    // The terminal state is stable: a later read answers exactly the same.
    let (status, again) = harness.call("GetWorkflowRun", request).await;
    assert_eq!(
        status, 200,
        "case settled re-read: expected GetWorkflowRun to succeed, got {status} {again:?}"
    );
    assert_eq!(
        again["RunDetail"], last["RunDetail"],
        "case settled re-read: a settled run must not move again, got {:?}",
        again["RunDetail"]
    );
}

/// The `OverrideParameters` maps the omission table covers: a `map<string, document>`, so
/// the rows differ along the number of entries and the JSON type and nesting of the
/// values, with the two forms of "no overrides at all" (absent, and an empty map) first.
fn override_parameter_rows() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("absent", serde_json::Value::Null),
        ("an empty map", serde_json::json!({})),
        ("one string entry", serde_json::json!({"env": "test"})),
        (
            "a scalar of every JSON type",
            serde_json::json!({"retries": 3, "ratio": 0.5, "dry_run": true, "note": "mixed"}),
        ),
        (
            "a nested object and a list",
            serde_json::json!({
                "config": {"region": "us-east-1", "tags": ["a", "b"]},
                "partitions": [1, 2, 3],
            }),
        ),
    ]
}

#[tokio::test]
async fn an_optional_run_member_is_reported_only_when_the_run_holds_it() {
    let harness = Harness::new("ap-south-1");
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "optionalRunMembers",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/optional.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string();

    // The task-instance ids each row seeds. A run of this emulator produces none (no
    // Apache Airflow engine executes a definition here), so the non-empty rows seed the
    // store the reader reads — the same records `GetTaskInstance` serves.
    let seeded_tasks: &[&[&str]] = &[
        &[],
        &["extract.task-01"],
        &["c.third-task", "a.first-task", "b.second-task"],
    ];

    for (parameter_case, parameters) in override_parameter_rows() {
        for ids in seeded_tasks {
            let case = format!("{parameter_case} + {} task instance(s)", ids.len());
            let mut start = serde_json::json!({"WorkflowArn": arn});
            if !parameters.is_null() {
                start["OverrideParameters"] = parameters.clone();
            }
            let (status, started) = harness.call("StartWorkflowRun", start).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
            );
            let run_id = started["RunId"]
                .as_str()
                .expect("a started run has a RunId")
                .to_string();
            for id in *ids {
                seed(&harness, &arn, &run_id, instance(id, "SUCCESS"));
            }

            let (status, answered) = harness
                .call(
                    "GetWorkflowRun",
                    serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetWorkflowRun to succeed, got {status} {answered:?}"
            );

            let supplied = parameters
                .as_object()
                .filter(|entries| !entries.is_empty())
                .cloned();
            match supplied {
                None => assert!(
                    answered["OverrideParameters"].is_null(),
                    "case {case:?}: a run that holds no override parameters omits the \
                     member rather than answering an empty map, got {:?}",
                    answered["OverrideParameters"]
                ),
                Some(entries) => assert_eq!(
                    answered["OverrideParameters"],
                    serde_json::Value::Object(entries),
                    "case {case:?}: the stored overrides must come back exactly as \
                     supplied, got {:?}",
                    answered["OverrideParameters"]
                ),
            }

            let listed = &answered["RunDetail"]["TaskInstances"];
            if ids.is_empty() {
                assert!(
                    listed.is_null(),
                    "case {case:?}: a run holding no task instance omits TaskInstances \
                     rather than answering an empty list, got {listed:?}"
                );
            } else {
                // The store keys them in one total order, so the reported ids are sorted.
                let mut expected: Vec<&str> = ids.to_vec();
                expected.sort_unstable();
                let reported: Vec<&str> = listed
                    .as_array()
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    reported, expected,
                    "case {case:?}: TaskInstances must name the instances the run holds, \
                     got {listed:?}"
                );
            }
        }
    }
}

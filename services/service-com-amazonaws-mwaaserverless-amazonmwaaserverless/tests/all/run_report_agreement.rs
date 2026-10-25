// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule: every operation that REPORTS a stored run's status
//! reports the status the clock has already made due for it, so two reads of the same
//! stored run at the same instant never disagree.
//!
//! `StartWorkflowRun` is a reporter too whenever its `@idempotencyToken` replays a run it
//! already started — "This token prevents duplicate workflow run requests"
//! (`smithy_query(query="operation", operation="StartWorkflowRun")`, `ClientToken`) — so a
//! replay must answer the status `GetWorkflowRun` and `ListWorkflowRuns` answer for the
//! same record, not the one the run held when it was admitted.
//!
//! The first suite drives the schedule from SEEDED clock fields, so the agreement is
//! asserted over every state the clock can have moved a run into (including the
//! intermediate `QUEUED` and `STOPPING`) without racing the wall clock; the second walks a
//! real run with the real clock, which is the sequence a client that retries a timed-out
//! start performs.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    RUN_ADMITTED_AFTER, RUN_SETTLE_AFTER, RUN_STATUS_FAILED, RUN_STATUS_QUEUED,
    RUN_STATUS_STARTING, RUN_STATUS_STOPPED, RUN_STATUS_STOPPING,
};

use crate::run_lifecycle_progression::with_run;
use crate::task_instance_reads::epoch_seconds;
use crate::workflow_lifecycle::{role_arn, Harness};

/// Create a workflow and start a run under it with `token`, returning
/// `(WorkflowArn, RunId, the request the replay repeats)`.
async fn run_started_with_token(
    harness: &Harness,
    name: &str,
    token: &str,
) -> (String, String, serde_json::Value) {
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/replay.yaml"},
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

    let request = serde_json::json!({"WorkflowArn": arn, "ClientToken": token});
    let (status, started) = harness.call("StartWorkflowRun", request.clone()).await;
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
    (arn, run_id, request)
}

/// The status `ListWorkflowRuns` reports for one run of a workflow.
async fn listed_status(harness: &Harness, workflow_arn: &str, run_id: &str) -> Option<String> {
    let (status, listed) = harness
        .call(
            "ListWorkflowRuns",
            serde_json::json!({"WorkflowArn": workflow_arn}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected ListWorkflowRuns to succeed, got {status} {listed:?}"
    );
    listed["WorkflowRuns"]
        .as_array()
        .and_then(|runs| {
            runs.iter()
                .find(|run| run["RunId"].as_str() == Some(run_id))
        })
        .and_then(|run| run["RunDetailSummary"]["Status"].as_str())
        .map(str::to_string)
}

/// One row of the table: the state the stored run is held in, when its next change falls
/// due, and the status EVERY reporter must then answer for it.
struct Row {
    case: &'static str,
    /// The `WorkflowRunStatus` the stored run is in before anything reads it.
    status: &'static str,
    /// `next_status_at`, this many milliseconds after the row's reference instant — a
    /// negative value puts the deadline in the past, so the change is already owed.
    due_in_millis: i64,
    /// The status every reporter must answer.
    expected_status: &'static str,
}

/// The rows. They differ along the state the run is held in, whether its next change is
/// owed, and how overdue it is (so that NO change, ONE change or the WHOLE remaining
/// schedule is owed at the moment of the replay) — not merely along the run's identity.
/// The states are the `WorkflowRunStatus` values a run of this emulator passes through
/// (`smithy_query(query="shape", shape="WorkflowRunStatus")`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "starting/nothing owed yet",
            status: RUN_STATUS_STARTING,
            due_in_millis: 10_000,
            expected_status: RUN_STATUS_STARTING,
        },
        Row {
            case: "starting/admission owed",
            status: RUN_STATUS_STARTING,
            due_in_millis: 0,
            expected_status: RUN_STATUS_QUEUED,
        },
        Row {
            case: "starting/whole schedule overdue",
            status: RUN_STATUS_STARTING,
            due_in_millis: -5_000,
            expected_status: RUN_STATUS_FAILED,
        },
        Row {
            case: "queued/nothing owed yet",
            status: RUN_STATUS_QUEUED,
            due_in_millis: 10_000,
            expected_status: RUN_STATUS_QUEUED,
        },
        Row {
            case: "queued/terminal change owed",
            status: RUN_STATUS_QUEUED,
            due_in_millis: -1,
            expected_status: RUN_STATUS_FAILED,
        },
        Row {
            case: "stopping/shutdown not complete yet",
            status: RUN_STATUS_STOPPING,
            due_in_millis: 10_000,
            expected_status: RUN_STATUS_STOPPING,
        },
        Row {
            case: "stopping/shutdown complete",
            status: RUN_STATUS_STOPPING,
            due_in_millis: -2_000,
            expected_status: RUN_STATUS_STOPPED,
        },
        Row {
            case: "failed/terminal state is never left",
            status: RUN_STATUS_FAILED,
            due_in_millis: -10_000,
            expected_status: RUN_STATUS_FAILED,
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
async fn a_client_token_replay_reports_the_same_status_every_other_reader_does() {
    let harness = Harness::new("us-east-2");
    let token = "replay-token";
    let (arn, run_id, request) = run_started_with_token(&harness, "replayAgreement", token).await;

    for row in rows() {
        let reference = SystemTime::now();
        let started_on = reference - Duration::from_secs(30);
        with_run(&harness, &arn, &run_id, |run| {
            run.status = row.status.to_string();
            run.created_at = started_on;
            run.started_on = started_on;
            run.modified_at = started_on;
            run.next_status_at = shifted(reference, row.due_in_millis);
            // Every non-terminal row starts from a run that has not completed; the
            // terminal row keeps the completion it already recorded.
            if row.status == RUN_STATUS_FAILED {
                run.completed_on = Some(started_on);
                run.duration = Some(0);
                run.error_message = Some("a failure this run already recorded".to_string());
            } else {
                run.completed_on = None;
                run.duration = None;
                run.error_message = None;
            }
        });

        // The REPLAY reads first, so a reporter that skipped the owed transitions would
        // serve the stale status here and be contradicted by the two reads below.
        let (status, replay) = harness.call("StartWorkflowRun", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the token replay to succeed, got {status} {replay:?}",
            row.case
        );
        assert_eq!(
            replay["RunId"].as_str(),
            Some(run_id.as_str()),
            "case {:?}: a repeated ClientToken must replay the run it started, got {:?}",
            row.case,
            replay["RunId"]
        );
        assert_eq!(
            replay["Status"].as_str(),
            Some(row.expected_status),
            "case {:?}: expected the replay to report Status {:?}, got {:?}",
            row.case,
            row.expected_status,
            replay["Status"]
        );

        let (status, read) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetWorkflowRun to succeed, got {status} {read:?}",
            row.case
        );
        assert_eq!(
            read["RunDetail"]["RunState"].as_str(),
            replay["Status"].as_str(),
            "case {:?}: the replay and GetWorkflowRun must agree on the status of one \
             stored run, got {:?} after {:?}",
            row.case,
            read["RunDetail"]["RunState"],
            replay["Status"]
        );
        assert_eq!(
            listed_status(&harness, &arn, &run_id).await.as_deref(),
            replay["Status"].as_str(),
            "case {:?}: the replay and ListWorkflowRuns must agree on the status of one \
             stored run, got {:?}",
            row.case,
            replay["Status"]
        );

        // The replay reports the run, so it reports the instant the run started, not the
        // instant of the retry.
        let expected_start = epoch_seconds(&read["RunDetail"]["StartedOn"]);
        let replayed_start = epoch_seconds(&replay["StartedAt"]);
        assert!(
            expected_start.is_some()
                && replayed_start
                    .zip(expected_start)
                    .is_some_and(|(got, want)| (got - want).abs() < 0.002),
            "case {:?}: expected the replay to report StartedAt {expected_start:?}, got \
             {replayed_start:?}",
            row.case
        );

        // Replaying a token is a report, so it starts no run and moves nothing further:
        // the second replay answers exactly what the first did.
        let (status, again) = harness.call("StartWorkflowRun", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the second replay to succeed, got {status} {again:?}",
            row.case
        );
        assert_eq!(
            again, replay,
            "case {:?}: a replay must not change what a later caller sees, got {again:?} \
             after {replay:?}",
            row.case
        );
        assert_eq!(
            harness.stored_run_count(&arn),
            1,
            "case {:?}: a repeated ClientToken must not start a second run, got {} stored",
            row.case,
            harness.stored_run_count(&arn)
        );
    }
}

#[tokio::test]
async fn a_replay_after_the_schedule_elapsed_agrees_with_the_run_it_started() {
    let harness = Harness::new("eu-central-1");
    let token = "retry-after-timeout";
    let (arn, run_id, request) = run_started_with_token(&harness, "replayRealClock", token).await;

    // What a client that retries a start it never saw the answer to does: the clock has
    // meanwhile owed the run its whole remaining schedule.
    tokio::time::sleep(RUN_ADMITTED_AFTER + RUN_SETTLE_AFTER + Duration::from_millis(300)).await;

    let (status, replay) = harness.call("StartWorkflowRun", request).await;
    assert_eq!(
        status, 200,
        "case real clock: expected the token replay to succeed, got {status} {replay:?}"
    );
    assert_eq!(
        replay["Status"].as_str(),
        Some(RUN_STATUS_FAILED),
        "case real clock: a replay served after the schedule elapsed must report the \
         status the run reached unpolled, got {:?}",
        replay["Status"]
    );

    let (status, read) = harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case real clock: expected GetWorkflowRun to succeed, got {status} {read:?}"
    );
    assert_eq!(
        read["RunDetail"]["RunState"].as_str(),
        replay["Status"].as_str(),
        "case real clock: the read a moment after the replay must not contradict it, got \
         {:?} after {:?}",
        read["RunDetail"]["RunState"],
        replay["Status"]
    );
}

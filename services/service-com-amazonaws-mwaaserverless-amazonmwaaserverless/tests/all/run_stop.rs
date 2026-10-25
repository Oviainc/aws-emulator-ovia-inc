// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StopWorkflowRun`.
//!
//! The rule under test: a stop addressed to a run the endpoint holds moves that run out
//! of execution — a run that has NOT finished goes to `STOPPING` and reaches `STOPPED`
//! once the shutdown's span is up, while a run that already finished keeps the terminal
//! status, completion instant and reason it finished with — and the call reports the
//! status the STORE then holds, which every reader of that run reports too. A reference
//! that resolves to no run of this endpoint (absent workflow, absent run, a workflow of
//! another region) is the modelled `ResourceNotFoundException` and changes nothing.
//!
//! The table spans the class the rule generalises over: EVERY `WorkflowRunStatus` the
//! model declares as the run's state before the stop — the ones on their way somewhere
//! and the ones a run never leaves — plus a repeat of the stop on each, since the
//! operation is `@idempotent`. The first suite seeds those states deterministically (a
//! run of this emulator only ever reaches three of them on its own) so the status a stop
//! answers cannot race the clock; the second then walks one real run with the real clock
//! and checks what a polling client observes; the third covers the refusals.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    RUN_STATUS_FAILED, RUN_STATUS_QUEUED, RUN_STATUS_STARTING, RUN_STATUS_STOPPED,
    RUN_STATUS_STOPPING, RUN_STATUS_SUCCESS, RUN_STATUS_TIMEOUT, RUN_STOPPED_AFTER,
    RUN_TERMINAL_STATUSES,
};

use crate::missing_resource_lookups::{ghost_arns, LOOKUP_MISS_STATUS, WORKFLOW_MISSING};
use crate::run_lifecycle_progression::with_run;
use crate::task_instance_reads::epoch_seconds;
use crate::workflow_lifecycle::{role_arn, Harness};

/// Every `WorkflowRunStatus` value the model declares, in the model's own order
/// (`smithy_query(query="shape", shape="WorkflowRunStatus")`). `RUNNING` has no
/// constant in `crate::state` because nothing here assigns it — no Apache Airflow
/// engine executes a definition in this emulator — but it IS a state a stop has to
/// answer for, so the table covers it.
const RUN_STATUSES: &[&str] = &[
    RUN_STATUS_STARTING,
    RUN_STATUS_QUEUED,
    "RUNNING",
    RUN_STATUS_SUCCESS,
    RUN_STATUS_FAILED,
    RUN_STATUS_TIMEOUT,
    RUN_STATUS_STOPPING,
    RUN_STATUS_STOPPED,
];

/// The entity-level message for an absent run, as `missing_resource_lookups` records
/// it (recorded example, fixture `mwaa-serverless-resource-not-found-errors`).
const RUN_MISSING: &str = "Workflow run not found";

/// The reason a run of this emulator records when it reaches a terminal status on its
/// own. Seeded here on the already-finished rows so the test can show a stop neither
/// rewrites nor clears it.
const SEEDED_REASON: &str = "a reason this run already recorded";

/// Create a workflow and report its ARN.
async fn stored_workflow(harness: &Harness, name: &str) -> String {
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/stop.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string()
}

/// Start a run under `workflow_arn` and report its `RunId`.
async fn started_run(harness: &Harness, workflow_arn: &str) -> String {
    let (status, started) = harness
        .call(
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": workflow_arn}),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    started["RunId"]
        .as_str()
        .expect("setup: a started run has a RunId")
        .to_string()
}

/// `GetWorkflowRun` for one run, as `(status, body)`.
async fn read_run(
    harness: &Harness,
    workflow_arn: &str,
    run_id: &str,
) -> (u16, serde_json::Value) {
    harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": workflow_arn, "RunId": run_id}),
        )
        .await
}

/// `StopWorkflowRun` for one run, as `(status, body)`.
async fn stop_run(
    harness: &Harness,
    workflow_arn: &str,
    run_id: &str,
) -> (u16, serde_json::Value) {
    harness
        .call(
            "StopWorkflowRun",
            serde_json::json!({"WorkflowArn": workflow_arn, "RunId": run_id}),
        )
        .await
}

#[tokio::test]
async fn a_stop_ends_a_running_run_and_leaves_a_finished_one_as_it_finished() {
    let harness = Harness::new("us-east-1");
    let arn = stored_workflow(&harness, "stopTable").await;
    let run_id = started_run(&harness, &arn).await;
    let version = with_run(&harness, &arn, &run_id, |run| run.workflow_version.clone());

    for seeded in RUN_STATUSES {
        let terminal = RUN_TERMINAL_STATUSES.contains(seeded);
        // The status the stop must report: `STOPPING` for a run there is still
        // something to stop, and the status it already finished in otherwise.
        let expected = if terminal { *seeded } else { RUN_STATUS_STOPPING };
        // Whether THIS call is the one that starts the shutdown: a run already
        // stopping is one the stop finds nothing further to do for.
        let newly_stopped = !terminal && *seeded != RUN_STATUS_STOPPING;

        // Seed the row's state with its NEXT change far away, so the status the stop
        // answers is the one the stop itself produced rather than one the clock moved
        // mid-test. A finished row carries the completion it already recorded.
        let reference = SystemTime::now();
        let completed_on = reference - Duration::from_secs(5);
        let seeded_deadline = reference + Duration::from_secs(3_600);
        with_run(&harness, &arn, &run_id, |run| {
            run.status = (*seeded).to_string();
            run.started_on = reference - Duration::from_secs(9);
            run.created_at = reference - Duration::from_secs(9);
            run.modified_at = reference - Duration::from_secs(9);
            run.next_status_at = seeded_deadline;
            if terminal {
                run.completed_on = Some(completed_on);
                run.duration = Some(4);
                run.error_message = Some(SEEDED_REASON.to_string());
            } else {
                run.completed_on = None;
                run.duration = None;
                run.error_message = None;
            }
        });

        let (status, answered) = stop_run(&harness, &arn, &run_id).await;
        assert_eq!(
            status, 200,
            "case {seeded:?}: expected StopWorkflowRun to succeed, got {status} {answered:?}"
        );
        assert_eq!(
            answered["Status"].as_str(),
            Some(expected),
            "case {seeded:?}: expected Status {expected:?}, got {:?}",
            answered["Status"]
        );
        assert_eq!(
            answered["RunId"].as_str(),
            Some(run_id.as_str()),
            "case {seeded:?}: expected RunId {run_id:?}, got {:?}",
            answered["RunId"]
        );
        assert_eq!(
            answered["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {seeded:?}: expected WorkflowArn {arn:?}, got {:?}",
            answered["WorkflowArn"]
        );
        assert_eq!(
            answered["WorkflowVersion"].as_str(),
            Some(version.as_str()),
            "case {seeded:?}: expected the run's own WorkflowVersion {version:?}, got {:?}",
            answered["WorkflowVersion"]
        );

        // The span the shutdown occupies is never zero, so the `STOPPING` this call
        // reported is still observable: the run's next change falls in the FUTURE, and
        // no later than the shutdown's span from the call. A run that was ALREADY
        // stopping keeps the deadline it had instead, which is what makes the stop
        // idempotent rather than restarting the shutdown.
        let due = with_run(&harness, &arn, &run_id, |run| run.next_status_at);
        if newly_stopped {
            assert!(
                due > reference && due <= SystemTime::now() + RUN_STOPPED_AFTER,
                "case {seeded:?}: the stop must leave {RUN_STATUS_STOPPED} due within \
                 {RUN_STOPPED_AFTER:?} and not yet, got a deadline {:?} from the call",
                due.duration_since(reference)
            );
            // Pin the deadline out of reach so the rest of the row observes the
            // `STOPPING` phase rather than racing the shutdown's span.
            with_run(&harness, &arn, &run_id, |run| {
                run.next_status_at = seeded_deadline;
            });
        } else {
            assert_eq!(
                due, seeded_deadline,
                "case {seeded:?}: a run the stop found nothing to do for keeps the \
                 deadline it had"
            );
        }

        // The stop PERSISTED: the implemented reader of the same run reports exactly
        // the status the stop answered.
        let (status, read) = read_run(&harness, &arn, &run_id).await;
        assert_eq!(
            status, 200,
            "case {seeded:?}: expected GetWorkflowRun to succeed, got {status} {read:?}"
        );
        let detail = &read["RunDetail"];
        assert_eq!(
            detail["RunState"].as_str(),
            Some(expected),
            "case {seeded:?}: expected the stored run to report RunState {expected:?}, \
             got {:?}",
            detail["RunState"]
        );

        if terminal {
            // A finished run is left exactly as it finished.
            let want = completed_on
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("an instant after the epoch")
                .as_secs_f64();
            assert!(
                epoch_seconds(&detail["CompletedOn"])
                    .is_some_and(|seconds| (seconds - want).abs() < 0.002),
                "case {seeded:?}: a finished run keeps the CompletedOn it recorded \
                 ({want} epoch seconds), got {:?}",
                detail["CompletedOn"]
            );
            assert_eq!(
                detail["Duration"].as_i64(),
                Some(4),
                "case {seeded:?}: a finished run keeps the Duration it recorded, got {:?}",
                detail["Duration"]
            );
            assert_eq!(
                detail["ErrorMessage"].as_str(),
                Some(SEEDED_REASON),
                "case {seeded:?}: a stop must not rewrite the reason a finished run \
                 recorded, got {:?}",
                detail["ErrorMessage"]
            );
        } else {
            // The shutdown is under way, not complete: nothing is reported for a
            // completion that has not happened, and the run did not fail.
            assert!(
                detail["CompletedOn"].is_null(),
                "case {seeded:?}: a run that is still {RUN_STATUS_STOPPING} has not \
                 completed, so CompletedOn is absent, got {:?}",
                detail["CompletedOn"]
            );
            assert!(
                detail["Duration"].is_null(),
                "case {seeded:?}: a run that is still {RUN_STATUS_STOPPING} reports no \
                 Duration, got {:?}",
                detail["Duration"]
            );
            assert!(
                detail["ErrorMessage"].is_null(),
                "case {seeded:?}: a run the caller stopped did not fail, so it reports no \
                 ErrorMessage, got {:?}",
                detail["ErrorMessage"]
            );
        }

        // `@idempotent`: a repeated stop answers the same status and moves nothing —
        // not the deadline of a run already stopping, not a finished run's record.
        let (status, again) = stop_run(&harness, &arn, &run_id).await;
        assert_eq!(
            status, 200,
            "case {seeded:?}: expected the repeated stop to succeed, got {status} {again:?}"
        );
        assert_eq!(
            again["Status"].as_str(),
            Some(expected),
            "case {seeded:?}: expected the repeated stop to report {expected:?}, got {:?}",
            again["Status"]
        );
        assert_eq!(
            with_run(&harness, &arn, &run_id, |run| run.next_status_at),
            seeded_deadline,
            "case {seeded:?}: a repeated stop must not move the deadline the run holds"
        );
    }

    // No stop created or removed a run.
    assert_eq!(
        harness.stored_run_count(&arn),
        1,
        "the workflow must still hold exactly the one run that was started, got {}",
        harness.stored_run_count(&arn)
    );
}

#[tokio::test]
async fn a_real_stopped_run_is_observed_reaching_stopped_and_stays_there() {
    let harness = Harness::new("eu-west-1");
    let arn = stored_workflow(&harness, "stopProgression").await;
    let run_id = started_run(&harness, &arn).await;

    let (status, stopped) = stop_run(&harness, &arn, &run_id).await;
    assert_eq!(
        status, 200,
        "case real stop: expected StopWorkflowRun to succeed, got {status} {stopped:?}"
    );
    assert_eq!(
        stopped["Status"].as_str(),
        Some(RUN_STATUS_STOPPING),
        "case real stop: a run that had not finished enters {RUN_STATUS_STOPPING}, got {:?}",
        stopped["Status"]
    );

    // Poll the way a client waiting for the shutdown does, recording each CHANGE.
    let mut observed: Vec<String> = vec![RUN_STATUS_STOPPING.to_string()];
    let mut last = serde_json::Value::Null;
    let deadline = SystemTime::now() + Duration::from_secs(10);
    while SystemTime::now() < deadline {
        let (status, answered) = read_run(&harness, &arn, &run_id).await;
        assert_eq!(
            status, 200,
            "case real stop: expected GetWorkflowRun to succeed, got {status} {answered:?}"
        );
        let state = answered["RunDetail"]["RunState"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if observed.last() != Some(&state) {
            observed.push(state.clone());
        }
        last = answered;
        if state == RUN_STATUS_STOPPED {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        observed.last().map(String::as_str),
        Some(RUN_STATUS_STOPPED),
        "case real stop: a stopped run must reach {RUN_STATUS_STOPPED} within \
         {RUN_STOPPED_AFTER:?}, got {observed:?}"
    );
    assert_eq!(
        observed,
        vec![
            RUN_STATUS_STOPPING.to_string(),
            RUN_STATUS_STOPPED.to_string()
        ],
        "case real stop: the stop pre-empts the run's own schedule, so the only states a \
         client observes are {RUN_STATUS_STOPPING} then {RUN_STATUS_STOPPED}, got \
         {observed:?}"
    );

    let detail = &last["RunDetail"];
    assert!(
        epoch_seconds(&detail["CompletedOn"]).is_some(),
        "case real stop: a run whose shutdown is complete carries CompletedOn, got {:?}",
        detail["CompletedOn"]
    );
    assert!(
        detail["Duration"]
            .as_i64()
            .is_some_and(|seconds| seconds >= 0),
        "case real stop: a run whose shutdown is complete carries a Duration, got {:?}",
        detail["Duration"]
    );
    assert!(
        detail["ErrorMessage"].is_null(),
        "case real stop: a run the caller stopped did not fail, got {:?}",
        detail["ErrorMessage"]
    );

    // Every reader of that run reports the same status, and the terminal status is
    // stable: a later read and a later stop both answer exactly the same.
    let (status, listed) = harness
        .call(
            "ListWorkflowRuns",
            serde_json::json!({"WorkflowArn": arn, "MaxResults": 10}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case real stop: expected ListWorkflowRuns to succeed, got {status} {listed:?}"
    );
    assert_eq!(
        listed["WorkflowRuns"][0]["RunDetailSummary"]["Status"].as_str(),
        Some(RUN_STATUS_STOPPED),
        "case real stop: the listing must report the status the run holds, got {:?}",
        listed["WorkflowRuns"]
    );

    let (status, again) = read_run(&harness, &arn, &run_id).await;
    assert_eq!(
        status, 200,
        "case real stop: expected the repeated read to succeed, got {status} {again:?}"
    );
    assert_eq!(
        again["RunDetail"], last["RunDetail"],
        "case real stop: a finished run must not move again, got {:?}",
        again["RunDetail"]
    );

    let (status, restopped) = stop_run(&harness, &arn, &run_id).await;
    assert_eq!(
        status, 200,
        "case real stop: expected the stop of a finished run to succeed, got {status} \
         {restopped:?}"
    );
    assert_eq!(
        restopped["Status"].as_str(),
        Some(RUN_STATUS_STOPPED),
        "case real stop: stopping a finished run reports the status it finished in, got {:?}",
        restopped["Status"]
    );
}

#[tokio::test]
async fn a_stop_of_a_run_this_endpoint_does_not_hold_is_refused_and_changes_nothing() {
    let harness = Harness::new("us-west-2");
    let arn = stored_workflow(&harness, "stopRefusals").await;
    let run_id = started_run(&harness, &arn).await;

    // The rows: a workflow no store ever held, a stored workflow holding no such run,
    // and the stored run addressed at ANOTHER region's endpoint, which does not serve
    // the workflows created through this one. They differ in which reference is absent
    // and in the form, length and region of the identifier.
    let mut cases: Vec<(String, String, String, String)> = Vec::new();
    for ghost in ghost_arns() {
        cases.push((
            format!("absent workflow {ghost}"),
            harness.region().to_string(),
            ghost,
            run_id.clone(),
        ));
    }
    for absent in ["r", "run-0000000000000000", "a.b-c_d", "RUN.9_absent-xyz"] {
        cases.push((
            format!("absent run {absent}"),
            harness.region().to_string(),
            arn.clone(),
            absent.to_string(),
        ));
    }
    for region in ["eu-central-1", "ap-southeast-2"] {
        cases.push((
            format!("the stored run at {region}'s endpoint"),
            region.to_string(),
            arn.clone(),
            run_id.clone(),
        ));
    }

    for (case, region, workflow_arn, requested_run) in cases {
        let (status, answered) = harness
            .call_in(
                &region,
                "StopWorkflowRun",
                serde_json::json!({"WorkflowArn": workflow_arn, "RunId": requested_run}),
            )
            .await;
        assert_eq!(
            status, LOOKUP_MISS_STATUS,
            "case {case:?}: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ResourceNotFoundException")),
            "case {case:?}: expected a ResourceNotFoundException, got {:?}",
            answered["__type"]
        );
        assert_eq!(
            answered["Message"].as_str(),
            Some(RUN_MISSING),
            "case {case:?}: the stop resolves the RUN, so its refusal names that entity \
             and not {WORKFLOW_MISSING:?}, got {:?}",
            answered["Message"]
        );

        // The refusal wrote nothing: the stored run is untouched, so it is still
        // stoppable at its own endpoint.
        let state = with_run(&harness, &arn, &run_id, |run| run.status.clone());
        assert_ne!(
            state, RUN_STATUS_STOPPING,
            "case {case:?}: a refused stop must not stop the run it did not resolve, got \
             {state:?}"
        );
    }

    let (status, stopped) = stop_run(&harness, &arn, &run_id).await;
    assert_eq!(
        status, 200,
        "case owning endpoint: the run is still stoppable where it lives, got {status} \
         {stopped:?}"
    );
}

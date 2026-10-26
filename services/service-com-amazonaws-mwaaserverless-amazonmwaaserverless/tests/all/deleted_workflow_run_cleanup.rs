// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the run cleanup `DeleteWorkflow` performs.
//!
//! The rule under test: removing a workflow definition ends the executions of it that
//! are still in flight — "Amazon Managed Workflows for Apache Airflow Serverless
//! ensures that all associated resources are properly cleaned up, including stopping
//! any running executions, removing scheduled triggers, and cleaning up execution
//! history" (`smithy_query(query="doc", shape="DeleteWorkflow")`). So every run of
//! what the delete removes that has not finished enters the documented shutdown —
//! `STOPPING`, then `STOPPED` "once all cleanup is complete"
//! (`smithy_query(query="doc", shape="StopWorkflowRun")`) — instead of carrying on
//! along the schedule it had before the delete, while a run that had ALREADY finished
//! keeps the status, completion and reason it finished with, because there is no
//! execution of it left to stop.
//!
//! The scope of the cleanup is the scope of the removal: a delete that names no
//! version removes the workflow, so it covers every run of it, and a delete that takes
//! one version of several removes only that version, so it covers exactly the runs of
//! THAT version and leaves a run of a version the workflow keeps untouched.
//!
//! The tables span the class the rule generalises over: EVERY `WorkflowRunStatus` the
//! model declares as a run's state before the delete (the ones on their way somewhere
//! and the ones a run never leaves), and the version a run belongs to against the
//! version the delete takes, over workflows holding one, two and three versions. The
//! seeded suites pin the stored clock fields (the same seam `run_stop` and
//! `run_lifecycle_progression` use) so each phase is asserted exactly rather than
//! raced against the wall clock; the last suite then drives a real delete on the real
//! clock and checks the states a polling client observes.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    Workflow, NO_AIRFLOW_ENGINE, RUN_STATUS_FAILED, RUN_STATUS_QUEUED, RUN_STATUS_STARTING,
    RUN_STATUS_STOPPED, RUN_STATUS_STOPPING, RUN_STATUS_SUCCESS, RUN_STATUS_TIMEOUT,
    RUN_STOPPED_AFTER, RUN_TERMINAL_STATUSES,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ComAmazonawsMwaaserverlessAmazonmwaaserverlessState;

use crate::run_lifecycle_progression::with_run;
use crate::task_instance_reads::epoch_seconds;
use crate::workflow_lifecycle::{role_arn, Harness};

/// Every `WorkflowRunStatus` value the model declares, in the model's own order
/// (`smithy_query(query="shape", shape="WorkflowRunStatus")`). `RUNNING` has no
/// constant in `crate::state` because nothing here assigns it — no Apache Airflow
/// engine executes a definition in this emulator — but it IS a state the delete's
/// cleanup has to answer for, so the table covers it.
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

/// The reason seeded on a row whose run had already finished before the delete, so the
/// suite can show the cleanup neither rewrites nor clears it.
const SEEDED_REASON: &str = "a reason this run already recorded";

/// How far out of reach a seeded deadline is put: far enough that no change is owed
/// while the row is asserted.
const OUT_OF_REACH: Duration = Duration::from_secs(3_600);

/// Create a workflow holding `versions` immutable versions (the create makes the
/// first, each update appends another) and report its ARN.
async fn workflow_with_versions(harness: &Harness, name: &str, versions: usize) -> String {
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string();
    for index in 1..versions {
        let (status, updated) = harness
            .call(
                "UpdateWorkflow",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "DefinitionS3Location": {
                        "Bucket": "defs",
                        "ObjectKey": format!("dags/v{}.yaml", index + 1),
                    },
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup {name:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );
    }
    arn
}

/// Start a run of `workflow_arn`, on `version` when one is named and on the workflow's
/// latest otherwise, and report its `RunId`.
async fn started_run(harness: &Harness, workflow_arn: &str, version: Option<&str>) -> String {
    let mut request = serde_json::json!({"WorkflowArn": workflow_arn});
    if let Some(version) = version {
        request["WorkflowVersion"] = serde_json::Value::String(version.to_string());
    }
    let (status, started) = harness.call("StartWorkflowRun", request).await;
    assert_eq!(
        status, 200,
        "setup: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    started["RunId"]
        .as_str()
        .expect("setup: a started run has a RunId")
        .to_string()
}

/// Delete `workflow_arn`, naming `version` when one is given, and report the answered
/// body so a row can show the delete itself succeeded.
async fn delete_workflow(
    harness: &Harness,
    workflow_arn: &str,
    version: Option<&str>,
) -> serde_json::Value {
    let mut request = serde_json::json!({"WorkflowArn": workflow_arn});
    if let Some(version) = version {
        request["WorkflowVersion"] = serde_json::Value::String(version.to_string());
    }
    let (status, deleted) = harness.call("DeleteWorkflow", request).await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );
    deleted
}

/// Edit the stored workflow in place: `deletion_due_at` is the clock field the teardown
/// is driven from, so holding it out of reach is how a row asserts the `DELETING` phase
/// without racing the removal.
fn with_workflow<R>(harness: &Harness, arn: &str, edit: impl FnOnce(&mut Workflow) -> R) -> R {
    let state = harness
        .engine()
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    let mut store = state.lock();
    let workflow = store
        .workflows
        .get_mut(arn)
        .expect("setup: the workflow is stored");
    edit(workflow)
}

/// `GetWorkflowRun` for one run, as `(status, body)`.
async fn read_run(harness: &Harness, workflow_arn: &str, run_id: &str) -> (u16, serde_json::Value) {
    harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": workflow_arn, "RunId": run_id}),
        )
        .await
}

/// The `Status` `ListWorkflowRuns` reports for one run of `workflow_arn`, and `None`
/// when the listing does not hold it.
async fn listed_status(harness: &Harness, workflow_arn: &str, run_id: &str) -> Option<String> {
    let (status, listed) = harness
        .call(
            "ListWorkflowRuns",
            serde_json::json!({"WorkflowArn": workflow_arn, "MaxResults": 100}),
        )
        .await;
    assert_eq!(status, 200, "ListWorkflowRuns failed: {listed:?}");
    listed["WorkflowRuns"].as_array().and_then(|entries| {
        entries
            .iter()
            .find(|entry| entry["RunId"].as_str() == Some(run_id))
            .and_then(|entry| {
                entry["RunDetailSummary"]["Status"]
                    .as_str()
                    .map(str::to_string)
            })
    })
}

#[tokio::test]
async fn deleting_a_workflow_stops_its_unfinished_runs_whatever_state_they_were_in() {
    let harness = Harness::new("us-east-1");

    for (index, seeded) in RUN_STATUSES.iter().enumerate() {
        let terminal = RUN_TERMINAL_STATUSES.contains(seeded);
        // What the cleanup must leave the run in: the shutdown's `STOPPING` for a run
        // there is still an execution to stop, and the status it already finished in
        // otherwise.
        let expected = if terminal { *seeded } else { RUN_STATUS_STOPPING };
        // Whether THIS delete is the one that starts the shutdown: a run already
        // stopping is one it finds nothing further to do for.
        let newly_stopped = !terminal && *seeded != RUN_STATUS_STOPPING;

        let arn = workflow_with_versions(&harness, &format!("cleanup{index}"), 1).await;
        let run_id = started_run(&harness, &arn, None).await;

        // Seed the row's state with its next change far away, so the status it reports
        // after the delete is the one the delete produced rather than one the clock
        // moved mid-row. A finished row carries the completion it already recorded.
        let reference = SystemTime::now();
        let completed_on = reference - Duration::from_secs(5);
        let seeded_deadline = reference + OUT_OF_REACH;
        with_run(&harness, &arn, &run_id, |run| {
            run.status = (*seeded).to_string();
            run.created_at = reference - Duration::from_secs(9);
            run.started_on = reference - Duration::from_secs(9);
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

        delete_workflow(&harness, &arn, None).await;
        // Hold the record's removal out of reach, so what follows asserts the run
        // during the `DELETING` phase rather than after the workflow has gone.
        with_workflow(&harness, &arn, |workflow| {
            workflow.deletion_due_at = Some(SystemTime::now() + OUT_OF_REACH);
        });

        // The shutdown's span is never zero, so the `STOPPING` the cleanup produced is
        // observable: the run's next change falls in the future, and no later than the
        // shutdown's span from the delete. A run that was ALREADY stopping keeps the
        // deadline it had, and so does one that had already finished.
        let due = with_run(&harness, &arn, &run_id, |run| run.next_status_at);
        if newly_stopped {
            assert!(
                due > reference && due <= SystemTime::now() + RUN_STOPPED_AFTER,
                "case {seeded:?}: the cleanup must leave {RUN_STATUS_STOPPED} due within \
                 {RUN_STOPPED_AFTER:?} and not yet, got a deadline {:?} from the delete",
                due.duration_since(reference)
            );
            // Pin it out of reach so the rest of the row observes the `STOPPING` phase.
            with_run(&harness, &arn, &run_id, |run| {
                run.next_status_at = seeded_deadline;
            });
        } else {
            assert_eq!(
                due, seeded_deadline,
                "case {seeded:?}: a run the cleanup found no execution to stop in keeps \
                 the deadline it had"
            );
        }

        // The implemented reader of the run reports exactly that.
        let (status, read) = read_run(&harness, &arn, &run_id).await;
        assert_eq!(
            status, 200,
            "case {seeded:?}: a run of a workflow still being cleaned up is still \
             readable, got {status} {read:?}"
        );
        let detail = &read["RunDetail"];
        assert_eq!(
            detail["RunState"].as_str(),
            Some(expected),
            "case {seeded:?}: expected the stored run to report RunState {expected:?}, \
             got {:?}",
            detail["RunState"]
        );
        // The defect this pins: the delete must not leave the run being admitted or
        // failed on its pre-delete schedule.
        assert_ne!(
            detail["RunState"].as_str(),
            Some(RUN_STATUS_QUEUED),
            "case {seeded:?}: a run of a deleted workflow may not be admitted further, \
             got {:?}",
            detail["RunState"]
        );
        assert_ne!(
            detail["ErrorMessage"].as_str(),
            Some(NO_AIRFLOW_ENGINE),
            "case {seeded:?}: the cleanup stops the run, so nothing may fail it for want \
             of an execution engine, got {:?}",
            detail["ErrorMessage"]
        );
        assert_eq!(
            listed_status(&harness, &arn, &run_id).await.as_deref(),
            Some(expected),
            "case {seeded:?}: the listing must report the status the run holds"
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
                "case {seeded:?}: the cleanup must not rewrite the reason a finished run \
                 recorded, got {:?}",
                detail["ErrorMessage"]
            );
        } else {
            // The shutdown is under way, not complete, and a run the service stopped
            // did not fail.
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
                "case {seeded:?}: a run the cleanup stopped did not fail, so it reports \
                 no ErrorMessage, got {:?}",
                detail["ErrorMessage"]
            );
        }

        // Repeating the delete starts nothing further: the run keeps the shutdown it is
        // already on.
        delete_workflow(&harness, &arn, None).await;
        assert_eq!(
            with_run(&harness, &arn, &run_id, |run| (
                run.status.clone(),
                run.next_status_at
            )),
            (expected.to_string(), seeded_deadline),
            "case {seeded:?}: a repeated delete must neither restart nor re-deadline the \
             run's shutdown"
        );
        // The cleanup stops the runs; it does not drop them before the record goes.
        assert_eq!(
            harness.stored_run_count(&arn),
            1,
            "case {seeded:?}: the workflow being cleaned up still holds its run"
        );
    }
}

/// One row of the version-scoped table: how many versions the workflow holds and
/// whether the delete takes the latest one.
struct VersionCase {
    case: &'static str,
    name: &'static str,
    versions: usize,
    delete_the_latest: bool,
}

#[tokio::test]
async fn a_version_scoped_delete_stops_only_the_runs_of_the_version_it_takes() {
    // The rows differ along the version count and along whether the version the delete
    // takes is the workflow's latest, because the cleanup's scope is the SET of runs
    // belonging to what was removed, not the position of the version in the history.
    let rows = [
        VersionCase {
            case: "two versions, the older deleted",
            name: "scopedOlderOfTwo",
            versions: 2,
            delete_the_latest: false,
        },
        VersionCase {
            case: "two versions, the latest deleted",
            name: "scopedLatestOfTwo",
            versions: 2,
            delete_the_latest: true,
        },
        VersionCase {
            case: "three versions, an older one deleted",
            name: "scopedOlderOfThree",
            versions: 3,
            delete_the_latest: false,
        },
        VersionCase {
            case: "three versions, the latest deleted",
            name: "scopedLatestOfThree",
            versions: 3,
            delete_the_latest: true,
        },
    ];

    for row in rows {
        let harness = Harness::new("eu-west-1");
        let arn = workflow_with_versions(&harness, row.name, row.versions).await;
        let stored = harness.stored_versions(&arn);
        assert_eq!(
            stored.len(),
            row.versions,
            "setup {:?}: expected {} stored versions, got {stored:?}",
            row.case,
            row.versions
        );

        // One run per version, each pinned to the version it runs, and each seeded so
        // nothing is owed: what moves is then exactly what the delete moved.
        let reference = SystemTime::now();
        let seeded_deadline = reference + OUT_OF_REACH;
        let mut runs: Vec<(String, String)> = Vec::new();
        for (version, _) in &stored {
            let run_id = started_run(&harness, &arn, Some(version)).await;
            with_run(&harness, &arn, &run_id, |run| {
                run.status = RUN_STATUS_STARTING.to_string();
                run.next_status_at = seeded_deadline;
            });
            runs.push((version.clone(), run_id));
        }

        let doomed = stored
            .iter()
            .find(|(_, is_latest)| *is_latest == row.delete_the_latest)
            .map(|(version, _)| version.clone())
            .expect("setup: the workflow holds both a latest and an older version");
        delete_workflow(&harness, &arn, Some(&doomed)).await;

        for (version, run_id) in &runs {
            let removed = version == &doomed;
            let expected = if removed {
                RUN_STATUS_STOPPING
            } else {
                RUN_STATUS_STARTING
            };
            let (state, due) = with_run(&harness, &arn, run_id, |run| {
                (run.status.clone(), run.next_status_at)
            });
            assert_eq!(
                state, expected,
                "case {:?}: the run of version {version} (removed: {removed}) must report \
                 {expected}, got {state}",
                row.case
            );
            if removed {
                assert!(
                    due > reference && due <= SystemTime::now() + RUN_STOPPED_AFTER,
                    "case {:?}: the stopped run's {RUN_STATUS_STOPPED} must fall due \
                     within {RUN_STOPPED_AFTER:?}, got {:?} from the delete",
                    row.case,
                    due.duration_since(reference)
                );
            } else {
                assert_eq!(
                    due, seeded_deadline,
                    "case {:?}: a run of a version the workflow keeps is not in the \
                     cleanup's scope, so its schedule must not move",
                    row.case
                );
            }
            let (status, read) = read_run(&harness, &arn, run_id).await;
            assert_eq!(
                status, 200,
                "case {:?}: the run of version {version} must still be readable, got \
                 {status} {read:?}",
                row.case
            );
            assert_eq!(
                read["RunDetail"]["RunState"].as_str(),
                Some(expected),
                "case {:?}: the reader of the run of version {version} must report \
                 {expected}, got {:?}",
                row.case,
                read["RunDetail"]["RunState"]
            );
        }

        // Now take the whole workflow: every run of it that has not finished is in the
        // cleanup's scope, including the ones the version-scoped delete left alone.
        let after_scoped: Vec<SystemTime> = runs
            .iter()
            .map(|(_, run_id)| with_run(&harness, &arn, run_id, |run| run.next_status_at))
            .collect();
        delete_workflow(&harness, &arn, None).await;
        with_workflow(&harness, &arn, |workflow| {
            workflow.deletion_due_at = Some(SystemTime::now() + OUT_OF_REACH);
        });
        for ((version, run_id), was_due) in runs.iter().zip(after_scoped) {
            let (state, due) = with_run(&harness, &arn, run_id, |run| {
                (run.status.clone(), run.next_status_at)
            });
            assert_eq!(
                state, RUN_STATUS_STOPPING,
                "case {:?}: deleting the workflow stops the run of version {version} too, \
                 got {state}",
                row.case
            );
            if version == &doomed {
                assert_eq!(
                    due, was_due,
                    "case {:?}: a run already stopping keeps the shutdown deadline it has",
                    row.case
                );
            }
        }
    }
}

#[tokio::test]
async fn a_client_that_deletes_and_polls_a_run_observes_stopping_then_stopped() {
    let harness = Harness::new("ap-south-1");
    let arn = workflow_with_versions(&harness, "polledRunCleanup", 1).await;
    let run_id = started_run(&harness, &arn, None).await;

    delete_workflow(&harness, &arn, None).await;
    // The run's shutdown and the record's removal are two independent deadlines; hold
    // the removal out of reach so this suite watches the run's own schedule rather than
    // racing the workflow out of the store.
    with_workflow(&harness, &arn, |workflow| {
        workflow.deletion_due_at = Some(SystemTime::now() + OUT_OF_REACH);
    });

    // Poll the way a client that deleted the workflow and is waiting for its execution
    // to end does, recording each CHANGE. Nothing but the clock drives the shutdown, so
    // polling neither hurries it nor holds it back.
    let mut observed: Vec<String> = Vec::new();
    let mut last = serde_json::Value::Null;
    let deadline = SystemTime::now() + Duration::from_secs(10);
    while SystemTime::now() < deadline {
        let (status, answered) = read_run(&harness, &arn, &run_id).await;
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
        if state == RUN_STATUS_STOPPED {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        observed,
        vec![
            RUN_STATUS_STOPPING.to_string(),
            RUN_STATUS_STOPPED.to_string()
        ],
        "case poll: the delete stops the run, so a client observes {RUN_STATUS_STOPPING} \
         then {RUN_STATUS_STOPPED} within {RUN_STOPPED_AFTER:?} and nothing else; got \
         {observed:?}"
    );

    let detail = &last["RunDetail"];
    assert!(
        epoch_seconds(&detail["CompletedOn"]).is_some(),
        "case poll: a run whose shutdown is complete carries CompletedOn, got {:?}",
        detail["CompletedOn"]
    );
    assert!(
        detail["Duration"]
            .as_i64()
            .is_some_and(|seconds| seconds >= 0),
        "case poll: a run whose shutdown is complete carries a Duration, got {:?}",
        detail["Duration"]
    );
    assert!(
        detail["ErrorMessage"].is_null(),
        "case poll: a run the service stopped did not fail, so it reports no \
         ErrorMessage, got {:?}",
        detail["ErrorMessage"]
    );
    assert_eq!(
        listed_status(&harness, &arn, &run_id).await.as_deref(),
        Some(RUN_STATUS_STOPPED),
        "case poll: every reader of the run reports the status it holds"
    );

    // The terminal status is stable: a later read answers identically, and a stop of
    // the already stopped run reports the status it finished in.
    let (status, again) = read_run(&harness, &arn, &run_id).await;
    assert_eq!(
        status, 200,
        "case poll: expected the repeated read to succeed, got {status} {again:?}"
    );
    assert_eq!(
        again["RunDetail"], last["RunDetail"],
        "case poll: a finished run must not move again, got {:?}",
        again["RunDetail"]
    );
    let (status, stopped) = harness
        .call(
            "StopWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case poll: expected StopWorkflowRun to succeed, got {status} {stopped:?}"
    );
    assert_eq!(
        stopped["Status"].as_str(),
        Some(RUN_STATUS_STOPPED),
        "case poll: stopping a run the cleanup already stopped reports {RUN_STATUS_STOPPED}, \
         got {:?}",
        stopped["Status"]
    );
}

// SPDX-License-Identifier: CC0-1.0

//! A STAGE of a pipeline runs one execution at a time, and an execution is never held out
//! of the pipeline because another one is in flight.
//!
//! The rule under test: **an execution runs every stage whose turn is free and waits only
//! at the boundary of a stage another execution holds — whatever that other execution is
//! waiting for (a disabled transition, a manual approval, a job worker) — and in the
//! default `SUPERSEDED` execution mode only the most recent execution waiting at a
//! boundary enters it, the ones it caught up with reaching the terminal `Superseded`
//! status. `QUEUED` keeps every waiting execution and lets them in in the order they
//! arrived; a `PARALLEL` execution waits for no other one at all. Being let in is itself a
//! clock-owed step, so a stage keeps reporting the run that just left it until the
//! execution that waited really enters.**
//!
//! The rows differ along what the rule generalises over: what the earlier execution is
//! waiting for when the later one starts (a disabled inbound transition, which parks it at
//! a BOUNDARY, against a manual approval, which parks it INSIDE the stage), which boundary
//! that is, and the pipeline's `executionMode`.
//!
//! Every assertion goes through the router, over the wire each service speaks, so nothing
//! here can pass by reading a handler's internals.

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, status_of, wait_for_terminal,
    Emulator, WAIT,
};

/// The bytes every row's source object carries.
const SOURCE_BYTES: &[u8] = b"stage locking bytes";

/// A manual-approval stage: it holds its stage until `PutApprovalResult` answers the token
/// it publishes, which is one of the two ways a row parks the earlier execution.
fn approval_stage(name: &str) -> Value {
    json!({
        "name": name,
        "actions": [{
            "name": "Gate",
            "actionTypeId": {
                "category": "Approval", "owner": "AWS", "provider": "Manual", "version": "1"
            },
        }],
    })
}

/// Every action of one execution as `(stageName, status)`, as `ListActionExecutions`
/// reports it.
async fn actions_of(emulator: &Emulator, pipeline: &str, execution: &str) -> Vec<(String, String)> {
    let answer = emulator
        .ok(
            "ListActionExecutions",
            &json!({
                "pipelineName": pipeline,
                "filter": {"pipelineExecutionId": execution},
            }),
        )
        .await;
    answer["actionExecutionDetails"]
        .as_array()
        .map(|details| {
            details
                .iter()
                .map(|detail| {
                    (
                        detail["stageName"].as_str().unwrap_or_default().to_string(),
                        detail["status"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Poll until `read` answers `Ok`, or fail naming the case and the last state seen.
async fn settle<T, F, Fut>(case: &str, what: &str, mut read: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        match read().await {
            Ok(value) => return value,
            Err(observed) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "case {case:?}: {what} was not reached within {WAIT:?}; last observed \
                     {observed}"
                );
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    }
}

/// The approval token one stage of one pipeline is publishing, if it is publishing one.
async fn approval_token(emulator: &Emulator, pipeline: &str, stage: &str) -> Option<String> {
    let state = emulator.ok("GetPipelineState", &json!({"name": pipeline})).await;
    state["stageStates"]
        .as_array()?
        .iter()
        .find(|reported| reported["stageName"] == json!(stage))
        .and_then(|reported| reported["actionStates"][0]["latestExecution"]["token"].as_str())
        .map(str::to_string)
}

/// One row: what the earlier execution is waiting for, and the mode the pipeline runs in.
struct Row {
    /// The label every assertion of this row names.
    case: &'static str,
    /// `PipelineDeclaration$executionMode`, absent for the documented default.
    mode: Option<&'static str>,
    /// The stages after the source stage: the second one is where the earlier execution
    /// is parked.
    stages: &'static [&'static str],
    /// Park the earlier execution by disabling the inbound transition into `held`
    /// (otherwise it is parked INSIDE the approval stage, waiting for a decision).
    hold_transition: bool,
    /// The stage the earlier execution is parked at or in.
    held: &'static str,
    /// Whether the earlier execution must be superseded once the later one catches up
    /// with it at the same boundary.
    supersedes: bool,
}

/// The rows, spanning what the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "the default mode, parked at a disabled boundary into the second stage",
        mode: None,
        stages: &["Deploy", "After"],
        hold_transition: true,
        held: "Deploy",
        supersedes: true,
    },
    Row {
        case: "the default mode, parked at a disabled boundary into the third stage",
        mode: Some("SUPERSEDED"),
        stages: &["Deploy", "After"],
        hold_transition: true,
        held: "After",
        supersedes: true,
    },
    Row {
        case: "the default mode, parked inside a manual approval stage",
        mode: None,
        stages: &["Approve", "Deploy"],
        hold_transition: false,
        held: "Approve",
        // The execution that is INSIDE a stage is not waiting for the pipeline's next
        // turn, so nothing supersedes it.
        supersedes: false,
    },
    Row {
        case: "QUEUED, parked at a disabled boundary",
        mode: Some("QUEUED"),
        stages: &["Deploy", "After"],
        hold_transition: true,
        held: "Deploy",
        supersedes: false,
    },
    Row {
        case: "PARALLEL, with an approval the earlier execution is waiting in",
        mode: Some("PARALLEL"),
        stages: &["Approve", "Deploy"],
        hold_transition: false,
        held: "Approve",
        supersedes: false,
    },
];

/// The stages one row declares, in order: an S3 source stage, then an approval stage or a
/// deploy stage per declared name.
fn stages_of(row: &Row) -> Value {
    let mut stages = vec![source_stage("source-bucket", "source.zip", "SourceArtifact")];
    for name in row.stages {
        if *name == "Approve" {
            stages.push(approval_stage(name));
        } else {
            stages.push(deploy_stage(
                name,
                "SourceArtifact",
                "deploy-bucket",
                &format!("{name}.zip"),
                false,
            ));
        }
    }
    Value::Array(stages)
}

/// A boundary that HELD an execution does not let it in at the instant it is released:
/// being let in is a clock-owed step of its own, so the stage keeps reporting the run that
/// just left it — and only a span later does the execution that waited take the stage with
/// a run, and an approval token, of its own.
#[tokio::test]
async fn the_execution_that_waited_takes_the_stage_a_span_after_it_is_released() {
    let case = "a second execution waiting for the approval stage";
    let emulator = Emulator::new();
    let pipeline = "handover";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.bucket("deploy-bucket").await;
    emulator
        .put_object("source-bucket", "source.zip", SOURCE_BYTES)
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage("source-bucket", "source.zip", "SourceArtifact"),
                    approval_stage("Approve"),
                    deploy_stage("Deploy", "SourceArtifact", "deploy-bucket", "handover.zip", false),
                ]),
                None,
            ),
        )
        .await;

    // The pipeline's own run reaches the approval and holds the stage.
    let earlier_token = settle(case, "the first execution's approval token", || async {
        approval_token(&emulator, pipeline, "Approve")
            .await
            .ok_or_else(|| "no token yet".to_string())
    })
    .await;

    // A second execution runs its source stage and waits at the approval boundary.
    let later = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await["pipelineExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the start must report an id"))
        .to_string();
    settle(case, "the later execution's source stage Succeeded", || async {
        let reported = actions_of(&emulator, pipeline, &later).await;
        if reported
            .iter()
            .any(|(stage, status)| stage == "Source" && status == "Succeeded")
        {
            Ok(())
        } else {
            Err(format!("{reported:?}"))
        }
    })
    .await;

    // Answering the token settles the run that HELD the stage, and the stage reports that
    // run — the execution that was waiting has not taken it over yet.
    emulator
        .ok(
            "PutApprovalResult",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Approve",
                "actionName": "Gate",
                "token": earlier_token,
                "result": {"summary": "go", "status": "Approved"},
            }),
        )
        .await;
    let state = emulator.ok("GetPipelineState", &json!({"name": pipeline})).await;
    let approve = state["stageStates"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|reported| reported["stageName"] == json!("Approve"))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no Approve stage in {state}"));
    assert_eq!(
        approve["actionStates"][0]["latestExecution"]["status"],
        json!("Succeeded"),
        "case {case:?}: the stage reports the run that was approved until the next \
         execution really enters, got {approve}"
    );

    // A span later the execution that waited has taken the stage, with its own run and
    // its own token, and answering that one carries it to the end.
    let later_token = settle(case, "the later execution's own approval token", || async {
        match approval_token(&emulator, pipeline, "Approve").await {
            Some(token) if token != earlier_token => Ok(token),
            other => Err(format!("{other:?}")),
        }
    })
    .await;
    emulator
        .ok(
            "PutApprovalResult",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Approve",
                "actionName": "Gate",
                "token": later_token,
                "result": {"summary": "go", "status": "Approved"},
            }),
        )
        .await;
    let settled = wait_for_terminal(&emulator, pipeline, &later).await;
    assert_eq!(
        settled["pipelineExecution"]["status"],
        json!("Succeeded"),
        "case {case:?}: the execution that waited runs to the end, got {settled} with \
         actions {:?}",
        actions_of(&emulator, pipeline, &later).await
    );
}

/// A later execution runs the stages whose turn is free while an earlier one is parked,
/// and the mode decides what becomes of the earlier one.
#[tokio::test]
async fn an_execution_is_never_held_out_of_the_pipeline_by_another_one() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let pipeline = "locked";
        emulator.bucket(&artifact_bucket(pipeline)).await;
        emulator.bucket("source-bucket").await;
        emulator.bucket("deploy-bucket").await;
        emulator
            .put_object("source-bucket", "source.zip", SOURCE_BYTES)
            .await;
        if row.hold_transition {
            // Declared before the pipeline exists is impossible, so the hold is placed
            // right after the create — which is also what a caller does.
            emulator
                .ok(
                    "CreatePipeline",
                    &declaration(pipeline, stages_of(row), row.mode),
                )
                .await;
            emulator
                .ok(
                    "DisableStageTransition",
                    &json!({
                        "pipelineName": pipeline,
                        "stageName": row.held,
                        "transitionType": "Inbound",
                        "reason": "held by the suite",
                    }),
                )
                .await;
        } else {
            emulator
                .ok(
                    "CreatePipeline",
                    &declaration(pipeline, stages_of(row), row.mode),
                )
                .await;
        }

        // The execution `CreatePipeline` started of its own reaches the stage this row
        // parks it at, and stays there.
        let earlier = settle(case, "the earlier execution parked", || async {
            let listed = emulator
                .ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
                .await;
            let summary = listed["pipelineExecutionSummaries"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|summary| summary["trigger"]["triggerType"] == json!("CreatePipeline"))
                .cloned();
            let Some(summary) = summary else {
                return Err(format!("{listed}"));
            };
            let id = summary["pipelineExecutionId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let parked = if row.hold_transition {
                // Parked at the boundary: every stage before it has run.
                actions_of(&emulator, pipeline, &id)
                    .await
                    .iter()
                    .any(|(stage, status)| stage == "Source" && status == "Succeeded")
            } else {
                approval_token(&emulator, pipeline, row.held).await.is_some()
            };
            if parked {
                Ok(id)
            } else {
                Err(format!(
                    "{:?}",
                    actions_of(&emulator, pipeline, &id).await
                ))
            }
        })
        .await;

        // A second execution: it must RUN, rather than wait for the parked one to leave.
        let later = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the start must report an id"))
            .to_string();
        settle(case, "the later execution's source stage Succeeded", || async {
            let reported = actions_of(&emulator, pipeline, &later).await;
            if reported
                .iter()
                .any(|(stage, status)| stage == "Source" && status == "Succeeded")
            {
                Ok(())
            } else {
                Err(format!(
                    "actions {reported:?}, status {:?}",
                    status_of(&emulator, pipeline, &later).await
                ))
            }
        })
        .await;

        // What becomes of the earlier execution is what the mode says.
        if row.supersedes {
            settle(case, "the earlier execution Superseded", || async {
                let status = status_of(&emulator, pipeline, &earlier).await;
                if status == "Superseded" {
                    Ok(())
                } else {
                    Err(status)
                }
            })
            .await;
        } else {
            let status = status_of(&emulator, pipeline, &earlier).await;
            assert_eq!(
                status, "InProgress",
                "case {case:?}: the earlier execution must keep its place, got {status:?}"
            );
        }

        // Releasing what parked the earlier execution lets both run on, and the later
        // execution reaches a terminal status with its deliveries performed.
        if row.hold_transition {
            emulator
                .ok(
                    "EnableStageTransition",
                    &json!({
                        "pipelineName": pipeline,
                        "stageName": row.held,
                        "transitionType": "Inbound",
                    }),
                )
                .await;
        } else {
            // Each execution that reaches the approval publishes its own token, and each
            // one has to be answered.
            for _ in 0..2 {
                let Some(token) = settle(case, "an approval token to answer", || async {
                    match approval_token(&emulator, pipeline, row.held).await {
                        Some(token) => Ok(Some(token)),
                        None => {
                            // Both executions may already have been approved.
                            let status = status_of(&emulator, pipeline, &later).await;
                            if status == "Succeeded" {
                                Ok(None)
                            } else {
                                Err(status)
                            }
                        }
                    }
                })
                .await
                else {
                    break;
                };
                emulator
                    .ok(
                        "PutApprovalResult",
                        &json!({
                            "pipelineName": pipeline,
                            "stageName": row.held,
                            "actionName": "Gate",
                            "token": token,
                            "result": {"summary": "go", "status": "Approved"},
                        }),
                    )
                    .await;
            }
        }

        let settled = wait_for_terminal(&emulator, pipeline, &later).await;
        assert_eq!(
            settled["pipelineExecution"]["status"],
            json!("Succeeded"),
            "case {case:?}: the later execution must finish once the hold is released, got \
             {settled} with actions {:?}",
            actions_of(&emulator, pipeline, &later).await
        );
        for stage in row.stages {
            if *stage == "Approve" {
                continue;
            }
            let (status, bytes) = emulator
                .get_object("deploy-bucket", &format!("{stage}.zip"))
                .await;
            assert_eq!(
                (status, bytes.as_slice()),
                (200, SOURCE_BYTES),
                "case {case:?}: stage {stage} must have delivered its object"
            );
        }
    }
}


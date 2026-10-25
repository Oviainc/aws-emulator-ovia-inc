// SPDX-License-Identifier: CC0-1.0

//! `RollbackStage`, end to end over the wire the service speaks, against the execution
//! plane the rest of this crate drives.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded case: a
//! rollback of a stage starts a NEW execution of the pipeline whose plan is that stage
//! alone, run with what the target execution ran it with — so the stage's work is
//! performed again against the target's artifacts and the state that execution left is
//! restored — reported as an execution of type `ROLLBACK` carrying the target's id as
//! its rollback metadata; and a target that is not a completed run of that stage of
//! that pipeline, at the pipeline's current version, is refused with the error the
//! model declares for that state and starts nothing.
//!
//! The harness is `execution_engine`'s (`Emulator`, `declaration`, the stage builders),
//! because the execution plane needs S3 mounted beside CodePipeline; this suite reuses
//! it rather than growing a second copy.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, wait_for_terminal, Emulator, WAIT,
};

/// The source bytes the rollback target runs with — the state a rollback restores.
const TARGET_BYTES: &[u8] = b"the bytes the rollback target deployed";

/// The source bytes every execution after the target runs with.
const LATER_BYTES: &[u8] = b"the bytes deployed after the target, and then rolled back";

/// The bucket every test pipeline's source object lives in.
const SOURCE_BUCKET: &str = "source-bucket";

/// The bucket every test pipeline's deploy actions deliver to.
const DEPLOY_BUCKET: &str = "deploy-bucket";

/// The object key the source stage reads.
const SOURCE_KEY: &str = "source.zip";

/// An emulator holding one pipeline with a source stage and one deploy stage per entry
/// of `deploy_keys`, with its artifact store, its source object and its delivery bucket
/// in place.
async fn emulator_with(pipeline: &str, deploy_keys: &[&str]) -> Emulator {
    let emulator = Emulator::new();
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    emulator
        .put_object(SOURCE_BUCKET, SOURCE_KEY, TARGET_BYTES)
        .await;
    emulator
        .ok("CreatePipeline", &pipeline_declaration(pipeline, deploy_keys))
        .await;
    emulator
}

/// The declaration of a pipeline delivering the source artifact once per entry of
/// `deploy_keys`, each from its own stage `Deploy<index>`.
fn pipeline_declaration(pipeline: &str, deploy_keys: &[&str]) -> Value {
    let mut stages = vec![source_stage(SOURCE_BUCKET, SOURCE_KEY, "SourceArtifact")];
    for (index, key) in deploy_keys.iter().enumerate() {
        stages.push(deploy_stage(
            &stage_name(index),
            "SourceArtifact",
            DEPLOY_BUCKET,
            key,
            false,
        ));
    }
    declaration(pipeline, json!(stages), None)
}

/// The name of the stage that delivers `deploy_keys[index]`.
fn stage_name(index: usize) -> String {
    format!("Deploy{index}")
}

/// Start an execution of `pipeline`, wait for it to settle and answer its id with the
/// status it settled at.
async fn run(emulator: &Emulator, pipeline: &str) -> (String, String) {
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await;
    let execution = started["pipelineExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("the start reports an execution id, got {started}"))
        .to_string();
    let settled = wait_for_terminal(emulator, pipeline, &execution).await;
    let status = settled["pipelineExecution"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (execution, status)
}

/// What `GetPipelineExecution` reports for one execution.
async fn execution_of(emulator: &Emulator, pipeline: &str, execution: &str) -> Value {
    emulator
        .ok(
            "GetPipelineExecution",
            &json!({"pipelineName": pipeline, "pipelineExecutionId": execution}),
        )
        .await["pipelineExecution"]
        .clone()
}

/// The summary `ListPipelineExecutions` reports for one execution.
async fn summary_of(emulator: &Emulator, pipeline: &str, execution: &str) -> Value {
    let listed = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
        .await;
    listed["pipelineExecutionSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["pipelineExecutionId"] == json!(execution))
        .unwrap_or_else(|| panic!("the listing holds execution {execution}, got {listed}"))
}

/// The `latestExecution` `GetPipelineState` reports for one stage.
async fn latest_execution_of(emulator: &Emulator, pipeline: &str, stage: &str) -> Value {
    let state = emulator
        .ok("GetPipelineState", &json!({"name": pipeline}))
        .await;
    state["stageStates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|reported| reported["stageName"] == json!(stage))
        .map(|reported| reported["latestExecution"].clone())
        .unwrap_or_else(|| panic!("the state reports stage {stage}, got {state}"))
}

/// One row of [`a_rollback_re_runs_the_named_stage_against_the_targets_artifacts`].
struct RollbackRow {
    case: &'static str,
    /// The object each of the pipeline's deploy stages delivers.
    deploy_keys: &'static [&'static str],
    /// Which of those stages is rolled back.
    rolled_back: usize,
    /// How many standard executions run between the rollback target and the rollback.
    intervening: usize,
}

/// A rollback re-runs the named stage against the target execution's artifacts: the
/// state that execution left is restored, no other stage runs again, and the rollback is
/// reported as its own `ROLLBACK` execution naming the target it rolled back to.
#[tokio::test]
async fn a_rollback_re_runs_the_named_stage_against_the_targets_artifacts() {
    let rows: Vec<RollbackRow> = vec![
        RollbackRow {
            case: "the only stage, one execution later",
            deploy_keys: &["app.zip"],
            rolled_back: 0,
            intervening: 1,
        },
        RollbackRow {
            case: "the first of two stages",
            deploy_keys: &["first.zip", "second.zip"],
            rolled_back: 0,
            intervening: 1,
        },
        RollbackRow {
            case: "the last of two stages",
            deploy_keys: &["first.zip", "second.zip"],
            rolled_back: 1,
            intervening: 1,
        },
        RollbackRow {
            case: "two executions after the target",
            deploy_keys: &["app.zip"],
            rolled_back: 0,
            intervening: 2,
        },
    ];

    for RollbackRow {
        case,
        deploy_keys,
        rolled_back,
        intervening,
    } in rows
    {
        let pipeline = format!("rollback-{}", case.replace([' ', ','], "-"));
        let emulator = emulator_with(&pipeline, deploy_keys).await;

        // The execution the rollback will target: it delivers `TARGET_BYTES`.
        let (target, status) = run(&emulator, &pipeline).await;
        assert_eq!(
            status, "Succeeded",
            "case {case:?}: the target execution must deliver, got {status}"
        );

        // Then the pipeline runs on with different source bytes, overwriting every
        // delivery the target made.
        emulator
            .put_object(SOURCE_BUCKET, SOURCE_KEY, LATER_BYTES)
            .await;
        let mut later = Vec::new();
        for _ in 0..intervening {
            let (execution, status) = run(&emulator, &pipeline).await;
            assert_eq!(
                status, "Succeeded",
                "case {case:?}: execution {execution} must deliver, got {status}"
            );
            later.push(execution);
        }
        for key in deploy_keys {
            let (status, bytes) = emulator.get_object(DEPLOY_BUCKET, key).await;
            assert_eq!(
                (status, bytes.as_slice()),
                (200, LATER_BYTES),
                "case {case:?}: {key} carries the later bytes before the rollback"
            );
        }

        let stage = stage_name(rolled_back);
        let rolled = emulator
            .ok(
                "RollbackStage",
                &json!({
                    "pipelineName": pipeline,
                    "stageName": stage,
                    "targetPipelineExecutionId": target,
                }),
            )
            .await;
        let rollback = rolled["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the rollback reports an id, got {rolled}"))
            .to_string();
        assert!(
            rollback != target && !later.contains(&rollback),
            "case {case:?}: a rollback is its own execution, got {rollback:?} \
             against target {target:?} and {later:?}"
        );

        // The rollback is an execution of the pipeline in its own right, typed ROLLBACK
        // and naming the execution it rolls back to.
        let reported = execution_of(&emulator, &pipeline, &rollback).await;
        assert_eq!(
            (
                reported["executionType"].clone(),
                reported["rollbackMetadata"].clone(),
                reported["trigger"]["triggerType"].clone()
            ),
            (
                json!("ROLLBACK"),
                json!({"rollbackTargetPipelineExecutionId": target}),
                json!("ManualRollback")
            ),
            "case {case:?}: unexpected rollback execution, got {reported}"
        );
        let listed = summary_of(&emulator, &pipeline, &rollback).await;
        assert_eq!(
            (
                listed["executionType"].clone(),
                listed["rollbackMetadata"].clone()
            ),
            (
                json!("ROLLBACK"),
                json!({"rollbackTargetPipelineExecutionId": target})
            ),
            "case {case:?}: both readers must report the rollback the same way, got {listed}"
        );

        let settled = wait_for_terminal(&emulator, &pipeline, &rollback).await;
        assert_eq!(
            settled["pipelineExecution"]["status"],
            json!("Succeeded"),
            "case {case:?}: the rollback runs the stage again, got {settled}"
        );

        // The rolled-back stage's delivery is the target execution's again, and no
        // other stage ran: every other delivery is still the later one.
        for (index, key) in deploy_keys.iter().enumerate() {
            let expected: &[u8] = if index == rolled_back {
                TARGET_BYTES
            } else {
                LATER_BYTES
            };
            let (status, bytes) = emulator.get_object(DEPLOY_BUCKET, key).await;
            assert_eq!(
                (status, bytes.as_slice()),
                (200, expected),
                "case {case:?}: unexpected bytes delivered to {key} after rolling back \
                 stage {stage}"
            );
        }

        // The stage that was rolled back reports the rollback as the execution that
        // last entered it; the source stage, which the rollback never ran, still
        // reports the last standard execution.
        let newest = later.last().cloned().unwrap_or_else(|| target.clone());
        assert_eq!(
            latest_execution_of(&emulator, &pipeline, &stage).await,
            json!({"pipelineExecutionId": rollback, "status": "Succeeded", "type": "ROLLBACK"}),
            "case {case:?}: stage {stage} was last entered by the rollback"
        );
        assert_eq!(
            latest_execution_of(&emulator, &pipeline, "Source").await,
            json!({"pipelineExecutionId": newest, "status": "Succeeded", "type": "STANDARD"}),
            "case {case:?}: the rollback does not re-run the source stage"
        );

        // The target execution is untouched by having been rolled back to: it is still
        // the standard execution it was, with no rollback metadata of its own.
        let target_reported = execution_of(&emulator, &pipeline, &target).await;
        assert_eq!(
            (
                target_reported["status"].clone(),
                target_reported["executionType"].clone(),
                target_reported["rollbackMetadata"].clone()
            ),
            (json!("Succeeded"), json!("STANDARD"), Value::Null),
            "case {case:?}: the target must be unchanged, got {target_reported}"
        );
    }
}

/// A rollback naming something that is not a completed run of that stage of that
/// pipeline is refused with the error the model declares for it, and starts nothing.
#[tokio::test]
async fn a_rollback_naming_what_cannot_be_rolled_back_is_refused() {
    let pipeline = "refused-rollback";
    let emulator = emulator_with(pipeline, &["app.zip"]).await;
    let (target, status) = run(&emulator, pipeline).await;
    assert_eq!(status, "Succeeded", "the target execution must deliver");

    // A second pipeline of the same emulator, so one pipeline's execution id can be
    // offered to the other.
    emulator.bucket(&artifact_bucket("neighbour")).await;
    emulator
        .ok("CreatePipeline", &pipeline_declaration("neighbour", &["n.zip"]))
        .await;
    let (neighbour, _) = run(&emulator, "neighbour").await;

    // A third pipeline whose source object is not there, so its execution FAILS in the
    // source stage and never enters the deploy stage at all.
    emulator.bucket(&artifact_bucket("never-reached")).await;
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": {
                "name": "never-reached",
                "roleArn": "arn:aws:iam::123456789012:role/service-role/cp",
                "artifactStore": {"type": "S3", "location": artifact_bucket("never-reached")},
                "stages": [
                    source_stage(SOURCE_BUCKET, "absent.zip", "SourceArtifact"),
                    deploy_stage("Deploy0", "SourceArtifact", DEPLOY_BUCKET, "never.zip", false),
                ],
            }}),
        )
        .await;
    let (never_reached, status) = run(&emulator, "never-reached").await;
    assert_eq!(
        status, "Failed",
        "the execution of a pipeline whose source is absent fails"
    );

    // (case, pipelineName, stageName, targetPipelineExecutionId, expected error code)
    let rows: Vec<(&str, &str, &str, String, &str)> = vec![
        (
            "a pipeline the store does not hold",
            "missing",
            "Deploy0",
            target.clone(),
            "PipelineNotFoundException",
        ),
        (
            "a stage the pipeline does not declare",
            pipeline,
            "Deploy7",
            target.clone(),
            "StageNotFoundException",
        ),
        (
            "an execution id no pipeline minted",
            pipeline,
            "Deploy0",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "PipelineExecutionNotFoundException",
        ),
        (
            "an execution of another pipeline",
            pipeline,
            "Deploy0",
            neighbour.clone(),
            "PipelineExecutionNotFoundException",
        ),
        (
            "an execution that never entered the stage",
            "never-reached",
            "Deploy0",
            never_reached.clone(),
            "UnableToRollbackStageException",
        ),
        (
            // `PipelineExecutionId` is modelled
            // `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`, so the
            // boundary refuses this before any handler runs.
            "an id the modelled pattern rejects",
            pipeline,
            "Deploy0",
            "not-a-uuid".to_string(),
            "ValidationException",
        ),
        (
            // The same pattern: its hex digits are lowercase.
            "an id with uppercase hex",
            pipeline,
            "Deploy0",
            target.to_uppercase(),
            "ValidationException",
        ),
    ];

    for (case, pipeline_name, stage_name, target_id, expected) in rows {
        let (status, code) = emulator
            .error_code(
                "RollbackStage",
                &json!({
                    "pipelineName": pipeline_name,
                    "stageName": stage_name,
                    "targetPipelineExecutionId": target_id,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: unexpected refusal"
        );
    }

    // Nothing was started by any of the refusals: the pipeline still holds exactly the
    // one execution it ran.
    let listed = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
        .await;
    assert_eq!(
        listed["pipelineExecutionSummaries"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "a refused rollback starts no execution, got {listed}"
    );

    // And the rollback the store CAN serve is still servable after them.
    let rolled = emulator
        .ok(
            "RollbackStage",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Deploy0",
                "targetPipelineExecutionId": target,
            }),
        )
        .await;
    assert!(
        rolled["pipelineExecutionId"].as_str().is_some_and(|id| id != target),
        "the refusals left the stage rollable, got {rolled}"
    );
}

/// A target execution that ran an EARLIER version of the pipeline cannot be rolled back
/// to: the stage it ran is not the stage the declaration now describes.
#[tokio::test]
async fn a_target_of_an_earlier_pipeline_version_is_refused() {
    let pipeline = "outdated-target";
    let emulator = emulator_with(pipeline, &["before.zip"]).await;
    let (target, status) = run(&emulator, pipeline).await;
    assert_eq!(status, "Succeeded", "the target execution must deliver");

    // The declaration changes, so the pipeline moves to its next version while the
    // execution stays pinned to the one it ran.
    emulator
        .ok("UpdatePipeline", &pipeline_declaration(pipeline, &["after.zip"]))
        .await;

    let (status, code) = emulator
        .error_code(
            "RollbackStage",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Deploy0",
                "targetPipelineExecutionId": target,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "PipelineExecutionOutdatedException"),
        "an execution of an earlier pipeline version is not a rollback target"
    );

    // An execution of the CURRENT version is one, and rolling back to it re-runs the
    // stage as that version declares it.
    let (current, status) = run(&emulator, pipeline).await;
    assert_eq!(status, "Succeeded", "the new version's execution delivers");
    let rolled = emulator
        .ok(
            "RollbackStage",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Deploy0",
                "targetPipelineExecutionId": current,
            }),
        )
        .await;
    let rollback = rolled["pipelineExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("the rollback reports an id, got {rolled}"))
        .to_string();
    let settled = wait_for_terminal(&emulator, pipeline, &rollback).await;
    assert_eq!(
        settled["pipelineExecution"]["status"],
        json!("Succeeded"),
        "the rollback of the current version's execution runs, got {settled}"
    );
    assert_eq!(
        settled["pipelineExecution"]["pipelineVersion"],
        json!(2),
        "a rollback runs the pipeline version it was asked for, got {settled}"
    );
}

/// A stage an execution is still running cannot be rolled back under it; once that run
/// has finished, the same request is served.
#[tokio::test]
async fn a_rollback_of_a_stage_that_is_still_running_is_refused() {
    let pipeline = "running-stage";
    let emulator = Emulator::new();
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    emulator
        .put_object(SOURCE_BUCKET, SOURCE_KEY, TARGET_BYTES)
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, "SourceArtifact"),
                    {
                        "name": "Gate",
                        "actions": [{
                            "name": "Approve",
                            "actionTypeId": {
                                "category": "Approval",
                                "owner": "AWS",
                                "provider": "Manual",
                                "version": "1",
                            },
                            "configuration": {"CustomData": "ship it?"},
                        }],
                    },
                ]),
                None,
            ),
        )
        .await;
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await;
    let execution = started["pipelineExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("the start reports an execution id, got {started}"))
        .to_string();

    // The approval holds the stage, so the stage is running while the token waits.
    let token = approval_token(&emulator, pipeline).await;
    let (status, code) = emulator
        .error_code(
            "RollbackStage",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gate",
                "targetPipelineExecutionId": execution,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "UnableToRollbackStageException"),
        "a stage that is still running cannot be rolled back"
    );

    emulator
        .ok(
            "PutApprovalResult",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gate",
                "actionName": "Approve",
                "result": {"summary": "looks good", "status": "Approved"},
                "token": token,
            }),
        )
        .await;
    let settled = wait_for_terminal(&emulator, pipeline, &execution).await;
    assert_eq!(
        settled["pipelineExecution"]["status"],
        json!("Succeeded"),
        "the approved execution finishes the stage, got {settled}"
    );

    // Now the stage HAS a completed run, so the same request is served and the rollback
    // runs the stage again — which puts the approval back in front of a new token.
    let rolled = emulator
        .ok(
            "RollbackStage",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Gate",
                "targetPipelineExecutionId": execution,
            }),
        )
        .await;
    let rollback = rolled["pipelineExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("the rollback reports an id, got {rolled}"))
        .to_string();
    assert_ne!(
        rollback, execution,
        "a rollback is its own execution, got the target's id back"
    );
    let fresh = approval_token(&emulator, pipeline).await;
    assert_ne!(
        fresh, token,
        "the rolled-back stage runs its approval again, under a new token"
    );
    assert_eq!(
        latest_execution_of(&emulator, pipeline, "Gate").await,
        json!({"pipelineExecutionId": rollback, "status": "InProgress", "type": "ROLLBACK"}),
        "the rolled-back stage is being run by the rollback execution"
    );
}

/// The token the `Gate` stage's approval action publishes once the stage reaches it.
async fn approval_token(emulator: &Emulator, pipeline: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let state = emulator
            .ok("GetPipelineState", &json!({"name": pipeline}))
            .await;
        let token = state["stageStates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|stage| stage["actionStates"].as_array())
            .flatten()
            .find(|action| action["actionName"] == json!("Approve"))
            .and_then(|action| action["latestExecution"]["token"].as_str())
            .map(str::to_string);
        if let Some(token) = token {
            return token;
        }
        assert!(
            Instant::now() < deadline,
            "the approval action must publish a token, last state was {state}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The concurrent-execution SLOTS a pipeline occupies, probed across the clock: which
//! executions hold one, and when it is given back.
//!
//! The rule under test: an execution occupies one of its pipeline's slots from the
//! moment it is recorded until it reaches a terminal `PipelineExecutionStatus`, so what
//! decides whether a further trigger is admitted is the set of executions that have not
//! completed — and nothing else. The rows span what that rule generalises over: whether
//! the execution's actions are work the engine performs (so the clock carries the
//! execution to a terminal status and the slot comes back) or work that waits for a
//! reporter (so the slot stays taken), and whether the trigger is a revision report or
//! a start.
//!
//! The suite exists because the two sides of that rule are what make the modelled
//! `ConcurrentPipelineExecutionsLimitExceededException` reachable at all: a pipeline
//! whose executions complete is never at its ceiling however many triggers it is sent,
//! while one whose executions wait reaches it at the bound and not before.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so the engine reaches the
//! emulator's own S3 service the way the production emulator does, and every assertion
//! goes through the router.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_CONCURRENT_EXECUTIONS_PER_PIPELINE;

use crate::execution_engine::Emulator;
use crate::pipeline_lifecycle::ROLE_ARN;

/// The sentence the service states this condition with, written out here so a reword
/// of the handler's wording fails this suite: it is the text the model documents
/// `ConcurrentPipelineExecutionsLimitExceededException` as, and it carries no variable
/// part.
const AT_THE_LIMIT: &str = "The pipeline has reached the limit for concurrent pipeline executions.";

/// Long enough for every step one of these one-action executions is owed to have come
/// due (`state::execution::ACTION_STEP` is the span each step waits), so a status that
/// has not moved after it has not moved for want of time.
const BEYOND_THE_STEPS: Duration = Duration::from_millis(1_500);

/// The `PipelineExecutionStatus` values an execution can still leave — the ones that
/// hold a slot.
const NON_TERMINAL: [&str; 2] = ["InProgress", "Stopping"];

/// A pipeline whose source action is `(owner, provider)`, in `PARALLEL` mode so that
/// every admitted execution runs on its own rather than waiting for the one before it.
fn pipeline(name: &str, owner: &str, provider: &str) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "pipelineType": "V2",
        "executionMode": "PARALLEL",
        "artifactStore": {"type": "S3", "location": "artifacts"},
        "stages": [
            {
                "name": "Source",
                "actions": [{
                    "name": "Src",
                    "actionTypeId": {
                        "category": "Source",
                        "owner": owner,
                        "provider": provider,
                        "version": "1",
                    },
                    "configuration": {"S3Bucket": "sources", "S3ObjectKey": "app.zip"},
                    "outputArtifacts": [{"name": "SourceOut"}],
                }],
            },
            {
                "name": "Approve",
                "actions": [{
                    "name": "Manual",
                    "actionTypeId": {
                        "category": "Approval",
                        "owner": "AWS",
                        "provider": "Manual",
                        "version": "1",
                    },
                }],
            },
        ],
    })
}

/// Report a new revision for the pipeline's source action.
async fn put_revision(emulator: &Emulator, pipeline: &str, revision: &str) -> (u16, Value) {
    emulator
        .cp(
            "PutActionRevision",
            &json!({
                "pipelineName": pipeline,
                "stageName": "Source",
                "actionName": "Src",
                "actionRevision": {
                    "revisionId": revision,
                    "revisionChangeId": format!("change-{revision}"),
                    "created": 1_760_000_000_i64,
                },
            }),
        )
        .await
}

/// The statuses the pipeline's retained executions carry, as
/// `ListPipelineExecutions` reports them.
async fn statuses(emulator: &Emulator, pipeline: &str) -> Vec<String> {
    let listed = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
        .await;
    listed["pipelineExecutionSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["status"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: the slot comes back when the execution reaches a terminal status, so a
/// pipeline whose executions the clock carries to one is admitted a further trigger
/// however many it has already been sent.
#[tokio::test]
async fn a_completed_execution_gives_its_slot_back() {
    let emulator = Emulator::new();
    // An `AWS`-owned S3 source action: work the engine performs itself, so the clock
    // carries each execution to a terminal status (here `Failed`, because no source
    // object was ever put where the declaration points — nothing is reported as having
    // run when nothing ran).
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": pipeline("served", "AWS", "S3")}),
        )
        .await;

    // More rounds than the pipeline has slots would be refused if a completed
    // execution kept its slot; each round waits for the steps the clock owes.
    for round in 0..3 {
        let (status, answered) = put_revision(&emulator, "served", &format!("rev-{round}")).await;
        assert_eq!(
            (status, answered["newRevision"].clone()),
            (200, json!(true)),
            "round {round}: a new revision of a pipeline with a free slot is admitted, \
             got {status} with {answered}"
        );
        tokio::time::sleep(BEYOND_THE_STEPS).await;
        let statuses = statuses(&emulator, "served").await;
        let in_flight = statuses
            .iter()
            .filter(|status| NON_TERMINAL.contains(&status.as_str()))
            .count();
        assert_eq!(
            in_flight, 0,
            "round {round}: every execution the clock has carried to a terminal status \
             has given its slot back, got {statuses:?}"
        );
    }
}

/// RULE: an execution whose action WAITS holds its slot, so the triggers fill the
/// pipeline's slots and the one past the bound is refused with the error the model
/// declares for exactly that condition — whichever call sent it.
#[tokio::test]
async fn executions_that_wait_fill_the_pipelines_slots() {
    let emulator = Emulator::new();
    // A `Custom`-owned source action: its work is a job this account's own worker
    // reports the outcome of, so the execution stays non-terminal until one does.
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": pipeline("waiting", "Custom", "MySource")}),
        )
        .await;

    for index in 0..MAX_CONCURRENT_EXECUTIONS_PER_PIPELINE {
        let (status, answered) = put_revision(&emulator, "waiting", &format!("rev-{index}")).await;
        assert_eq!(
            status, 200,
            "revision {index} is inside the bound of {MAX_CONCURRENT_EXECUTIONS_PER_PIPELINE}, \
             got {status} with {answered}"
        );
    }
    // The clock changes nothing here: the slots are held by executions waiting for a
    // report, not by steps that have not come due.
    tokio::time::sleep(BEYOND_THE_STEPS).await;
    let waiting = statuses(&emulator, "waiting").await;
    let in_flight = waiting
        .iter()
        .filter(|status| NON_TERMINAL.contains(&status.as_str()))
        .count();
    assert_eq!(
        in_flight, MAX_CONCURRENT_EXECUTIONS_PER_PIPELINE,
        "every execution waiting for its job worker holds a slot, got {waiting:?}"
    );

    // One past the bound, through either call that starts an execution: the same
    // accounting and the same refusal answer both.
    let (status, response) = put_revision(&emulator, "waiting", "one-too-many").await;
    assert_eq!(
        (
            status,
            response["__type"].as_str(),
            response["message"].as_str()
        ),
        (
            400,
            Some("ConcurrentPipelineExecutionsLimitExceededException"),
            Some(AT_THE_LIMIT)
        ),
        "a revision report past the bound is refused, got {status} with {response}"
    );
    let (status, response) = emulator
        .cp("StartPipelineExecution", &json!({"name": "waiting"}))
        .await;
    assert_eq!(
        (
            status,
            response["__type"].as_str(),
            response["message"].as_str()
        ),
        (
            400,
            Some("ConcurrentPipelineExecutionsLimitExceededException"),
            Some(AT_THE_LIMIT)
        ),
        "a start past the same bound is refused the same way, got {status} with {response}"
    );

    // And the refusals started nothing: the pipeline still holds exactly its bound.
    let retained = statuses(&emulator, "waiting").await;
    assert_eq!(
        retained.len(),
        MAX_CONCURRENT_EXECUTIONS_PER_PIPELINE,
        "a refused trigger records no execution, got {retained:?}"
    );
}

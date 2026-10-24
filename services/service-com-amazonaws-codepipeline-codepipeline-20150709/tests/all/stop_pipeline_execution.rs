// SPDX-License-Identifier: CC0-1.0

//! `StopPipelineExecution`, end to end over the wire the service speaks, against the
//! execution plane the rest of this crate drives.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! case:
//!
//! * a stop accepted for an execution that has not reached a terminal status reports
//!   THAT execution's id and settles it at `Stopped`, whichever way it was asked to
//!   stop, and nothing the execution had left to do is performed afterwards;
//! * the caller's `reason` is persisted and reported back by every reader that models
//!   it — `PipelineExecutionSummary$stopTrigger` and the execution's `statusSummary` —
//!   and a reason the caller did not give is not invented;
//! * an execution that can no longer be stopped, or an id that names no execution of
//!   the named pipeline, is refused with the error the model declares for that state,
//!   and the refusal changes nothing;
//! * an execution waiting behind another one is stoppable on its own, without
//!   disturbing the execution that is running.
//!
//! The harness is `execution_engine`'s (`Emulator`, `declaration`, the stage builders),
//! because the execution plane needs S3 mounted beside CodePipeline; this suite reuses
//! it rather than growing a second copy.

use std::time::Duration;

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, status_of, wait_for_terminal,
    Emulator,
};

/// The bytes the source action of these pipelines reads.
const SOURCE_BYTES: &[u8] = b"bytes of a run that gets stopped";

/// Long enough that any step the engine still had to take would have become due
/// (`state::execution::ACTION_STEP` is 300 ms), so a status that has not moved after it
/// is one nothing is going to move.
const BEYOND_A_STEP: Duration = Duration::from_millis(900);

/// An emulator holding one pipeline, with its artifact store, its source object and its
/// delivery bucket in place: `deploy_keys` names one delivery per deploy stage.
async fn emulator_with(pipeline: &str, deploy_keys: &[&str], execution_mode: Option<&str>) -> Emulator {
    let emulator = Emulator::new();
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket("source-bucket").await;
    emulator.bucket("deploy-bucket").await;
    emulator
        .put_object("source-bucket", "source.zip", SOURCE_BYTES)
        .await;

    let mut stages = vec![source_stage("source-bucket", "source.zip", "SourceArtifact")];
    for (index, key) in deploy_keys.iter().enumerate() {
        stages.push(deploy_stage(
            &format!("Deploy{index}"),
            "SourceArtifact",
            "deploy-bucket",
            key,
            false,
        ));
    }
    emulator
        .ok(
            "CreatePipeline",
            &declaration(pipeline, json!(stages), execution_mode),
        )
        .await;
    emulator
}

/// Start an execution of `pipeline` and answer its id.
async fn start(emulator: &Emulator, pipeline: &str) -> String {
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await;
    started["pipelineExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("the start reports an execution id, got {started}"))
        .to_string()
}

/// The summary `ListPipelineExecutions` reports for `execution`.
async fn summary(emulator: &Emulator, pipeline: &str, execution: &str) -> Value {
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

/// One row of [`a_stop_settles_the_execution_and_nothing_runs_after_it`]'s table.
struct StopRow {
    case: &'static str,
    /// `StopPipelineExecutionInput$abandon`: abandon the in-progress actions, or let
    /// them finish first.
    abandon: bool,
    /// The deliveries the pipeline's deploy stages would have made.
    deploy_keys: &'static [&'static str],
    /// `PipelineDeclaration$executionMode`, absent for the documented default.
    execution_mode: Option<&'static str>,
}

/// A stop reports the execution it stopped, settles that execution at `Stopped`, and
/// nothing the execution had left to do happens afterwards — whichever way the caller
/// asked for the stop, and however many stages were still ahead of it.
#[tokio::test]
async fn a_stop_settles_the_execution_and_nothing_runs_after_it() {
    let rows: Vec<StopRow> = vec![
        StopRow {
            case: "abandon the in-progress actions",
            abandon: true,
            deploy_keys: &["abandoned.zip"],
            execution_mode: None,
        },
        StopRow {
            case: "let the in-progress actions finish",
            abandon: false,
            deploy_keys: &["waited.zip"],
            execution_mode: None,
        },
        StopRow {
            case: "abandon, with two stages still ahead",
            abandon: true,
            deploy_keys: &["abandoned-one.zip", "abandoned-two.zip"],
            execution_mode: Some("PARALLEL"),
        },
        StopRow {
            case: "wait, with two stages still ahead",
            abandon: false,
            deploy_keys: &["waited-one.zip", "waited-two.zip"],
            execution_mode: Some("QUEUED"),
        },
    ];

    for StopRow {
        case,
        abandon,
        deploy_keys,
        execution_mode,
    } in rows
    {
        let pipeline = format!("stop-{}", case.replace([' ', ','], "-"));
        let emulator = emulator_with(&pipeline, deploy_keys, execution_mode).await;
        let execution = start(&emulator, &pipeline).await;

        let stopped = emulator
            .ok(
                "StopPipelineExecution",
                &json!({
                    "pipelineName": pipeline,
                    "pipelineExecutionId": execution,
                    "abandon": abandon,
                }),
            )
            .await;
        assert_eq!(
            stopped["pipelineExecutionId"],
            json!(execution),
            "case {case:?}: the stop reports the execution it stopped, got {stopped}"
        );

        let settled = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            settled["pipelineExecution"]["status"],
            json!("Stopped"),
            "case {case:?}: a stopped execution settles at Stopped, got {settled}"
        );
        assert_eq!(
            summary(&emulator, &pipeline, &execution).await["status"],
            json!("Stopped"),
            "case {case:?}: the listing must report the same status as the reader"
        );

        // Nothing runs after the stop, however long the clock runs on, so none of the
        // deliveries the execution still had ahead of it is made.
        tokio::time::sleep(BEYOND_A_STEP).await;
        assert_eq!(
            status_of(&emulator, &pipeline, &execution).await,
            "Stopped",
            "case {case:?}: a stopped execution stays stopped"
        );
        for key in deploy_keys {
            let (status, _) = emulator.get_object("deploy-bucket", key).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a stopped execution delivers nothing, but {key} is there"
            );
        }
    }
}

/// The caller's `reason` is persisted with the stop and reported back by every reader
/// that models it; a reason the caller did not give is not invented.
#[tokio::test]
async fn a_stop_records_the_reason_the_caller_gave() {
    // `StopPipelineExecutionReason` is modelled len 0..200
    // (`smithy_query(query="operation",operation="StopPipelineExecution")`), so the
    // empty string and a 200-character reason are both inside the bound.
    let longest = "r".repeat(200);
    // (case, abandon, the reason the caller gives)
    let rows: Vec<(&str, bool, Option<&str>)> = vec![
        ("no reason at all", false, None),
        ("a sentence", true, Some("rolling back a bad deployment")),
        ("the empty reason, at the modelled minimum", true, Some("")),
        ("a reason at the modelled maximum", false, Some(longest.as_str())),
        (
            "punctuation and non-ASCII",
            true,
            Some("stopped by on-call — ticket #4711 (50% rollout)"),
        ),
    ];

    for (case, abandon, reason) in rows {
        let pipeline = format!("reason-{}", case.replace([' ', ','], "-"));
        let emulator = emulator_with(&pipeline, &["delivered.zip"], None).await;
        let execution = start(&emulator, &pipeline).await;

        let mut request = json!({
            "pipelineName": pipeline,
            "pipelineExecutionId": execution,
            "abandon": abandon,
        });
        if let Some(reason) = reason {
            request["reason"] = json!(reason);
        }
        emulator.ok("StopPipelineExecution", &request).await;
        wait_for_terminal(&emulator, &pipeline, &execution).await;

        // `StopExecutionTrigger$reason` is "the user-specified reason the pipeline was
        // stopped", so it carries exactly what the caller gave and nothing when the
        // caller gave nothing.
        let listed = summary(&emulator, &pipeline, &execution).await;
        let expected_trigger = match reason {
            Some(reason) => json!({"reason": reason}),
            None => json!({}),
        };
        assert_eq!(
            listed["stopTrigger"], expected_trigger,
            "case {case:?}: unexpected stopTrigger, got {listed}"
        );
        let expected_summary = match reason {
            Some(reason) => json!(reason),
            None => Value::Null,
        };
        assert_eq!(
            listed["statusSummary"], expected_summary,
            "case {case:?}: the summary of the status is the reason the stop gave, got {listed}"
        );
        let fetched = emulator
            .ok(
                "GetPipelineExecution",
                &json!({"pipelineName": pipeline, "pipelineExecutionId": execution}),
            )
            .await;
        assert_eq!(
            fetched["pipelineExecution"]["statusSummary"], expected_summary,
            "case {case:?}: both readers must report the stop the same way, got {fetched}"
        );
    }
}

/// An execution that has already settled cannot be stopped again, whichever way the
/// second request asks, and the refusal leaves the record as it was.
#[tokio::test]
async fn an_execution_that_has_settled_cannot_be_stopped_again() {
    // (case, the abandon flag of the request that comes after the stop)
    let rows: Vec<(&str, bool)> = vec![
        ("ask to abandon it again", true),
        ("ask to wait for it instead", false),
    ];

    for (case, abandon) in rows {
        let pipeline = format!("settled-{}", case.replace(' ', "-"));
        let emulator = emulator_with(&pipeline, &["delivered.zip"], None).await;
        let execution = start(&emulator, &pipeline).await;
        emulator
            .ok(
                "StopPipelineExecution",
                &json!({
                    "pipelineName": pipeline,
                    "pipelineExecutionId": execution,
                    "abandon": true,
                    "reason": "the first stop",
                }),
            )
            .await;
        wait_for_terminal(&emulator, &pipeline, &execution).await;

        let (status, code) = emulator
            .error_code(
                "StopPipelineExecution",
                &json!({
                    "pipelineName": pipeline,
                    "pipelineExecutionId": execution,
                    "abandon": abandon,
                    "reason": "the second stop",
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineExecutionNotStoppableException"),
            "case {case:?}: an execution that has already stopped is not stoppable"
        );

        // The refused request recorded nothing: the status and the reason are still the
        // first stop's.
        let listed = summary(&emulator, &pipeline, &execution).await;
        assert_eq!(
            (listed["status"].clone(), listed["stopTrigger"].clone()),
            (json!("Stopped"), json!({"reason": "the first stop"})),
            "case {case:?}: a refused stop must change nothing, got {listed}"
        );
    }
}

/// A stop naming something the store does not hold is refused, nothing is stopped, and
/// the execution that IS running stays stoppable.
#[tokio::test]
async fn a_stop_naming_what_the_store_does_not_hold_is_refused() {
    let emulator = emulator_with("stoppable", &["delivered.zip"], Some("PARALLEL")).await;
    let execution = start(&emulator, "stoppable").await;

    // A second pipeline of the same emulator, so one pipeline's execution id can be
    // offered to the other.
    emulator.bucket(&artifact_bucket("neighbour")).await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                "neighbour",
                json!([source_stage("source-bucket", "source.zip", "SourceArtifact")]),
                None,
            ),
        )
        .await;
    let neighbour_execution = start(&emulator, "neighbour").await;

    // (case, pipelineName, pipelineExecutionId, expected error code)
    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            "a pipeline the store does not hold",
            "missing",
            execution.clone(),
            "PipelineNotFoundException",
        ),
        (
            "an execution id no pipeline minted",
            "stoppable",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "PipelineExecutionNotStoppableException",
        ),
        (
            "an execution of another pipeline",
            "stoppable",
            neighbour_execution.clone(),
            "PipelineExecutionNotStoppableException",
        ),
        (
            // `PipelineExecutionId` is modelled
            // `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`, so the
            // boundary refuses this before any handler runs.
            "an id the modelled pattern rejects",
            "stoppable",
            "not-a-uuid".to_string(),
            "ValidationException",
        ),
        (
            // The same pattern: its hex digits are lowercase.
            "an id with uppercase hex",
            "stoppable",
            execution.to_uppercase(),
            "ValidationException",
        ),
    ];

    for (case, pipeline_name, pipeline_execution_id, expected) in rows {
        let (status, code) = emulator
            .error_code(
                "StopPipelineExecution",
                &json!({
                    "pipelineName": pipeline_name,
                    "pipelineExecutionId": pipeline_execution_id,
                    "abandon": true,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: unexpected refusal"
        );
    }

    // Nothing was stopped: the execution is still there to stop, and stopping it now
    // settles it.
    let stopped = emulator
        .ok(
            "StopPipelineExecution",
            &json!({
                "pipelineName": "stoppable",
                "pipelineExecutionId": execution,
                "abandon": true,
            }),
        )
        .await;
    assert_eq!(
        stopped["pipelineExecutionId"],
        json!(execution),
        "the refused stops left the execution stoppable, got {stopped}"
    );
    let settled = wait_for_terminal(&emulator, "stoppable", &execution).await;
    assert_eq!(
        settled["pipelineExecution"]["status"],
        json!("Stopped"),
        "the accepted stop settles the execution, got {settled}"
    );
}

/// An execution waiting behind another one is stoppable on its own: it settles at
/// `Stopped` while the execution that is running carries on and delivers what its
/// actions produce.
#[tokio::test]
async fn the_execution_waiting_behind_another_can_be_stopped_on_its_own() {
    // QUEUED mode runs one execution at a time and holds the next one waiting.
    let emulator = emulator_with("queued-stop", &["delivered.zip"], Some("QUEUED")).await;
    let running = start(&emulator, "queued-stop").await;
    let waiting = start(&emulator, "queued-stop").await;
    assert_ne!(
        running, waiting,
        "two starts mint two executions, got {running} twice"
    );

    let stopped = emulator
        .ok(
            "StopPipelineExecution",
            &json!({
                "pipelineName": "queued-stop",
                "pipelineExecutionId": waiting,
                "abandon": true,
                "reason": "not needed any more",
            }),
        )
        .await;
    assert_eq!(
        stopped["pipelineExecutionId"],
        json!(waiting),
        "the stop reports the waiting execution, got {stopped}"
    );
    assert_eq!(
        status_of(&emulator, "queued-stop", &waiting).await,
        "Stopped",
        "the waiting execution settles at Stopped"
    );

    // The execution that was running is untouched by the other one's stop: it reaches
    // its own terminal status and its delivery is there.
    let settled = wait_for_terminal(&emulator, "queued-stop", &running).await;
    assert_eq!(
        settled["pipelineExecution"]["status"],
        json!("Succeeded"),
        "the running execution is not stopped by stopping the one waiting, got {settled}"
    );
    let (status, bytes) = emulator.get_object("deploy-bucket", "delivered.zip").await;
    assert_eq!(
        (status, bytes.as_slice()),
        (200, SOURCE_BYTES),
        "the running execution still delivers what its actions produce"
    );
    assert_eq!(
        status_of(&emulator, "queued-stop", &waiting).await,
        "Stopped",
        "the stopped execution stays stopped while the other one runs"
    );
}

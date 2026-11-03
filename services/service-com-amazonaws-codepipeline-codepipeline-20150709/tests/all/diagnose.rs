// SPDX-License-Identifier: CC0-1.0

//! How an execution ENTERS a stage, and what a start that follows another one does to a
//! run still at that boundary — end to end over the wire the service speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: entering a stage is a transition the CLOCK owes, like every
//! other step of an execution, so a run is at the boundary of its first stage for a span
//! after the call that started it — and `PipelineDeclaration$executionMode` decides what
//! a newer run that reaches the same boundary does to it. In `SUPERSEDED`, the default,
//! the newer one takes the stage and the one it caught up with reaches the terminal
//! `Superseded` status; in `QUEUED` they enter in the order they arrived; in `PARALLEL`
//! each runs on its own. Nothing is superseded once a run is IN the stage.
//!
//! The rows span exactly what the rule generalises over: the three `ExecutionMode`
//! values, against the two runs every one of these pipelines has — the one its creation
//! started (`TriggerType` `CreatePipeline`) and the one the test starts right after it.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so these pipelines run their
//! real actions through the emulated S3 service and every assertion goes through the
//! router.

use serde_json::json;

use crate::execution_engine::{
    artifact_bucket, creation_run, declaration, deploy_stage, source_stage, status_of,
    wait_for_terminal, Emulator,
};

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "entry-source";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The bucket the deploy action delivers into.
const DEPLOY_BUCKET: &str = "entry-deploy";

/// The artifact the source action produces.
const ARTIFACT: &str = "SourceArtifact";

/// The bytes the source object carries.
const PAYLOAD: &[u8] = b"the entry bytes";

/// RULE: a start that follows another run within the span that run is still at the
/// boundary for is what `executionMode` decides between — and whichever run takes the
/// stage runs it to the end and delivers.
#[tokio::test]
async fn what_a_following_start_does_to_the_run_at_the_boundary_is_the_execution_mode() {
    // (case, the executionMode declared, the status the run the CREATION started
    //  reaches, the status the following start reaches)
    let rows: Vec<(&str, Option<&str>, &str, &str)> = vec![
        (
            // "The default mode is SUPERSEDED" (`PipelineDeclaration$executionMode`), so
            // a declaration stating none is the same row as the one that states it.
            "the default mode",
            None,
            "Superseded",
            "Succeeded",
        ),
        ("SUPERSEDED", Some("SUPERSEDED"), "Superseded", "Succeeded"),
        ("QUEUED", Some("QUEUED"), "Succeeded", "Succeeded"),
        ("PARALLEL", Some("PARALLEL"), "Succeeded", "Succeeded"),
    ];

    for (case, mode, created_run_status, started_run_status) in rows {
        let emulator = Emulator::new();
        let pipeline = format!("entry-{}", case.to_lowercase().replace(' ', "-"));
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator.bucket(DEPLOY_BUCKET).await;
        emulator.put_object(SOURCE_BUCKET, SOURCE_KEY, PAYLOAD).await;
        let delivered = format!("{pipeline}.zip");
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                        deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, &delivered, false),
                    ]),
                    mode,
                ),
            )
            .await;
        // Creating a pipeline runs it once, and that run is still at the boundary of its
        // first stage: this start reaches the same boundary.
        let created = creation_run(&emulator, &pipeline).await;
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await["pipelineExecutionId"]
            .as_str()
            .expect("the start reports an execution id")
            .to_string();

        let answer = wait_for_terminal(&emulator, &pipeline, &started).await;
        assert_eq!(
            answer["pipelineExecution"]["status"],
            json!(started_run_status),
            "case {case:?}: expected the started run to reach {started_run_status}, got \
             {answer}"
        );
        let answer = wait_for_terminal(&emulator, &pipeline, &created).await;
        assert_eq!(
            answer["pipelineExecution"]["status"],
            json!(created_run_status),
            "case {case:?}: expected the run the creation started to reach \
             {created_run_status}, got {answer}"
        );
        if created_run_status == "Superseded" {
            let summary = answer["pipelineExecution"]["statusSummary"]
                .as_str()
                .unwrap_or_default();
            assert!(
                summary.contains("superseded"),
                "case {case:?}: a superseded run says why, got {summary:?}"
            );
        }

        // Whatever took the stage ran it to the end: the artifact is delivered.
        let (status, bytes) = emulator.get_object(DEPLOY_BUCKET, &delivered).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the run that took the stage delivers, got {status}"
        );
        assert_eq!(
            bytes, PAYLOAD,
            "case {case:?}: the delivered bytes must be the source bytes"
        );
    }
}

/// RULE, the other half: a run that is already IN a stage is not superseded — a start
/// that arrives after it has entered waits at that boundary and runs afterwards, so both
/// reach `Succeeded`.
#[tokio::test]
async fn a_run_that_has_entered_the_stage_is_not_superseded() {
    let emulator = Emulator::new();
    let pipeline = "entry-entered";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    emulator.put_object(SOURCE_BUCKET, SOURCE_KEY, PAYLOAD).await;
    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                    deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, "entered.zip", false),
                ]),
                Some("SUPERSEDED"),
            ),
        )
        .await;
    // The run the creation started is let past the boundary and carried to its own
    // terminal status BEFORE anything else starts, so no start ever catches it waiting.
    let created = creation_run(&emulator, pipeline).await;
    wait_for_terminal(&emulator, pipeline, &created).await;
    assert_eq!(
        status_of(&emulator, pipeline, &created).await,
        "Succeeded",
        "the run the creation started runs when nothing catches it at the boundary"
    );

    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await["pipelineExecutionId"]
        .as_str()
        .expect("the start reports an execution id")
        .to_string();
    let answer = wait_for_terminal(&emulator, pipeline, &started).await;
    assert_eq!(
        answer["pipelineExecution"]["status"],
        json!("Succeeded"),
        "a start that follows a finished run runs on its own, got {answer}"
    );
    assert_eq!(
        status_of(&emulator, pipeline, &created).await,
        "Succeeded",
        "the earlier run keeps the status it reached"
    );
}

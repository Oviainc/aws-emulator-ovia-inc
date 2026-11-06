// SPDX-License-Identifier: CC0-1.0

//! What the SERVICE does after `StartPipelineExecution` returns, with no further call
//! from the client.
//!
//! The rule under test: starting a pipeline execution hands the run to the service —
//! every step the clock then makes due (entering the next stage, performing the next
//! action's work) is materialised by the service itself, so an execution nothing
//! withholds produces its artifacts and reaches a terminal `PipelineExecutionStatus`
//! without the client calling CodePipeline again. The only thing that keeps it out of a
//! stage is a hold the API models: a disabled inbound transition withholds the stage
//! until `EnableStageTransition` releases it, and the release is driven to its end by the
//! service too.
//!
//! So every wait here polls the emulated S3 service ONLY: a suite that polled
//! `GetPipelineExecution` would be the caller driving the execution and could not tell a
//! service that runs a pipeline from one that merely moves when asked. The CodePipeline
//! reads come afterwards, to check what the service published about a run it had already
//! finished.
//!
//! The rows span what the rule generalises over: how many stages follow the source, what
//! the delivered key looks like, whether the deploy extracts the artifact, and how many
//! entries and bytes the artifact carries. In those rows the source object is uploaded
//! only AFTER the pipelines exist, so the run a pipeline's own creation starts fails at
//! its source and delivers nothing: every object they observe was delivered by the
//! execution the test started. The last test is the other side of that — the run a
//! CREATION starts, which the service drives the same way.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so every assertion goes
//! through the same router the production emulator builds.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, zip_of, Emulator, WAIT,
};

/// The bucket every deploy action in this suite delivers to.
const DEPLOY_BUCKET: &str = "unattended-deploys";

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "unattended-source";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The artifact the source action produces and the deploy actions consume.
const ARTIFACT: &str = "SourceArtifact";

/// How long a poll of the artifact store waits between reads.
const POLL: Duration = Duration::from_millis(25);

/// Poll S3 — and NOTHING else — until `key` is delivered, answering its bytes.
///
/// This is the whole point of the suite: between the start and the read that finds the
/// object, no CodePipeline operation is called at all, so an object that appears was
/// delivered by the service driving the execution on its own.
async fn delivered(emulator: &Emulator, bucket: &str, key: &str, case: &str) -> Vec<u8> {
    let deadline = Instant::now() + WAIT;
    let mut last = 0;
    while Instant::now() < deadline {
        let (status, bytes) = emulator.get_object(bucket, key).await;
        if status == 200 {
            return bytes;
        }
        last = status;
        tokio::time::sleep(POLL).await;
    }
    panic!(
        "case {case:?}: s3://{bucket}/{key} was never delivered; the client made no \
         CodePipeline call after the start, so the service never drove the execution \
         (the last read of the key answered HTTP {last})"
    );
}

/// The status `GetPipelineExecution` reports for one execution right now.
async fn reported_status(emulator: &Emulator, pipeline: &str, execution: &str) -> String {
    let answer = emulator
        .ok(
            "GetPipelineExecution",
            &json!({"pipelineName": pipeline, "pipelineExecutionId": execution}),
        )
        .await;
    answer["pipelineExecution"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Start `pipeline` and answer the execution id the start minted.
async fn start(emulator: &Emulator, pipeline: &str) -> String {
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await;
    started["pipelineExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("starting {pipeline} must report an execution id: {started}"))
        .to_string()
}

/// What one row expects in the deploy bucket, as `(key, bytes)` pairs resolved against
/// the source archive the row's entries make.
type Expected = fn(&[u8]) -> Vec<(String, Vec<u8>)>;

/// One row of the table: a pipeline shape and the objects its started execution must
/// deliver without being asked again.
struct Row {
    /// What this row is about, named in every assertion.
    case: &'static str,
    /// The entries the source archive carries.
    entries: Vec<(&'static str, Vec<u8>)>,
    /// The stages that follow the source stage.
    deploys: Vec<Value>,
    /// The objects the deploy stages must write into [`DEPLOY_BUCKET`].
    expected: Expected,
}

#[tokio::test]
async fn a_started_execution_is_driven_to_its_delivery_without_another_client_call() {
    let rows = vec![
        Row {
            case: "one deploy stage, delivering the artifact unextracted",
            entries: vec![("only.txt", b"one entry".to_vec())],
            deploys: vec![deploy_stage(
                "Deploy",
                ARTIFACT,
                DEPLOY_BUCKET,
                "flat.zip",
                false,
            )],
            expected: |archive| vec![("flat.zip".to_string(), archive.to_vec())],
        },
        Row {
            case: "a delivered key nested under several segments",
            entries: vec![("a.txt", b"first".to_vec()), ("b/c.txt", b"second".to_vec())],
            deploys: vec![deploy_stage(
                "Deploy",
                ARTIFACT,
                DEPLOY_BUCKET,
                "deployed/nested/deep/payload.zip",
                false,
            )],
            expected: |archive| {
                vec![(
                    "deployed/nested/deep/payload.zip".to_string(),
                    archive.to_vec(),
                )]
            },
        },
        Row {
            case: "an extracting deploy, one object per archive entry",
            entries: vec![
                ("root.txt", b"at the root".to_vec()),
                ("dir/leaf.bin", vec![7_u8; 4_096]),
            ],
            deploys: vec![deploy_stage(
                "Deploy",
                ARTIFACT,
                DEPLOY_BUCKET,
                "unpacked",
                true,
            )],
            expected: |_| {
                vec![
                    (
                        "unpacked/root.txt".to_string(),
                        b"at the root".to_vec(),
                    ),
                    ("unpacked/dir/leaf.bin".to_string(), vec![7_u8; 4_096]),
                ]
            },
        },
        Row {
            case: "two deploy stages, each delivering after the one before it",
            entries: vec![("payload.bin", vec![3_u8; 64 * 1024])],
            deploys: vec![
                deploy_stage("DeployOne", ARTIFACT, DEPLOY_BUCKET, "first/one.zip", false),
                deploy_stage("DeployTwo", ARTIFACT, DEPLOY_BUCKET, "second/two.zip", false),
            ],
            expected: |archive| {
                vec![
                    ("first/one.zip".to_string(), archive.to_vec()),
                    ("second/two.zip".to_string(), archive.to_vec()),
                ]
            },
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let pipeline = format!("unattended-{index}");
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator.bucket(DEPLOY_BUCKET).await;

        let mut stages = vec![source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT)];
        stages.extend(row.deploys);
        emulator
            .ok(
                "CreatePipeline",
                &declaration(&pipeline, json!(stages), None),
            )
            .await;

        // The source object appears only now, so the run the creation started has
        // already failed at its source action and delivered nothing: every object read
        // below belongs to the execution started next.
        let borrowed: Vec<(&str, &[u8])> = row
            .entries
            .iter()
            .map(|(name, body)| (*name, body.as_slice()))
            .collect();
        let archive = zip_of(&borrowed);
        emulator
            .put_object(SOURCE_BUCKET, SOURCE_KEY, &archive)
            .await;

        let execution = start(&emulator, &pipeline).await;

        for (key, body) in (row.expected)(&archive) {
            let read = delivered(&emulator, DEPLOY_BUCKET, &key, case).await;
            assert_eq!(
                read.len(),
                body.len(),
                "case {case:?}: s3://{DEPLOY_BUCKET}/{key} must carry {} bytes, got {}",
                body.len(),
                read.len()
            );
            assert!(
                read == body,
                "case {case:?}: s3://{DEPLOY_BUCKET}/{key} must carry the bytes the \
                 artifact holds, got {} bytes that differ",
                read.len()
            );
        }

        // The delivery has already happened, so the status the service publishes for the
        // run it finished must be the terminal one.
        let status = reported_status(&emulator, &pipeline, &execution).await;
        assert_eq!(
            status, "Succeeded",
            "case {case:?}: every action of {execution} ran, so the execution must report \
             Succeeded, got {status:?}"
        );
    }
}

/// Two identical pipelines started in the same instant, one of them holding its deploy
/// stage: the unheld one delivers on its own, the held one delivers nothing until the
/// hold is lifted — and then the service drives that one to its delivery too.
#[tokio::test]
async fn a_disabled_inbound_transition_is_the_only_thing_that_withholds_a_delivery() {
    let emulator = Emulator::new();
    let held = "unattended-held";
    let twin = "unattended-twin";
    let held_key = "deployed/held.zip";
    let twin_key = "deployed/twin.zip";

    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    for pipeline in [held, twin] {
        emulator.bucket(&artifact_bucket(pipeline)).await;
    }
    for (pipeline, key) in [(held, held_key), (twin, twin_key)] {
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                        deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, key, false),
                    ]),
                    None,
                ),
            )
            .await;
    }

    emulator
        .ok(
            "DisableStageTransition",
            &json!({
                "pipelineName": held,
                "stageName": "Deploy",
                "transitionType": "Inbound",
                "reason": "held for this test",
            }),
        )
        .await;

    // Nothing is past the boundary yet: the runs the creations started had no source
    // object to read.
    let archive = zip_of(&[("payload.txt", b"the deployed bytes".as_slice())]);
    for key in [held_key, twin_key] {
        let (status, _) = emulator.get_object(DEPLOY_BUCKET, key).await;
        assert_ne!(
            status, 200,
            "case \"before the start\": s3://{DEPLOY_BUCKET}/{key} must not exist yet, \
             got HTTP {status}"
        );
    }
    emulator
        .put_object(SOURCE_BUCKET, SOURCE_KEY, &archive)
        .await;

    let held_run = start(&emulator, held).await;
    let twin_run = start(&emulator, twin).await;

    // From here until the twin's object is read, no CodePipeline operation is called.
    let twin_body = delivered(&emulator, DEPLOY_BUCKET, twin_key, "the unheld twin").await;
    assert!(
        twin_body == archive,
        "case \"the unheld twin\": the delivered object must carry the source bytes, got \
         {} bytes",
        twin_body.len()
    );
    let (held_status, _) = emulator.get_object(DEPLOY_BUCKET, held_key).await;
    assert_ne!(
        held_status, 200,
        "case \"the held pipeline\": its Deploy inbound transition is disabled, so nothing \
         may be delivered under {held_key}, got HTTP {held_status}"
    );

    let twin_status = reported_status(&emulator, twin, &twin_run).await;
    assert_eq!(
        twin_status, "Succeeded",
        "case \"the unheld twin\": the execution that delivered must report Succeeded, got \
         {twin_status:?}"
    );
    let held_status_now = reported_status(&emulator, held, &held_run).await;
    assert_eq!(
        held_status_now, "InProgress",
        "case \"the held pipeline\": an execution held out of a stage is still in flight, \
         got {held_status_now:?}"
    );

    emulator
        .ok(
            "EnableStageTransition",
            &json!({
                "pipelineName": held,
                "stageName": "Deploy",
                "transitionType": "Inbound",
            }),
        )
        .await;
    let held_body = delivered(&emulator, DEPLOY_BUCKET, held_key, "the released pipeline").await;
    assert!(
        held_body == archive,
        "case \"the released pipeline\": enabling the transition must let the service \
         deliver the same bytes, got {} bytes",
        held_body.len()
    );
}

/// The rule holds for whichever call starts an execution, not only for the explicit
/// start: creating a pipeline runs it once (`ExecutionTrigger$triggerType` carries
/// `CreatePipeline` for exactly that run), so that run too is carried to its delivery
/// with no further call from the client.
#[tokio::test]
async fn the_run_a_pipeline_s_own_creation_starts_is_driven_the_same_way() {
    let emulator = Emulator::new();
    let pipeline = "unattended-creation-run";
    let key = "deployed/created.zip";

    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    // The source object exists BEFORE the pipeline does, so the run the creation starts
    // has something to read: it is the only execution of this pipeline there will be.
    let archive = zip_of(&[("created.txt", b"delivered by the creation run".as_slice())]);
    emulator
        .put_object(SOURCE_BUCKET, SOURCE_KEY, &archive)
        .await;

    emulator
        .ok(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                    deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, key, false),
                ]),
                None,
            ),
        )
        .await;

    // No CodePipeline call from here until the object is read.
    let body = delivered(&emulator, DEPLOY_BUCKET, key, "the creation run").await;
    assert!(
        body == archive,
        "case \"the creation run\": the delivered object must carry the source bytes, got \
         {} bytes",
        body.len()
    );

    let listed = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
        .await;
    let summaries = listed["pipelineExecutionSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "case \"the creation run\": creating the pipeline starts exactly one execution, \
         got {listed}"
    );
    assert_eq!(
        summaries[0]["trigger"]["triggerType"],
        json!("CreatePipeline"),
        "case \"the creation run\": the execution that delivered is the creation's own, \
         got {listed}"
    );
    assert_eq!(
        summaries[0]["status"],
        json!("Succeeded"),
        "case \"the creation run\": every action ran, so the execution must report \
         Succeeded, got {listed}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! What the CLOCK owes an accepted evaluation job, over the same `MultiServiceRouter` the
//! emulator boots (`crate::support`).
//!
//! `evaluation_jobs.rs` covers what a create mints and what a read reports while the job is
//! still running; this suite covers the end of that run — the one thing a client that
//! creates a job and waits for it depends on.
//!
//! The rules under test, each asserted over a SET of jobs rather than over one request:
//!
//! 1. **An accepted job's non-terminal `InProgress` status is owed its move to the terminal
//!    `Completed` by the clock**: the move is due a fixed span after the create that
//!    started the run (the span is stored as a due INSTANT, so the assertions below age the
//!    stored record instead of sleeping), a read before that instant still reports the job
//!    running, and a read after it reports `Completed` — stamped at the instant the move was
//!    owed, with no `failureMessages` and with every other member the job was created with
//!    unchanged. The move is owed by the CLOCK and not by the read, so reading twice answers
//!    alike.
//! 2. **A stop replaces the completion the clock owed**: a job a caller stopped walks
//!    `Stopping` -> `Stopped` and never reports `Completed`, however much of its run had
//!    already elapsed when the stop arrived.
//! 3. **A completed job is TERMINAL**: `BatchDeleteEvaluationJob` accepts it ("An evaluation
//!    job can only be deleted if it has following status FAILED, COMPLETED, and STOPPED") and
//!    a stop of it is refused with 400 `ValidationException`, since "Stops an evaluation job
//!    that is current being created or running" leaves nothing to stop.
//!
//! Every expectation is derived from the model
//! (`smithy_query(query="operation",operation="CreateEvaluationJob,GetEvaluationJob,StopEvaluationJob,BatchDeleteEvaluationJob")`
//! / `query="shape"`), except the length of the emulator's run span, which is the handler's
//! own and is read off the constant the handler stores.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use bedrock::state::EvaluationJobRecord;

use crate::support::{
    emulator, label, wall_clock_seconds, Emulator, Reply, ACCOUNT, REGION,
};

// ---------------------------------------------------------------------------------------
// Values the model admits, and the spans the handlers hold.
// ---------------------------------------------------------------------------------------

/// `EvaluationJobStatus` is `[InProgress, Completed, Failed, Stopping, Stopped, Deleting]`
/// (`smithy_query(query="shape",shape="EvaluationJobStatus")`): the status an accepted job
/// carries while it runs, the terminal one its run ends in, the two a stop walks it through,
/// and the one a batch delete moves it to.
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";
const STOPPING: &str = "Stopping";
const STOPPED: &str = "Stopped";
const DELETING: &str = "Deleting";

/// The emulator's spans for running an evaluation and for winding one down. Both are held
/// as a due INSTANT on the stored record rather than waited on, which is what lets these
/// rows age a job instead of sleeping through its run.
const COMPLETE_AFTER: Duration = bedrock::state::EVALUATION_JOB_COMPLETE_AFTER;
const STOPPED_AFTER: Duration = bedrock::state::EVALUATION_JOB_STOPPED_AFTER;

/// A `RoleArn` the model admits (pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`) and
/// an `EvaluationOutputDataConfig$s3Uri` its own pattern admits.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/bedrock-evaluation";
const OUTPUT_URI: &str = "s3://evaluation-results/model-eval/";

/// `EvaluationBedrockModelIdentifier` values the model admits — bare model ids of the
/// `<provider>.<model>` form its pattern spells.
const MODEL_IDS: &[&str] = &[
    "anthropic.claude-v2",
    "amazon.titan-text-express-v1",
    "meta.llama3-8b-instruct-v1",
];

/// `EvaluationTaskType` is `[Summarization, Classification, QuestionAndAnswer, Generation,
/// Custom]` (`smithy_query(query="shape",shape="EvaluationTaskType")`).
const TASK_TYPES: &[&str] = &["Generation", "Classification", "QuestionAndAnswer"];

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// One `EvaluationDatasetMetricConfig`: a task type, a prompt dataset and the metric names
/// (`smithy_query(query="shape",shape="EvaluationDatasetMetricConfig")`, all three
/// `@required`).
fn dataset_metric_config(task_type: &str, dataset: &str, metrics: &[&str]) -> Value {
    json!({
        "taskType": task_type,
        "dataset": { "name": dataset },
        "metricNames": metrics,
    })
}

/// An `automated` `EvaluationConfig` — the variant whose `EvaluationJobType` is `Automated`.
fn automated_config(task_type: &str, dataset: &str, metrics: &[&str]) -> Value {
    json!({
        "automated": {
            "datasetMetricConfigs": [dataset_metric_config(task_type, dataset, metrics)],
        },
    })
}

/// A `human` `EvaluationConfig` — the variant whose `EvaluationJobType` is `Human`.
fn human_config(task_type: &str, dataset: &str, metric: &str) -> Value {
    json!({
        "human": {
            "humanWorkflowConfig": {
                "flowDefinitionArn":
                    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:flow-definition/eval-review"),
                "instructions": "Rate each response.",
            },
            "customMetrics": [{ "name": metric, "ratingMethod": "ComparisonChoice" }],
            "datasetMetricConfigs": [dataset_metric_config(task_type, dataset, &[metric])],
        },
    })
}

/// One `EvaluationModelConfig` naming a Bedrock model. `inferenceParams` is sent explicitly
/// because the member is modelled `@default("{}")`
/// (`smithy_query(query="shape",shape="EvaluationBedrockModel")`).
fn bedrock_model(model_id: &str) -> Value {
    json!({
        "bedrockModel": {
            "modelIdentifier": model_id,
            "inferenceParams": "{\"temperature\":0.2}",
        },
    })
}

/// An `EvaluationInferenceConfig` naming `models`.
fn models_inference(models: Vec<Value>) -> Value {
    json!({ "models": models })
}

/// A `CreateEvaluationJob` body carrying the four `@required` members.
fn create_body(job_name: &str, config: Value, inference: Value) -> Value {
    json!({
        "jobName": job_name,
        "roleArn": ROLE_ARN,
        "outputDataConfig": { "s3Uri": OUTPUT_URI },
        "evaluationConfig": config,
        "inferenceConfig": inference,
    })
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/evaluation-jobs", Some(body))
        .await
}

async fn get_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/evaluation-jobs/{}", label(identifier)),
        None,
    )
    .await
}

/// `StopEvaluationJob` is bound to `POST /evaluation-job/{jobIdentifier}/stop` and carries
/// no member beside that label, so the call sends no body.
async fn stop_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "POST",
        &format!("/evaluation-job/{}/stop", label(identifier)),
        None,
    )
    .await
}

/// `BatchDeleteEvaluationJob` is bound to `POST /evaluation-jobs/batch-delete` and carries
/// its `jobIdentifiers` list in the body.
async fn batch_delete_in_region(emu: &Emulator, region: &str, identifiers: &[String]) -> Reply {
    emu.call_in_region(
        region,
        "POST",
        "/evaluation-jobs/batch-delete",
        Some(json!({ "jobIdentifiers": identifiers })),
    )
    .await
}

/// Create the job `body` describes in `region` and hand back the `jobArn` the create minted.
async fn given_job(emu: &Emulator, case: &str, region: &str, body: Value) -> String {
    let created = create_in_region(emu, region, body).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: the create that seeds this row must be accepted, got {} {:?}",
        created.status, created.body
    );
    created.text("jobArn")
}

// ---------------------------------------------------------------------------------------
// Reading and aging the store the handlers share.
// ---------------------------------------------------------------------------------------

/// The stored record the ARN names, cloned out of the store the handlers share.
fn stored(emu: &Emulator, arn: &str) -> EvaluationJobRecord {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let store = state.lock();
    store
        .evaluation_jobs
        .get(arn)
        .cloned()
        .unwrap_or_else(|| panic!("the store must hold the created job {arn}"))
}

/// Age the stored job `arn` names by `by`, as though it had been created that much earlier:
/// its own timeline is untouched and only how much of it has elapsed changes, so a rule
/// about a span of wall-clock time needs no sleep.
fn age(emu: &Emulator, arn: &str, by: Duration) {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let job = store
        .evaluation_jobs
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the store must hold the created job {arn}"));
    job.created_at -= by;
    job.last_modified_at -= by;
    for due in [&mut job.completed_due_at, &mut job.stopped_due_at] {
        if let Some(instant) = due.as_mut() {
            *instant -= by;
        }
    }
}

/// A read's body with every member the CLOCK bears on removed — the status the move is
/// about, the stamp it carries and the creation instant the rows above shift to age a job —
/// so what is left is everything the transition had to leave alone.
fn apart_from_the_clock(body: &Value) -> Value {
    let mut body = body.clone();
    if let Some(members) = body.as_object_mut() {
        members.remove("status");
        members.remove("lastModifiedTime");
        members.remove("creationTime");
    }
    body
}

/// The rows rule 1 and rule 2 run over: a case name, the region the job lives in, its name
/// and the configuration pair the create carries.
fn rows() -> Vec<(&'static str, &'static str, String, Value, Value)> {
    vec![
        (
            "automated over one model",
            REGION,
            "completing-automated".to_string(),
            automated_config(
                TASK_TYPES[0],
                "Builtin.RealToxicityPrompts",
                &["Builtin.Toxicity"],
            ),
            models_inference(vec![bedrock_model(MODEL_IDS[0])]),
        ),
        (
            "human over two models",
            REGION,
            "completing-human".to_string(),
            human_config(TASK_TYPES[1], "custom-prompts", "readability"),
            models_inference(vec![
                bedrock_model(MODEL_IDS[0]),
                bedrock_model(MODEL_IDS[2]),
            ]),
        ),
        (
            "automated over a precomputed inference source, named at the minimum length",
            REGION,
            "c".to_string(),
            automated_config(TASK_TYPES[2], "custom-prompts", &["Builtin.Accuracy"]),
            models_inference(vec![json!({
                "precomputedInferenceSource": {
                    "inferenceSourceIdentifier": "recorded-responses.v3",
                },
            })]),
        ),
        (
            "a job of another region, named at the maximum length",
            "eu-central-1",
            format!("c{}", "z".repeat(62)),
            automated_config(
                TASK_TYPES[0],
                "Builtin.NaturalQuestions",
                &["Builtin.Robustness"],
            ),
            models_inference(vec![bedrock_model(MODEL_IDS[1])]),
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// 1. The run ends: InProgress is owed its move to Completed by the clock.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_accepted_evaluation_job_completes_on_the_clock() {
    let emu = emulator();

    for (case, region, name, config, inference) in rows() {
        let arn = given_job(
            &emu,
            case,
            region,
            create_body(&name, config, inference),
        )
        .await;

        // The create recorded WHEN the run is owed its end — one span after the call that
        // started it — rather than leaving the job pinned in its non-terminal status.
        let record = stored(&emu, &arn);
        assert_eq!(
            record.completed_due_at,
            Some(record.created_at + COMPLETE_AFTER),
            "case {case:?}: an accepted job is owed its terminal status one run span after \
             the create, got {:?} against a creation at {:?}",
            record.completed_due_at,
            record.created_at
        );

        // The span is not zero, so a caller that reads the job while the run is still owed
        // sees the running status the real service reports.
        let running = get_in_region(&emu, region, &arn).await;
        assert_eq!(
            running.get("status"),
            json!(IN_PROGRESS),
            "case {case:?}: a job whose run is still owed is {IN_PROGRESS}, got {:?}",
            running.get("status")
        );

        // Still not due: two thirds of the run has elapsed.
        age(&emu, &arn, COMPLETE_AFTER * 2 / 3);
        let almost = get_in_region(&emu, region, &arn).await;
        assert_eq!(
            almost.get("status"),
            json!(IN_PROGRESS),
            "case {case:?}: the move is owed at the due instant and not before it, so the job \
             is still {IN_PROGRESS}, got {:?}",
            almost.get("status")
        );

        // Due: the rest of the span has passed, so the next call that touches the record
        // reports the terminal status the run ended in — nobody had to ask for it.
        age(&emu, &arn, COMPLETE_AFTER / 3 + Duration::from_millis(50));
        let settled = get_in_region(&emu, region, &arn).await;
        assert_eq!(
            settled.get("status"),
            json!(COMPLETED),
            "case {case:?}: a job whose run the clock has ended is {COMPLETED}, got {:?}",
            settled.get("status")
        );
        // The stamp is the instant the move was OWED — one run span after the creation —
        // and not the instant of the call that happened to read it.
        let elapsed = settled.instant("lastModifiedTime") - settled.instant("creationTime");
        assert!(
            (elapsed - COMPLETE_AFTER.as_secs_f64()).abs() < 0.05,
            "case {case:?}: completing the job is what last changed it, at the instant the \
             move was owed ({} s after the creation), got {elapsed} s",
            COMPLETE_AFTER.as_secs_f64()
        );
        assert!(
            settled.instant("lastModifiedTime") <= wall_clock_seconds(),
            "case {case:?}: the move was owed before this read, so its stamp is not in the \
             future, got {:?}",
            settled.get("lastModifiedTime")
        );
        // "A list of strings that specify why the evaluation job failed to create": a job
        // that ran to completion has none.
        assert_eq!(
            settled.get("failureMessages"),
            Value::Null,
            "case {case:?}: a job that completed reports no failureMessages, got {:?}",
            settled.get("failureMessages")
        );
        // The transition moved the status and nothing else.
        assert_eq!(
            apart_from_the_clock(&settled.body),
            apart_from_the_clock(&running.body),
            "case {case:?}: the move changes the status alone, got {:?} then {:?}",
            running.body,
            settled.body
        );

        // The move is owed by the CLOCK, not by the read: reading again reports the same
        // job, status and instants.
        let again = get_in_region(&emu, region, &arn).await;
        assert_eq!(
            again.body, settled.body,
            "case {case:?}: a read reports the job, it does not move it on, got {:?} then {:?}",
            settled.body, again.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. A stop replaces the completion the clock owed.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_stopped_evaluation_job_reaches_stopped_and_never_completed() {
    let emu = emulator();

    // Rows differ in HOW MUCH of the run had elapsed when the stop arrived, beside the
    // configurations of `rows()`: none of it, half of it and two thirds of it.
    let elapsed_before_the_stop = [
        ("stopped straight after the create", Duration::ZERO),
        ("stopped halfway through the run", COMPLETE_AFTER / 2),
        (
            "stopped two thirds of the way through the run",
            COMPLETE_AFTER * 2 / 3,
        ),
    ];

    for ((case, region, name, config, inference), (when, elapsed)) in
        rows().into_iter().zip(elapsed_before_the_stop.iter().cycle())
    {
        let case = format!("{case} {when}");
        let arn = given_job(
            &emu,
            &case,
            region,
            create_body(&name, config, inference),
        )
        .await;
        age(&emu, &arn, *elapsed);

        let stopped = stop_in_region(&emu, region, &arn).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: StopEvaluationJob is bound to @http(code: 200), got {} {:?}",
            stopped.status, stopped.body
        );
        let stopping = get_in_region(&emu, region, &arn).await;
        assert_eq!(
            stopping.get("status"),
            json!(STOPPING),
            "case {case:?}: a stop moves a running job to {STOPPING}, got {:?}",
            stopping.get("status")
        );

        // Past the end of the run the stop pre-empted, AND past the wind-down span: the job
        // the caller stopped reports the terminal status the STOP leads to, never the one
        // the completed run would have.
        age(&emu, &arn, COMPLETE_AFTER + STOPPED_AFTER);
        let settled = get_in_region(&emu, region, &arn).await;
        assert_eq!(
            settled.get("status"),
            json!(STOPPED),
            "case {case:?}: a stopped job ends {STOPPED} and never {COMPLETED}, got {:?}",
            settled.get("status")
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. A completed job is terminal: it is deletable and it cannot be stopped.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_completed_evaluation_job_is_terminal() {
    let emu = emulator();

    for (case, region, name, config, inference) in rows() {
        let arn = given_job(
            &emu,
            case,
            region,
            create_body(&name, config, inference),
        )
        .await;
        age(&emu, &arn, COMPLETE_AFTER);
        let completed = get_in_region(&emu, region, &arn).await;
        assert_eq!(
            completed.get("status"),
            json!(COMPLETED),
            "case {case:?}: the job this row deletes must have completed first, got {:?}",
            completed.body
        );

        // "Stops an evaluation job that is current being created or running": the run of a
        // completed job is over, so there is nothing left to stop.
        let refused = stop_in_region(&emu, region, &arn).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: ValidationException is modelled @httpError(400), got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: a job whose run is over cannot be stopped, got {:?} {:?}",
            refused.error,
            refused.body
        );
        assert!(
            refused.text("message").contains(&arn),
            "case {case:?}: the refusal names the job the stop addressed, got {:?}",
            refused.get("message")
        );

        // "An evaluation job can only be deleted if it has following status FAILED,
        // COMPLETED, and STOPPED": a completed job is one a batch delete accepts, which is
        // the leg a client can only reach once the run has ended.
        let deleted = batch_delete_in_region(&emu, region, std::slice::from_ref(&arn)).await;
        assert_eq!(
            deleted.status, 202,
            "case {case:?}: BatchDeleteEvaluationJob is modelled @http(code: 202), got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.get("evaluationJobs"),
            json!([{ "jobIdentifier": arn, "jobStatus": DELETING }]),
            "case {case:?}: a completed job is accepted for deletion and reported {DELETING}, \
             got {:?}",
            deleted.body
        );
    }
}

/// The due instants are the ONLY state this emulator holds about when a job's status moves,
/// so a record a create wrote carries one and a record whose move has been materialised
/// carries none: that is what makes the move happen once rather than on every read.
#[tokio::test]
async fn the_due_instant_is_cleared_once_the_move_is_materialised() {
    let emu = emulator();
    let (case, region, name, config, inference) = rows().remove(0);
    let arn = given_job(
        &emu,
        case,
        region,
        create_body(&name, config, inference),
    )
    .await;

    let before = stored(&emu, &arn);
    assert!(
        before.completed_due_at.is_some(),
        "case {case:?}: a running job is owed its terminal status, got {:?}",
        before.completed_due_at
    );

    age(&emu, &arn, COMPLETE_AFTER);
    let read = get_in_region(&emu, region, &arn).await;
    assert_eq!(
        read.get("status"),
        json!(COMPLETED),
        "case {case:?}: the read materialises the move the clock owed, got {:?}",
        read.body
    );

    let after = stored(&emu, &arn);
    assert_eq!(
        after.completed_due_at, None,
        "case {case:?}: a job whose move has been materialised is owed nothing further, got \
         {:?}",
        after.completed_due_at
    );
    assert_eq!(
        after.last_modified_at,
        after.created_at + COMPLETE_AFTER,
        "case {case:?}: the materialised move is stamped at the instant it was owed, one run \
         span after the creation at {:?}, got {:?}",
        after.created_at,
        after.last_modified_at
    );
}

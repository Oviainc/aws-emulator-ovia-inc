// SPDX-License-Identifier: CC0-1.0

//! The evaluation job resource end-to-end: `CreateEvaluationJob` writes it and
//! `GetEvaluationJob` reads it back, over the same `MultiServiceRouter` the emulator boots
//! (`crate::support`).
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A created job round-trips**: `CreateEvaluationJob` answers the modelled 202 with a
//!    `jobArn` of the form `EvaluationJobArn` declares, and every member
//!    `GetEvaluationJob` reports comes back out of the stored record — for requests
//!    differing in evaluation configuration (automated and human), inference
//!    configuration (models, a precomputed inference source, rag configs), application
//!    type, key spelling, description, tag count and name length (both modelled bounds).
//! 2. **`jobType` is the `EvaluationConfig` variant the request carried**: `automated` ->
//!    `Automated`, `human` -> `Human`, over the rows of 1 and never read off anything else.
//! 3. **`status` is `InProgress` and a read changes nothing**: the job is reported running
//!    from the create onwards, and two reads of one job answer identically.
//! 4. **A `jobName` names at most one job per account per region**, so a create reusing one
//!    conflicts with 400 `ConflictException` while the same name in another region is
//!    minted.
//! 5. **A replayed `clientRequestToken` mints nothing**: the replay answers with the job
//!    the first call minted, and the name the replay carried stays unminted.
//! 6. **An identifier no stored job answers to is refused** with 404
//!    `ResourceNotFoundException`.
//! 7. **An automated job supports a single model or inference profile**, so an automated
//!    configuration naming two is refused with 400 `ValidationException` while the same two
//!    under a human configuration are accepted.
//! 8. **More than `MAX_TAGS_PER_RESOURCE` tags is refused** with 400 `ValidationException`,
//!    and nothing is stored.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateEvaluationJob,GetEvaluationJob")` /
//! `query="shape"`).

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model admits for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// `RoleArn` values the model admits (pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const ROLE_ARNS: &[&str] = &[
    "arn:aws:iam::000000000000:role/bedrock-evaluation",
    "arn:aws:iam::000000000000:role/service-role/BedrockEvaluationExecution",
];

/// `S3Uri` values `EvaluationOutputDataConfig$s3Uri` admits (pattern
/// `^s3://[a-z0-9][-.a-z0-9]{1,61}[a-z0-9](?:/…)?/?$`).
const OUTPUT_URIS: &[&str] = &[
    "s3://evaluation-results/model-eval/",
    "s3://evaluation.results.bucket/runs/2026",
];

/// `EvaluationBedrockModelIdentifier` values the model admits — a bare model id of the
/// `<provider>.<model>` form its pattern spells.
const MODEL_IDS: &[&str] = &[
    "anthropic.claude-v2",
    "amazon.titan-text-express-v1",
    "meta.llama3-8b-instruct-v1",
];

/// A KMS key id `customerEncryptionKeyId` admits, and the alias the same `KmsKeyId`
/// pattern admits beside it and the key ARN form.
const KMS_KEY_ID: &str = "11111111-2222-3333-4444-555555555555";
const KMS_KEY_ALIAS: &str = "alias/evaluation-jobs";

/// `EvaluationTaskType` is `[Summarization, Classification, QuestionAndAnswer, Generation,
/// Custom]` (`smithy_query(query="shape",shape="EvaluationTaskType")`).
const TASK_TYPES: &[&str] = &["Summarization", "QuestionAndAnswer", "Custom"];

/// `EvaluationJobStatus` is `[InProgress, Completed, Failed, Stopping, Stopped, Deleting]`
/// (`smithy_query(query="shape",shape="EvaluationJobStatus")`); an accepted job runs.
const IN_PROGRESS: &str = "InProgress";

/// `EvaluationJobType` is `[Human, Automated]`
/// (`smithy_query(query="shape",shape="EvaluationJobType")`).
const AUTOMATED: &str = "Automated";
const HUMAN: &str = "Human";

/// The most tags one resource of this service may carry, as the handler enforces it.
const MAX_TAGS: usize =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MAX_TAGS_PER_RESOURCE;

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

/// An `automated` `EvaluationConfig` — the variant whose `EvaluationJobType` is
/// `Automated`.
fn automated_config(task_type: &str, dataset: &str, metrics: &[&str]) -> Value {
    json!({
        "automated": {
            "datasetMetricConfigs": [dataset_metric_config(task_type, dataset, metrics)],
        },
    })
}

/// A `human` `EvaluationConfig` — the variant whose `EvaluationJobType` is `Human`.
fn human_config(task_type: &str, dataset: &str, metric: &str, rating_method: &str) -> Value {
    json!({
        "human": {
            "humanWorkflowConfig": {
                "flowDefinitionArn":
                    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:flow-definition/eval-review"),
                "instructions": "Rate each response.",
            },
            "customMetrics": [{ "name": metric, "ratingMethod": rating_method }],
            "datasetMetricConfigs": [dataset_metric_config(task_type, dataset, &[metric])],
        },
    })
}

/// One `EvaluationModelConfig` naming a Bedrock model.
///
/// `inferenceParams` is sent explicitly on every row because the member is modelled
/// `@default("{}")` (`smithy_query(query="shape",shape="EvaluationBedrockModel")`), so a
/// request that omits it is not what a read reports back.
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

/// A `CreateEvaluationJob` body carrying the four `@required` members, plus whatever
/// `extra` adds.
fn create_body(job_name: &str, role: &str, output_uri: &str, config: Value, inference: Value) -> Value {
    json!({
        "jobName": job_name,
        "roleArn": role,
        "outputDataConfig": { "s3Uri": output_uri },
        "evaluationConfig": config,
        "inferenceConfig": inference,
    })
}

/// `body` with every member of `extra` added to it.
fn with(mut body: Value, extra: Value) -> Value {
    let members = body.as_object_mut().expect("an object");
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        members.insert(member, value);
    }
    body
}

/// `n` distinct tags, each key and value inside `TagKey`/`TagValue`'s own pattern
/// (`^[a-zA-Z0-9\s._:/=+@-]*$`).
fn tags(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| json!({ "key": format!("key-{index}"), "value": format!("value-{index}") }))
            .collect(),
    )
}

async fn create(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/evaluation-jobs", Some(body)).await
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/evaluation-jobs", Some(body))
        .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/evaluation-jobs/{}", label(identifier)),
        None,
    )
    .await
}

// ---------------------------------------------------------------------------------------
// Expectations the model fixes.
// ---------------------------------------------------------------------------------------

/// Whether `value` is an `EvaluationJobArn` of `region` whose resource id is the
/// `[a-z0-9]{12}` that shape declares.
fn is_minted_arn(value: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:bedrock:{region}:{ACCOUNT}:evaluation-job/");
    value.strip_prefix(&prefix).is_some_and(|id| {
        id.len() == 12 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// The member `name` of a create body, or `Value::Null` when it carried none.
fn sent(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// The `EvaluationJobType` the `evaluationConfig` of `body` names: the union's variant.
fn expected_job_type(body: &Value) -> &'static str {
    let config = body.get("evaluationConfig").expect("an evaluationConfig");
    if config.get("automated").is_some() {
        AUTOMATED
    } else {
        HUMAN
    }
}

// ---------------------------------------------------------------------------------------
// 1-3. A created job round-trips through a read, with the type and status it is owed.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_evaluation_job_round_trips_through_a_read() {
    let emu = emulator();

    // Rows differ along what the rule generalises over: the evaluation configuration
    // (automated and human), the inference configuration (each `models` variant and
    // `ragConfigs`), the application type, the key spelling, the optional description and
    // tags, and the name lengths the model admits (`EvaluationJobName` is
    // `@length(1..63)`).
    let long_name = format!("j{}", "x".repeat(62));
    let rows: Vec<(&str, Value)> = vec![
        (
            "automated, required members only",
            create_body(
                "eval-one",
                ROLE_ARNS[0],
                OUTPUT_URIS[0],
                automated_config(TASK_TYPES[0], "Builtin.Gigaword", &["Builtin.Accuracy"]),
                models_inference(vec![bedrock_model(MODEL_IDS[0])]),
            ),
        ),
        (
            "automated, a description, a key ARN, tags and an application type",
            with(
                create_body(
                    "eval-two",
                    ROLE_ARNS[1],
                    OUTPUT_URIS[1],
                    automated_config(
                        TASK_TYPES[1],
                        "Builtin.NaturalQuestions",
                        &["Builtin.Accuracy", "Builtin.Robustness"],
                    ),
                    models_inference(vec![bedrock_model(MODEL_IDS[1])]),
                ),
                json!({
                    "jobDescription": "nightly accuracy and robustness run",
                    "customerEncryptionKeyId":
                        format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{KMS_KEY_ID}"),
                    "jobTags": tags(3),
                    "applicationType": "ModelEvaluation",
                }),
            ),
        ),
        (
            "automated over a precomputed inference source, with a bare key id",
            with(
                create_body(
                    "eval-three",
                    ROLE_ARNS[0],
                    OUTPUT_URIS[0],
                    automated_config(TASK_TYPES[2], "custom-prompts", &["Builtin.Toxicity"]),
                    models_inference(vec![json!({
                        "precomputedInferenceSource": {
                            "inferenceSourceIdentifier": "recorded-responses.v1",
                        },
                    })]),
                ),
                json!({ "customerEncryptionKeyId": KMS_KEY_ID }),
            ),
        ),
        (
            "human, two models and a key alias",
            with(
                create_body(
                    "eval-four",
                    ROLE_ARNS[1],
                    OUTPUT_URIS[1],
                    human_config(
                        TASK_TYPES[0],
                        "custom-prompts",
                        "readability",
                        "ComparisonChoice",
                    ),
                    models_inference(vec![
                        bedrock_model(MODEL_IDS[0]),
                        bedrock_model(MODEL_IDS[2]),
                    ]),
                ),
                json!({ "customerEncryptionKeyId": KMS_KEY_ALIAS }),
            ),
        ),
        (
            "automated over a knowledge base, with ragConfigs",
            with(
                create_body(
                    "eval-five",
                    ROLE_ARNS[0],
                    OUTPUT_URIS[0],
                    automated_config(
                        TASK_TYPES[1],
                        "custom-prompts",
                        &["Builtin.ContextRelevance"],
                    ),
                    json!({
                        "ragConfigs": [{
                            "precomputedRagSourceConfig": {
                                "retrieveSourceConfig": {
                                    "ragSourceIdentifier": "kb-retrieve-only.v1",
                                },
                            },
                        }],
                    }),
                ),
                json!({ "applicationType": "RagEvaluation" }),
            ),
        ),
        (
            "a name at the minimum modelled length",
            create_body(
                "a",
                ROLE_ARNS[0],
                OUTPUT_URIS[1],
                automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
                models_inference(vec![bedrock_model(MODEL_IDS[1])]),
            ),
        ),
        (
            "a name at the maximum modelled length",
            create_body(
                &long_name,
                ROLE_ARNS[1],
                OUTPUT_URIS[0],
                automated_config(TASK_TYPES[2], "Builtin.Wikitext2", &["Builtin.Toxicity"]),
                models_inference(vec![bedrock_model(MODEL_IDS[2])]),
            ),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, body) in rows {
        let created = create(&emu, body.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateEvaluationJob is bound to @http(code: 202), got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.content_type.as_deref(),
            Some("application/json"),
            "case {case:?}: a restJson1 response carries application/json, got {:?}",
            created.content_type
        );
        let job_arn = created.text("jobArn");
        assert!(
            is_minted_arn(&job_arn, REGION),
            "case {case:?}: jobArn must be an EvaluationJobArn of the signing region, got \
             {job_arn:?}"
        );
        assert!(
            !minted.contains(&job_arn),
            "case {case:?}: every create mints its own jobArn, but {job_arn:?} was already \
             minted ({minted:?})"
        );
        minted.push(job_arn.clone());

        let got = get(&emu, &job_arn).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: GetEvaluationJob must answer 200, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.get("jobArn"),
            json!(job_arn),
            "case {case:?}: the read must report the job the create minted, got {:?}",
            got.body
        );
        for member in [
            "jobName",
            "jobDescription",
            "roleArn",
            "customerEncryptionKeyId",
            "applicationType",
            "evaluationConfig",
            "inferenceConfig",
            "outputDataConfig",
        ] {
            assert_eq!(
                got.get(member),
                sent(&body, member),
                "case {case:?}: {member} must come back as the create sent it, got {:?}",
                got.get(member)
            );
        }
        // 2. `jobType` is the `EvaluationConfig` variant the request carried.
        assert_eq!(
            got.get("jobType"),
            json!(expected_job_type(&body)),
            "case {case:?}: jobType is the evaluationConfig variant, got {:?}",
            got.get("jobType")
        );
        // 3. The job is reported running, with no failure to report.
        assert_eq!(
            got.get("status"),
            json!(IN_PROGRESS),
            "case {case:?}: an accepted evaluation job is reported {IN_PROGRESS}, got {:?}",
            got.get("status")
        );
        assert_eq!(
            got.get("failureMessages"),
            Value::Null,
            "case {case:?}: a running job has no failureMessages, got {:?}",
            got.get("failureMessages")
        );
        let creation = got.instant("creationTime");
        let modified = got.instant("lastModifiedTime");
        assert!(
            (creation - modified).abs() < f64::EPSILON,
            "case {case:?}: a job nothing has changed was last modified when it was created, \
             got creationTime {creation} and lastModifiedTime {modified}"
        );

        // A read reports, so reading again answers the same thing.
        let again = get(&emu, &job_arn).await;
        assert_eq!(
            again.body, got.body,
            "case {case:?}: GetEvaluationJob must not change what a later caller sees, got \
             {:?} then {:?}",
            got.body, again.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// The wire form of the two instants a read reports.
// ---------------------------------------------------------------------------------------

/// Both `Timestamp` members of `GetEvaluationJobResponse` carry
/// `@timestampFormat("date-time")`
/// (`smithy_query(query="operation",operation="GetEvaluationJob")`), so their member
/// schemas must report that format — which is what makes the restJson1 codec write the RFC
/// 3339 string the rows above read with `Reply::instant` rather than the protocol's default
/// epoch number.
#[test]
fn every_date_time_member_schema_carries_the_format() {
    use ::aws_smithy_schema::traits::TimestampFormat;
    use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::ops::types::GetEvaluationJobOutput;

    let rows = vec![
        (
            "GetEvaluationJobResponse$creationTime",
            &GetEvaluationJobOutput::M_CREATION_TIME,
        ),
        (
            "GetEvaluationJobResponse$lastModifiedTime",
            &GetEvaluationJobOutput::M_LAST_MODIFIED_TIME,
        ),
    ];
    for (case, schema) in rows {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // The control row: a member the model gives no `@timestampFormat` reports none, so the
    // assertion above is about the trait and not about every member schema.
    let status = GetEvaluationJobOutput::M_STATUS
        .timestamp_format()
        .map(|trait_| trait_.format());
    assert!(
        status.is_none(),
        "case GetEvaluationJobResponse$status: the model gives this member no \
         @timestampFormat, got {status:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 4. A jobName names at most one job per account per region.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_job_name_names_at_most_one_job_per_account_per_region() {
    let emu = emulator();

    // Rows differ in the name's length and in the configuration the second create carries,
    // so the conflict is about the NAME and not about the body repeating.
    let long_name = format!("n{}", "y".repeat(62));
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "same name, same configuration",
            "duplicate-one".to_string(),
            automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
        ),
        (
            "same name, a human configuration instead",
            "duplicate-two".to_string(),
            human_config(TASK_TYPES[1], "custom-prompts", "accuracy", "ThumbsUpDown"),
        ),
        (
            "a name at the maximum modelled length",
            long_name,
            automated_config(TASK_TYPES[2], "Builtin.Wikitext2", &["Builtin.Toxicity"]),
        ),
        (
            "a name at the minimum modelled length",
            "z".to_string(),
            automated_config(TASK_TYPES[0], "Builtin.Gigaword", &["Builtin.Accuracy"]),
        ),
    ];

    for (case, name, second_config) in rows {
        let first = create(
            &emu,
            create_body(
                &name,
                ROLE_ARNS[0],
                OUTPUT_URIS[0],
                automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
                models_inference(vec![bedrock_model(MODEL_IDS[0])]),
            ),
        )
        .await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first create must be accepted, got {} {:?}",
            first.status, first.body
        );
        let first_arn = first.text("jobArn");

        let second = create(
            &emu,
            create_body(
                &name,
                ROLE_ARNS[1],
                OUTPUT_URIS[1],
                second_config.clone(),
                models_inference(vec![bedrock_model(MODEL_IDS[1])]),
            ),
        )
        .await;
        assert_eq!(
            second.status, 400,
            "case {case:?}: ConflictException is modelled @httpError(400), got {} {:?}",
            second.status, second.body
        );
        assert_eq!(
            second.error.as_deref(),
            Some("ConflictException"),
            "case {case:?}: reusing a job name in one region conflicts, got {:?} {:?}",
            second.error,
            second.body
        );

        // The same name in ANOTHER region names another job, which is minted.
        let elsewhere = create_in_region(
            &emu,
            "eu-central-1",
            create_body(
                &name,
                ROLE_ARNS[0],
                OUTPUT_URIS[0],
                second_config,
                models_inference(vec![bedrock_model(MODEL_IDS[2])]),
            ),
        )
        .await;
        assert_eq!(
            elsewhere.status, 202,
            "case {case:?}: the same name in another region is another job, got {} {:?}",
            elsewhere.status, elsewhere.body
        );
        let elsewhere_arn = elsewhere.text("jobArn");
        assert!(
            is_minted_arn(&elsewhere_arn, "eu-central-1"),
            "case {case:?}: the other region's job carries that region's ARN, got \
             {elsewhere_arn:?}"
        );
        assert_ne!(
            elsewhere_arn, first_arn,
            "case {case:?}: the two regions' jobs are two jobs, got {elsewhere_arn:?} twice"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. A replayed clientRequestToken mints nothing.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_mints_nothing() {
    let emu = emulator();

    // Rows differ in what the REPLAY carries beside the token: the same body, a different
    // name, and a different configuration — the token decides, not the body.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("the same body", "token-one", "replay-one", "replay-one"),
        (
            "a different job name",
            "token-two",
            "replay-two",
            "replay-two-other",
        ),
        (
            "a different name and configuration",
            "token-three-0123456789",
            "replay-three",
            "replay-three-other",
        ),
    ];

    for (case, token, first_name, replay_name) in rows {
        let first = create(
            &emu,
            with(
                create_body(
                    first_name,
                    ROLE_ARNS[0],
                    OUTPUT_URIS[0],
                    automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
                    models_inference(vec![bedrock_model(MODEL_IDS[0])]),
                ),
                json!({ "clientRequestToken": token }),
            ),
        )
        .await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first create must be accepted, got {} {:?}",
            first.status, first.body
        );
        let first_arn = first.text("jobArn");

        let replay = create(
            &emu,
            with(
                create_body(
                    replay_name,
                    ROLE_ARNS[1],
                    OUTPUT_URIS[1],
                    human_config(TASK_TYPES[1], "custom-prompts", "accuracy", "ThumbsUpDown"),
                    models_inference(vec![bedrock_model(MODEL_IDS[1])]),
                ),
                json!({ "clientRequestToken": token }),
            ),
        )
        .await;
        assert_eq!(
            replay.status, 202,
            "case {case:?}: a replayed @idempotencyToken is not an error, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.text("jobArn"),
            first_arn,
            "case {case:?}: the replay must answer with the job the first call minted, got {:?}",
            replay.body
        );

        // The read still reports the FIRST request's job, so the replay stored nothing.
        let got = get(&emu, &first_arn).await;
        assert_eq!(
            got.get("jobName"),
            json!(first_name),
            "case {case:?}: the stored job is the one the first call minted, got {:?}",
            got.get("jobName")
        );

        if replay_name != first_name {
            // And the name the replay carried was never minted, so it is still free.
            let fresh = create(
                &emu,
                create_body(
                    replay_name,
                    ROLE_ARNS[0],
                    OUTPUT_URIS[0],
                    automated_config(TASK_TYPES[2], "Builtin.Gigaword", &["Builtin.Toxicity"]),
                    models_inference(vec![bedrock_model(MODEL_IDS[2])]),
                ),
            )
            .await;
            assert_eq!(
                fresh.status, 202,
                "case {case:?}: the replay minted no job under {replay_name:?}, so that name \
                 is still free, got {} {:?}",
                fresh.status, fresh.body
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 6. An identifier no stored job answers to is refused.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_no_stored_job_answers_to_is_refused() {
    let emu = emulator();

    let created = create(
        &emu,
        create_body(
            "known-job",
            ROLE_ARNS[0],
            OUTPUT_URIS[0],
            automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
            models_inference(vec![bedrock_model(MODEL_IDS[0])]),
        ),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the create that seeds this suite must be accepted, got {} {:?}",
        created.status, created.body
    );
    let job_arn = created.text("jobArn");
    let job_id = job_arn
        .rsplit_once('/')
        .expect("an evaluation-job ARN carries its id after the slash")
        .1
        .to_string();

    // Rows span the ways a well-formed `EvaluationJobIdentifier` can name no stored job:
    // an id that was never minted, the live id in another region, and the live id under
    // another account.
    let rows: Vec<(&str, String)> = vec![
        (
            "an id that was never minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:evaluation-job/aaaaaaaaaaaa"),
        ),
        (
            "the live id in another region",
            format!("arn:aws:bedrock:eu-central-1:{ACCOUNT}:evaluation-job/{job_id}"),
        ),
        (
            "the live id under another account",
            format!("arn:aws:bedrock:{REGION}:999999999999:evaluation-job/{job_id}"),
        ),
    ];

    for (case, identifier) in rows {
        let got = get(&emu, &identifier).await;
        assert_eq!(
            got.status, 404,
            "case {case:?}: ResourceNotFoundException is modelled @httpError(404), got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: an identifier naming no stored job is not found, got {:?} {:?}",
            got.error,
            got.body
        );
    }

    // The live identifier still answers, so the refusals above removed nothing.
    let still = get(&emu, &job_arn).await;
    assert_eq!(
        still.status, 200,
        "the stored job must still answer after the refusals, got {} {:?}",
        still.status, still.body
    );
}

// ---------------------------------------------------------------------------------------
// 7. An automated job supports a single model or inference profile.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_automated_job_supports_a_single_model() {
    let emu = emulator();

    // Rows span the two `EvaluationModelConfig` variants in the pair, and the human
    // configuration that the SAME pair is admitted under.
    let pairs: Vec<(&str, Vec<Value>)> = vec![
        (
            "two bedrock models",
            vec![bedrock_model(MODEL_IDS[0]), bedrock_model(MODEL_IDS[1])],
        ),
        (
            "a bedrock model and a precomputed inference source",
            vec![
                bedrock_model(MODEL_IDS[2]),
                json!({
                    "precomputedInferenceSource": {
                        "inferenceSourceIdentifier": "recorded-responses.v2",
                    },
                }),
            ],
        ),
    ];

    for (index, (case, models)) in pairs.into_iter().enumerate() {
        let refused = create(
            &emu,
            create_body(
                &format!("too-many-models-{index}"),
                ROLE_ARNS[0],
                OUTPUT_URIS[0],
                automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
                models_inference(models.clone()),
            ),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: ValidationException is modelled @httpError(400), got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: an automated job naming two models is refused, got {:?} {:?}",
            refused.error,
            refused.body
        );

        // The same pair under a human configuration is what the model's doc admits.
        let accepted = create(
            &emu,
            create_body(
                &format!("human-two-models-{index}"),
                ROLE_ARNS[0],
                OUTPUT_URIS[0],
                human_config(TASK_TYPES[0], "custom-prompts", "accuracy", "ComparisonChoice"),
                models_inference(models.clone()),
            ),
        )
        .await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: a human job compares up to two models, got {} {:?}",
            accepted.status, accepted.body
        );

        // And one model under the automated configuration is accepted, so the refusal is
        // about the COUNT.
        let single = create(
            &emu,
            create_body(
                &format!("automated-one-model-{index}"),
                ROLE_ARNS[0],
                OUTPUT_URIS[0],
                automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
                models_inference(vec![models[0].clone()]),
            ),
        )
        .await;
        assert_eq!(
            single.status, 202,
            "case {case:?}: an automated job naming one model is accepted, got {} {:?}",
            single.status, single.body
        );

        // The refused create stored nothing, so its name is still free.
        let reused = create(
            &emu,
            create_body(
                &format!("too-many-models-{index}"),
                ROLE_ARNS[0],
                OUTPUT_URIS[0],
                automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
                models_inference(vec![models[0].clone()]),
            ),
        )
        .await;
        assert_eq!(
            reused.status, 202,
            "case {case:?}: the refused create minted nothing, so its name is still free, got \
             {} {:?}",
            reused.status, reused.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 8. The per-resource tag bound.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn more_tags_than_a_resource_may_carry_is_refused() {
    let emu = emulator();

    // Rows walk the bound: none, one, exactly the bound, and one past it.
    let rows: Vec<(&str, usize, u16)> = vec![
        ("no tags", 0, 202),
        ("one tag", 1, 202),
        ("the bound itself", MAX_TAGS, 202),
        ("one past the bound", MAX_TAGS + 1, 400),
    ];

    for (case, count, expected) in rows {
        let name = format!("tagged-{count}");
        let reply = create(
            &emu,
            with(
                create_body(
                    &name,
                    ROLE_ARNS[0],
                    OUTPUT_URIS[0],
                    automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
                    models_inference(vec![bedrock_model(MODEL_IDS[0])]),
                ),
                json!({ "jobTags": tags(count) }),
            ),
        )
        .await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: {count} tags must answer {expected}, got {} {:?}",
            reply.status, reply.body
        );

        if expected == 400 {
            assert_eq!(
                reply.error.as_deref(),
                Some("ValidationException"),
                "case {case:?}: more tags than a resource may carry is a ValidationException, \
                 got {:?} {:?}",
                reply.error,
                reply.body
            );
            // Nothing was stored, so the name is still free.
            let retried = create(
                &emu,
                with(
                    create_body(
                        &name,
                        ROLE_ARNS[0],
                        OUTPUT_URIS[0],
                        automated_config(TASK_TYPES[0], "Builtin.BoolQ", &["Builtin.Accuracy"]),
                        models_inference(vec![bedrock_model(MODEL_IDS[0])]),
                    ),
                    json!({ "jobTags": tags(MAX_TAGS) }),
                ),
            )
            .await;
            assert_eq!(
                retried.status, 202,
                "case {case:?}: the refused create minted nothing, so its name is still free, \
                 got {} {:?}",
                retried.status, retried.body
            );
        } else {
            let job_arn = reply.text("jobArn");
            let got = get(&emu, &job_arn).await;
            assert_eq!(
                got.status, 200,
                "case {case:?}: the accepted job must read back, got {} {:?}",
                got.status, got.body
            );
        }
    }
}

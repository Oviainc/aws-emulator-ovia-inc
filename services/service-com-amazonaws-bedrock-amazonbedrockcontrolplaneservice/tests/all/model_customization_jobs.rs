// SPDX-License-Identifier: CC0-1.0

//! The model customization job resource end-to-end: `CreateModelCustomizationJob` writes
//! it, `GetModelCustomizationJob` reads it back and `StopModelCustomizationJob` winds it
//! down, over the same `MultiServiceRouter` the emulator boots (`crate::support`).
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A created job round-trips**: the create answers the modelled 201 with a `jobArn` of
//!    the form `ModelCustomizationJobArn` declares, and every member
//!    `GetModelCustomizationJob` reports comes back out of the stored record — for requests
//!    differing in base model spelling, customization type, hyperparameters, validation
//!    data, VPC configuration, KMS key spelling, tag counts and name lengths (both modelled
//!    bounds). Both `ModelCustomizationJobIdentifier` spellings — the job ARN and the bare
//!    `jobName` — address the same job.
//! 2. **An identifier no stored job answers to is refused** with 404
//!    `ResourceNotFoundException`, over rows spanning a never-created name, a live name in
//!    another region, a well-formed ARN that was never minted and a prefix of a live name.
//! 3. **`baseModelIdentifier` is resolved against the published catalog**: an identifier
//!    that names neither a model the catalog offers nor a custom model of this account is
//!    404 `ResourceNotFoundException`, and a customization the catalog does not publish for
//!    the resolved model is 400 `ValidationException` — over rows spanning every
//!    customization the catalog's `customizationsSupported` carries.
//! 4. **`status` is non-terminal first and settles to `Completed`**, and completing the job
//!    is what brings its output model into being: `InProgress` with no `endTime` straight
//!    after the create and no custom model yet, `Completed` with an `endTime` once
//!    `MODEL_CUSTOMIZATION_JOB_COMPLETE_AFTER` has passed, with `GetCustomModel` then
//!    reporting that model as `Active` and carrying the job's own members. The settle is
//!    owed by the clock, not by the read, so reading twice reports the same instants.
//! 5. **A stop moves a running job to `Stopping` and then to `Stopped`**, a second stop is
//!    an idempotent no-op that moves nothing, a stop of a job that completed is refused with
//!    `ValidationException`, a stop of an unknown identifier is 404 — and a stopped job
//!    produces NO custom model.
//! 6. **A replayed `clientRequestToken` mints nothing**: the replay answers with the job the
//!    first call minted and the names the replay carried stay unminted, while a fresh token
//!    mints another job.
//! 7. **A `jobName` and a `customModelName` each name at most one resource per account per
//!    region**, so a create reusing either conflicts while the same names in another region
//!    are minted.
//! 8. **More than `MAX_TAGS_PER_RESOURCE` tags on either tagged resource is refused** with
//!    `TooManyTagsException`, and nothing is stored.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateModelCustomizationJob,GetModelCustomizationJob,StopModelCustomizationJob")`
//! / `query="shape"`), or from the published foundation model catalog this service reports
//! through `ListFoundationModels`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::support::{date_time_seconds, emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model and the published catalog admit for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// The published foundation model catalog this service serves: the rows
/// `ListFoundationModels` reports (`state::FOUNDATION_MODELS`). The base models these rows
/// carry are PICKED OUT OF IT by what each row publishes in `customizationsSupported`,
/// rather than pinned by id, so a row follows the catalog the service serves instead of a
/// spelling it may add to or drop.
const CATALOG: &[service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FoundationModel] =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FOUNDATION_MODELS;

/// The `index`-th base model whose published `customizationsSupported` is `FINE_TUNING`
/// and nothing else.
fn fine_tuning_only(index: usize) -> &'static str {
    let models: Vec<&'static str> = CATALOG
        .iter()
        .filter(|model| {
            model.customizations_supported.len() == 1
                && model.customizations_supported[0] == FINE_TUNING
        })
        .map(|model| model.model_id)
        .collect();
    models.get(index).copied().unwrap_or_else(|| {
        panic!(
            "the catalog must publish at least {} fine-tuning-only base model(s), it publishes \
             {}",
            index + 1,
            models.len()
        )
    })
}

/// A base model the catalog publishes as supporting `customization`.
fn model_supporting(customization: &str) -> &'static str {
    CATALOG
        .iter()
        .find(|model| model.customizations_supported.contains(&customization))
        .map(|model| model.model_id)
        .unwrap_or_else(|| {
            panic!("the catalog must publish a base model supporting {customization}")
        })
}

/// A base model the catalog publishes with NO customization at all.
fn uncustomizable_model() -> &'static str {
    CATALOG
        .iter()
        .find(|model| model.customizations_supported.is_empty())
        .map(|model| model.model_id)
        .unwrap_or_else(|| {
            panic!("the catalog must publish a base model that supports no customization")
        })
}

/// `RoleArn` values the model admits (pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const ROLE_ARNS: &[&str] = &[
    "arn:aws:iam::000000000000:role/bedrock-customization",
    "arn:aws:iam::000000000000:role/service-role/BedrockCustomizationExecution",
];

/// `S3Uri` values the model admits (pattern
/// `^s3://[a-z0-9][-.a-z0-9]{1,61}[a-z0-9](?:/…)?/?$`).
const TRAINING_URIS: &[&str] = &[
    "s3://customization-data/train.jsonl",
    "s3://customization.data.bucket/fine-tuning/train",
];
const OUTPUT_URIS: &[&str] = &[
    "s3://customization-output/",
    "s3://customization.data.bucket/output",
];

/// A KMS key ARN `customModelKmsKeyId` admits (its pattern's key-ARN alternative), and the
/// bare 36-character key id and the alias the same pattern admits beside it.
const KMS_KEY_ID: &str = "11111111-2222-3333-4444-555555555555";
const KMS_KEY_ALIAS: &str = "alias/custom-models";

/// `ModelCustomizationJobStatus` is `[InProgress, Completed, Failed, Stopping, Stopped]`
/// (`smithy_query(query="shape",shape="ModelCustomizationJobStatus")`).
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";
const STOPPING: &str = "Stopping";
const STOPPED: &str = "Stopped";

/// `CustomizationType` values (`smithy_query(query="shape",shape="CustomizationType")`).
const FINE_TUNING: &str = "FINE_TUNING";
const CONTINUED_PRE_TRAINING: &str = "CONTINUED_PRE_TRAINING";
const DISTILLATION: &str = "DISTILLATION";

/// The most tags one resource of this service may carry, as `TooManyTagsException`
/// documents it ("50 tags per resource") and the handler enforces it.
const MAX_TAGS: usize =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MAX_TAGS_PER_RESOURCE;

/// The emulator's span for running a customization, and for winding one down, which the
/// handlers store as due instants rather than wait on.
const COMPLETE_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_CUSTOMIZATION_JOB_COMPLETE_AFTER;
const STOPPED_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_CUSTOMIZATION_JOB_STOPPED_AFTER;

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// A `CreateModelCustomizationJob` body carrying only the six `@required` members, plus
/// whatever `extra` adds.
fn create_body(
    job_name: &str,
    model_name: &str,
    base_model: &str,
    role: &str,
    training_uri: &str,
    output_uri: &str,
    extra: Value,
) -> Value {
    let mut body = json!({
        "jobName": job_name,
        "customModelName": model_name,
        "baseModelIdentifier": base_model,
        "roleArn": role,
        "trainingDataConfig": { "s3Uri": training_uri },
        "outputDataConfig": { "s3Uri": output_uri },
    });
    let members = body.as_object_mut().expect("an object");
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        members.insert(member, value);
    }
    body
}

/// A minimal create body over a fine-tuning base model.
fn simple_body(job_name: &str, model_name: &str) -> Value {
    create_body(
        job_name,
        model_name,
        fine_tuning_only(0),
        ROLE_ARNS[0],
        TRAINING_URIS[0],
        OUTPUT_URIS[0],
        json!({}),
    )
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
    emu.call("POST", "/model-customization-jobs", Some(body)).await
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/model-customization-jobs", Some(body))
        .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/model-customization-jobs/{}", label(identifier)),
        None,
    )
    .await
}

async fn get_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/model-customization-jobs/{}", label(identifier)),
        None,
    )
    .await
}

async fn stop(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "POST",
        &format!("/model-customization-jobs/{}/stop", label(identifier)),
        None,
    )
    .await
}

async fn get_custom_model(emu: &Emulator, identifier: &str) -> Reply {
    emu.call("GET", &format!("/custom-models/{}", label(identifier)), None)
        .await
}

// ---------------------------------------------------------------------------------------
// Expectations the model and the catalog fix.
// ---------------------------------------------------------------------------------------

/// Whether `value` is an ARN of `resource_type` in `REGION` carrying `base_model` as its
/// base-model segment and the `[a-z0-9]{12}` id both `ModelCustomizationJobArn` and
/// `CustomModelArn` declare.
fn is_minted_arn(value: &str, resource_type: &str, base_model: &str) -> bool {
    let prefix = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:{resource_type}/{base_model}/");
    value.strip_prefix(&prefix).is_some_and(|id| {
        id.len() == 12 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// The `FoundationModelArn` a read reports for a base model id: an EMPTY account field,
/// since a foundation model is published by AWS rather than owned by the reading account.
fn foundation_model_arn(model_id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}::foundation-model/{model_id}")
}

/// The `outputModelKmsKeyArn` a read reports for a request's `customModelKmsKeyId`.
///
/// `customModelKmsKeyId` admits a key ARN, a bare 36-character key id or an alias, while
/// `KmsKeyArn` (`^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`)
/// admits only a key ARN: so a key ARN passes through, a bare id is completed into one, and
/// an alias — which resolves to a key id only inside KMS — is reported by nothing.
fn expected_kms_key_arn(body: &Value) -> Value {
    let Some(key_id) = body.get("customModelKmsKeyId").and_then(Value::as_str) else {
        return Value::Null;
    };
    if key_id.starts_with("arn:") {
        return if key_id.contains(":key/") {
            json!(key_id)
        } else {
            Value::Null
        };
    }
    let is_key_id = key_id.len() == 36
        && key_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if is_key_id {
        json!(format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{key_id}"))
    } else {
        Value::Null
    }
}

/// The member `name` of a create body, or `Value::Null` when it carried none.
fn sent(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// The `message` a refusal states.
fn refusal_message(reply: &Reply) -> String {
    reply.get("message").as_str().unwrap_or_default().to_string()
}

/// The instant a CUSTOM MODEL read reports, in epoch seconds, read from either wire form.
///
/// The JSON codec renders a `Timestamp` member from its member schema's own format, and the
/// custom-model surface's members take the protocol's epoch-seconds default rather than the
/// `date-time` string the customization job's own read carries — the rule under test here is
/// the INSTANT, so this reads either form rather than pinning one.
fn model_instant(case: &str, reply: &Reply, name: &str) -> f64 {
    let member = reply.get(name);
    if let Some(text) = member.as_str() {
        return date_time_seconds(case, text);
    }
    member.as_f64().unwrap_or_else(|| {
        panic!(
            "case {case}: member {name} is a Timestamp, so it is either an RFC 3339 string or \
             epoch seconds, got {member}"
        )
    })
}

// ---------------------------------------------------------------------------------------
// 1. A created job round-trips through a read, under both identifier spellings.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_customization_job_round_trips_through_a_read() {
    let emu = emulator();

    // Rows differ along what the rule generalises over: the base model and the spelling it
    // is named by, the customization type, the hyperparameters, the validation data, the
    // VPC configuration, the KMS key spelling, how many tags each tagged resource carries,
    // and the name lengths the model admits (`JobName` is `@length(1..63)` and
    // `CustomModelName` `@length(1..63)`).
    let long_job_name = format!("j{}", "x".repeat(62));
    let long_model_name = format!("m{}", "y".repeat(62));
    let rows: Vec<(&str, Value)> = vec![
        (
            "required members only",
            simple_body("tune-one", "TunedOne"),
        ),
        (
            "the base model named by its foundation-model ARN",
            create_body(
                "tune-two",
                "Tuned_Two",
                &foundation_model_arn(fine_tuning_only(1)),
                ROLE_ARNS[1],
                TRAINING_URIS[1],
                OUTPUT_URIS[1],
                json!({
                    "customizationType": FINE_TUNING,
                    "hyperParameters": { "epochCount": "2", "learningRate": "0.00001" },
                }),
            ),
        ),
        (
            "continued pre-training, validators and a key ARN",
            create_body(
                "tune.three",
                "Tuned-Three",
                model_supporting(CONTINUED_PRE_TRAINING),
                ROLE_ARNS[0],
                TRAINING_URIS[0],
                OUTPUT_URIS[1],
                json!({
                    "customizationType": CONTINUED_PRE_TRAINING,
                    "validationDataConfig": {
                        "validators": [
                            { "s3Uri": "s3://customization-data/validate-a.jsonl" },
                            { "s3Uri": "s3://customization-data/validate-b.jsonl" },
                        ],
                    },
                    "customModelKmsKeyId": format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{KMS_KEY_ID}"),
                    "jobTags": tags(1),
                    "customModelTags": tags(MAX_TAGS),
                }),
            ),
        ),
        (
            "distillation with its customizationConfig and a VPC",
            create_body(
                "tune-four",
                "TunedFour",
                model_supporting(DISTILLATION),
                ROLE_ARNS[1],
                TRAINING_URIS[1],
                OUTPUT_URIS[0],
                json!({
                    "customizationType": DISTILLATION,
                    "customizationConfig": {
                        "distillationConfig": {
                            "teacherModelConfig": {
                                "teacherModelIdentifier": fine_tuning_only(2),
                                "maxResponseLengthForInference": 512,
                            },
                        },
                    },
                    "vpcConfig": {
                        "subnetIds": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"],
                        "securityGroupIds": ["sg-0123456789abcdef0"],
                    },
                    "customModelKmsKeyId": KMS_KEY_ID,
                }),
            ),
        ),
        (
            "a key alias, which resolves to a key id only inside KMS",
            create_body(
                "tune-five",
                "TunedFive",
                fine_tuning_only(2),
                ROLE_ARNS[0],
                TRAINING_URIS[0],
                OUTPUT_URIS[0],
                json!({ "customModelKmsKeyId": KMS_KEY_ALIAS }),
            ),
        ),
        (
            "names at the minimum modelled length",
            create_body(
                "a",
                "b",
                fine_tuning_only(1),
                ROLE_ARNS[1],
                TRAINING_URIS[1],
                OUTPUT_URIS[1],
                json!({}),
            ),
        ),
        (
            "names at the maximum modelled length",
            create_body(
                &long_job_name,
                &long_model_name,
                fine_tuning_only(0),
                ROLE_ARNS[0],
                TRAINING_URIS[0],
                OUTPUT_URIS[1],
                json!({ "clientRequestToken": "a-token-for-the-longest-names" }),
            ),
        ),
    ];

    for (case, body) in rows {
        let created = create(&emu, body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateModelCustomizationJob is bound to @http(code: 201), got {} \
             {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.content_type.as_deref(),
            Some("application/json"),
            "case {case:?}: a restJson1 response carries application/json, got {:?}",
            created.content_type
        );
        let job_arn = created.text("jobArn");
        // The base model segment the ARN carries is the catalog row's own `modelId`,
        // whichever spelling the request named it by.
        let base_model_id = body
            .get("baseModelIdentifier")
            .and_then(Value::as_str)
            .expect("a baseModelIdentifier string")
            .rsplit_once(":foundation-model/")
            .map_or_else(
                || {
                    body.get("baseModelIdentifier")
                        .and_then(Value::as_str)
                        .expect("a baseModelIdentifier string")
                        .to_string()
                },
                |(_, id)| id.to_string(),
            );
        assert!(
            is_minted_arn(&job_arn, "model-customization-job", &base_model_id),
            "case {case:?}: jobArn must be a ModelCustomizationJobArn of the signing region \
             carrying the base model, got {job_arn:?}"
        );

        let job_name = sent(&body, "jobName");
        let readers: Vec<(&str, String)> = vec![
            ("by jobArn", job_arn.clone()),
            (
                "by jobName",
                job_name.as_str().expect("a jobName string").to_string(),
            ),
        ];
        for (reader, identifier) in readers {
            let got = get(&emu, &identifier).await;
            assert_eq!(
                got.status, 200,
                "case {case:?} read {reader}: GetModelCustomizationJob must answer 200, got {} \
                 {:?}",
                got.status, got.body
            );
            assert_eq!(
                got.get("jobArn"),
                json!(job_arn),
                "case {case:?} read {reader}: the read must report the job the create minted, \
                 got {:?}",
                got.body
            );
            assert_eq!(
                got.get("jobName"),
                sent(&body, "jobName"),
                "case {case:?} read {reader}: jobName must come back as the create sent it, \
                 got {:?}",
                got.get("jobName")
            );
            assert_eq!(
                got.get("outputModelName"),
                sent(&body, "customModelName"),
                "case {case:?} read {reader}: outputModelName is the customModelName the \
                 create sent, got {:?}",
                got.get("outputModelName")
            );
            for member in [
                "roleArn",
                "trainingDataConfig",
                "outputDataConfig",
                "hyperParameters",
                "vpcConfig",
                "customizationConfig",
                "clientRequestToken",
            ] {
                assert_eq!(
                    got.get(member),
                    sent(&body, member),
                    "case {case:?} read {reader}: {member} must come back as the create sent \
                     it, got {:?}",
                    got.get(member)
                );
            }
            // `validationDataConfig` is optional in the request and REQUIRED in the read,
            // and its `validators` list admits zero entries — so a create that named none
            // is the job with no validators.
            let expected_validation = match sent(&body, "validationDataConfig") {
                Value::Null => json!({ "validators": [] }),
                sent => sent,
            };
            assert_eq!(
                got.get("validationDataConfig"),
                expected_validation,
                "case {case:?} read {reader}: validationDataConfig must report the validators \
                 the create named, got {:?}",
                got.get("validationDataConfig")
            );
            // `customizationType` is optional in the request, and this operation "creates a
            // fine-tuning job", so a create that named none is a fine-tuning job.
            let expected_type = match sent(&body, "customizationType") {
                Value::Null => json!(FINE_TUNING),
                sent => sent,
            };
            assert_eq!(
                got.get("customizationType"),
                expected_type,
                "case {case:?} read {reader}: customizationType must be the one the create \
                 named, or FINE_TUNING, got {:?}",
                got.get("customizationType")
            );
            assert_eq!(
                got.get("baseModelArn"),
                json!(foundation_model_arn(&base_model_id)),
                "case {case:?} read {reader}: baseModelArn is the FoundationModelArn of the \
                 catalog row the request resolved to, got {:?}",
                got.get("baseModelArn")
            );
            assert_eq!(
                got.get("outputModelKmsKeyArn"),
                expected_kms_key_arn(&body),
                "case {case:?} read {reader}: outputModelKmsKeyArn must be the key ARN the \
                 request's key id names, got {:?}",
                got.get("outputModelKmsKeyArn")
            );
            let output_model_arn = got.text("outputModelArn");
            assert!(
                is_minted_arn(&output_model_arn, "custom-model", &base_model_id),
                "case {case:?} read {reader}: outputModelArn must be a CustomModelArn of the \
                 signing region carrying the base model, got {output_model_arn:?}"
            );
            assert_eq!(
                got.text("status"),
                IN_PROGRESS,
                "case {case:?} read {reader}: a customization runs after the create returns, \
                 got {:?}",
                got.body
            );
            assert!(
                got.get("endTime").is_null(),
                "case {case:?} read {reader}: a running job has not ended, got {:?}",
                got.get("endTime")
            );
            assert!(
                (got.instant("lastModifiedTime") - got.instant("creationTime")).abs()
                    < f64::EPSILON,
                "case {case:?} read {reader}: nothing has changed the job since it was \
                 created, got {:?}",
                got.body
            );
            // No customization ran, so there are no metrics and no sub-task statuses to
            // report rather than fabricated ones.
            for member in [
                "trainingMetrics",
                "validationMetrics",
                "statusDetails",
                "failureMessage",
            ] {
                assert!(
                    got.get(member).is_null(),
                    "case {case:?} read {reader}: {member} is reported by nothing until the \
                     customization computes it, got {:?}",
                    got.get(member)
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// 2. An identifier no stored job answers to.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_no_job_answers_to_is_not_found() {
    let emu = emulator();
    let created = create(&emu, simple_body("live-job", "LiveModel")).await;
    assert_eq!(
        created.status, 201,
        "the live job must be created, got {} {:?}",
        created.status, created.body
    );
    let live_arn = created.text("jobArn");

    // A job created in ANOTHER region, so its name resolves only there.
    let other_region = "eu-central-1";
    let elsewhere = create_in_region(
        &emu,
        other_region,
        simple_body("elsewhere-job", "ElsewhereModel"),
    )
    .await;
    assert_eq!(
        elsewhere.status, 201,
        "the other region's job must be created, got {} {:?}",
        elsewhere.status, elsewhere.body
    );

    let never_minted = format!(
        "arn:aws:bedrock:{REGION}:{ACCOUNT}:model-customization-job/{}/abcdef012345",
        fine_tuning_only(0)
    );
    let other_region_arn = format!(
        "arn:aws:bedrock:{other_region}:{ACCOUNT}:model-customization-job/{}/abcdef012345",
        fine_tuning_only(0)
    );
    let rows: Vec<(&str, String)> = vec![
        ("a name no create ever used", "no-such-job".to_string()),
        ("a live name of another region", "elsewhere-job".to_string()),
        ("a well-formed ARN never minted", never_minted),
        ("an ARN of another region", other_region_arn),
        ("a prefix of a live name", "live".to_string()),
    ];
    for (case, identifier) in rows {
        let got = get(&emu, &identifier).await;
        assert_eq!(
            got.status, 404,
            "case {case:?}: an unknown job is ResourceNotFoundException (404), got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal must carry the modelled error code, got {:?}",
            got.error
        );
        assert!(
            refusal_message(&got).contains(&identifier),
            "case {case:?}: the message names the identifier that answered to nothing, got \
             {:?}",
            refusal_message(&got)
        );
    }

    // The live job is still readable, and the other region's job is readable THERE.
    let still = get(&emu, &live_arn).await;
    assert_eq!(
        still.status, 200,
        "the live job must still be readable after the refusals, got {} {:?}",
        still.status, still.body
    );
    let there = get_in_region(&emu, other_region, "elsewhere-job").await;
    assert_eq!(
        there.status, 200,
        "the other region's job is readable in its own region, got {} {:?}",
        there.status, there.body
    );
}

// ---------------------------------------------------------------------------------------
// 3. The base model and the customization the catalog publishes for it.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_base_model_the_account_cannot_customize_is_not_found() {
    let emu = emulator();

    // Rows span the two kinds of resource a `baseModelIdentifier` names — a FOUNDATION
    // model, which the catalog publishes, and a CUSTOM model, which this account holds — and
    // the spellings each is named by.
    let rows: Vec<(&str, String)> = vec![
        (
            "a bare id no catalog row carries",
            "acme.not-a-model-v1".to_string(),
        ),
        (
            "a foundation-model ARN of a model no row carries",
            foundation_model_arn("acme.not-a-model-v1"),
        ),
        (
            "a foundation-model ARN of another region",
            "arn:aws:bedrock:eu-central-1::foundation-model/acme.not-a-model-v1".to_string(),
        ),
        (
            "a custom-model ARN this account does not hold",
            format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{}/abcdef012345",
                fine_tuning_only(0)
            ),
        ),
    ];
    for (case, base_model) in rows {
        let refused = create(
            &emu,
            create_body(
                "job-for-unknown-base",
                "ModelForUnknownBase",
                &base_model,
                ROLE_ARNS[0],
                TRAINING_URIS[0],
                OUTPUT_URIS[0],
                json!({}),
            ),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: an identifier that names no base model to customize is \
             ResourceNotFoundException (404), got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal must carry the modelled error code, got {:?}",
            refused.error
        );
        assert!(
            refusal_message(&refused).contains(&base_model),
            "case {case:?}: the message names the identifier that named no base model, got \
             {:?}",
            refusal_message(&refused)
        );
        let after = get(&emu, "job-for-unknown-base").await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: a refused create stores nothing, got {} {:?}",
            after.status, after.body
        );
    }
}

#[tokio::test]
async fn a_customization_the_base_model_is_not_published_for_is_refused() {
    let emu = emulator();

    // Rows span the catalog's own `customizationsSupported` sets: a model published for
    // none, and models published for one customization asked for another.
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        (
            "a model published with no customization at all",
            uncustomizable_model(),
            FINE_TUNING,
            false,
        ),
        (
            "fine-tuning-only model asked for continued pre-training",
            fine_tuning_only(0),
            CONTINUED_PRE_TRAINING,
            false,
        ),
        (
            "fine-tuning-only model asked for distillation",
            fine_tuning_only(1),
            DISTILLATION,
            false,
        ),
        (
            "a pre-training model asked for distillation",
            model_supporting(CONTINUED_PRE_TRAINING),
            DISTILLATION,
            false,
        ),
        (
            "a pre-training model asked for continued pre-training",
            model_supporting(CONTINUED_PRE_TRAINING),
            CONTINUED_PRE_TRAINING,
            true,
        ),
        (
            "a distillation model asked for distillation",
            model_supporting(DISTILLATION),
            DISTILLATION,
            true,
        ),
        (
            "a fine-tuning model asked for fine-tuning",
            fine_tuning_only(2),
            FINE_TUNING,
            true,
        ),
    ];

    for (index, (case, base_model, customization_type, accepted)) in rows.into_iter().enumerate() {
        let job_name = format!("customization-row-{index}");
        let model_name = format!("CustomizationRow{index}");
        let reply = create(
            &emu,
            create_body(
                &job_name,
                &model_name,
                base_model,
                ROLE_ARNS[0],
                TRAINING_URIS[0],
                OUTPUT_URIS[0],
                json!({ "customizationType": customization_type }),
            ),
        )
        .await;
        if accepted {
            assert_eq!(
                reply.status, 201,
                "case {case:?}: the catalog publishes this customization for the model, got {} \
                 {:?}",
                reply.status, reply.body
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a customization the catalog does not publish for the model is \
             ValidationException (400), got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal must carry the modelled error code, got {:?}",
            reply.error
        );
        assert!(
            refusal_message(&reply).contains(customization_type),
            "case {case:?}: the message names the customization that was asked for, got {:?}",
            refusal_message(&reply)
        );
        let after = get(&emu, &job_name).await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: a refused create stores nothing, got {} {:?}",
            after.status, after.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. The status the clock owes, and the custom model completing the job produces.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_running_job_settles_to_completed_and_produces_its_custom_model() {
    assert!(
        COMPLETE_AFTER >= Duration::from_millis(250) && COMPLETE_AFTER <= Duration::from_millis(750),
        "the customization span must be observable but short, got {COMPLETE_AFTER:?}"
    );

    let emu = emulator();
    let body = create_body(
        "settling-job",
        "SettlingModel",
        model_supporting(CONTINUED_PRE_TRAINING),
        ROLE_ARNS[0],
        TRAINING_URIS[0],
        OUTPUT_URIS[0],
        json!({
            "customizationType": CONTINUED_PRE_TRAINING,
            "hyperParameters": { "epochCount": "1" },
            "validationDataConfig": {
                "validators": [{ "s3Uri": "s3://customization-data/validate.jsonl" }],
            },
        }),
    );
    let created = create(&emu, body.clone()).await;
    assert_eq!(
        created.status, 201,
        "the job must be created, got {} {:?}",
        created.status, created.body
    );
    let job_arn = created.text("jobArn");

    let immediate = get(&emu, &job_arn).await;
    assert_eq!(
        immediate.text("status"),
        IN_PROGRESS,
        "case read while customizing: the job is still running, got {:?}",
        immediate.body
    );
    let output_model_arn = immediate.text("outputModelArn");
    let while_running = get_custom_model(&emu, &output_model_arn).await;
    assert_eq!(
        while_running.status, 404,
        "case read while customizing: the custom model does not exist until the job \
         completes, got {} {:?}",
        while_running.status, while_running.body
    );

    tokio::time::sleep(COMPLETE_AFTER + Duration::from_millis(150)).await;
    let settled = get(&emu, &job_arn).await;
    assert_eq!(
        settled.text("status"),
        COMPLETED,
        "case read once the span has passed: the customization must have completed, got {:?}",
        settled.body
    );
    let ended = settled.instant("endTime");
    assert!(
        ended > settled.instant("creationTime"),
        "case read once the span has passed: the job ended after it was created, got {:?}",
        settled.body
    );
    assert!(
        (settled.instant("lastModifiedTime") - ended).abs() < f64::EPSILON,
        "case read once the span has passed: completing the job is what last changed it, got \
         {:?}",
        settled.body
    );

    // The transition was owed by the CLOCK, so a second read reports the very same instants
    // rather than ones moved on by the read itself.
    let again = get(&emu, &job_arn).await;
    assert_eq!(
        again.body, settled.body,
        "case read twice: a read reports the job, it does not move it on, got {:?} then {:?}",
        settled.body, again.body
    );

    // Completing the job is what brought the custom model into being, and that model is the
    // one the job named: readable under its ARN and under its bare name.
    for (reader, identifier) in [
        ("by modelArn", output_model_arn.clone()),
        ("by modelName", "SettlingModel".to_string()),
    ] {
        let model = get_custom_model(&emu, &identifier).await;
        assert_eq!(
            model.status, 200,
            "case read the produced model {reader}: GetCustomModel must answer 200, got {} \
             {:?}",
            model.status, model.body
        );
        assert_eq!(
            model.get("modelArn"),
            json!(output_model_arn),
            "case read the produced model {reader}: the model is the one the job named, got \
             {:?}",
            model.body
        );
        assert_eq!(
            model.get("modelName"),
            sent(&body, "customModelName"),
            "case read the produced model {reader}: the model carries the name the job gave \
             it, got {:?}",
            model.get("modelName")
        );
        assert_eq!(
            model.text("modelStatus"),
            "Active",
            "case read the produced model {reader}: a model a completed job produced is ready \
             to use, got {:?}",
            model.body
        );
        assert_eq!(
            model.get("jobArn"),
            json!(job_arn),
            "case read the produced model {reader}: the model names the job that produced it, \
             got {:?}",
            model.get("jobArn")
        );
        assert_eq!(
            model.get("jobName"),
            sent(&body, "jobName"),
            "case read the produced model {reader}: the model names that job, got {:?}",
            model.get("jobName")
        );
        assert_eq!(
            model.get("baseModelArn"),
            json!(foundation_model_arn(model_supporting(CONTINUED_PRE_TRAINING))),
            "case read the produced model {reader}: the model descends from the base model \
             the job customized, got {:?}",
            model.get("baseModelArn")
        );
        assert_eq!(
            model.get("customizationType"),
            sent(&body, "customizationType"),
            "case read the produced model {reader}: the model carries the customization the \
             job ran, got {:?}",
            model.get("customizationType")
        );
        for member in [
            "hyperParameters",
            "trainingDataConfig",
            "validationDataConfig",
            "outputDataConfig",
        ] {
            assert_eq!(
                model.get(member),
                settled.get(member),
                "case read the produced model {reader}: {member} is the job's, read off the \
                 one record that holds it, got {:?} against {:?}",
                model.get(member),
                settled.get(member)
            );
        }
        // The two instants come off the wire in different renderings — the job's as the
        // RFC 3339 string its member schema declares, the model's as the epoch-seconds
        // default of the custom-model surface — so they are compared to the millisecond
        // rather than bit for bit.
        assert!(
            (model_instant("the produced model", &model, "creationTime") - ended).abs() < 1e-3,
            "case read the produced model {reader}: the model came into being when the job \
             completed, got {:?}",
            model.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. Stopping a job.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_stop_winds_a_running_job_down_and_is_idempotent() {
    assert!(
        STOPPED_AFTER >= Duration::from_millis(250) && STOPPED_AFTER <= Duration::from_millis(750),
        "the stop span must be observable but short, got {STOPPED_AFTER:?}"
    );

    let emu = emulator();
    let created = create(&emu, simple_body("stopping-job", "StoppingModel")).await;
    assert_eq!(
        created.status, 201,
        "the job must be created, got {} {:?}",
        created.status, created.body
    );
    let job_arn = created.text("jobArn");
    let output_model_arn = get(&emu, &job_arn).await.text("outputModelArn");

    // Both identifier spellings stop the same job; the first stop is the one that moves it.
    let stopped = stop(&emu, "stopping-job").await;
    assert_eq!(
        stopped.status, 200,
        "case stop a running job: StopModelCustomizationJob is bound to @http(code: 200), got \
         {} {:?}",
        stopped.status, stopped.body
    );
    let stopping = get(&emu, &job_arn).await;
    assert_eq!(
        stopping.text("status"),
        STOPPING,
        "case stop a running job: stopping is asynchronous, so the job winds down first, got \
         {:?}",
        stopping.body
    );
    assert!(
        stopping.get("endTime").is_null(),
        "case stop a running job: a job that is winding down has not ended, got {:?}",
        stopping.get("endTime")
    );

    // A second stop asks for what is already in flight: an idempotent no-op that moves
    // neither the status nor the instants.
    let again = stop(&emu, &job_arn).await;
    assert_eq!(
        again.status, 200,
        "case stop a stopping job: the stop already asked for is answered again, got {} {:?}",
        again.status, again.body
    );
    let unmoved = get(&emu, &job_arn).await;
    assert_eq!(
        unmoved.body, stopping.body,
        "case stop a stopping job: the second stop moves nothing, got {:?} then {:?}",
        stopping.body, unmoved.body
    );

    tokio::time::sleep(STOPPED_AFTER + Duration::from_millis(150)).await;
    let terminal = get(&emu, &job_arn).await;
    assert_eq!(
        terminal.text("status"),
        STOPPED,
        "case read once the stop span has passed: the job must have stopped, got {:?}",
        terminal.body
    );
    assert!(
        terminal.instant("endTime") > terminal.instant("creationTime"),
        "case read once the stop span has passed: the job ended after it was created, got {:?}",
        terminal.body
    );

    // A stop of a job that already stopped is the same no-op, and a stopped job never
    // produced the model it was customizing.
    let third = stop(&emu, &job_arn).await;
    assert_eq!(
        third.status, 200,
        "case stop a stopped job: the stop is an idempotent no-op, got {} {:?}",
        third.status, third.body
    );
    let after = get(&emu, &job_arn).await;
    assert_eq!(
        after.body, terminal.body,
        "case stop a stopped job: nothing about the job moves, got {:?} then {:?}",
        terminal.body, after.body
    );
    let model = get_custom_model(&emu, &output_model_arn).await;
    assert_eq!(
        model.status, 404,
        "case stop a running job: a job that was stopped produces no custom model, got {} {:?}",
        model.status, model.body
    );
}

#[tokio::test]
async fn a_stop_of_a_job_that_is_no_longer_active_is_refused() {
    let emu = emulator();
    let created = create(&emu, simple_body("completing-job", "CompletingModel")).await;
    assert_eq!(
        created.status, 201,
        "the job must be created, got {} {:?}",
        created.status, created.body
    );
    let job_arn = created.text("jobArn");

    tokio::time::sleep(COMPLETE_AFTER + Duration::from_millis(150)).await;
    let completed = get(&emu, &job_arn).await;
    assert_eq!(
        completed.text("status"),
        COMPLETED,
        "the job must have completed before the stop, got {:?}",
        completed.body
    );

    let rows: Vec<(&str, String)> = vec![
        ("by jobArn", job_arn.clone()),
        ("by jobName", "completing-job".to_string()),
    ];
    for (case, identifier) in rows {
        let refused = stop(&emu, &identifier).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a job that ran to its end is no longer active, so the stop is \
             ValidationException (400), got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal must carry the modelled error code, got {:?}",
            refused.error
        );
    }
    let unmoved = get(&emu, &job_arn).await;
    assert_eq!(
        unmoved.text("status"),
        COMPLETED,
        "a refused stop moves nothing, got {:?}",
        unmoved.body
    );

    // An identifier no job answers to is not found, whichever spelling it is.
    for (case, identifier) in [
        ("a name no create used", "no-such-job".to_string()),
        (
            "a well-formed ARN never minted",
            format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:model-customization-job/{}/abcdef012345",
                fine_tuning_only(0)
            ),
        ),
    ] {
        let refused = stop(&emu, &identifier).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: stopping an unknown job is ResourceNotFoundException (404), got {} \
             {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal must carry the modelled error code, got {:?}",
            refused.error
        );
    }
}

// ---------------------------------------------------------------------------------------
// 6. Idempotency tokens.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_mints_nothing() {
    let emu = emulator();
    let token = "token-for-the-first-customization";

    let first = create(
        &emu,
        create_body(
            "token-job-one",
            "TokenModelOne",
            fine_tuning_only(0),
            ROLE_ARNS[0],
            TRAINING_URIS[0],
            OUTPUT_URIS[0],
            json!({ "clientRequestToken": token }),
        ),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "the first create must be accepted, got {} {:?}",
        first.status, first.body
    );
    let minted = first.text("jobArn");

    // The replay carries OTHER names: what it answers is the job the first call minted, and
    // the names it carried stay unminted.
    let replay = create(
        &emu,
        create_body(
            "token-job-two",
            "TokenModelTwo",
            fine_tuning_only(1),
            ROLE_ARNS[1],
            TRAINING_URIS[1],
            OUTPUT_URIS[1],
            json!({ "clientRequestToken": token }),
        ),
    )
    .await;
    assert_eq!(
        replay.status, 201,
        "case replayed token: a replay is answered, not refused, got {} {:?}",
        replay.status, replay.body
    );
    assert_eq!(
        replay.text("jobArn"),
        minted,
        "case replayed token: the replay answers with the job the first call minted, got {:?}",
        replay.body
    );
    for unminted in ["token-job-two", "TokenModelTwo"] {
        let got = get(&emu, unminted).await;
        assert_eq!(
            got.status, 404,
            "case replayed token: {unminted} must stay unminted, got {} {:?}",
            got.status, got.body
        );
    }

    // A FRESH token over the same names mints another job.
    let fresh = create(
        &emu,
        create_body(
            "token-job-two",
            "TokenModelTwo",
            fine_tuning_only(1),
            ROLE_ARNS[1],
            TRAINING_URIS[1],
            OUTPUT_URIS[1],
            json!({ "clientRequestToken": "another-token" }),
        ),
    )
    .await;
    assert_eq!(
        fresh.status, 201,
        "case fresh token: a fresh token mints another job, got {} {:?}",
        fresh.status, fresh.body
    );
    assert_ne!(
        fresh.text("jobArn"),
        minted,
        "case fresh token: the job it mints is not the first one, got {:?}",
        fresh.body
    );
}

// ---------------------------------------------------------------------------------------
// 7. The names a create may not reuse.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_job_name_and_a_custom_model_name_each_name_one_resource_per_region() {
    let emu = emulator();
    let created = create(&emu, simple_body("taken-job", "TakenModel")).await;
    assert_eq!(
        created.status, 201,
        "the first job must be created, got {} {:?}",
        created.status, created.body
    );
    let minted = created.text("jobArn");

    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a jobName a live job already holds",
            "taken-job",
            "AnotherModel",
            "taken-job",
        ),
        (
            "a customModelName a live job is already customizing",
            "another-job",
            "TakenModel",
            "TakenModel",
        ),
    ];
    for (case, job_name, model_name, named) in rows {
        let refused = create(&emu, simple_body(job_name, model_name)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: reusing the name is ConflictException (400), got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ConflictException"),
            "case {case:?}: the refusal must carry the modelled error code, got {:?}",
            refused.error
        );
        assert!(
            refusal_message(&refused).contains(named),
            "case {case:?}: the message names the name that is already taken, got {:?}",
            refusal_message(&refused)
        );
    }

    // The same names in ANOTHER region name another job, so they are minted there.
    let elsewhere = create_in_region(&emu, "us-east-2", simple_body("taken-job", "TakenModel")).await;
    assert_eq!(
        elsewhere.status, 201,
        "case the same names in another region: they name another job, got {} {:?}",
        elsewhere.status, elsewhere.body
    );
    assert_ne!(
        elsewhere.text("jobArn"),
        minted,
        "case the same names in another region: the job minted there is another one, got {:?}",
        elsewhere.body
    );

    // Once the job has completed, its output model holds the name as a CUSTOM MODEL, and a
    // create still may not reuse it.
    tokio::time::sleep(COMPLETE_AFTER + Duration::from_millis(150)).await;
    let model = get_custom_model(&emu, "TakenModel").await;
    assert_eq!(
        model.status, 200,
        "the completed job's model must be readable, got {} {:?}",
        model.status, model.body
    );
    let refused = create(&emu, simple_body("job-after-completion", "TakenModel")).await;
    assert_eq!(
        refused.error.as_deref(),
        Some("ConflictException"),
        "case a customModelName a custom model already holds: the create conflicts, got {:?} \
         {:?}",
        refused.error,
        refused.body
    );
}

// ---------------------------------------------------------------------------------------
// 8. The per-resource tag bound.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn more_tags_than_a_resource_may_carry_is_refused() {
    let emu = emulator();
    let rows: Vec<(&str, &str)> = vec![("jobTags", "jobTags"), ("customModelTags", "customModelTags")];
    for (case, member) in rows {
        let refused = create(
            &emu,
            create_body(
                "too-many-tags",
                "TooManyTagsModel",
                fine_tuning_only(0),
                ROLE_ARNS[0],
                TRAINING_URIS[0],
                OUTPUT_URIS[0],
                json!({ member: tags(MAX_TAGS + 1) }),
            ),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: more than {MAX_TAGS} tags on one resource is TooManyTagsException \
             (400), got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("TooManyTagsException"),
            "case {case:?}: the refusal must carry the modelled error code, got {:?}",
            refused.error
        );
        assert!(
            refusal_message(&refused).contains(member),
            "case {case:?}: the message names the member that carried too many tags, got {:?}",
            refusal_message(&refused)
        );
        let after = get(&emu, "too-many-tags").await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: a refused create stores nothing, got {} {:?}",
            after.status, after.body
        );
    }

    // Exactly the bound is accepted on both lists at once.
    let accepted = create(
        &emu,
        create_body(
            "at-the-tag-bound",
            "AtTheTagBoundModel",
            fine_tuning_only(0),
            ROLE_ARNS[0],
            TRAINING_URIS[0],
            OUTPUT_URIS[0],
            json!({ "jobTags": tags(MAX_TAGS), "customModelTags": tags(MAX_TAGS) }),
        ),
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case exactly the bound: {MAX_TAGS} tags on each resource is accepted, got {} {:?}",
        accepted.status, accepted.body
    );
}

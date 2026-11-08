// SPDX-License-Identifier: CC0-1.0

//! The model customization job resource as a client can observe it: what
//! `GetModelCustomizationJob` reports, what `ListModelCustomizationJobs` lists, what
//! `StopModelCustomizationJob` winds down, and what the clock owes a running job — over the
//! same `MultiServiceRouter` the emulator boots (`crate::support`).
//!
//! `CreateModelCustomizationJob` is what mints these jobs, and the `iam:PassRole` decision
//! that gates it — a role of the caller's own account is passed, another account's is not —
//! is asserted in `model_customization_pass_role.rs`, which also reads an admitted mint back
//! through `GetModelCustomizationJob`. The jobs THIS suite reads span states and shapes a
//! single create cannot reach at once, so they are written into the store the handlers share
//! — through the SAME minting helpers and the same shapes the create uses — which is also
//! how a rule about a span of wall-clock time is asserted without sleeping through one.
//!
//! The rules under test, each asserted over a SET of records rather than over one:
//!
//! 1. **A stored job round-trips through a read**: every member
//!    `GetModelCustomizationJob` reports comes out of the stored record, for records
//!    differing in customization type, hyperparameters, validation data, VPC configuration,
//!    KMS key, tag counts and name lengths (both modelled bounds), and both
//!    `ModelCustomizationJobIdentifier` spellings — the job ARN and the bare `jobName` —
//!    address the same job. The listing reports the same record through the summary shape.
//! 2. **An identifier no stored job answers to is refused** with 404
//!    `ResourceNotFoundException` — on the read and on the stop — over rows spanning a name
//!    no job carries, a live name of another region, a well-formed ARN that was never
//!    minted and a prefix of a live name. A job ARN naming ANOTHER region is refused one
//!    step earlier instead, with this service's own ARN sentence (400
//!    `ValidationException`, `arn_inputs.rs`): such an ARN is not usable at this endpoint
//!    at all, so it is never resolved against the store.
//! 3. **The listing is filtered, ordered and paged** by the operation's own members:
//!    `nameContains`, `statusEquals` and the creation-time bounds each have to hold,
//!    `sortOrder` turns the order round, `maxResults` bounds the page and `nextToken`
//!    resumes it with neither a gap nor a repeat — and a job of another region is listed
//!    only there.
//! 4. **`status` is non-terminal first and settles to `Completed`**, and completing the job
//!    is what brings its output model into being, which `GetCustomModel` then reports as
//!    `Active`. The settle is owed by the clock, not by the read, so reading twice reports
//!    the same instants.
//! 5. **A stop moves a running job to `Stopping` and then to `Stopped`**, a second stop is
//!    an idempotent no-op, a stop of a job that completed is refused with
//!    `ValidationException` — and a stopped job produces NO custom model.
//!
//! Every expectation is derived from the model
//! (`smithy_query(query="operation",operation="GetModelCustomizationJob,ListModelCustomizationJobs,StopModelCustomizationJob")`
//! / `query="shape"`), or from the published foundation model catalog this service reports
//! through `ListFoundationModels`.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use bedrock::ops::types::{
    OutputDataConfig, Tag, TrainingDataConfig, ValidationDataConfig, Validator, VpcConfig,
};
use bedrock::state::ModelCustomizationJobRecord;

use crate::support::{date_time_seconds, emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model and the published catalog admit for the records these rows carry.
// ---------------------------------------------------------------------------------------

/// The published foundation model catalog this service serves (`state::FOUNDATION_MODELS`),
/// so a row follows the catalog rather than a spelling it may add to or drop.
const CATALOG: &[bedrock::state::FoundationModel] = bedrock::state::FOUNDATION_MODELS;

/// `RoleArn` values the model admits (pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const ROLE_ARNS: &[&str] = &[
    "arn:aws:iam::000000000000:role/bedrock-customization",
    "arn:aws:iam::000000000000:role/service-role/BedrockCustomizationExecution",
];

/// `S3Uri` values the model admits.
const TRAINING_URIS: &[&str] = &[
    "s3://customization-data/train.jsonl",
    "s3://customization.data.bucket/fine-tuning/train",
];
const OUTPUT_URIS: &[&str] = &["s3://customization-output/", "s3://customization.data.bucket/output"];

/// `ModelCustomizationJobStatus` is `[InProgress, Completed, Failed, Stopping, Stopped]`
/// (`smithy_query(query="shape",shape="ModelCustomizationJobStatus")`).
const IN_PROGRESS: &str = bedrock::state::MODEL_CUSTOMIZATION_JOB_STATUS_IN_PROGRESS;
const COMPLETED: &str = bedrock::state::MODEL_CUSTOMIZATION_JOB_STATUS_COMPLETED;
const STOPPING: &str = bedrock::state::MODEL_CUSTOMIZATION_JOB_STATUS_STOPPING;
const STOPPED: &str = bedrock::state::MODEL_CUSTOMIZATION_JOB_STATUS_STOPPED;

/// `CustomizationType` values (`smithy_query(query="shape",shape="CustomizationType")`).
const FINE_TUNING: &str = "FINE_TUNING";
const CONTINUED_PRE_TRAINING: &str = "CONTINUED_PRE_TRAINING";
const DISTILLATION: &str = "DISTILLATION";

/// The most tags one resource of this service may carry ("50 tags per resource").
const MAX_TAGS: usize = bedrock::state::MAX_TAGS_PER_RESOURCE;

/// The emulator's spans for running a customization and for winding one down, which the
/// handlers store as due instants rather than wait on.
const COMPLETE_AFTER: Duration = bedrock::state::MODEL_CUSTOMIZATION_JOB_COMPLETE_AFTER;
const STOPPED_AFTER: Duration = bedrock::state::MODEL_CUSTOMIZATION_JOB_STOPPED_AFTER;

/// The `index`-th base model whose published `customizationsSupported` carries
/// `FINE_TUNING`.
fn fine_tuning_model(index: usize) -> &'static str {
    let models: Vec<&'static str> = CATALOG
        .iter()
        .filter(|model| model.customizations_supported.contains(&FINE_TUNING))
        .map(|model| model.model_id)
        .collect();
    models.get(index).copied().unwrap_or_else(|| {
        panic!(
            "the catalog must publish at least {} fine-tuning base model(s), it publishes {}",
            index + 1,
            models.len()
        )
    })
}

/// The `FoundationModelArn` a read reports for a base model id: an EMPTY account field,
/// since a foundation model is published by AWS rather than owned by the reading account.
fn foundation_model_arn(model_id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}::foundation-model/{model_id}")
}

/// `n` distinct tags, each key and value inside `TagKey`/`TagValue`'s own pattern.
fn tag_list(n: usize) -> Vec<Tag> {
    (0..n)
        .map(|index| Tag {
            key: format!("key-{index}"),
            value: format!("value-{index}"),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// The store the handlers share: seeding a job, reading one back, and aging one.
// ---------------------------------------------------------------------------------------

/// Store the job an admitted `CreateModelCustomizationJob` would have written, and hand back
/// its `ModelCustomizationJobArn`.
///
/// The ARN, the output model's ARN and the base model's ARN come from the state helpers the
/// create used ([`bedrock::state::model_customization_job_arn`] and friends), so a seeded
/// record is the record a create would have left and a read of it is a read of the handler's
/// own output. `tweak` is where a row differs from the baseline.
fn seed(
    emu: &Emulator,
    region: &str,
    job_name: &str,
    model_name: &str,
    base_model: &str,
    tweak: impl FnOnce(&mut ModelCustomizationJobRecord),
) -> String {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let now = SystemTime::now();
    let mut store = state.lock();
    let job_id = store.mint_model_customization_job_id(now);
    let model_id = store.mint_custom_model_id(now);
    let arn = bedrock::state::model_customization_job_arn(region, base_model, &job_id);
    let mut record = ModelCustomizationJobRecord {
        arn: arn.clone(),
        name: job_name.to_string(),
        region: region.to_string(),
        client_request_token: None,
        role_arn: ROLE_ARNS[0].to_string(),
        output_model_name: model_name.to_string(),
        output_model_arn: bedrock::state::custom_model_arn(region, base_model, &model_id),
        base_model_arn: bedrock::state::foundation_model_arn(region, base_model),
        customization_type: bedrock::state::CUSTOMIZATION_TYPE_FINE_TUNING,
        output_model_kms_key_arn: None,
        hyper_parameters: None,
        training_data_config: TrainingDataConfig {
            s3_uri: Some(TRAINING_URIS[0].to_string()),
            invocation_logs_config: None,
        },
        validation_data_config: ValidationDataConfig::default(),
        output_data_config: OutputDataConfig {
            s3_uri: OUTPUT_URIS[0].to_string(),
        },
        vpc_config: None,
        customization_config: None,
        job_tags: Vec::new(),
        custom_model_tags: Vec::new(),
        status: IN_PROGRESS,
        completed_due_at: Some(now + COMPLETE_AFTER),
        stopped_due_at: None,
        created_at: now,
        last_modified_at: now,
        ended_at: None,
    };
    tweak(&mut record);
    store
        .model_customization_job_names
        .insert((region.to_string(), record.name.clone()), arn.clone());
    store.model_customization_jobs.insert(arn.clone(), record);
    arn
}

/// The stored record the ARN names, cloned out of the store the handlers share.
fn stored(emu: &Emulator, arn: &str) -> ModelCustomizationJobRecord {
    let state = bedrock::state::service_state(&*emu.engine)
        .expect("the shared engine carries this service's store");
    let store = state.lock();
    store
        .model_customization_jobs
        .get(arn)
        .cloned()
        .unwrap_or_else(|| panic!("the store must hold the seeded job {arn}"))
}

/// Age the stored job `arn` names by `by`, as though it had been submitted that much
/// earlier: the ONE shift of a job's instants this crate's suites share
/// ([`crate::support::age_model_customization_job`]), so a rule about a span of wall-clock
/// time needs no sleep.
fn age(emu: &Emulator, arn: &str, by: Duration) {
    crate::support::age_model_customization_job(emu, arn, by);
}

// ---------------------------------------------------------------------------------------
// Requests.
// ---------------------------------------------------------------------------------------

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    get_in_region(emu, REGION, identifier).await
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

async fn list(emu: &Emulator, query: &str) -> Reply {
    list_in_region(emu, REGION, query).await
}

async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/model-customization-jobs".to_string()
    } else {
        format!("/model-customization-jobs?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
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

/// The summaries a listing reports, with an omitted list read as no rows.
fn summaries(reply: &Reply) -> Vec<Value> {
    match reply.get("modelCustomizationJobSummaries") {
        Value::Null => Vec::new(),
        Value::Array(rows) => rows,
        other => panic!("modelCustomizationJobSummaries must be a list, got {other}"),
    }
}

/// The `jobArn` of every summary a listing reported, in the order it reported them.
fn listed_arns(reply: &Reply) -> Vec<String> {
    summaries(reply)
        .iter()
        .map(|summary| {
            summary
                .get("jobArn")
                .and_then(Value::as_str)
                .expect("every summary carries the required jobArn")
                .to_string()
        })
        .collect()
}

/// The `message` a refusal states.
fn refusal_message(reply: &Reply) -> String {
    reply.get("message").as_str().unwrap_or_default().to_string()
}

/// The instant a CUSTOM MODEL read reports, in epoch seconds, read from either wire form:
/// the custom-model surface takes the protocol's epoch-seconds default where the job's own
/// read carries the `date-time` string its member schema declares.
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

/// The stored record's instant as the wire reports it, in epoch seconds.
fn seconds(at: SystemTime) -> f64 {
    bedrock::state::epoch_seconds(at)
}

// ---------------------------------------------------------------------------------------
// 1. A stored job round-trips through a read, under both identifier spellings.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_stored_job_round_trips_through_a_read_and_the_listing() {
    let emu = emulator();

    let long_job_name = format!("j{}", "x".repeat(62));
    let long_model_name = format!("m{}", "y".repeat(62));
    // Rows differ along what the read generalises over: the base model, the customization
    // type, the hyperparameters, the validation data, the VPC configuration, the KMS key,
    // how many tags each tagged resource carries, and the name lengths the model admits.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "the baseline record",
            "tune-one".to_string(),
            "TunedOne".to_string(),
            fine_tuning_model(0),
        ),
        (
            "continued pre-training with validators and a key",
            "tune.two".to_string(),
            "Tuned_Two".to_string(),
            fine_tuning_model(1),
        ),
        (
            "distillation with hyperparameters, a VPC and tags",
            "tune-three".to_string(),
            "Tuned-Three".to_string(),
            fine_tuning_model(0),
        ),
        (
            "names at the minimum modelled length",
            "a".to_string(),
            "b".to_string(),
            fine_tuning_model(1),
        ),
        (
            "names at the maximum modelled length",
            long_job_name,
            long_model_name,
            fine_tuning_model(0),
        ),
    ];

    for (case, job_name, model_name, base_model) in rows {
        let arn = seed(&emu, REGION, &job_name, &model_name, base_model, |record| {
            match case {
                "continued pre-training with validators and a key" => {
                    record.customization_type = CONTINUED_PRE_TRAINING;
                    record.validation_data_config = ValidationDataConfig {
                        validators: vec![
                            Validator {
                                s3_uri: "s3://customization-data/validate-a.jsonl".to_string(),
                            },
                            Validator {
                                s3_uri: "s3://customization-data/validate-b.jsonl".to_string(),
                            },
                        ],
                    };
                    record.output_model_kms_key_arn = Some(format!(
                        "arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555"
                    ));
                    record.training_data_config.s3_uri = Some(TRAINING_URIS[1].to_string());
                    record.output_data_config.s3_uri = OUTPUT_URIS[1].to_string();
                }
                "distillation with hyperparameters, a VPC and tags" => {
                    record.customization_type = DISTILLATION;
                    record.hyper_parameters = Some(HashMap::from([
                        ("epochCount".to_string(), "2".to_string()),
                        ("learningRate".to_string(), "0.00001".to_string()),
                    ]));
                    record.vpc_config = Some(VpcConfig {
                        subnet_ids: vec!["subnet-0a1b2c3d".to_string()],
                        security_group_ids: vec!["sg-0123456789abcdef0".to_string()],
                    });
                    record.job_tags = tag_list(1);
                    record.custom_model_tags = tag_list(MAX_TAGS);
                    record.client_request_token = Some("a-token".to_string());
                    record.role_arn = ROLE_ARNS[1].to_string();
                }
                _ => {}
            }
        });
        let record = stored(&emu, &arn);

        for (reader, identifier) in [("by jobArn", arn.clone()), ("by jobName", job_name.clone())] {
            let got = get(&emu, &identifier).await;
            assert_eq!(
                got.status, 200,
                "case {case:?} read {reader}: GetModelCustomizationJob must answer 200, got {} \
                 {:?}",
                got.status, got.body
            );
            assert_eq!(
                got.get("jobArn"),
                json!(record.arn),
                "case {case:?} read {reader}: both spellings address the one job, got {:?}",
                got.body
            );
            assert_eq!(
                got.get("jobName"),
                json!(record.name),
                "case {case:?} read {reader}: jobName comes out of the stored record, got {:?}",
                got.get("jobName")
            );
            assert_eq!(
                got.get("outputModelName"),
                json!(record.output_model_name),
                "case {case:?} read {reader}: outputModelName comes out of the stored record, \
                 got {:?}",
                got.get("outputModelName")
            );
            assert_eq!(
                got.get("outputModelArn"),
                json!(record.output_model_arn),
                "case {case:?} read {reader}: outputModelArn comes out of the stored record, \
                 got {:?}",
                got.get("outputModelArn")
            );
            assert_eq!(
                got.get("roleArn"),
                json!(record.role_arn),
                "case {case:?} read {reader}: roleArn comes out of the stored record, got {:?}",
                got.get("roleArn")
            );
            assert_eq!(
                got.get("clientRequestToken"),
                record
                    .client_request_token
                    .as_ref()
                    .map_or(Value::Null, |token| json!(token)),
                "case {case:?} read {reader}: clientRequestToken comes out of the stored \
                 record, got {:?}",
                got.get("clientRequestToken")
            );
            assert_eq!(
                got.get("baseModelArn"),
                json!(foundation_model_arn(base_model)),
                "case {case:?} read {reader}: baseModelArn is the FoundationModelArn of the \
                 base model, got {:?}",
                got.get("baseModelArn")
            );
            assert_eq!(
                got.get("customizationType"),
                json!(record.customization_type),
                "case {case:?} read {reader}: customizationType comes out of the stored \
                 record, got {:?}",
                got.get("customizationType")
            );
            assert_eq!(
                got.get("outputModelKmsKeyArn"),
                record
                    .output_model_kms_key_arn
                    .as_ref()
                    .map_or(Value::Null, |key| json!(key)),
                "case {case:?} read {reader}: outputModelKmsKeyArn comes out of the stored \
                 record, got {:?}",
                got.get("outputModelKmsKeyArn")
            );
            assert_eq!(
                got.get("trainingDataConfig"),
                json!({ "s3Uri": record.training_data_config.s3_uri }),
                "case {case:?} read {reader}: trainingDataConfig comes out of the stored \
                 record, got {:?}",
                got.get("trainingDataConfig")
            );
            assert_eq!(
                got.get("outputDataConfig"),
                json!({ "s3Uri": record.output_data_config.s3_uri }),
                "case {case:?} read {reader}: outputDataConfig comes out of the stored record, \
                 got {:?}",
                got.get("outputDataConfig")
            );
            // `validationDataConfig` is REQUIRED in the read and its `validators` list
            // admits zero entries, so a job with no validation data is the job with no
            // validators.
            let validators: Vec<Value> = record
                .validation_data_config
                .validators
                .iter()
                .map(|validator| json!({ "s3Uri": validator.s3_uri }))
                .collect();
            assert_eq!(
                got.get("validationDataConfig"),
                json!({ "validators": validators }),
                "case {case:?} read {reader}: validationDataConfig reports the stored \
                 validators, got {:?}",
                got.get("validationDataConfig")
            );
            assert_eq!(
                got.text("status"),
                IN_PROGRESS,
                "case {case:?} read {reader}: a customization that has not finished is \
                 running, got {:?}",
                got.body
            );
            assert!(
                got.get("endTime").is_null(),
                "case {case:?} read {reader}: a running job has not ended, got {:?}",
                got.get("endTime")
            );
            assert!(
                (got.instant("creationTime") - seconds(record.created_at)).abs() < 1e-3,
                "case {case:?} read {reader}: creationTime is the instant the record carries, \
                 got {:?}",
                got.get("creationTime")
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

        // The listing reports the SAME record through the summary shape.
        let listed = list(&emu, &format!("nameContains={job_name}")).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListModelCustomizationJobs must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let rows = summaries(&listed);
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: the job must be listed exactly once, got {:?}",
            listed.body
        );
        let summary = &rows[0];
        for (member, expected) in [
            ("jobArn", json!(record.arn)),
            ("jobName", json!(record.name)),
            ("baseModelArn", json!(record.base_model_arn)),
            ("status", json!(IN_PROGRESS)),
            ("customModelArn", json!(record.output_model_arn)),
            ("customModelName", json!(record.output_model_name)),
            ("customizationType", json!(record.customization_type)),
        ] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                expected,
                "case {case:?}: the summary's {member} comes out of the stored record, got {:?}",
                summary
            );
        }
        assert!(
            summary.get("endTime").is_none(),
            "case {case:?}: a running job has no end to report, got {summary:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. An identifier no stored job answers to.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_no_job_answers_to_is_not_found() {
    let emu = emulator();
    let live_arn = seed(&emu, REGION, "live-job", "LiveModel", fine_tuning_model(0), |_| {});
    let other_region = "eu-central-1";
    seed(
        &emu,
        other_region,
        "elsewhere-job",
        "ElsewhereModel",
        fine_tuning_model(0),
        |_| {},
    );

    let never_minted = format!(
        "arn:aws:bedrock:{REGION}:{ACCOUNT}:model-customization-job/{}/abcdef012345",
        fine_tuning_model(0)
    );
    let rows: Vec<(&str, String)> = vec![
        ("a name no job carries", "no-such-job".to_string()),
        ("a live name of another region", "elsewhere-job".to_string()),
        ("a well-formed ARN never minted", never_minted),
        ("a prefix of a live name", "live".to_string()),
    ];
    for (case, identifier) in rows {
        for (operation, reply) in [
            ("read", get(&emu, &identifier).await),
            ("stop", stop(&emu, &identifier).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case:?} ({operation}): an unknown job is ResourceNotFoundException \
                 (404), got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?} ({operation}): the refusal carries the modelled error code, got \
                 {:?}",
                reply.error
            );
            assert!(
                refusal_message(&reply).contains(&identifier),
                "case {case:?} ({operation}): the message names the identifier that answered \
                 to nothing, got {:?}",
                refusal_message(&reply)
            );
        }
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
// 3. The listing's filters, order and pages.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_filters_orders_and_pages() {
    let emu = emulator();

    // Four jobs of this region, created in a known order, differing in name and status, and
    // one of another region which this region's listing must not report. Each is backdated
    // rather than aged: the rule under test is the ORDER and the filters, so the
    // completion each running job is still owed stays in the future.
    let base = fine_tuning_model(0);
    let backdate = |record: &mut ModelCustomizationJobRecord, seconds: u64| {
        let ago = Duration::from_secs(seconds);
        record.created_at -= ago;
        record.last_modified_at -= ago;
    };
    let oldest = seed(&emu, REGION, "alpha-oldest", "AlphaOldest", base, |record| {
        backdate(record, 40);
    });
    let stopped = seed(&emu, REGION, "beta-stopped", "BetaStopped", base, |record| {
        backdate(record, 30);
        record.status = STOPPED;
        record.completed_due_at = None;
        record.ended_at = Some(record.created_at);
    });
    let middle = seed(&emu, REGION, "alpha-middle", "AlphaMiddle", base, |record| {
        backdate(record, 20);
    });
    let newest = seed(&emu, REGION, "gamma-newest", "GammaNewest", base, |record| {
        backdate(record, 10);
    });
    let elsewhere = seed(
        &emu,
        "eu-central-1",
        "alpha-elsewhere",
        "AlphaElsewhere",
        base,
        |_| {},
    );

    // Oldest first is the order an absent `sortOrder` takes; every job here was created
    // before the next one, so the order is fully determined.
    let ascending = vec![
        oldest.clone(),
        stopped.clone(),
        middle.clone(),
        newest.clone(),
    ];
    let mut descending = ascending.clone();
    descending.reverse();

    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter at all", String::new(), ascending.clone()),
        (
            "sortOrder=Ascending, which is the default",
            "sortOrder=Ascending".to_string(),
            ascending.clone(),
        ),
        (
            "sortOrder=Descending turns the order round",
            "sortOrder=Descending".to_string(),
            descending.clone(),
        ),
        (
            "sortBy=CreationTime, the only field it admits",
            "sortBy=CreationTime".to_string(),
            ascending.clone(),
        ),
        (
            "nameContains on a fragment two names share",
            "nameContains=alpha".to_string(),
            vec![oldest.clone(), middle.clone()],
        ),
        (
            "nameContains on a whole name",
            "nameContains=gamma-newest".to_string(),
            vec![newest.clone()],
        ),
        (
            "nameContains on a fragment no name carries",
            "nameContains=delta".to_string(),
            Vec::new(),
        ),
        (
            "statusEquals on the status one job holds",
            format!("statusEquals={STOPPED}"),
            vec![stopped.clone()],
        ),
        (
            "statusEquals on the status the others hold",
            format!("statusEquals={IN_PROGRESS}"),
            vec![oldest.clone(), middle.clone(), newest.clone()],
        ),
        (
            "statusEquals on a status no job holds",
            format!("statusEquals={COMPLETED}"),
            Vec::new(),
        ),
        (
            "two filters, both of which must hold",
            format!("nameContains=alpha&statusEquals={IN_PROGRESS}"),
            vec![oldest.clone(), middle.clone()],
        ),
    ];
    for (case, query, expected) in rows {
        let listed = list(&emu, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the listing must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed_arns(&listed),
            expected,
            "case {case:?}: expected exactly those jobs in that order, got {:?}",
            listed.body
        );
        assert!(
            listed.get("nextToken").is_null(),
            "case {case:?}: a page that holds every row reports no further page, got {:?}",
            listed.get("nextToken")
        );
    }

    // `maxResults` bounds the page and `nextToken` resumes it: the pages partition the
    // listing with neither a gap nor a repeat.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..4 {
        let query = match token.as_deref() {
            Some(token) => format!("maxResults=2&nextToken={}", label(token)),
            None => "maxResults=2".to_string(),
        };
        let listed = list(&emu, &query).await;
        assert_eq!(
            listed.status, 200,
            "case page {page}: the listing must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let arns = listed_arns(&listed);
        assert!(
            arns.len() <= 2,
            "case page {page}: maxResults=2 bounds the page, got {:?}",
            listed.body
        );
        seen.extend(arns);
        token = listed.get("nextToken").as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, ascending,
        "the pages must partition the listing in its order, got {seen:?}"
    );

    // The other region's job is listed only there.
    let elsewhere_listed = list_in_region(&emu, "eu-central-1", "").await;
    assert_eq!(
        listed_arns(&elsewhere_listed),
        vec![elsewhere],
        "a job is listed in the region that holds it, got {:?}",
        elsewhere_listed.body
    );

    // A token this service never issued is a refused request, not a page.
    let refused = list(&emu, "nextToken=not-a-token-this-service-issued").await;
    assert_eq!(
        refused.status, 400,
        "a nextToken this service never issued is ValidationException (400), got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ValidationException"),
        "the refusal carries the modelled error code, got {:?}",
        refused.error
    );
}

// ---------------------------------------------------------------------------------------
// 4. The status the clock owes, and the custom model completing the job produces.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_running_job_settles_to_completed_and_produces_its_custom_model() {
    let emu = emulator();
    let base = fine_tuning_model(0);
    let arn = seed(&emu, REGION, "settling-job", "SettlingModel", base, |record| {
        record.customization_type = CONTINUED_PRE_TRAINING;
        record.hyper_parameters = Some(HashMap::from([("epochCount".to_string(), "1".to_string())]));
        record.validation_data_config = ValidationDataConfig {
            validators: vec![Validator {
                s3_uri: "s3://customization-data/validate.jsonl".to_string(),
            }],
        };
        record.custom_model_tags = tag_list(2);
    });
    let output_model_arn = stored(&emu, &arn).output_model_arn;

    // While the customization is still owed, the job is running and the model it produces
    // does not exist.
    let running = get(&emu, &arn).await;
    assert_eq!(
        running.text("status"),
        IN_PROGRESS,
        "case read while customizing: the job is still running, got {:?}",
        running.body
    );
    let while_running = get_custom_model(&emu, &output_model_arn).await;
    assert_eq!(
        while_running.status, 404,
        "case read while customizing: the custom model does not exist until the job \
         completes, got {} {:?}",
        while_running.status, while_running.body
    );

    // Once the span has passed, the job has completed — the clock owed it, so no caller had
    // to ask.
    age(&emu, &arn, COMPLETE_AFTER + Duration::from_millis(50));
    let settled = get(&emu, &arn).await;
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
    let again = get(&emu, &arn).await;
    assert_eq!(
        again.body, settled.body,
        "case read twice: a read reports the job, it does not move it on, got {:?} then {:?}",
        settled.body, again.body
    );
    // The listing reports the settled status too, and `statusEquals` filters on it.
    let listed = list(&emu, &format!("statusEquals={COMPLETED}")).await;
    assert_eq!(
        listed_arns(&listed),
        vec![arn.clone()],
        "case listed once the span has passed: the completed job is the one listed, got {:?}",
        listed.body
    );

    // Completing the job is what brought the custom model into being, under its ARN and
    // under its bare name.
    for (reader, identifier) in [
        ("by modelArn", output_model_arn.clone()),
        ("by modelName", "SettlingModel".to_string()),
    ] {
        let model = get_custom_model(&emu, &identifier).await;
        assert_eq!(
            model.status, 200,
            "case read the produced model {reader}: GetCustomModel must answer 200, got {} {:?}",
            model.status, model.body
        );
        assert_eq!(
            model.get("modelArn"),
            json!(output_model_arn),
            "case read the produced model {reader}: the model is the one the job named, got {:?}",
            model.body
        );
        assert_eq!(
            model.get("modelName"),
            json!("SettlingModel"),
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
            json!(arn),
            "case read the produced model {reader}: the model names the job that produced it, \
             got {:?}",
            model.get("jobArn")
        );
        assert_eq!(
            model.get("baseModelArn"),
            json!(foundation_model_arn(base)),
            "case read the produced model {reader}: the model descends from the base model \
             the job customized, got {:?}",
            model.get("baseModelArn")
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
                "case read the produced model {reader}: {member} is the job's, read off the one \
                 record that holds it, got {:?} against {:?}",
                model.get(member),
                settled.get(member)
            );
        }
        // The two instants come off the wire in different renderings, so they are compared
        // to the millisecond rather than bit for bit.
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
    let emu = emulator();
    let arn = seed(
        &emu,
        REGION,
        "stopping-job",
        "StoppingModel",
        fine_tuning_model(0),
        |_| {},
    );
    let output_model_arn = stored(&emu, &arn).output_model_arn;

    // Both identifier spellings stop the same job; the first stop is the one that moves it.
    let stopped = stop(&emu, "stopping-job").await;
    assert_eq!(
        stopped.status, 200,
        "case stop a running job: StopModelCustomizationJob is bound to @http(code: 200), got \
         {} {:?}",
        stopped.status, stopped.body
    );
    let stopping = get(&emu, &arn).await;
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

    // A second stop asks for what is already in flight: an idempotent no-op.
    let second = stop(&emu, &arn).await;
    assert_eq!(
        second.status, 200,
        "case stop a stopping job: the stop already asked for is answered again, got {} {:?}",
        second.status, second.body
    );
    let unmoved = get(&emu, &arn).await;
    assert_eq!(
        unmoved.body, stopping.body,
        "case stop a stopping job: the second stop moves nothing, got {:?} then {:?}",
        stopping.body, unmoved.body
    );

    age(&emu, &arn, STOPPED_AFTER + Duration::from_millis(50));
    let terminal = get(&emu, &arn).await;
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
    let third = stop(&emu, &arn).await;
    assert_eq!(
        third.status, 200,
        "case stop a stopped job: the stop is an idempotent no-op, got {} {:?}",
        third.status, third.body
    );
    let after = get(&emu, &arn).await;
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
    let arn = seed(
        &emu,
        REGION,
        "completing-job",
        "CompletingModel",
        fine_tuning_model(0),
        |_| {},
    );
    age(&emu, &arn, COMPLETE_AFTER + Duration::from_millis(50));
    let completed = get(&emu, &arn).await;
    assert_eq!(
        completed.text("status"),
        COMPLETED,
        "the job must have completed before the stop, got {:?}",
        completed.body
    );

    for (case, identifier) in [
        ("by jobArn", arn.clone()),
        ("by jobName", "completing-job".to_string()),
    ] {
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
            "case {case:?}: the refusal carries the modelled error code, got {:?}",
            refused.error
        );
    }
    let unmoved = get(&emu, &arn).await;
    assert_eq!(
        unmoved.text("status"),
        COMPLETED,
        "a refused stop moves nothing, got {:?}",
        unmoved.body
    );
}

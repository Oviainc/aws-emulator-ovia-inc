// SPDX-License-Identifier: CC0-1.0

//! The imported model end-to-end: `CreateModelImportJob` starts the import that creates
//! it and `GetImportedModel` reads it back, over the same `MultiServiceRouter` the
//! emulator boots (`crate::support`).
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A model a completed import created round-trips**: `GetImportedModel` answers 200
//!    and reports every member out of the stored import job — for requests differing in
//!    data source, KMS key spelling, tag counts and name lengths (both modelled bounds) —
//!    under BOTH `ImportedModelIdentifier` spellings, the imported model's ARN and its
//!    bare name. The members Amazon Bedrock reads off the model artifacts
//!    (`modelArchitecture`, `instructSupported`, `customModelUnits`) are reported by
//!    nothing, because this emulator never reads those bytes.
//! 2. **A model exists only once its import has completed**: while the job is
//!    `InProgress` the model it imports is refused with 404 `ResourceNotFoundException`
//!    under either spelling, and once `IMPORT_JOB_COMPLETE_AFTER` has passed the same
//!    read answers 200 — and the model's `creationTime` is the instant the import ended,
//!    which the clock owed, so reading twice reports the very same body.
//! 3. **An identifier no imported model answers to is refused as the spelling it IS**: an
//!    `imported-model` ARN that names none is 404 `ResourceNotFoundException` carrying that
//!    resource's own not-found sentence, while a bare model NAME that resolves to nothing is
//!    no ARN to report missing but an identifier this service cannot read as a model, which
//!    is 400 `ValidationException` carrying the one sentence Amazon Bedrock states for that
//!    class. Rows span a never-imported name, a live name in another region, a well-formed
//!    imported-model ARN that was never minted, an ARN of another region and a prefix of a
//!    live name.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateModelImportJob,GetModelImportJob,GetImportedModel")`
//! / `query="shape"`).

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

/// `S3DataSource$s3Uri` values the model admits (`smithy_query(query="shape",
/// shape="S3DataSource")`: pattern `^s3://[a-z0-9][-.a-z0-9]{1,61}[a-z0-9](?:/…)?/?$`).
const S3_URIS: &[&str] = &[
    "s3://imported-models/qwen2-7b/",
    "s3://imported-models/deepseek-r1-distill/weights",
    "s3://model.imports.bucket/llama-3-8b",
];

/// `RoleArn` values the model admits (pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const ROLE_ARNS: &[&str] = &[
    "arn:aws:iam::000000000000:role/bedrock-model-import",
    "arn:aws:iam::000000000000:role/service-role/BedrockImportExecution",
];

/// A KMS key id `importedModelKmsKeyId` admits (its pattern's bare-key-id alternative)
/// and the alias the same pattern admits beside it.
const KMS_KEY_ID: &str = "11111111-2222-3333-4444-555555555555";
const KMS_KEY_ALIAS: &str = "alias/imported-models";

/// A second region, so a rule about the region an identifier is resolved in is asserted
/// across two of them rather than inside one.
const OTHER_REGION: &str = "eu-central-1";

/// `ModelImportJobStatus` values this lifecycle passes through
/// (`smithy_query(query="shape",shape="ModelImportJobStatus")`).
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";

/// The emulator's span for running an import, which `CreateModelImportJob` stores as a due
/// instant rather than waits on.
const IMPORT_SPAN: std::time::Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::IMPORT_JOB_COMPLETE_AFTER;

/// The three members Amazon Bedrock derives from the imported model's artifacts
/// (`smithy_query(query="operation",operation="GetImportedModel")`).
const ARTIFACT_MEMBERS: &[&str] = &["modelArchitecture", "instructSupported", "customModelUnits"];

/// The two fixed sentences an identifier that resolves to no imported model is refused
/// with, written out here rather than read off the crate so this suite pins the wording a
/// client reads.
const INVALID_MODEL_IDENTIFIER: &str = "The provided model identifier is invalid.";
const IMPORTED_MODEL_NOT_FOUND: &str = "Imported model not found";

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// One `modelDataSource` union value: its only member is `s3DataSource`
/// (`smithy_query(query="shape",shape="ModelDataSource")`).
fn s3_data_source(s3_uri: &str) -> Value {
    json!({ "s3DataSource": { "s3Uri": s3_uri } })
}

/// A `CreateModelImportJob` body carrying only the four `@required` members, plus
/// whatever `extra` adds.
fn create_body(job_name: &str, model_name: &str, s3_uri: &str, role: &str, extra: Value) -> Value {
    let mut body = json!({
        "jobName": job_name,
        "importedModelName": model_name,
        "roleArn": role,
        "modelDataSource": s3_data_source(s3_uri),
    });
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

async fn start_import(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/model-import-jobs", Some(body))
        .await
}

async fn get_job(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/model-import-jobs/{}", label(identifier)),
        None,
    )
    .await
}

async fn get_model(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/imported-models/{}", label(identifier)),
        None,
    )
    .await
}

/// The member `name` of a create body, or `Value::Null` when it carried none.
fn sent(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// The `modelKmsKeyArn` a read reports for a request's `importedModelKmsKeyId`.
///
/// `importedModelKmsKeyId` admits a key ARN, a bare 36-character key id or an alias, while
/// `KmsKeyArn` (`^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`)
/// admits only a key ARN: so a key ARN passes through, a bare id is completed into one,
/// and an alias — which resolves to a key id only inside KMS — is reported by nothing.
fn expected_kms_key_arn(body: &Value, region: &str) -> Value {
    let Some(key_id) = body.get("importedModelKmsKeyId").and_then(Value::as_str) else {
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
        json!(format!("arn:aws:kms:{region}:{ACCOUNT}:key/{key_id}"))
    } else {
        Value::Null
    }
}

/// Whether `value` is an `ImportedModelArn` of `region`: the resource id it declares is
/// `[a-z0-9]{12}`.
fn is_imported_model_arn(value: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:bedrock:{region}:{ACCOUNT}:imported-model/");
    value.strip_prefix(&prefix).is_some_and(|id| {
        id.len() == 12 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// Sleep past the import span, so the transition the clock owes is due.
async fn wait_out_the_import() {
    tokio::time::sleep(IMPORT_SPAN + std::time::Duration::from_millis(150)).await;
}

// ---------------------------------------------------------------------------------------
// 1. A model a completed import created round-trips, under both identifier spellings.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_imported_model_round_trips_under_both_identifier_spellings() {
    let emu = emulator();

    // Rows differ along what the rule generalises over: the data source, the KMS key
    // spelling, how many tags the imported model carries, and the name lengths the model
    // admits (`ImportedModelName` is `@length(1..63)`, pattern `^([0-9a-zA-Z][_-]?)+$`).
    let long_job_name = format!("j{}", "x".repeat(62));
    let long_model_name = format!("m{}", "y".repeat(62));
    let rows: Vec<(&str, Value)> = vec![
        (
            "required members only",
            create_body("import-one", "ImportedOne", S3_URIS[0], ROLE_ARNS[0], json!({})),
        ),
        (
            "a key ARN and a nested data source prefix",
            create_body(
                "import-two",
                "Imported_Two",
                S3_URIS[1],
                ROLE_ARNS[1],
                json!({
                    "importedModelKmsKeyId":
                        format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{KMS_KEY_ID}"),
                }),
            ),
        ),
        (
            "a bare key id and tags on the imported model",
            create_body(
                "import.three",
                "Imported-Three",
                S3_URIS[2],
                ROLE_ARNS[0],
                json!({
                    "importedModelKmsKeyId": KMS_KEY_ID,
                    "importedModelTags": tags(3),
                }),
            ),
        ),
        (
            "a key alias, which resolves to a key id only inside KMS",
            create_body(
                "import-four",
                "ImportedFour",
                S3_URIS[0],
                ROLE_ARNS[1],
                json!({ "importedModelKmsKeyId": KMS_KEY_ALIAS }),
            ),
        ),
        (
            "a name at the minimum modelled length",
            create_body("a", "b", S3_URIS[1], ROLE_ARNS[0], json!({})),
        ),
        (
            "a name at the maximum modelled length",
            create_body(
                &long_job_name,
                &long_model_name,
                S3_URIS[2],
                ROLE_ARNS[1],
                json!({}),
            ),
        ),
    ];

    for (case, body) in rows {
        let created = start_import(&emu, REGION, body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateModelImportJob is bound to @http(code: 201), got {} {:?}",
            created.status, created.body
        );
        let job_arn = created.text("jobArn");

        wait_out_the_import().await;
        let job = get_job(&emu, REGION, &job_arn).await;
        assert_eq!(
            job.text("status"),
            COMPLETED,
            "case {case:?}: the import must have completed before the model is read, got {:?}",
            job.body
        );
        let model_arn = job.text("importedModelArn");
        assert!(
            is_imported_model_arn(&model_arn, REGION),
            "case {case:?}: the job's importedModelArn must be an ImportedModelArn of the \
             signing region, got {model_arn:?}"
        );
        let model_name = sent(&body, "importedModelName");
        let model_name = model_name.as_str().expect("an importedModelName string");

        let readers: Vec<(&str, &str)> = vec![("by modelArn", &model_arn), ("by modelName", model_name)];
        for (reader, identifier) in readers {
            let got = get_model(&emu, REGION, identifier).await;
            assert_eq!(
                got.status, 200,
                "case {case:?} read {reader}: GetImportedModel must answer 200, got {} {:?}",
                got.status, got.body
            );
            assert_eq!(
                got.content_type.as_deref(),
                Some("application/json"),
                "case {case:?} read {reader}: a restJson1 response carries application/json, got \
                 {:?}",
                got.content_type
            );
            assert_eq!(
                got.get("modelArn"),
                json!(model_arn),
                "case {case:?} read {reader}: modelArn must be the ARN the import minted, got \
                 {:?}",
                got.body
            );
            assert_eq!(
                got.get("modelName"),
                json!(model_name),
                "case {case:?} read {reader}: modelName must be the name the create sent, got {:?}",
                got.get("modelName")
            );
            assert_eq!(
                got.get("jobArn"),
                json!(job_arn),
                "case {case:?} read {reader}: jobArn must be the import job's ARN, got {:?}",
                got.get("jobArn")
            );
            assert_eq!(
                got.get("jobName"),
                sent(&body, "jobName"),
                "case {case:?} read {reader}: jobName must be the import job's name, got {:?}",
                got.get("jobName")
            );
            assert_eq!(
                got.get("modelDataSource"),
                sent(&body, "modelDataSource"),
                "case {case:?} read {reader}: modelDataSource must come back as the create sent \
                 it, got {:?}",
                got.get("modelDataSource")
            );
            assert_eq!(
                got.get("modelKmsKeyArn"),
                expected_kms_key_arn(&body, REGION),
                "case {case:?} read {reader}: modelKmsKeyArn must be the key ARN the request's \
                 key id names, got {:?}",
                got.get("modelKmsKeyArn")
            );
            // The model was created by the import, so it came into being when that import
            // ended — the instant the job reports as its `endTime`.
            assert!(
                (got.instant("creationTime") - job.instant("endTime")).abs() < f64::EPSILON,
                "case {case:?} read {reader}: the model's creationTime is the instant its \
                 import ended, got {:?} against job {:?}",
                got.get("creationTime"),
                job.get("endTime")
            );
            for member in ARTIFACT_MEMBERS {
                assert!(
                    got.get(member).is_null(),
                    "case {case:?} read {reader}: {member} is read off the model artifacts, \
                     which this emulator does not read, so it must be absent, got {:?}",
                    got.get(member)
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// 2. The model exists only once its import has completed.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_model_is_readable_only_once_its_import_has_completed() {
    let emu = emulator();
    let body = create_body(
        "settling-import",
        "SettlingImportedModel",
        S3_URIS[0],
        ROLE_ARNS[0],
        json!({ "importedModelKmsKeyId": KMS_KEY_ID }),
    );
    let created = start_import(&emu, REGION, body.clone()).await;
    assert_eq!(
        created.status, 201,
        "CreateModelImportJob must answer 201, got {} {:?}",
        created.status, created.body
    );
    let job_arn = created.text("jobArn");

    let running = get_job(&emu, REGION, &job_arn).await;
    assert_eq!(
        running.text("status"),
        IN_PROGRESS,
        "case read while importing: the job is still running, got {:?}",
        running.body
    );
    let model_arn = running.text("importedModelArn");

    // Both spellings of the identifier, while the import has not finished: the model the
    // import will create does not exist yet, and each spelling is refused as the spelling it
    // IS — an `imported-model` ARN as a resource that is not there, a bare model NAME as an
    // identifier this service cannot resolve to a model at all.
    for (case, identifier, status, code) in [
        (
            "by modelArn while importing",
            model_arn.as_str(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "by modelName while importing",
            "SettlingImportedModel",
            400,
            "ValidationException",
        ),
    ] {
        let early = get_model(&emu, REGION, identifier).await;
        assert_eq!(
            (early.status, early.error.as_deref()),
            (status, Some(code)),
            "case {case}: an import that has not completed has created no model, so \
             {identifier:?} is refused {status} {code}, got {} {:?}",
            early.status,
            early.body
        );
    }

    wait_out_the_import().await;
    let settled = get_model(&emu, REGION, &model_arn).await;
    assert_eq!(
        settled.status, 200,
        "case read once the import span has passed: the model must be readable, got {} {:?}",
        settled.status, settled.body
    );
    let job = get_job(&emu, REGION, &job_arn).await;
    assert!(
        (settled.instant("creationTime") - job.instant("endTime")).abs() < f64::EPSILON,
        "case read once the import span has passed: the model was created when the import \
         ended, got {:?} against job {:?}",
        settled.get("creationTime"),
        job.get("endTime")
    );

    // The transition was owed by the CLOCK, so a second read reports the very same body
    // rather than one moved on by the read itself.
    let again = get_model(&emu, REGION, &model_arn).await;
    assert_eq!(
        again.body, settled.body,
        "case read twice: a read reports the model, it does not move it on, got {:?} then {:?}",
        settled.body, again.body
    );

    // And the bare name addresses the very same model as the ARN.
    let by_name = get_model(&emu, REGION, "SettlingImportedModel").await;
    assert_eq!(
        by_name.body, settled.body,
        "case read by name: both ImportedModelIdentifier spellings address one model, got {:?} \
         then {:?}",
        settled.body, by_name.body
    );
}

// ---------------------------------------------------------------------------------------
// The wire form of the one timestamp this response carries.
// ---------------------------------------------------------------------------------------

/// `GetImportedModelResponse$creationTime` carries `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="GetImportedModelResponse")`), so its member schema
/// reports that format and the wire carries an RFC 3339 STRING rather than the epoch
/// number the protocol default would write.
#[test]
fn the_creation_time_member_schema_carries_the_date_time_format() {
    use ::aws_smithy_schema::traits::TimestampFormat;
    use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::ops::types::GetImportedModelOutput;

    let format = GetImportedModelOutput::M_CREATION_TIME
        .timestamp_format()
        .map(|trait_| trait_.format());
    assert!(
        matches!(format, Some(TimestampFormat::DateTime)),
        "case GetImportedModelResponse$creationTime: the member schema must carry \
         @timestampFormat(date-time), got {format:?}"
    );

    // The control row: a member the model gives no `@timestampFormat` reports none, so the
    // assertion above is about the trait and not about every member schema.
    let without = GetImportedModelOutput::M_MODEL_ARN
        .timestamp_format()
        .map(|trait_| trait_.format());
    assert!(
        without.is_none(),
        "case GetImportedModelResponse$modelArn: the model gives this member no \
         @timestampFormat, got {without:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 3. An identifier no imported model answers to.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_identifier_no_imported_model_answers_to_is_refused() {
    let emu = emulator();

    // One live model in each region, so a row can address a name that lives in the OTHER
    // one.
    let here = start_import(
        &emu,
        REGION,
        create_body("live-import", "LiveModel", S3_URIS[0], ROLE_ARNS[0], json!({})),
    )
    .await;
    assert_eq!(
        here.status, 201,
        "the live import must be accepted, got {} {:?}",
        here.status, here.body
    );
    let there = start_import(
        &emu,
        OTHER_REGION,
        create_body(
            "elsewhere-import",
            "ElsewhereModel",
            S3_URIS[1],
            ROLE_ARNS[1],
            json!({}),
        ),
    )
    .await;
    assert_eq!(
        there.status, 201,
        "the import in the other region must be accepted, got {} {:?}",
        there.status, there.body
    );
    wait_out_the_import().await;
    let live_model_arn = get_job(&emu, REGION, &here.text("jobArn"))
        .await
        .text("importedModelArn");
    let elsewhere_model_arn = get_job(&emu, OTHER_REGION, &there.text("jobArn"))
        .await
        .text("importedModelArn");

    // Rows span the ways an identifier can name nothing in the signing region: a name
    // nothing imported, a name that lives in another region, a well-formed ARN that was
    // never minted, an ARN of another region, and a prefix of a live name — each refused as
    // the spelling it IS, with the fixed sentence its class carries.
    let never_minted = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:imported-model/aaaaaaaaaaaa");
    let rows: Vec<(&str, &str, u16, &str, &str)> = vec![
        (
            "a name nothing imported",
            "NoSuchModel",
            400,
            "ValidationException",
            INVALID_MODEL_IDENTIFIER,
        ),
        (
            "a name that lives in another region",
            "ElsewhereModel",
            400,
            "ValidationException",
            INVALID_MODEL_IDENTIFIER,
        ),
        (
            "a well-formed ARN never minted",
            never_minted.as_str(),
            404,
            "ResourceNotFoundException",
            IMPORTED_MODEL_NOT_FOUND,
        ),
        (
            "an ARN of another region",
            elsewhere_model_arn.as_str(),
            404,
            "ResourceNotFoundException",
            IMPORTED_MODEL_NOT_FOUND,
        ),
        (
            "a prefix of a live name",
            "Live",
            400,
            "ValidationException",
            INVALID_MODEL_IDENTIFIER,
        ),
    ];
    for (case, identifier, status, code, sentence) in rows {
        let refused = get_model(&emu, REGION, identifier).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (status, Some(code)),
            "case {case}: {identifier:?} names no imported model in this region, so it is \
             refused {status} {code}, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.get("message").as_str(),
            Some(sentence),
            "case {case}: the refusal states the fixed sentence of its class, got {:?}",
            refused.get("message")
        );
    }

    // The live model of each region is still readable in its own region.
    for (case, region, identifier) in [
        ("the live model by ARN", REGION, live_model_arn.as_str()),
        ("the live model by name", REGION, "LiveModel"),
        (
            "the other region's model in its own region",
            OTHER_REGION,
            elsewhere_model_arn.as_str(),
        ),
    ] {
        let got = get_model(&emu, region, identifier).await;
        assert_eq!(
            got.status, 200,
            "case {case}: {identifier:?} must still resolve in {region}, got {} {:?}",
            got.status, got.body
        );
    }
}

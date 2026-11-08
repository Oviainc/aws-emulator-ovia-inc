// SPDX-License-Identifier: CC0-1.0

//! The model import job resource end-to-end: `CreateModelImportJob` writes it and
//! `GetModelImportJob` reads it back, over the same `MultiServiceRouter` the emulator
//! boots (`crate::support`).
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A created job round-trips**: `CreateModelImportJob` answers the modelled 201 with a
//!    `jobArn` of the form `ModelImportJobArn` declares, and every member
//!    `GetModelImportJob` reports comes back out of the stored record — for requests
//!    differing in data source, VPC configuration, KMS key spelling, tag counts and name
//!    lengths (both modelled bounds). Both `ModelImportJobIdentifier` spellings — the job
//!    ARN and the bare `jobName` — address the same job.
//! 2. **An identifier no stored job answers to is refused** with 404
//!    `ResourceNotFoundException`, over rows spanning a never-created name, a live name in
//!    another region, a well-formed ARN that was never minted and a prefix of a live name.
//!    A job ARN naming ANOTHER region is refused one step earlier instead, with this
//!    service's own ARN sentence (400 `ValidationException`, `arn_inputs.rs`): such an ARN
//!    is not usable at this endpoint at all, so it is never resolved against the store.
//! 3. **`status` is non-terminal first and settles to `Completed`**: `InProgress` straight
//!    after the create with no `endTime`, `Completed` with an `endTime` once
//!    `IMPORT_JOB_COMPLETE_AFTER` has passed — and the settle is owed by the clock, not by
//!    the read, so reading twice reports the same instants.
//! 4. **A replayed `clientRequestToken` mints nothing**: the replay answers with the job
//!    the first call minted and the names the replay carried stay unminted, while a fresh
//!    token mints another job.
//! 5. **A `jobName` and an `importedModelName` each name at most one resource per account
//!    per region**, so a create reusing either conflicts while the same names in another
//!    region are minted.
//! 6. **More than `MAX_TAGS_PER_RESOURCE` tags on either tagged resource is refused** with
//!    `TooManyTagsException`, and nothing is stored.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateModelImportJob,GetModelImportJob")` /
//! `query="shape"`).

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Values the model admits for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// `S3DataSource$s3Uri` values the model admits (`smithy_query(query="shape",
/// shape="S3DataSource")`: pattern `^s3://[a-z0-9][-.a-z0-9]{1,61}[a-z0-9](?:/…)?/?$`).
const S3_URIS: &[&str] = &[
    "s3://imported-models/mistral-7b/",
    "s3://imported-models/deepseek-r1-distill/weights",
    "s3://model.imports.bucket/llama-3-8b",
];

/// `RoleArn` values the model admits (pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const ROLE_ARNS: &[&str] = &[
    "arn:aws:iam::000000000000:role/bedrock-model-import",
    "arn:aws:iam::000000000000:role/service-role/BedrockImportExecution",
];

/// A KMS key ARN `importedModelKmsKeyId` admits (its pattern's key-ARN alternative), and
/// the bare 36-character key id and the alias the same pattern admits beside it.
const KMS_KEY_ID: &str = "11111111-2222-3333-4444-555555555555";
const KMS_KEY_ALIAS: &str = "alias/imported-models";

/// `ModelImportJobStatus` is `[InProgress, Completed, Failed]`
/// (`smithy_query(query="shape",shape="ModelImportJobStatus")`); an accepted job runs and
/// then completes.
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";

/// The most tags one resource of this service may carry, as `TooManyTagsException`
/// documents it ("50 tags per resource") and the handler enforces it.
const MAX_TAGS: usize =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MAX_TAGS_PER_RESOURCE;

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

async fn create(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/model-import-jobs", Some(body)).await
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/model-import-jobs", Some(body))
        .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/model-import-jobs/{}", label(identifier)),
        None,
    )
    .await
}

async fn get_in_region(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/model-import-jobs/{}", label(identifier)),
        None,
    )
    .await
}

// ---------------------------------------------------------------------------------------
// Expectations the model fixes.
// ---------------------------------------------------------------------------------------

/// Whether `value` is an ARN of `resource_type` in `REGION` whose resource id is the
/// `[a-z0-9]{12}` both `ModelImportJobArn` and `ImportedModelArn` declare.
fn is_minted_arn(value: &str, resource_type: &str) -> bool {
    let prefix = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:{resource_type}/");
    value.strip_prefix(&prefix).is_some_and(|id| {
        id.len() == 12 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// The `importedModelKmsKeyArn` a read reports for a request's `importedModelKmsKeyId`.
///
/// `importedModelKmsKeyId` admits a key ARN, a bare 36-character key id or an alias, while
/// `KmsKeyArn` (`^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`)
/// admits only a key ARN: so a key ARN passes through, a bare id is completed into one,
/// and an alias — which resolves to a key id only inside KMS — is reported by nothing.
fn expected_kms_key_arn(body: &Value) -> Value {
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

// ---------------------------------------------------------------------------------------
// 1. A created job round-trips through a read, under both identifier spellings.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_import_job_round_trips_through_a_read() {
    let emu = emulator();

    // Rows differ along what the rule generalises over: the data source, the VPC
    // configuration, the KMS key spelling, how many tags each tagged resource carries, and
    // the name lengths the model admits (`JobName`/`ImportedModelName` are both
    // `@length(1..63)`).
    let long_job_name = format!("j{}", "x".repeat(62));
    let long_model_name = format!("m{}", "y".repeat(62));
    let rows: Vec<(&str, Value)> = vec![
        (
            "required members only",
            create_body("import-one", "ImportedOne", S3_URIS[0], ROLE_ARNS[0], json!({})),
        ),
        (
            "vpc configuration and a key ARN",
            create_body(
                "import-two",
                "Imported_Two",
                S3_URIS[1],
                ROLE_ARNS[1],
                json!({
                    "vpcConfig": {
                        "subnetIds": ["subnet-0a1b2c3d", "subnet-4e5f6a7b"],
                        "securityGroupIds": ["sg-0123456789abcdef0"],
                    },
                    "importedModelKmsKeyId":
                        format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{KMS_KEY_ID}"),
                }),
            ),
        ),
        (
            "a bare key id and tags on both resources",
            create_body(
                "import.three",
                "Imported-Three",
                S3_URIS[2],
                ROLE_ARNS[0],
                json!({
                    "importedModelKmsKeyId": KMS_KEY_ID,
                    "jobTags": tags(1),
                    "importedModelTags": tags(MAX_TAGS),
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
            "names at the minimum modelled length",
            create_body("a", "b", S3_URIS[1], ROLE_ARNS[0], json!({})),
        ),
        (
            "names at the maximum modelled length",
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
        let created = create(&emu, body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateModelImportJob is bound to @http(code: 201), got {} {:?}",
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
            is_minted_arn(&job_arn, "model-import-job"),
            "case {case:?}: jobArn must be a ModelImportJobArn of the signing region, got \
             {job_arn:?}"
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
                "case {case:?} read {reader}: GetModelImportJob must answer 200, got {} {:?}",
                got.status, got.body
            );
            assert_eq!(
                got.get("jobArn"),
                json!(job_arn),
                "case {case:?} read {reader}: the read must report the job the create minted, \
                 got {:?}",
                got.body
            );
            for member in ["jobName", "importedModelName", "roleArn", "modelDataSource"] {
                assert_eq!(
                    got.get(member),
                    sent(&body, member),
                    "case {case:?} read {reader}: {member} must come back as the create sent \
                     it, got {:?}",
                    got.get(member)
                );
            }
            assert_eq!(
                got.get("vpcConfig"),
                sent(&body, "vpcConfig"),
                "case {case:?} read {reader}: vpcConfig must come back as the create sent it, \
                 got {:?}",
                got.get("vpcConfig")
            );
            assert_eq!(
                got.get("importedModelKmsKeyArn"),
                expected_kms_key_arn(&body),
                "case {case:?} read {reader}: importedModelKmsKeyArn must be the key ARN the \
                 request's key id names, got {:?}",
                got.get("importedModelKmsKeyArn")
            );
            let model_arn = got.text("importedModelArn");
            assert!(
                is_minted_arn(&model_arn, "imported-model"),
                "case {case:?} read {reader}: importedModelArn must be an ImportedModelArn of \
                 the signing region, got {model_arn:?}"
            );
            assert_eq!(
                got.text("status"),
                IN_PROGRESS,
                "case {case:?} read {reader}: a job that has just started is running, got {:?}",
                got.body
            );
            assert!(
                got.get("endTime").is_null(),
                "case {case:?} read {reader}: endTime names the move to a terminal state, which \
                 has not happened, got {:?}",
                got.get("endTime")
            );
            assert!(
                got.get("failureMessage").is_null(),
                "case {case:?} read {reader}: a running job has no failure to report, got {:?}",
                got.get("failureMessage")
            );
            // Read as instants, which refuse anything but the modelled date-time string.
            let created_at = got.instant("creationTime");
            assert!(
                created_at > 0.0,
                "case {case:?} read {reader}: creationTime must name a real instant, got {:?}",
                got.get("creationTime")
            );
            assert!(
                (got.instant("lastModifiedTime") - created_at).abs() < f64::EPSILON,
                "case {case:?} read {reader}: nothing has changed the job yet, so \
                 lastModifiedTime is its creation instant, got {:?}",
                got.body
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 2. An identifier no stored job answers to.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_unknown_job_identifier_is_refused_as_not_found() {
    let emu = emulator();

    // One live job in each of two regions, so the refusals below are about the identifier
    // rather than about an empty store.
    let live = create(
        &emu,
        create_body("live-job", "LiveModel", S3_URIS[0], ROLE_ARNS[0], json!({})),
    )
    .await;
    assert_eq!(
        live.status, 201,
        "the live job must be created for this suite to say anything, got {} {:?}",
        live.status, live.body
    );
    let live_arn = live.text("jobArn");
    let elsewhere = create_in_region(
        &emu,
        "eu-west-1",
        create_body(
            "job-elsewhere",
            "ModelElsewhere",
            S3_URIS[1],
            ROLE_ARNS[0],
            json!({}),
        ),
    )
    .await;
    assert_eq!(
        elsewhere.status, 201,
        "the other region's job must be created, got {} {:?}",
        elsewhere.status, elsewhere.body
    );

    let rows: Vec<(&str, String)> = vec![
        ("a name no create ever used", "absent-job".to_string()),
        (
            "a job name that exists in another region",
            "job-elsewhere".to_string(),
        ),
        (
            "a well-formed job ARN that was never minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:model-import-job/aaaaaaaaaaaa"),
        ),
        (
            "a prefix of the live job's name",
            "live".to_string(),
        ),
    ];

    for (case, identifier) in rows {
        let reply = get(&emu, &identifier).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: ResourceNotFoundException is modelled @httpError(404), got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: an identifier naming no job is a not-found, got {:?}",
            reply.error
        );
        assert!(
            !refusal_message(&reply).is_empty(),
            "case {case:?}: the refusal must state a message, got {:?}",
            reply.body
        );
    }

    // The live job is still readable, so none of those reads changed the store.
    let still = get(&emu, &live_arn).await;
    assert_eq!(
        still.status, 200,
        "the live job must still be readable after the refusals, got {} {:?}",
        still.status, still.body
    );
}

// ---------------------------------------------------------------------------------------
// 3. The status the clock owes.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_running_import_job_settles_to_completed() {
    // The emulator's span for running an import, which the handler stores as a due instant
    // rather than waits on.
    let settle = service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::IMPORT_JOB_COMPLETE_AFTER;
    assert!(
        settle >= std::time::Duration::from_millis(250)
            && settle <= std::time::Duration::from_millis(750),
        "the import span must be observable but short, got {settle:?}"
    );

    let emu = emulator();
    let created = create(
        &emu,
        create_body(
            "settling-job",
            "SettlingModel",
            S3_URIS[0],
            ROLE_ARNS[0],
            json!({}),
        ),
    )
    .await;
    let job_arn = created.text("jobArn");

    // A caller acting straight after the create still sees the transient status.
    let immediate = get(&emu, &job_arn).await;
    assert_eq!(
        immediate.text("status"),
        IN_PROGRESS,
        "case read while importing: the job is still running, got {:?}",
        immediate.body
    );
    assert!(
        immediate.get("endTime").is_null(),
        "case read while importing: a running job has not ended, got {:?}",
        immediate.get("endTime")
    );

    tokio::time::sleep(settle + std::time::Duration::from_millis(150)).await;
    let settled = get(&emu, &job_arn).await;
    assert_eq!(
        settled.text("status"),
        COMPLETED,
        "case read once the span has passed: the import must have completed, got {:?}",
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

    // The transition was owed by the CLOCK, so a second read reports the very same
    // instants rather than ones moved on by the read itself.
    let again = get(&emu, &job_arn).await;
    assert_eq!(
        again.body, settled.body,
        "case read twice: a read reports the job, it does not move it on, got {:?} then {:?}",
        settled.body, again.body
    );
}

// ---------------------------------------------------------------------------------------
// 4. Idempotency tokens.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_mints_nothing() {
    let emu = emulator();
    let token = "token-for-the-first-import";

    let first = create(
        &emu,
        create_body(
            "token-minted",
            "TokenMinted",
            S3_URIS[0],
            ROLE_ARNS[0],
            json!({ "clientRequestToken": token }),
        ),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "the first create must be minted, got {} {:?}",
        first.status, first.body
    );
    let minted = first.text("jobArn");

    // The replay answers with what the first call minted even when it names another job,
    // and nothing is minted for those other names.
    let replay = create(
        &emu,
        create_body(
            "token-replayed",
            "TokenReplayed",
            S3_URIS[1],
            ROLE_ARNS[1],
            json!({ "clientRequestToken": token }),
        ),
    )
    .await;
    assert_eq!(
        replay.text("jobArn"),
        minted,
        "case replay: a replayed clientRequestToken answers with the job the first call \
         minted, got {:?}",
        replay.body
    );
    for unminted in ["token-replayed", "TokenReplayed"] {
        let reply = get(&emu, unminted).await;
        assert_eq!(
            reply.status, 404,
            "case replay: the replay must mint nothing for {unminted:?}, got {} {:?}",
            reply.status, reply.body
        );
    }
    let stored = get(&emu, &minted).await;
    assert_eq!(
        stored.get("jobName"),
        json!("token-minted"),
        "case replay: the stored job is the one the FIRST call described, got {:?}",
        stored.body
    );

    // A fresh token mints another job.
    let fresh = create(
        &emu,
        create_body(
            "token-fresh",
            "TokenFresh",
            S3_URIS[2],
            ROLE_ARNS[0],
            json!({ "clientRequestToken": "token-for-another-import" }),
        ),
    )
    .await;
    assert_ne!(
        fresh.text("jobArn"),
        minted,
        "case fresh token: another token mints another job, got {:?}",
        fresh.body
    );
}

// ---------------------------------------------------------------------------------------
// 5. Both names are unique per account per region.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_job_name_and_an_imported_model_name_are_unique_per_region() {
    let emu = emulator();
    let first = create(
        &emu,
        create_body("taken-job", "TakenModel", S3_URIS[0], ROLE_ARNS[0], json!({})),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "the first create must be minted, got {} {:?}",
        first.status, first.body
    );

    let rows: Vec<(&str, Value)> = vec![
        (
            "the job name again",
            create_body(
                "taken-job",
                "AnotherModel",
                S3_URIS[1],
                ROLE_ARNS[0],
                json!({}),
            ),
        ),
        (
            "the imported model name again",
            create_body(
                "another-job",
                "TakenModel",
                S3_URIS[1],
                ROLE_ARNS[0],
                json!({}),
            ),
        ),
        (
            "both names again",
            create_body("taken-job", "TakenModel", S3_URIS[2], ROLE_ARNS[1], json!({})),
        ),
    ];
    for (case, body) in rows {
        let reply = create(&emu, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: ConflictException is modelled @httpError(400), got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ConflictException"),
            "case {case:?}: reusing a name that names a resource is a conflict, got {:?}",
            reply.error
        );
    }

    // The same two names in another region name nothing, so they are minted there.
    let elsewhere = create_in_region(
        &emu,
        "ap-southeast-2",
        create_body("taken-job", "TakenModel", S3_URIS[0], ROLE_ARNS[0], json!({})),
    )
    .await;
    assert_eq!(
        elsewhere.status, 201,
        "case another region: a name is unique per region, so it is free there, got {} {:?}",
        elsewhere.status, elsewhere.body
    );
    let elsewhere_arn = elsewhere.text("jobArn");
    assert_ne!(
        elsewhere_arn,
        first.text("jobArn"),
        "case another region: the second region's job is its own resource, got {elsewhere_arn:?}"
    );
    let read_there = get_in_region(&emu, "ap-southeast-2", "taken-job").await;
    assert_eq!(
        read_there.get("jobArn"),
        json!(elsewhere_arn),
        "case another region: the name resolves to that region's job, got {:?}",
        read_there.body
    );
}

// ---------------------------------------------------------------------------------------
// 6. The per-resource tag bound.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn more_tags_than_a_resource_may_carry_are_refused() {
    let emu = emulator();

    // One row per TAGGED resource: the request carries tags for the job and for the model
    // it imports, and each list is held to the bound separately.
    let rows: Vec<(&str, Value)> = vec![
        (
            "jobTags over the bound",
            json!({ "jobTags": tags(MAX_TAGS + 1) }),
        ),
        (
            "importedModelTags over the bound",
            json!({ "importedModelTags": tags(MAX_TAGS + 1) }),
        ),
    ];
    for (index, (case, extra)) in rows.into_iter().enumerate() {
        let job_name = format!("tagged-job-{index}");
        let reply = create(
            &emu,
            create_body(
                &job_name,
                &format!("TaggedModel{index}"),
                S3_URIS[0],
                ROLE_ARNS[0],
                extra,
            ),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: TooManyTagsException is modelled @httpError(400), got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("TooManyTagsException"),
            "case {case:?}: more than {MAX_TAGS} tags on a resource is refused as such, got {:?}",
            reply.error
        );
        let stored = get(&emu, &job_name).await;
        assert_eq!(
            stored.status, 404,
            "case {case:?}: a refused create stores nothing, got {} {:?}",
            stored.status, stored.body
        );
    }

    // Exactly the bound on both lists is admitted, so the refusal is about the bound and
    // not about carrying tags at all.
    let admitted = create(
        &emu,
        create_body(
            "tagged-at-the-bound",
            "TaggedAtTheBound",
            S3_URIS[1],
            ROLE_ARNS[0],
            json!({ "jobTags": tags(MAX_TAGS), "importedModelTags": tags(MAX_TAGS) }),
        ),
    )
    .await;
    assert_eq!(
        admitted.status, 201,
        "case at the bound: {MAX_TAGS} tags per resource is admitted, got {} {:?}",
        admitted.status, admitted.body
    );
}

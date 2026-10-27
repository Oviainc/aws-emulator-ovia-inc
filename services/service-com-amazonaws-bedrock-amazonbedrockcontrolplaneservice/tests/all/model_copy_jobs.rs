// SPDX-License-Identifier: CC0-1.0

//! The model copy job resource end-to-end — `CreateModelCopyJob` submits it,
//! `GetModelCopyJob` reads it back and `ListModelCopyJobs` reports it — and the two
//! refusals the submission makes, over the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A value that is no ARN at all, submitted to a member whose modelled form admits
//!    nothing but the ARN of a resource Amazon Bedrock validates itself, is refused 400
//!    `ValidationException` with the service's own fixed sentence** — no member path, no
//!    rejected value, no quoted expression. A member that also admits a bare NAME, and a
//!    member declared as the ARN of a resource outside that catalog, keep the framework's
//!    constraint envelope: the MEMBER decides whether a value is read as an ARN.
//! 2. **A `custom-model` ARN submitted as a model source must embed a base model the
//!    published catalog carries**, or the submission is refused 400 `ValidationException`
//!    naming that base model; the `imported` segment names a model customized from none, so
//!    it carries no such requirement.
//! 3. **A submitted copy round-trips**: every member the read and the listing report comes
//!    out of the stored record, and the listing applies every filter the model declares.
//! 4. **The copy is `InProgress` first and settles to `Completed`**, which is what brings
//!    the COPIED model into being as a custom model of the target region — and the settle
//!    is owed by the clock, not by the read.
//! 5. **A replayed `clientRequestToken` copies nothing a second time**, and a
//!    `targetModelName` a custom model of the target region already holds is refused.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateModelCopyJob,GetModelCopyJob,ListModelCopyJobs")`),
//! or from the recorded sentences `crate::state` holds. The requests go through
//! `crate::support` — the ONE harness every suite in this crate drives the emulator with.

use std::time::Duration;

use serde_json::{json, Value};

use crate::support::{
    date_time_seconds, emulator, label, Emulator, Reply, ACCOUNT, REGION,
};

/// The region the source models these rows copy FROM live in. `CreateModelCopyJob` "Copies
/// a model to another region so that it can be used there", so the source lives in a
/// region other than the one the request addresses.
const SOURCE_REGION: &str = "us-east-1";

/// `ModelCopyJobStatus` is `[InProgress, Completed, Failed]`
/// (`smithy_query(query="shape",shape="ModelCopyJobStatus")`).
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";

/// How long after a submission the `InProgress` status is owed its move to `Completed`, as
/// the service states it.
const COMPLETE_AFTER: Duration =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MODEL_COPY_JOB_COMPLETE_AFTER;

/// The base-model segment a `CustomModelArn` carries for a model whose artifacts were
/// brought in rather than customized from a foundation model.
const IMPORTED_BASE: &str =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::CUSTOM_MODEL_IMPORTED_BASE;

/// A `KmsKeyArn`: `^arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/abcd1234-ab12-cd34-ef56-abcdef123456";

/// The marker of the framework's constraint envelope: the phrase a client branches on to
/// tell a constraint violation from a service's own sentence.
const CONSTRAINT_ENVELOPE_MARKER: &str = "validation error";

/// What this service answers for a value submitted to an ARN-only member that is no ARN,
/// written out here rather than read off the crate so the recorded sentence is pinned by
/// this suite independently of the constant that produces it.
const ARN_INVALID: &str = "Provided ARN is invalid.";

// ---------------------------------------------------------------------------------------
// Request helpers.
// ---------------------------------------------------------------------------------------

/// The `$.message` of an error envelope, or the empty string when it carries none.
fn message(reply: &Reply) -> String {
    reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

async fn submit_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/model-copy-jobs", Some(body))
        .await
}

async fn submit(emu: &Emulator, body: Value) -> Reply {
    submit_in_region(emu, REGION, body).await
}

async fn read(emu: &Emulator, job_arn: &str) -> Reply {
    emu.call("GET", &format!("/model-copy-jobs/{}", label(job_arn)), None)
        .await
}

async fn list(emu: &Emulator, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/model-copy-jobs".to_string()
    } else {
        format!("/model-copy-jobs?{query}")
    };
    emu.call("GET", &path, None).await
}

/// Create one custom model in `region` and hand back its `CustomModelArn` — the model the
/// rows below copy.
async fn given_custom_model(emu: &Emulator, region: &str, name: &str) -> String {
    let reply = emu
        .call_in_region(
            region,
            "POST",
            "/custom-models/create-custom-model",
            Some(json!({
                "modelName": name,
                "customModelDataSource": {
                    "modelPackageArnDataSource": {
                        "modelPackageArn":
                            "arn:aws:sagemaker:us-east-1:000000000000:model-package/export-1",
                    },
                },
            })),
        )
        .await;
    assert_eq!(
        reply.status, 202,
        "case {name:?}: CreateCustomModel must answer 202, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("modelArn")
}

/// Read one custom model of [`REGION`] back.
async fn read_custom_model(emu: &Emulator, identifier: &str) -> Reply {
    emu.call("GET", &format!("/custom-models/{}", label(identifier)), None)
        .await
}

/// Age the stored copy job `arn` names by `by`, as though it had been submitted that much
/// earlier: its creation instant and the instant its clock owes the completion at move back
/// together, so only the amount of its timeline that has already elapsed changes.
///
/// Cloned out of the store the handlers share, because the clock rule is about a span of
/// wall-clock time no test can sleep through.
fn age_job(emu: &Emulator, arn: &str, by: Duration) {
    let state =
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::service_state(
            &*emu.engine,
        )
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let job = store
        .model_copy_jobs
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the store must hold the submitted job {arn}"));
    job.created_at -= by;
    if let Some(due) = job.completed_due_at.as_mut() {
        *due -= by;
    }
}

// ---------------------------------------------------------------------------------------
// 1. A value that is no ARN, submitted to a member that admits nothing else.
// ---------------------------------------------------------------------------------------

/// What one refused request must be answered with.
#[derive(Debug)]
enum Expect {
    /// 400 `ValidationException` whose `$.message` is EXACTLY [`ARN_INVALID`].
    ServiceSentence,
    /// 400 `ValidationException` worded as the framework's constraint envelope: this
    /// service reads no ARN out of the member, so the model's own refusal stands.
    ConstraintEnvelope,
}

/// One request and the answer the rule demands.
struct Case {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    expect: Expect,
}

/// A `CreateModelCopyJob` body carrying `source` as its `sourceModelArn`.
fn copy_body(source: &str, target_name: &str) -> Value {
    json!({ "sourceModelArn": source, "targetModelName": target_name })
}

/// Every request this part of the suite drives.
///
/// The rows differ along what the rule generalises over: which member carries the value (a
/// body member and a URI label, on three operations), HOW the value fails to be an ARN
/// (no colons at all, too few fields, an empty partition, a wrong-cased `arn` prefix,
/// spaces), and the values the rule does NOT reach (a well-formed ARN of another vendor, a
/// member that admits a bare name beside an ARN, and a member declared as the ARN of a
/// resource outside the catalog this service checks itself).
fn cases() -> Vec<Case> {
    vec![
        Case {
            // RECORDED: the behavioral suite submits exactly this value, whose length is
            // past `ModelArn`'s modelled minimum of 20 so the member's `@length` does not
            // decide the refusal.
            case: "sourceModelArn, a value with no colon at all",
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(copy_body("not-an-arn-at-all-0123456789", "copied-model")),
            expect: Expect::ServiceSentence,
        },
        Case {
            case: "sourceModelArn, an ARN missing its resource field",
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(copy_body("arn:aws:bedrock:us-west-2:000000000000", "copied-model")),
            expect: Expect::ServiceSentence,
        },
        Case {
            case: "sourceModelArn, an ARN with an empty partition",
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(copy_body(
                "arn::bedrock:us-west-2:000000000000:custom-model/imported/abcdef123456",
                "copied-model",
            )),
            expect: Expect::ServiceSentence,
        },
        Case {
            case: "sourceModelArn, an upper-cased arn prefix",
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(copy_body(
                "ARN:AWS:BEDROCK:US-WEST-2:000000000000:CUSTOM-MODEL/IMPORTED/ABCDEF123456",
                "copied-model",
            )),
            expect: Expect::ServiceSentence,
        },
        Case {
            case: "sourceModelArn, a sentence a caller typed",
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(copy_body("the model I trained last week", "copied-model")),
            expect: Expect::ServiceSentence,
        },
        Case {
            // A URI LABEL, not a body member: `ModelCopyJobArn` admits nothing but the ARN
            // of a `model-copy-job`, which is one of the resources this service checks.
            case: "jobArn, a bare identifier on the read",
            method: "GET",
            path: "/model-copy-jobs/not-a-job-arn".to_string(),
            body: None,
            expect: Expect::ServiceSentence,
        },
        Case {
            // A third operation, whose `modelArn` is a `CustomModelArn`.
            case: "modelArn on the deployment create, a value with no colon",
            method: "POST",
            path: "/model-customization/custom-model-deployments".to_string(),
            body: Some(json!({
                "modelDeploymentName": "probe-deployment",
                "modelArn": "not-an-arn-at-all-0123456789",
            })),
            expect: Expect::ServiceSentence,
        },
        Case {
            // WELL FORMED as an ARN, but of another vendor's resource: there is an ARN to
            // read, so the refusal is the one the member's own expression words.
            case: "sourceModelArn, a well-formed ARN of another vendor",
            method: "POST",
            path: "/model-copy-jobs".to_string(),
            body: Some(copy_body("arn:aws:s3:::bedrock-model-exports", "copied-model")),
            expect: Expect::ConstraintEnvelope,
        },
        Case {
            // `ProvisionedModelId` admits a bare NAME beside an ARN, so a caller who
            // submits a name has named a resource rather than spelled an ARN.
            case: "provisionedModelId, which admits a bare name",
            method: "GET",
            path: "/provisioned-model-throughput/not%20a%20name!!".to_string(),
            body: None,
            expect: Expect::ConstraintEnvelope,
        },
        Case {
            // `EvaluationJobIdentifier` admits nothing but an ARN too, but of a resource
            // this service does NOT check itself: the corpus records the constraint
            // envelope for a value outside its form.
            case: "jobIdentifier of an evaluation job, outside the catalog",
            method: "GET",
            path: "/evaluation-jobs/not-a-job".to_string(),
            body: None,
            expect: Expect::ConstraintEnvelope,
        },
    ]
}

#[tokio::test]
async fn a_value_that_is_no_arn_is_refused_in_this_services_own_words() {
    let emu = emulator();
    for case in cases() {
        let reply = emu
            .call(case.method, &case.path, case.body.clone())
            .await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {:?}: expected 400 ValidationException, got {} {:?}",
            case.case,
            reply.status,
            reply.body
        );
        let answered = message(&reply);
        match case.expect {
            Expect::ServiceSentence => {
                assert_eq!(
                    answered, ARN_INVALID,
                    "case {:?}: expected the message {ARN_INVALID:?}, got {answered:?}",
                    case.case
                );
            }
            Expect::ConstraintEnvelope => {
                assert!(
                    answered.contains(CONSTRAINT_ENVELOPE_MARKER),
                    "case {:?}: expected the framework's constraint envelope (containing \
                     {CONSTRAINT_ENVELOPE_MARKER:?}), got {answered:?}",
                    case.case
                );
            }
        }
    }
}

#[tokio::test]
async fn the_arn_sentence_reports_nothing_about_the_request() {
    // The sentence is FIXED: a client that reads it learns the ARN is unusable, and the
    // value a caller submitted is not reflected back into it.
    let emu = emulator();
    for case in cases() {
        let Expect::ServiceSentence = case.expect else {
            continue;
        };
        let reply = emu
            .call(case.method, &case.path, case.body.clone())
            .await;
        let answered = message(&reply);
        for forbidden in [
            CONSTRAINT_ENVELOPE_MARKER,
            "failed to satisfy constraint",
            "regular expression pattern",
            "sourceModelArn",
            "jobArn",
            "modelArn",
            "not-an-arn-at-all",
        ] {
            assert!(
                !answered.contains(forbidden),
                "case {:?}: the ARN sentence must carry nothing about the request, but it \
                 carries {forbidden:?}: {answered:?}",
                case.case
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 2. The base model a `custom-model` source ARN embeds.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_source_whose_base_model_is_not_published_is_refused_naming_it() {
    let emu = emulator();
    // (case, the base-model segment the source ARN embeds, refused)
    //
    // The rows differ along what the rule generalises over: a fabricated `provider.model`
    // segment, a fabricated one carrying the `:`-separated version qualifiers a base model
    // id may end with, a segment naming a model the published catalog DOES carry
    // (`anthropic.claude-3-haiku-20240307-v1:0` is a row of
    // `ListFoundationModels`), and the `imported` segment, which names a model customized
    // from no base model at all.
    let rows: [(&str, &str, bool); 4] = [
        ("a fabricated base model", "sim2real-noexist.v1", true),
        (
            "a fabricated base model with version qualifiers",
            "sim2real-noexist.v1:0:8k",
            true,
        ),
        (
            "a base model the published catalog carries",
            "anthropic.claude-3-haiku-20240307-v1:0",
            false,
        ),
        ("the imported segment, which has no base model", IMPORTED_BASE, false),
    ];

    for (case, segment, refused) in rows {
        let source =
            format!("arn:aws:bedrock:{SOURCE_REGION}:{ACCOUNT}:custom-model/{segment}/abcdef123456");
        let reply = submit(&emu, copy_body(&source, "copied-model")).await;
        if refused {
            assert_eq!(
                (reply.status, reply.error.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected 400 ValidationException, got {} {:?}",
                reply.status,
                reply.body
            );
            let expected = format!(
                "The foundation model '{segment}' for custom model is not a valid foundation \
                 model"
            );
            assert_eq!(
                message(&reply),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                message(&reply)
            );
        } else {
            // The base model is one to copy a model of, so the submission gets past this
            // rule — and is refused for naming no model THIS ACCOUNT holds, which is a
            // different failure with a different status.
            assert_eq!(
                (reply.status, reply.error.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected 404 ResourceNotFoundException for a source nothing \
                 created, got {} {:?}",
                reply.status,
                reply.body
            );
        }
        // Nothing was submitted either way: no copy job exists.
        let listed = list(&emu, "").await;
        assert_eq!(
            listed.get("modelCopyJobSummaries"),
            json!([]),
            "case {case:?}: a refused submission creates no copy job, got {:?}",
            listed.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3 + 4. The round trip, the listing's filters, and the completion the clock owes.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_submitted_copy_round_trips_and_completes_into_a_custom_model() {
    let emu = emulator();
    let source = given_custom_model(&emu, SOURCE_REGION, "source-model").await;

    let submitted = submit(
        &emu,
        json!({
            "sourceModelArn": source,
            "targetModelName": "copied-model",
            "modelKmsKeyId": KMS_KEY_ARN,
            "targetModelTags": [{ "key": "team", "value": "research" }],
        }),
    )
    .await;
    assert_eq!(
        submitted.status, 201,
        "case \"the submission\": CreateModelCopyJob must answer 201, got {} {:?}",
        submitted.status, submitted.body
    );
    let job_arn = submitted.text("jobArn");
    let expected_prefix = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:model-copy-job/");
    assert!(
        job_arn.starts_with(&expected_prefix),
        "case \"the submission\": expected a jobArn under {expected_prefix:?}, got {job_arn:?}"
    );

    // Every member the read reports comes out of the stored record.
    let job = read(&emu, &job_arn).await;
    assert_eq!(
        job.status, 200,
        "case \"the read\": GetModelCopyJob must answer 200, got {} {:?}",
        job.status, job.body
    );
    let target_model_arn = job.text("targetModelArn");
    let expected_target_prefix =
        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:custom-model/{IMPORTED_BASE}/");
    for (member, expected) in [
        ("jobArn", json!(job_arn)),
        ("status", json!(IN_PROGRESS)),
        ("sourceModelArn", json!(source)),
        ("sourceModelName", json!("source-model")),
        ("sourceAccountId", json!(ACCOUNT)),
        ("targetModelName", json!("copied-model")),
        ("targetModelKmsKeyArn", json!(KMS_KEY_ARN)),
        (
            "targetModelTags",
            json!([{ "key": "team", "value": "research" }]),
        ),
        ("failureMessage", json!(null)),
    ] {
        assert_eq!(
            job.get(member),
            expected,
            "case {member:?}: expected {expected:?}, got {:?}",
            job.get(member)
        );
    }
    assert!(
        target_model_arn.starts_with(&expected_target_prefix),
        "case \"targetModelArn\": the copy is a custom model of the target region, so its ARN \
         must be under {expected_target_prefix:?}, got {target_model_arn:?}"
    );
    // The creation instant is an RFC 3339 string, as `@timestampFormat("date-time")` says.
    let created = date_time_seconds("creationTime", &job.text("creationTime"));
    assert!(
        created > 0.0,
        "case \"creationTime\": expected an instant after the epoch, got {created}"
    );

    // The listing reports the same record, and the summary agrees with the read.
    let listed = list(&emu, "").await;
    let summary = listed
        .get("modelCopyJobSummaries")
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .unwrap_or(Value::Null);
    for member in [
        "jobArn",
        "status",
        "creationTime",
        "sourceModelArn",
        "sourceModelName",
        "sourceAccountId",
        "targetModelArn",
        "targetModelName",
        "targetModelKmsKeyArn",
        "targetModelTags",
    ] {
        assert_eq!(
            summary.get(member).cloned().unwrap_or(Value::Null),
            job.get(member),
            "case {member:?}: the summary and the read must report the same value, got {:?} \
             and {:?}",
            summary.get(member),
            job.get(member)
        );
    }

    // Every filter the model declares, each in a row that admits the job and a row that
    // excludes it.
    let other_account = "123456789012";
    let rows: [(&str, String, bool); 10] = [
        ("statusEquals, the status it holds", "statusEquals=InProgress".to_string(), true),
        ("statusEquals, another status", "statusEquals=Completed".to_string(), false),
        (
            "sourceModelArnEquals, the source",
            format!("sourceModelArnEquals={}", label(&source)),
            true,
        ),
        (
            "sourceModelArnEquals, another model",
            format!(
                "sourceModelArnEquals={}",
                label(&format!(
                    "arn:aws:bedrock:{SOURCE_REGION}:{ACCOUNT}:custom-model/imported/zzzzzzzzzzzz"
                ))
            ),
            false,
        ),
        (
            "outputModelNameContains, a fragment of the name",
            "outputModelNameContains=copied".to_string(),
            true,
        ),
        (
            "outputModelNameContains, a fragment the name lacks",
            "outputModelNameContains=zzz".to_string(),
            false,
        ),
        (
            "sourceAccountEquals, this account",
            format!("sourceAccountEquals={ACCOUNT}"),
            true,
        ),
        (
            "sourceAccountEquals, another account",
            format!("sourceAccountEquals={other_account}"),
            false,
        ),
        (
            "creationTimeAfter, before the submission",
            "creationTimeAfter=2000-01-01T00%3A00%3A00Z".to_string(),
            true,
        ),
        (
            "creationTimeBefore, before the submission",
            "creationTimeBefore=2000-01-01T00%3A00%3A00Z".to_string(),
            false,
        ),
    ];
    for (case, query, admits) in rows {
        let filtered = list(&emu, &query).await;
        assert_eq!(
            filtered.status, 200,
            "case {case:?}: ListModelCopyJobs must answer 200, got {} {:?}",
            filtered.status, filtered.body
        );
        let reported: Vec<String> = filtered
            .get("modelCopyJobSummaries")
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|row| {
                row.get("jobArn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = if admits {
            vec![job_arn.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // The copied model does not exist while the copy is running: the completion is what
    // brings it into being.
    let early = read_custom_model(&emu, &target_model_arn).await;
    assert_eq!(
        early.status, 404,
        "case \"the copied model while the copy runs\": expected 404, got {} {:?}",
        early.status, early.body
    );

    // Once the span the clock owes has passed, the job is `Completed` and the copied model
    // is a custom model of the target region — without anything having read the job first.
    age_job(&emu, &job_arn, COMPLETE_AFTER);
    let copied = read_custom_model(&emu, &target_model_arn).await;
    assert_eq!(
        copied.status, 200,
        "case \"the copied model after the span\": expected 200, got {} {:?}",
        copied.status, copied.body
    );
    assert_eq!(
        copied.get("modelName"),
        json!("copied-model"),
        "case \"the copied model\": expected the target name, got {:?}",
        copied.body
    );
    let settled = read(&emu, &job_arn).await;
    assert_eq!(
        settled.get("status"),
        json!(COMPLETED),
        "case \"the job after the span\": expected {COMPLETED:?}, got {:?}",
        settled.body
    );
}

// ---------------------------------------------------------------------------------------
// 5. Idempotency, the target name index, and a read of a job nothing submitted.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_token_copies_nothing_new_and_a_taken_target_name_is_refused() {
    let emu = emulator();
    let source = given_custom_model(&emu, SOURCE_REGION, "idempotent-source").await;

    for (case, token) in [("Token1", "Token1"), ("token-2-abc", "token-2-abc")] {
        let mut body = copy_body(&source, &format!("copy-of-{token}"));
        body["clientRequestToken"] = json!(token);

        let first = submit(&emu, body.clone()).await;
        assert_eq!(
            first.status, 201,
            "case {case:?}: the first submission must be admitted, got {} {:?}",
            first.status, first.body
        );
        let replay = submit(&emu, body).await;
        assert_eq!(
            replay.status, 201,
            "case {case:?}: a replay must be admitted, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.text("jobArn"),
            first.text("jobArn"),
            "case {case:?}: a replay must answer with the job the first request submitted"
        );
    }

    // Two jobs, one per token — the replays added none.
    let listed = list(&emu, "").await;
    assert_eq!(
        listed.get("modelCopyJobSummaries").as_array().map(Vec::len),
        Some(2),
        "case \"the listing after two tokens and two replays\": expected two jobs, got {:?}",
        listed.body
    );

    // A `targetModelName` the target region already holds names a custom model that
    // exists, which the copy would have to mint a second time.
    let held = given_custom_model(&emu, REGION, "already-here").await;
    assert!(
        held.contains(REGION),
        "case \"the held model\": expected a model of this region, got {held:?}"
    );
    let refused = submit(&emu, copy_body(&source, "already-here")).await;
    assert_eq!(
        (refused.status, refused.error.as_deref()),
        (400, Some("ValidationException")),
        "case \"a target name already held\": expected 400 ValidationException, got {} {:?}",
        refused.status,
        refused.body
    );

    // And a well-formed job ARN nothing submitted is a not-found rather than a refusal.
    let unknown = read(
        &emu,
        &format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:model-copy-job/zzzzzzzzzzzz"),
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"a job nothing submitted\": expected 404 ResourceNotFoundException, got {} {:?}",
        unknown.status,
        unknown.body
    );
}

// SPDX-License-Identifier: CC0-1.0

//! What revoking a SIGNATURE does, end to end over HTTP.
//!
//! Every case goes through the router, so it exercises routing, deserialization,
//! the handler, the persisted record and serialization — the path a client takes.
//! The rules under test:
//!
//! * `RevokeSignature` records the revocation of the signature a stored signing
//!   job produced: the job carries the request's `reason`, when the revocation
//!   happened and the identity that performed it, and every later
//!   `DescribeSigningJob` reports that record unchanged. The job's `status` is
//!   untouched — `SigningStatus` is `InProgress | Failed | Succeeded` and has no
//!   revoked value, so the record is what carries the fact;
//! * a revocation is recorded ONCE: revoking an already-revoked job leaves the
//!   first record standing;
//! * a signature can be revoked exactly when the platform it was produced on
//!   publishes `revocationSupported` — the capability `GetSigningPlatform`
//!   reports — and a platform published without it refuses the call with the
//!   modelled 400 instead;
//! * a job this endpoint holds none of — an id nothing was started under, an
//!   owner the job does not have, another region's job — is the modelled 404,
//!   and a refused revoke writes nothing.
//!
//! The cases are tables: a row per input class the rules generalise over (the
//! reason's length at both published bounds, which optional members the request
//! carries, the platform, the job's status when it is revoked, how a job id
//! misses).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::ops::types::SIGNING_STATUS_VALUES;
use service_com_amazonaws_signer_wallabyservice::state::{
    SigningWorkflow, JOB_COMPLETION_SPAN, SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for unless a case names another one.
const REGION: &str = "us-west-2";

/// Another region's endpoint, for the cases about which region holds a job.
const OTHER_REGION: &str = "eu-west-1";

/// The status `RevokeSignature` answers a successful revocation with.
///
/// Its modelled output is `smithy.api#Unit`, so the reply has nothing to report:
/// `204 No Content`, the same envelope every other bodyless answer of this
/// service carries.
const NO_CONTENT_STATUS: u16 = 204;

/// The access key id every request below is signed with — the caller identity a
/// revocation records as the revoker, reported as an IAM principal ARN.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handlers fails.
const ACCOUNT_ID: &str = "000000000000";

/// An account id that is not [`ACCOUNT_ID`], satisfying `AccountId`'s
/// `@length(12, 12)` and `^[0-9]{12}$` so it reaches the handler.
const OTHER_ACCOUNT_ID: &str = "210987654321";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be created on
/// every published platform, including the ones that require it.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// A SigV4 credential scope naming this service for `region`. It is what routes a
/// request to signer, and its region is the one whose resources are addressed.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: this service and Amazon S3 over one shared engine.
///
/// A signing job of this workflow reads the object it signs out of Amazon S3 and
/// writes the signed artifact back there, so the suite that seeds jobs mounts S3
/// beside signer and seeds those objects through S3's own front door.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    super::code_signing_objects::mount_object_storage(&mut router, &engine);
    router
}

/// What the emulator answered: the HTTP status, the raw body and its JSON.
struct Answer {
    status: u16,
    raw_body: Vec<u8>,
    body: Value,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `code` member, or the empty string when the body carries none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{region}.amazonaws.com"))
        .header("authorization", signed_for(region));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body().to_vec();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        raw_body: bytes,
        body,
    }
}

async fn put_profile(router: &MultiServiceRouter, name: &str, platform_id: &str) -> Answer {
    call(
        router,
        REGION,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({
            "platformId": platform_id,
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await
}

async fn start_job(
    router: &MultiServiceRouter,
    region: &str,
    profile_name: &str,
    source_version: &str,
) -> Answer {
    call(
        router,
        region,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": {
                "bucketName": SOURCE_BUCKET,
                "key": SOURCE_KEY,
                "version": source_version,
            } },
            "destination": { "s3": { "bucketName": DESTINATION_BUCKET } },
            "profileName": profile_name,
            "clientRequestToken": format!("tok-{region}-{profile_name}"),
        })),
    )
    .await
}

/// The source bucket a seeded job signs out of, the key in it, and the bucket the
/// signed artifact is written to.
const SOURCE_BUCKET: &str = "src";
const SOURCE_KEY: &str = "code.zip";
const DESTINATION_BUCKET: &str = "dst";

async fn describe_job(router: &MultiServiceRouter, region: &str, job_id: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/signing-jobs/{job_id}"),
        None,
    )
    .await
}

async fn revoke_signature(
    router: &MultiServiceRouter,
    region: &str,
    job_id: &str,
    request: Value,
) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-jobs/{job_id}/revoke"),
        Some(request),
    )
    .await
}

/// Seed a profile on `platform_id`, the Amazon S3 objects a job of it reads and
/// writes, and a signing job started from it — answering the job's id.
async fn seed_job(router: &MultiServiceRouter, profile_name: &str, platform_id: &str) -> String {
    let put = put_profile(router, profile_name, platform_id).await;
    assert_eq!(
        put.status, 200,
        "seeding profile {profile_name} on {platform_id}: got {} {:?}",
        put.status, put.body
    );
    let version = super::code_signing_objects::seed_signing_objects(
        router,
        REGION,
        SOURCE_BUCKET,
        SOURCE_KEY,
        DESTINATION_BUCKET,
    )
    .await;
    let started = start_job(router, REGION, profile_name, &version).await;
    assert_eq!(
        started.status, 200,
        "seeding a job from {profile_name}: got {} {:?}",
        started.status, started.body
    );
    let job_id = started.str_member("jobId");
    assert!(
        !job_id.is_empty(),
        "seeding a job from {profile_name}: expected a jobId, got {:?}",
        started.body
    );
    job_id
}

/// The IAM principal ARN a revocation performed by these requests records: the
/// principal the request's credentials resolve to, which is the one IAM user
/// whose credentials this emulator publishes.
fn expected_revoker() -> Value {
    json!(format!("arn:aws:iam::{ACCOUNT_ID}:user/emulator"))
}

/// One revocation to perform. The rows differ along the reason's length (both
/// published bounds of `RevocationReasonString`'s `@length(min: 1, max: 500)`),
/// whether the request names the job's owner at all, and the platform the
/// signature was produced on.
struct RevocationCase {
    name: &'static str,
    platform_id: &'static str,
    reason: String,
    /// `jobOwner` to send, or `None` to omit the optional member.
    job_owner: Option<&'static str>,
}

fn revocation_cases() -> Vec<RevocationCase> {
    vec![
        // The smallest request the model admits: no jobOwner.
        RevocationCase {
            name: "Revoked_Lambda_Signature",
            platform_id: "AWSLambda-SHA384-ECDSA",
            reason: "key-compromise".to_string(),
            job_owner: None,
        },
        // `RevocationReasonString`'s @length minimum of 1.
        RevocationCase {
            name: "Revoked_Shortest_Reason",
            platform_id: "AWSLambda-SHA384-ECDSA",
            reason: "x".to_string(),
            job_owner: None,
        },
        // Its @length maximum of 500.
        RevocationCase {
            name: "Revoked_Longest_Reason",
            platform_id: "AWSLambda-SHA384-ECDSA",
            reason: "r".repeat(500),
            job_owner: None,
        },
        // `jobOwner` naming the account that owns the job.
        RevocationCase {
            name: "Revoked_With_Job_Owner",
            platform_id: "AWSLambda-SHA384-ECDSA",
            reason: "certificate material leaked".to_string(),
            job_owner: Some(ACCOUNT_ID),
        },
        // A reason carrying the whole printable range a client can send.
        RevocationCase {
            name: "Revoked_Verbose_Reason",
            platform_id: "AWSLambda-SHA384-ECDSA",
            reason: "key compromise: cert 'abc' (rotated), see case #7 — 50% re-signed"
                .to_string(),
            job_owner: Some(ACCOUNT_ID),
        },
    ]
}

/// Revoking a signature is a recorded fact: the job carries the request's reason,
/// when it happened and who did it, every later read reports the same record, and
/// a second revoke leaves the first record standing.
#[tokio::test]
async fn revoking_records_the_revocation_and_every_read_reports_it() {
    let router = emulator();

    for case in revocation_cases() {
        let job_id = seed_job(&router, case.name, case.platform_id).await;

        // Before the revoke the job carries no revocation record at all.
        let before = describe_job(&router, REGION, &job_id).await;
        assert_eq!(
            before.status, 200,
            "case {}: expected DescribeSigningJob 200, got {} {:?}",
            case.name, before.status, before.body
        );
        assert!(
            before.body["revocationRecord"].is_null(),
            "case {}: expected no revocationRecord on a job nobody revoked, got {:?}",
            case.name,
            before.body["revocationRecord"]
        );
        let status_before = before.str_member("status");

        let mut request = json!({ "reason": case.reason });
        if let Some(owner) = case.job_owner {
            request["jobOwner"] = json!(owner);
        }
        let revoked = revoke_signature(&router, REGION, &job_id, request).await;
        assert_eq!(
            revoked.status, NO_CONTENT_STATUS,
            "case {}: expected RevokeSignature {NO_CONTENT_STATUS}, got {} {:?}",
            case.name, revoked.status, revoked.body
        );
        assert!(
            revoked.raw_body.is_empty() || revoked.body == json!({}),
            "case {}: expected no members on a Unit output, got {:?}",
            case.name,
            String::from_utf8_lossy(&revoked.raw_body)
        );

        let after = describe_job(&router, REGION, &job_id).await;
        let record = &after.body["revocationRecord"];
        assert_eq!(
            record["reason"],
            json!(case.reason),
            "case {}: expected the record to carry the request's reason, got {:?}",
            case.name,
            record["reason"]
        );
        assert!(
            record["revokedAt"].is_number(),
            "case {}: expected a revokedAt timestamp, got {:?}",
            case.name,
            record["revokedAt"]
        );
        assert_eq!(
            record["revokedBy"],
            expected_revoker(),
            "case {}: expected revokedBy to name the calling identity as an IAM principal ARN, \
             got {:?}",
            case.name,
            record["revokedBy"]
        );

        // The job's status is NOT moved by a revocation: the enum has no revoked
        // value, and the signing itself still ran.
        let status_after = after.str_member("status");
        assert!(
            SIGNING_STATUS_VALUES.contains(&status_after.as_str()),
            "case {}: status {status_after:?} is not one of the modelled \
             {SIGNING_STATUS_VALUES:?}",
            case.name
        );
        assert_eq!(
            status_after, status_before,
            "case {}: expected the revoke to leave the job's status at {status_before:?}, \
             got {status_after:?}",
            case.name
        );
        // And the job keeps what it was started with.
        assert_eq!(
            after.str_member("platformId"),
            case.platform_id,
            "case {}: expected the job to keep its platform, got {:?}",
            case.name,
            after.body["platformId"]
        );
        assert_eq!(
            after.str_member("profileName"),
            case.name,
            "case {}: expected the job to keep its profile, got {:?}",
            case.name,
            after.body["profileName"]
        );

        // Reading does not move anything: a second read reports the same record.
        let again = describe_job(&router, REGION, &job_id).await;
        assert_eq!(
            again.body["revocationRecord"], after.body["revocationRecord"],
            "case {}: expected the revocation record to be stable across reads, got {:?} then \
             {:?}",
            case.name, after.body["revocationRecord"], again.body["revocationRecord"]
        );

        // Revoking again does not restate when, why or by whom trust ended.
        let second = revoke_signature(
            &router,
            REGION,
            &job_id,
            json!({ "reason": "revoked a second time" }),
        )
        .await;
        assert_eq!(
            second.status, NO_CONTENT_STATUS,
            "case {}: expected a repeated RevokeSignature {NO_CONTENT_STATUS}, got {} {:?}",
            case.name, second.status, second.body
        );
        let after_second = describe_job(&router, REGION, &job_id).await;
        assert_eq!(
            after_second.body["revocationRecord"], after.body["revocationRecord"],
            "case {}: expected the first revocation record to stand, got {:?} then {:?}",
            case.name, after.body["revocationRecord"], after_second.body["revocationRecord"]
        );
    }
}

/// A signature is revocable exactly when the platform it was produced on
/// publishes revocation support, and a platform published without it refuses the
/// call with the modelled 400 and writes nothing.
///
/// One row per published platform whose profiles this workflow signs, with the
/// expectation read off the service's own `GetSigningPlatform` answer rather than
/// written out here. A platform published for the container-image workflow has no
/// row: `StartSigningJob` does not serve it (`SignPayload` does), so there is no
/// job of one to revoke — see `start_signing_job_platform_workflow.rs`.
#[tokio::test]
async fn revocation_follows_the_platforms_published_revocation_support() {
    let router = emulator();

    for entry in SIGNING_PLATFORM_CATALOG
        .iter()
        .filter(|entry| entry.workflow == SigningWorkflow::CodeSigningJob)
    {
        let platform_id = entry.platform_id;
        let profile_name = format!("Platform_{}", platform_id.replace('-', "_"));
        let job_id = seed_job(&router, &profile_name, platform_id).await;

        let platform = call(
            &router,
            REGION,
            "GET",
            &format!("/signing-platforms/{platform_id}"),
            None,
        )
        .await;
        assert_eq!(
            platform.status, 200,
            "case {platform_id:?}: expected GetSigningPlatform 200, got {} {:?}",
            platform.status, platform.body
        );
        let supported = platform.body["revocationSupported"]
            .as_bool()
            .unwrap_or_else(|| {
                panic!(
                    "case {platform_id:?}: expected a boolean revocationSupported, got {:?}",
                    platform.body["revocationSupported"]
                )
            });

        let answer = revoke_signature(
            &router,
            REGION,
            &job_id,
            json!({ "reason": "platform capability case" }),
        )
        .await;
        let described = describe_job(&router, REGION, &job_id).await;

        if supported {
            assert_eq!(
                answer.status, NO_CONTENT_STATUS,
                "case {platform_id:?}: a platform publishing revocation support must revoke, got \
                 {} {:?}",
                answer.status, answer.body
            );
            assert!(
                !described.body["revocationRecord"].is_null(),
                "case {platform_id:?}: expected a revocation record on the job, got {:?}",
                described.body["revocationRecord"]
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {platform_id:?}: a platform publishing no revocation support must refuse \
                 with 400, got {} {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.shape().contains("ValidationException"),
                "case {platform_id:?}: expected a ValidationException, got {:?}",
                answer.body
            );
            assert!(
                answer.message().contains(platform_id),
                "case {platform_id:?}: expected the refusal to name the platform, got {:?}",
                answer.body["message"]
            );
            assert!(
                described.body["revocationRecord"].is_null(),
                "case {platform_id:?}: expected the refused revoke to write nothing, got {:?}",
                described.body["revocationRecord"]
            );
        }
    }
}

/// What revoking refuses, and with which status and code — and that a refused
/// revoke writes nothing.
#[tokio::test]
async fn revoking_what_this_endpoint_holds_no_job_for_is_refused() {
    let router = emulator();
    let job_id = seed_job(&router, "Refusal_Profile", "AWSLambda-SHA384-ECDSA").await;

    // (case, region, job id, request body, expected status, expected error shape)
    let cases: Vec<(&str, &str, String, Value, u16, &str)> = vec![
        (
            "a job id nothing was started under",
            REGION,
            "00000000-0000-0000-0000-000000000000".to_string(),
            json!({ "reason": "no such job" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a jobOwner the job does not have",
            REGION,
            job_id.clone(),
            json!({ "reason": "wrong owner", "jobOwner": OTHER_ACCOUNT_ID }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "another region's endpoint",
            OTHER_REGION,
            job_id.clone(),
            json!({ "reason": "wrong region" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a reason below the modelled minimum length",
            REGION,
            job_id.clone(),
            json!({ "reason": "" }),
            400,
            "ValidationException",
        ),
        (
            "a reason above the modelled maximum length",
            REGION,
            job_id.clone(),
            json!({ "reason": "r".repeat(501) }),
            400,
            "ValidationException",
        ),
        (
            "a jobOwner outside the modelled pattern",
            REGION,
            job_id.clone(),
            json!({ "reason": "bad owner", "jobOwner": "not-an-account" }),
            400,
            "ValidationException",
        ),
    ];

    for (case, region, id, request, status, shape) in cases {
        let answer = revoke_signature(&router, region, &id, request).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains(shape),
            "case {case:?}: expected the error shape {shape}, got {:?}",
            answer.body
        );
        if status == 404 {
            // A client branches on the code: a missing JOB is its own condition.
            assert_eq!(
                answer.code(),
                "JobNotFound",
                "case {case:?}: expected the job not-found code, got {:?}",
                answer.body["code"]
            );
        }
    }

    // And the job every refusal named is untouched.
    let described = describe_job(&router, REGION, &job_id).await;
    assert!(
        described.body["revocationRecord"].is_null(),
        "expected the refused revokes to have written no revocation record, got {:?}",
        described.body["revocationRecord"]
    );
}

/// A revocation recorded while the signing is still running survives the
/// completion the clock owes that job: the job reports both.
#[tokio::test]
async fn a_revocation_survives_the_jobs_completion() {
    let router = emulator();
    let job_id = seed_job(&router, "In_Progress_Profile", "AWSLambda-SHA384-ECDSA").await;

    let running = describe_job(&router, REGION, &job_id).await;
    assert_eq!(
        running.str_member("status"),
        "InProgress",
        "expected a just-started job to be InProgress, got {:?}",
        running.body["status"]
    );

    let revoked = revoke_signature(
        &router,
        REGION,
        &job_id,
        json!({ "reason": "revoked while still signing" }),
    )
    .await;
    assert_eq!(
        revoked.status, NO_CONTENT_STATUS,
        "expected RevokeSignature {NO_CONTENT_STATUS} on an InProgress job, got {} {:?}",
        revoked.status, revoked.body
    );

    let while_running = describe_job(&router, REGION, &job_id).await;
    let record = while_running.body["revocationRecord"].clone();
    assert_eq!(
        record["reason"],
        json!("revoked while still signing"),
        "expected the record to carry the request's reason, got {record:?}"
    );

    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;

    let done = describe_job(&router, REGION, &job_id).await;
    assert_eq!(
        done.str_member("status"),
        "Succeeded",
        "expected the job to have Succeeded once due, got {:?}",
        done.body["status"]
    );
    assert!(
        done.body["completedAt"].is_number(),
        "expected a completedAt on a Succeeded job, got {:?}",
        done.body["completedAt"]
    );
    assert_eq!(
        done.body["revocationRecord"], record,
        "expected the completion to leave the revocation record alone, got {:?} then {:?}",
        record, done.body["revocationRecord"]
    );
}

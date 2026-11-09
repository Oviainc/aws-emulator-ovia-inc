// SPDX-License-Identifier: CC0-1.0

//! Which signing profiles `StartSigningJob` can start a job for, end to end over
//! HTTP.
//!
//! Every case goes through the router, so it exercises routing, deserialization,
//! the handler, the persisted records and serialization — the path a client takes.
//! The rule under test:
//!
//! * AWS Signer runs two signing workflows, and a signing platform belongs to one
//!   of them: the S3 code-signing-job workflow `StartSigningJob` serves, and the
//!   payload workflow for container images that `SignPayload` serves. A signing
//!   operation is available only for profiles whose platform signs through its
//!   workflow, so `StartSigningJob` against a profile on a container-image
//!   platform is refused with the operation's modelled 400 `ValidationException`
//!   and starts nothing, while a profile on a code-signing platform keeps
//!   starting jobs.
//!
//! The cases are tables: a row per PUBLISHED platform, and — for the refused
//! workflow — a row per request shape the refusal must not depend on (which
//! optional members the source, destination and request carry).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::{service_state, SIGNING_PLATFORM_CATALOG};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for: what routes it to signer and
/// which region's profiles and jobs it addresses.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handlers fails.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN for `signingMaterial`, carried by every put below so a
/// profile can be created on each published platform, including the one whose
/// profile must hold the certificate its signatures are issued under.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The `partner` of the platforms AWS Signer publishes for CONTAINER-IMAGE
/// signing: the Notation (OCI) platforms, whose profiles are signed by handing the
/// payload digest to `SignPayload` rather than by signing an object in S3.
///
/// Source: `AddProfilePermissionRequest$action`'s documentation, which states
/// `signer:StartSigningJob` is not supported for container image workflows and
/// `signer:SignPayload` is not supported for AWS Lambda workflows
/// (`smithy_query(query="doc", shape="AddProfilePermissionRequest",
/// member="action")`). Derived HERE from the platform's partner rather than read
/// off the catalog entry the handler consults, so a handler that classified a
/// platform wrongly fails this suite.
const CONTAINER_IMAGE_PARTNER: &str = "Notation";

/// Whether `platform_id` names a container-image platform — the ones
/// [`CONTAINER_IMAGE_PARTNER`] owns, whose profiles `StartSigningJob` must refuse.
fn is_container_image_platform(platform_id: &str) -> bool {
    platform_id.starts_with(&format!("{CONTAINER_IMAGE_PARTNER}-"))
}

/// A SigV4 credential scope naming this service for [`REGION`].
fn signed_for_region() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine —
/// handed back beside the router so a case can read the stored jobs directly,
/// which is what says whether a refused start wrote anything at all.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `code` member, or the empty string when there is none.
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

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", signed_for_region());
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
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
    Answer { status, body }
}

async fn put_profile(router: &MultiServiceRouter, name: &str, platform_id: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({
            "platformId": platform_id,
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await
}

async fn start_job(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/signing-jobs", Some(request)).await
}

async fn describe_job(router: &MultiServiceRouter, job_id: &str) -> Answer {
    call(router, "GET", &format!("/signing-jobs/{job_id}"), None).await
}

/// The profile name of every signing job the store holds.
fn stored_job_profiles(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the signer store is reachable from the engine");
    let store = state.lock();
    store
        .jobs
        .values()
        .map(|job| job.profile_name.clone())
        .collect()
}

/// A profile name for `platform_id` satisfying `ProfileName`'s
/// `^[a-zA-Z0-9_]{2,}$`.
fn profile_name_for(platform_id: &str) -> String {
    format!("Workflow_{}", platform_id.replace('-', "_"))
}

/// A `StartSigningJob` body naming `profile_name`, in the smallest form the
/// operation admits.
fn job_request(profile_name: &str, token: &str) -> Value {
    json!({
        "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
        "destination": { "s3": { "bucketName": "dst" } },
        "profileName": profile_name,
        "clientRequestToken": token,
    })
}

/// `StartSigningJob` serves the S3 workflow only: one row per published platform,
/// with the expectation derived from the platform's own partner rather than from
/// the handler's catalog, and the job store checked after each answer.
#[tokio::test]
async fn start_signing_job_follows_the_platforms_signing_workflow() {
    let (router, engine) = emulator();
    let mut refused = 0;
    let mut started = 0;

    for entry in SIGNING_PLATFORM_CATALOG {
        let platform_id = entry.platform_id;
        let case = profile_name_for(platform_id);
        let put = put_profile(&router, &case, platform_id).await;
        assert_eq!(
            put.status, 200,
            "case {case}: seeding the profile must succeed, got {} {:?}",
            put.status, put.body
        );

        let answer = start_job(&router, job_request(&case, &format!("tok-{case}"))).await;

        if is_container_image_platform(platform_id) {
            refused += 1;
            assert_eq!(
                answer.status, 400,
                "case {case}: a container-image platform must refuse a signing job with 400, got \
                 {} {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.shape().contains("ValidationException"),
                "case {case}: expected a ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.code(),
                "ValidationException",
                "case {case}: expected the modelled validation code, got {:?}",
                answer.body["code"]
            );
            assert_eq!(
                answer.message(),
                format!("StartSigningJob is not supported for platform {platform_id}"),
                "case {case}: expected the refusal to name the operation and the platform, got \
                 {:?}",
                answer.body["message"]
            );
            assert!(
                answer.body["jobId"].is_null(),
                "case {case}: expected no jobId on a refused start, got {:?}",
                answer.body["jobId"]
            );
            let stored = stored_job_profiles(&engine);
            assert!(
                !stored.contains(&case),
                "case {case}: expected the refused start to store no job, got {stored:?}"
            );

            // The refusal is about the platform, so it does not become a success
            // on a replay of the same `clientRequestToken`: nothing was recorded
            // under it.
            let replayed = start_job(&router, job_request(&case, &format!("tok-{case}"))).await;
            assert_eq!(
                replayed.status, 400,
                "case {case}: expected the same token to be refused again, got {} {:?}",
                replayed.status, replayed.body
            );
            assert_eq!(
                replayed.message(),
                answer.message(),
                "case {case}: expected the replay to be refused identically, got {:?}",
                replayed.body["message"]
            );
        } else {
            started += 1;
            assert_eq!(
                answer.status, 200,
                "case {case}: a code-signing platform must start a job, got {} {:?}",
                answer.status, answer.body
            );
            let job_id = answer.str_member("jobId");
            assert!(
                !job_id.is_empty(),
                "case {case}: expected a jobId, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str_member("jobOwner"),
                ACCOUNT_ID,
                "case {case}: expected jobOwner {ACCOUNT_ID}, got {:?}",
                answer.body["jobOwner"]
            );

            // The started job is readable, and carries the platform it started on.
            let described = describe_job(&router, &job_id).await;
            assert_eq!(
                described.status, 200,
                "case {case}: expected DescribeSigningJob 200, got {} {:?}",
                described.status, described.body
            );
            assert_eq!(
                described.str_member("platformId"),
                platform_id,
                "case {case}: expected the job to carry its profile's platform, got {:?}",
                described.body["platformId"]
            );
            let stored = stored_job_profiles(&engine);
            assert!(
                stored.contains(&case),
                "case {case}: expected the started job to be stored, got {stored:?}"
            );
        }
    }

    // The table is only evidence of the rule if it holds rows on both sides of it.
    assert!(
        refused > 0 && started > 0,
        "expected the published catalog to hold both container-image and code-signing \
         platforms, got {refused} refused and {started} started"
    );
}

/// What the refusal does NOT depend on: the shape of the request. Every row names
/// a profile on a container-image platform and differs in which optional members
/// the source, the destination and the request carry; all of them are refused the
/// same way, and none of them starts a job.
#[tokio::test]
async fn the_container_image_refusal_does_not_depend_on_the_request_shape() {
    let (router, engine) = emulator();
    let platform_id = SIGNING_PLATFORM_CATALOG
        .iter()
        .map(|entry| entry.platform_id)
        .find(|id| is_container_image_platform(id))
        .expect("the published catalog holds a container-image platform");
    let profile = profile_name_for(platform_id);
    let put = put_profile(&router, &profile, platform_id).await;
    assert_eq!(
        put.status, 200,
        "seeding the profile: got {} {:?}",
        put.status, put.body
    );

    // (case, request body)
    let cases: Vec<(&str, Value)> = vec![
        (
            "a destination prefix",
            json!({
                "source": { "s3": { "bucketName": "src", "key": "code.zip", "version": "1" } },
                "destination": { "s3": { "bucketName": "dst", "prefix": "signed/" } },
                "profileName": profile,
                "clientRequestToken": "tok-prefix",
            }),
        ),
        (
            "the profile owner named",
            json!({
                "source": { "s3": { "bucketName": "src", "key": "nested/code.zip", "version": "2" } },
                "destination": { "s3": { "bucketName": "dst" } },
                "profileName": profile,
                "profileOwner": ACCOUNT_ID,
                "clientRequestToken": "tok-owner",
            }),
        ),
        (
            "a token at ClientRequestToken's shortest form",
            json!({
                "source": { "s3": { "bucketName": "other-src", "key": "a", "version": "9" } },
                "destination": { "s3": { "bucketName": "other-dst", "prefix": "" } },
                "profileName": profile,
                "clientRequestToken": "t",
            }),
        ),
        (
            "an idempotency token shaped like a uuid",
            json!({
                "source": { "s3": { "bucketName": "src", "key": "code.zip", "version": "1" } },
                "destination": { "s3": { "bucketName": "dst", "prefix": "a/b/c/" } },
                "profileName": profile,
                "clientRequestToken": "11111111-2222-3333-4444-555555555555",
            }),
        ),
    ];

    for (case, request) in cases {
        let answer = start_job(&router, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 whatever the request shape, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.message(),
            format!("StartSigningJob is not supported for platform {platform_id}"),
            "case {case:?}: expected the platform refusal, got {:?}",
            answer.body["message"]
        );
    }

    let stored = stored_job_profiles(&engine);
    assert!(
        stored.is_empty(),
        "expected no job to have been stored at all, got {stored:?}"
    );
}

/// The refusal is a judgement about the profile's PLATFORM, not about the profile
/// name: with one profile on a container-image platform and another on a
/// code-signing platform, the very same request is refused for the first and
/// served for the second — and the refusal names the platform it was refused for.
#[tokio::test]
async fn the_refusal_is_about_the_platform_and_not_the_profile_name() {
    let (router, _engine) = emulator();
    let container = SIGNING_PLATFORM_CATALOG
        .iter()
        .map(|entry| entry.platform_id)
        .find(|id| is_container_image_platform(id))
        .expect("the published catalog holds a container-image platform");
    let code_signing = SIGNING_PLATFORM_CATALOG
        .iter()
        .map(|entry| entry.platform_id)
        .find(|id| !is_container_image_platform(id))
        .expect("the published catalog holds a code-signing platform");

    let on_container = "Workflow_Container_Profile";
    assert_eq!(
        put_profile(&router, on_container, container).await.status,
        200,
        "seeding the container-image profile"
    );
    let refused = start_job(&router, job_request(on_container, "tok-container")).await;
    assert_eq!(
        refused.status, 400,
        "expected the container-image platform to refuse, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.message(),
        format!("StartSigningJob is not supported for platform {container}"),
        "expected the refusal to name the platform it was refused for, got {:?}",
        refused.body["message"]
    );

    let on_code_signing = "Workflow_Signing_Profile";
    assert_eq!(
        put_profile(&router, on_code_signing, code_signing).await.status,
        200,
        "seeding the profile on {code_signing}"
    );
    let accepted = start_job(&router, job_request(on_code_signing, "tok-code-signing")).await;
    assert_eq!(
        accepted.status, 200,
        "expected the profile on {code_signing} to start a job, got {} {:?}",
        accepted.status, accepted.body
    );
    let job_id = accepted.str_member("jobId");
    assert_eq!(
        describe_job(&router, &job_id).await.str_member("platformId"),
        code_signing,
        "expected the job to carry the platform it was started on"
    );

    // And a second refusal for the container-image profile starts nothing and
    // removes nothing: the job started on the other platform still exists.
    let refused_again = start_job(&router, job_request(on_container, "tok-container-again")).await;
    assert_eq!(
        refused_again.status, 400,
        "expected the container-image profile to refuse again, got {} {:?}",
        refused_again.status, refused_again.body
    );
    assert_eq!(
        describe_job(&router, &job_id).await.status,
        200,
        "expected the already-started job to survive"
    );
}

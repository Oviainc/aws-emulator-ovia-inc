// SPDX-License-Identifier: CC0-1.0

//! What `SignPayload` signs, and which signing profiles it refuses, end to end
//! over HTTP.
//!
//! Every case goes through the router, so it exercises routing, deserialization,
//! the handler, the persisted records and serialization — the path a client takes.
//! The rules under test:
//!
//! * A signing profile signs only while its lifecycle status is `Active`. A
//!   profile moved out of `Active` — `CancelSigningProfile` moves it to `Canceled`,
//!   `RevokeSigningProfile` to `Revoked` — refuses a payload signature with the
//!   operation's modelled 400 `ValidationException`, whose message names the
//!   profile the request named and the status the store holds for it, and the
//!   refused call signs nothing, records no job and leaves the status where it was.
//! * An `Active` profile whose platform is published for the payload workflow signs
//!   the payload the request carries: the answer carries a signature of exactly
//!   those bytes, verifiable under the public half of the key pair of the profile
//!   version that made it, and the signing job that made it is reported by
//!   `DescribeSigningJob`. A profile on a platform published for the S3
//!   signing-job workflow is refused instead.
//!
//! The cases are tables: a row per status a profile reaches out of `Active` and per
//! route that moves it there, a row per payload across the modelled payload window,
//! a row per PUBLISHED signing platform, and a row per `payloadFormat` the
//! operation does not sign.

use std::sync::Arc;

use aws_lc_rs::signature::{ParsedPublicKey, ECDSA_P384_SHA384_ASN1};
use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::{
    service_state, MAX_RETAINED_JOBS, SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for: what routes it to signer and
/// which region's profiles and jobs it addresses.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handlers fails.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN for `signingMaterial`, carried by every put below so a
/// profile can be created on each published platform, including the ones whose
/// profile must hold the certificate its signatures are issued under.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The one `payloadFormat` this operation signs, written out here rather than
/// imported so the exact bytes a client has to send are pinned independently of the
/// service's own constant.
///
/// Source: `SignPayloadRequest$payloadFormat`, documented "Payload content type.
/// The single valid type is application/vnd.cncf.notary.payload.v1+json."
/// (`smithy_query(query="doc", shape="SignPayloadRequest",
/// member="payloadFormat")`).
const PAYLOAD_FORMAT: &str = "application/vnd.cncf.notary.payload.v1+json";

/// The `partner` of the platforms AWS Signer publishes for CONTAINER-IMAGE
/// signing: the Notation (OCI) platforms, whose profiles are signed by handing the
/// payload over to `SignPayload`.
///
/// Source: `AddProfilePermissionRequest$action`'s documentation, which states
/// `signer:SignPayload` is not supported for AWS Lambda workflows and
/// `signer:StartSigningJob` is not supported for container image workflows
/// (`smithy_query(query="doc", shape="AddProfilePermissionRequest",
/// member="action")`). Derived HERE from the platform's id rather than read off the
/// catalog entry the handler consults, so a handler that classified a platform
/// wrongly fails this suite.
const PAYLOAD_WORKFLOW_PARTNER: &str = "Notation";

/// The modelled error shape (and the `x-amzn-errortype` restJson1 resolves it
/// from) every refusal below is answered with.
const ERROR_SHAPE_VALIDATION: &str = "ValidationException";

/// The modelled error shape of a 404 below.
const ERROR_SHAPE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The status a profile `CancelSigningProfile` has moved reports.
const STATUS_CANCELED: &str = "Canceled";

/// The status a profile `RevokeSigningProfile` has moved reports.
const STATUS_REVOKED: &str = "Revoked";

/// Whether `platform_id` names a platform of the payload workflow.
fn is_payload_platform(platform_id: &str) -> bool {
    platform_id.starts_with(&format!("{PAYLOAD_WORKFLOW_PARTNER}-"))
}

/// A published platform of the payload workflow — the one `SignPayload` serves.
fn payload_platform() -> &'static str {
    SIGNING_PLATFORM_CATALOG
        .iter()
        .map(|entry| entry.platform_id)
        .find(|id| is_payload_platform(id))
        .expect("the published catalog holds a payload-workflow platform")
}

/// A SigV4 credential scope naming this service for [`REGION`].
fn signed_for_region() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine —
/// handed back beside the router so a case can read the stored jobs and the key
/// material directly.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// What the emulator answered: the HTTP status, the error shape restJson1 resolves
/// from `x-amzn-errortype`, and the JSON body.
struct Answer {
    status: u16,
    error_type: String,
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

    /// A string member of the answer, or the empty string when it carries none.
    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The `signature` blob the answer carries, decoded.
    fn signature(&self, case: &str) -> Vec<u8> {
        let encoded = self.body["signature"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a signature, got {:?}", self.body));
        base64::decode(encoded)
            .unwrap_or_else(|error| panic!("case {case:?}: signature is not base64: {error}"))
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
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
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
    Answer {
        status,
        error_type,
        body,
    }
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

async fn get_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/signing-profiles/{name}"), None).await
}

async fn cancel_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "DELETE", &format!("/signing-profiles/{name}"), None).await
}

async fn revoke_profile(router: &MultiServiceRouter, name: &str, version: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": version,
            "reason": "the signing key was rotated",
            "effectiveTime": 1_700_000_000,
        })),
    )
    .await
}

/// A `SignPayload` request naming `profile_name`, carrying `payload` under
/// `payload_format`.
fn sign_request(profile_name: &str, payload: &[u8], payload_format: &str) -> Value {
    json!({
        "profileName": profile_name,
        "payload": base64::encode(payload),
        "payloadFormat": payload_format,
    })
}

async fn sign_payload(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/signing-jobs/with-payload", Some(request)).await
}

async fn describe_job(router: &MultiServiceRouter, job_id: &str) -> Answer {
    call(router, "GET", &format!("/signing-jobs/{job_id}"), None).await
}

/// Every signing job the store holds, by id — what says whether a refused call
/// recorded anything at all.
fn stored_job_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the signer store is reachable from the engine");
    let store = state.lock();
    store.jobs.keys().cloned().collect()
}

/// The PUBLIC half of the key pair the profile `name` holds for its payload
/// signatures, as the DER `SubjectPublicKeyInfo` a verifier parses.
fn signing_public_key(engine: &engine::InMemoryEngine, case: &str, name: &str) -> Vec<u8> {
    let state = service_state(engine).expect("the signer store is reachable from the engine");
    let store = state.lock();
    store
        .profile(REGION, name)
        .and_then(|profile| profile.payload_signing_key.as_ref())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the profile {name} to hold a payload signing key")
        })
        .public_key_info()
        .to_vec()
}

/// Whether `signature` is a signature of `payload` under the public half
/// `public_key` of a key pair.
///
/// The scheme is the one the payload-workflow platform publishes: ECDSA over the
/// NIST curve the published digest is paired with — `SHA384` on that platform's
/// `hashAlgorithmOptions`, so P-384 with SHA-384, ASN.1-encoded — stated here
/// rather than taken from the service, so a signature made under another scheme
/// fails this suite.
fn verifies(public_key: &[u8], payload: &[u8], signature: &[u8]) -> bool {
    ParsedPublicKey::new(&ECDSA_P384_SHA384_ASN1, public_key)
        .is_ok_and(|parsed| parsed.verify_sig(payload, signature).is_ok())
}

/// A payload of `len` bytes that is neither constant nor the payload of any other
/// length, so no signature can be right by accident of its input.
fn payload_of(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 37 + len) % 251).unwrap_or_default())
        .collect()
}

/// A profile name for `label` satisfying `ProfileName`'s `^[a-zA-Z0-9_]{2,}$`.
fn profile_name_for(label: &str) -> String {
    format!(
        "Payload_{}",
        label.replace([' ', '-', ',', '.'], "_").replace("__", "_")
    )
}

/// What moves a profile out of `Active`, and the status it reports afterwards.
enum OutOfActive {
    /// `CancelSigningProfile`.
    Cancel,
    /// `RevokeSigningProfile`.
    Revoke,
    /// `RevokeSigningProfile`, and then another `PutSigningProfile` under the same
    /// name: a revocation is carried forward, so the name is still `Revoked`.
    RevokeThenPut,
}

/// A profile out of `Active` signs no payload: the modelled 400 names the profile
/// and the status the store holds, and the refused call writes nothing.
///
/// Each row differs in which route moved the profile, which status that leaves it
/// in, and which payload the refused request carried; every row first signs
/// successfully while the profile is `Active`, so the refusal is evidence about the
/// status and not about the profile or the request.
#[tokio::test]
async fn a_profile_out_of_active_signs_no_payload() {
    let (router, engine) = emulator();
    let platform_id = payload_platform();

    // (case, what moves it out of Active, the status that leaves it in, payload)
    let cases: Vec<(&str, OutOfActive, &str, Vec<u8>)> = vec![
        (
            "canceled by CancelSigningProfile",
            OutOfActive::Cancel,
            STATUS_CANCELED,
            payload_of(64),
        ),
        (
            "revoked by RevokeSigningProfile",
            OutOfActive::Revoke,
            STATUS_REVOKED,
            payload_of(1),
        ),
        (
            "revoked and then written again under the same name",
            OutOfActive::RevokeThenPut,
            STATUS_REVOKED,
            payload_of(4096),
        ),
    ];

    for (case, mover, status, payload) in cases {
        let name = profile_name_for(case);
        let put = put_profile(&router, &name, platform_id).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: seeding the profile must succeed, got {} {:?}",
            put.status, put.body
        );
        let version = put.str_member("profileVersion");

        // The positive control: while the profile is Active the very same request
        // is signed.
        let signed = sign_payload(&router, sign_request(&name, &payload, PAYLOAD_FORMAT)).await;
        assert_eq!(
            signed.status, 200,
            "case {case:?}: an Active profile must sign, got {} {:?}",
            signed.status, signed.body
        );

        match mover {
            OutOfActive::Cancel => {
                // `CancelSigningProfile` models no output, so its success is the
                // no-content status.
                let moved = cancel_profile(&router, &name).await;
                assert_eq!(
                    moved.status, 204,
                    "case {case:?}: the cancel must succeed, got {} {:?}",
                    moved.status, moved.body
                );
            }
            OutOfActive::Revoke | OutOfActive::RevokeThenPut => {
                // `RevokeSigningProfile` models no output either.
                let moved = revoke_profile(&router, &name, &version).await;
                assert_eq!(
                    moved.status, 204,
                    "case {case:?}: the revoke must succeed, got {} {:?}",
                    moved.status, moved.body
                );
            }
        }
        if matches!(mover, OutOfActive::RevokeThenPut) {
            let reput = put_profile(&router, &name, platform_id).await;
            assert_eq!(
                reput.status, 200,
                "case {case:?}: re-putting the profile must succeed, got {} {:?}",
                reput.status, reput.body
            );
        }

        let reported = get_profile(&router, &name).await;
        assert_eq!(
            reported.str_member("status"),
            status,
            "case {case:?}: expected the profile to report {status}, got {:?}",
            reported.body["status"]
        );

        let jobs_before = stored_job_ids(&engine);
        let refused = sign_payload(&router, sign_request(&name, &payload, PAYLOAD_FORMAT)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, ERROR_SHAPE_VALIDATION,
            "case {case:?}: expected x-amzn-errortype {ERROR_SHAPE_VALIDATION}, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.shape(),
            ERROR_SHAPE_VALIDATION,
            "case {case:?}: expected a {ERROR_SHAPE_VALIDATION}, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.code(),
            ERROR_SHAPE_VALIDATION,
            "case {case:?}: expected the modelled code, got {:?}",
            refused.body["code"]
        );
        assert_eq!(
            refused.message(),
            format!("Signing profile with name: {name} is in state {status}"),
            "case {case:?}: expected the refusal to name the profile and its state, got {:?}",
            refused.body["message"]
        );
        assert!(
            refused.body["signature"].is_null() && refused.body["jobId"].is_null(),
            "case {case:?}: expected no signature and no jobId on a refused call, got {:?}",
            refused.body
        );
        assert_eq!(
            stored_job_ids(&engine),
            jobs_before,
            "case {case:?}: expected the refused call to record no job"
        );
        // The refusal is a read of the status, so it does not move it.
        assert_eq!(
            get_profile(&router, &name).await.str_member("status"),
            status,
            "case {case:?}: expected the refused call to leave the status at {status}"
        );
    }
}

/// An `Active` profile signs the payload it is handed: the answer carries a
/// signature of exactly those bytes, and the job that made it is reported.
///
/// A row per payload across the modelled window (`Payload` is `@length(1, 4096)`,
/// `smithy_query(query="operation", operation="SignPayload")`): each published
/// bound, a middling size, and one whose bytes are not text.
#[tokio::test]
async fn an_active_profile_signs_the_payload_it_is_handed() {
    let (router, engine) = emulator();
    let platform_id = payload_platform();
    let name = "Payload_Signing_Profile";
    let put = put_profile(&router, name, platform_id).await;
    assert_eq!(
        put.status, 200,
        "seeding the profile: got {} {:?}",
        put.status, put.body
    );
    let version = put.str_member("profileVersion");
    let version_arn = put.str_member("profileVersionArn");

    // (case, payload)
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("one byte, the modelled floor", payload_of(1)),
        ("a middling payload", payload_of(512)),
        ("4096 bytes, the modelled ceiling", payload_of(4096)),
        ("bytes that are not text", vec![0x00, 0xff, 0x80, 0x0a, 0x7f]),
    ];

    let mut signatures: Vec<Vec<u8>> = Vec::new();
    for (case, payload) in cases {
        let answer = sign_payload(&router, sign_request(name, &payload, PAYLOAD_FORMAT)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
        let job_id = answer.str_member("jobId");
        assert!(
            !job_id.is_empty(),
            "case {case:?}: expected a jobId, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.str_member("jobOwner"),
            ACCOUNT_ID,
            "case {case:?}: expected jobOwner {ACCOUNT_ID}, got {:?}",
            answer.body["jobOwner"]
        );
        assert_eq!(
            answer.body["metadata"]["jobId"].as_str().unwrap_or_default(),
            job_id,
            "case {case:?}: expected the metadata to name the signing job, got {:?}",
            answer.body["metadata"]
        );
        assert_eq!(
            answer.body["metadata"]["profileVersionArn"]
                .as_str()
                .unwrap_or_default(),
            version_arn,
            "case {case:?}: expected the metadata to name the profile version, got {:?}",
            answer.body["metadata"]
        );

        // The whole claim about the signature: it verifies over the payload the
        // request carried, under the public half of the key the profile version
        // signs with — and over nothing else.
        let signature = answer.signature(case);
        let public_key = signing_public_key(&engine, case, name);
        assert!(
            verifies(&public_key, &payload, &signature),
            "case {case:?}: expected the signature to verify over the payload, got {} bytes of \
             signature",
            signature.len()
        );
        let mut other = payload.clone();
        other.push(0x01);
        assert!(
            !verifies(&public_key, &other, &signature),
            "case {case:?}: expected the signature not to verify over other bytes"
        );
        assert!(
            !signature.is_empty() && signature != payload,
            "case {case:?}: expected a signature of the payload rather than the payload itself, \
             got {} bytes",
            signature.len()
        );
        signatures.push(signature);

        // The job that made the signature is reported, and carries what it signed
        // with.
        let described = describe_job(&router, &job_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeSigningJob 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_member("status"),
            "Succeeded",
            "case {case:?}: expected the job to report Succeeded, got {:?}",
            described.body["status"]
        );
        assert_eq!(
            described.str_member("platformId"),
            platform_id,
            "case {case:?}: expected the job to carry its profile's platform, got {:?}",
            described.body["platformId"]
        );
        assert_eq!(
            described.str_member("profileVersion"),
            version,
            "case {case:?}: expected the job to carry the version that signed, got {:?}",
            described.body["profileVersion"]
        );
        // Nothing was read from or written to S3, so the job names neither.
        assert!(
            described.body["source"].is_null() && described.body["signedObject"].is_null(),
            "case {case:?}: expected a payload-signed job to name no S3 object, got {:?}",
            described.body
        );
    }

    // Different payloads are different signatures: no row's answer is a constant.
    for (left, right) in (1..signatures.len()).map(|index| (&signatures[0], &signatures[index])) {
        assert_ne!(
            left, right,
            "expected each payload to be signed on its own, got one signature twice"
        );
    }
}

/// `SignPayload` serves the payload workflow only: one row per published platform,
/// with the expectation derived from the platform's own id rather than from the
/// handler's catalog, and the job store checked after each answer.
#[tokio::test]
async fn sign_payload_follows_the_platforms_signing_workflow() {
    let (router, engine) = emulator();
    let mut refused = 0;
    let mut signed = 0;

    for entry in SIGNING_PLATFORM_CATALOG {
        let platform_id = entry.platform_id;
        let case = format!("platform {platform_id}");
        let name = profile_name_for(platform_id);
        let put = put_profile(&router, &name, platform_id).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: seeding the profile must succeed, got {} {:?}",
            put.status, put.body
        );

        let payload = payload_of(32);
        let jobs_before = stored_job_ids(&engine);
        let answer = sign_payload(&router, sign_request(&name, &payload, PAYLOAD_FORMAT)).await;

        if is_payload_platform(platform_id) {
            signed += 1;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a payload-workflow platform must sign, got {} {:?}",
                answer.status, answer.body
            );
        } else {
            refused += 1;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a code-signing platform must refuse, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                ERROR_SHAPE_VALIDATION,
                "case {case:?}: expected the modelled code, got {:?}",
                answer.body["code"]
            );
            assert_eq!(
                answer.message(),
                format!("SignPayload is not supported for platform {platform_id}"),
                "case {case:?}: expected the refusal to name the operation and the platform, got \
                 {:?}",
                answer.body["message"]
            );
            assert_eq!(
                stored_job_ids(&engine),
                jobs_before,
                "case {case:?}: expected the refused call to record no job"
            );
        }
    }

    // The table is only evidence of the rule if it holds rows on both sides of it.
    assert!(
        refused > 0 && signed > 0,
        "expected the published catalog to hold both payload-workflow and code-signing platforms, \
         got {refused} refused and {signed} signed"
    );
}

/// The payload is signed only in the one content type the operation signs: a row
/// per `payloadFormat` the operation does not sign, each refused with the modelled
/// 400 that names the value it carried and the value it has to carry, and each
/// recording nothing.
#[tokio::test]
async fn a_payload_format_the_operation_does_not_sign_is_refused() {
    let (router, engine) = emulator();
    let name = "Payload_Format_Profile";
    let put = put_profile(&router, name, payload_platform()).await;
    assert_eq!(
        put.status, 200,
        "seeding the profile: got {} {:?}",
        put.status, put.body
    );

    // Rows around the one admitted value: another media type, the same value
    // differently cased, the same value with trailing space, a prefix of it, and
    // the empty string. `payloadFormat` is a bare `String` the model puts no
    // constraint on, so every one of these reaches the handler.
    let cases = [
        "application/octet-stream",
        "APPLICATION/VND.CNCF.NOTARY.PAYLOAD.V1+JSON",
        "application/vnd.cncf.notary.payload.v1+json ",
        "application/vnd.cncf.notary.payload.v1",
        "",
    ];

    let jobs_before = stored_job_ids(&engine);
    for case in cases {
        let answer = sign_payload(&router, sign_request(name, &payload_of(16), case)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, ERROR_SHAPE_VALIDATION,
            "case {case:?}: expected x-amzn-errortype {ERROR_SHAPE_VALIDATION}, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.message(),
            format!(
                "payloadFormat {case} is not supported. Supported values: [{PAYLOAD_FORMAT}]"
            ),
            "case {case:?}: expected the refusal to state the value and the rule, got {:?}",
            answer.body["message"]
        );
        assert!(
            answer.body["signature"].is_null(),
            "case {case:?}: expected no signature, got {:?}",
            answer.body
        );
    }
    assert_eq!(
        stored_job_ids(&engine),
        jobs_before,
        "expected no refused format to record a job"
    );

    // The admitted value signs, so the table above is about the format and not
    // about the profile or the payload.
    let signed = sign_payload(&router, sign_request(name, &payload_of(16), PAYLOAD_FORMAT)).await;
    assert_eq!(
        signed.status, 200,
        "expected the one admitted payload format to sign, got {} {:?}",
        signed.status, signed.body
    );
}

/// A payload signature is refused for a profile this region's endpoint holds none
/// of, and for one another account is named as owning: the modelled 404, whose
/// `code` names the missing profile so a client can tell it from every other 404.
#[tokio::test]
async fn a_profile_that_resolves_to_nothing_signs_no_payload() {
    let (router, engine) = emulator();
    let held = "Payload_Held_Profile";
    assert_eq!(
        put_profile(&router, held, payload_platform()).await.status,
        200,
        "seeding the profile"
    );

    // (case, request)
    let cases: Vec<(&str, Value)> = vec![
        (
            "a profile name nothing was created under",
            sign_request("Payload_Absent_Profile", &payload_of(8), PAYLOAD_FORMAT),
        ),
        (
            "a held profile another account is named as owning",
            json!({
                "profileName": held,
                "profileOwner": "210987654321",
                "payload": base64::encode(payload_of(8).as_slice()),
                "payloadFormat": PAYLOAD_FORMAT,
            }),
        ),
    ];

    let jobs_before = stored_job_ids(&engine);
    for (case, request) in cases {
        let answer = sign_payload(&router, request).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, ERROR_SHAPE_NOT_FOUND,
            "case {case:?}: expected x-amzn-errortype {ERROR_SHAPE_NOT_FOUND}, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.code(),
            "ProfileNotFound",
            "case {case:?}: expected the profile-specific code, got {:?}",
            answer.body["code"]
        );
    }
    assert_eq!(
        stored_job_ids(&engine),
        jobs_before,
        "expected no unresolved profile to record a job"
    );
}

/// The job store this signing call writes to is BOUNDED: it keeps at most
/// [`MAX_RETAINED_JOBS`] jobs, dropping the one started longest ago, so the memory
/// one process gives to signing jobs cannot grow without end.
///
/// The bound is this emulator's, not an AWS rule, so what is asserted is the store
/// itself: a real signed job is put in through the operation, then the store is
/// filled past the bound through the one writer both signing operations insert
/// through.
#[tokio::test]
async fn the_job_store_keeps_at_most_the_jobs_it_retains() {
    let (router, engine) = emulator();
    let name = "Payload_Retention_Profile";
    assert_eq!(
        put_profile(&router, name, payload_platform()).await.status,
        200,
        "seeding the profile"
    );
    let signed = sign_payload(&router, sign_request(name, &payload_of(24), PAYLOAD_FORMAT)).await;
    assert_eq!(
        signed.status, 200,
        "expected the signature to be served, got {} {:?}",
        signed.status, signed.body
    );
    let first = signed.str_member("jobId");

    let state = service_state(&*engine).expect("the signer store is reachable from the engine");
    let mut store = state.lock();
    let template = store
        .jobs
        .get(&first)
        .cloned()
        .expect("the signed job is in the store");
    // Every filler id sorts after the signed job's, which is the uuid-shaped one
    // the store minted, so the signed job is the oldest and the first to go.
    for index in 0..MAX_RETAINED_JOBS {
        let mut job = template.clone();
        job.job_id = format!("filler-{index:08}");
        store.insert_job(job);
    }
    assert_eq!(
        store.jobs.len(),
        MAX_RETAINED_JOBS,
        "expected the store to keep at most {MAX_RETAINED_JOBS} jobs, got {}",
        store.jobs.len()
    );
    assert!(
        !store.jobs.contains_key(&first),
        "expected the job started longest ago to have been dropped, got it still held"
    );
    assert!(
        store.jobs.contains_key(&format!("filler-{:08}", MAX_RETAINED_JOBS - 1)),
        "expected the job stored last to be held"
    );
}

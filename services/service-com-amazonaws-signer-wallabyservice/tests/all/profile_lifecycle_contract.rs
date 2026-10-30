// SPDX-License-Identifier: CC0-1.0

//! The signing-profile lifecycle a client runs — create, revoke, read, cancel —
//! end to end over HTTP, with the three rules that lifecycle turns on:
//!
//! * a `PutSigningProfile` that omits `signingMaterial` is SERVED on every
//!   platform AWS Signer issues the signing certificate for itself, and refused
//!   only on a platform whose profile holds a certificate the caller brought —
//!   `signingMaterial` is optional in the model, so which platforms need it is a
//!   fact of the published catalog, not of the request's shape;
//! * an answer whose modelled output carries no body states the length it has
//!   (`content-length: 0`), whatever status that operation answers;
//! * a member that reports the principal of a call (a revocation's `revokedBy`, a
//!   job's `requestedBy`) reports the IAM principal the request's credentials
//!   RESOLVE to, which is one identity for every credential this emulator accepts
//!   — never the access key id the request happened to present.
//!
//! Each case is a table whose rows differ along what its rule generalises over:
//! every platform the catalog publishes (and both halves of the signing-material
//! split), each operation whose output carries no body, and a credential scope's
//! access key id, its form (long-term or session-shaped) and its region.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::SIGNING_PLATFORM_CATALOG;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handlers fails
/// this suite.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN for `signingMaterial`, so a put that carries the
/// certificate is never refused for the certificate's own value.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The status a Signer answer with no body carries, and the length such an answer
/// states.
///
/// Recorded: every empty-bodied answer of this service — the three operations
/// whose output is `smithy.api#Unit` and the two tagging writers, whose response
/// structures declare no members — comes back `204` carrying a stated length. An
/// answer served over a socket reaches the client without the header on a `204`
/// — the HTTP/1.1 writer this emulator is served by removes `content-length`
/// there — so what is pinned is what the service itself states, which the
/// in-process front door serves unchanged.
const NO_CONTENT_STATUS: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// The IAM principal the emulator resolves a signed request as: the one IAM user
/// whose credentials it publishes, the identity `sts:GetCallerIdentity` answers
/// with. Spelled out rather than imported so a principal invented from the
/// request fails this suite.
fn caller_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:user/emulator")
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One credential scope: what a request presents as its SigV4 `Credential=`.
#[derive(Clone, Copy)]
struct Caller {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The access key id the request is signed with.
    access_key_id: &'static str,
    /// The region the scope names, which is the region whose endpoint is served.
    region: &'static str,
}

/// The callers a revocation is performed by. The rows differ in the access key
/// id's value and its FORM — an IAM user's long-term key (`AKIA`), a session
/// key (`ASIA`), the emulator's own published prefix, and an id in neither
/// family — and in the region the scope names, because none of that changes which
/// principal the credentials resolve to.
const CALLERS: &[Caller] = &[
    Caller {
        case: "a long-term key in us-west-2",
        access_key_id: "AKIDEXAMPLE",
        region: "us-west-2",
    },
    Caller {
        case: "an IAM user's long-term key in eu-west-1",
        access_key_id: "AKIAIOSFODNN7EXAMPLE",
        region: "eu-west-1",
    },
    Caller {
        case: "a session-shaped key in ap-southeast-1",
        access_key_id: "ASIAIOSFODNN7EXAMPLE",
        region: "ap-southeast-1",
    },
    Caller {
        case: "the emulator's own published key prefix in us-east-1",
        access_key_id: "EMULATOR0123456789AB",
        region: "us-east-1",
    },
];

/// What the emulator answered: status, raw body, JSON body and the headers this
/// suite reads.
struct Answer {
    status: u16,
    raw_body: Vec<u8>,
    body: Value,
    content_length: Option<String>,
}

impl Answer {
    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// One request, signed for `caller`.
async fn call(
    router: &MultiServiceRouter,
    caller: &Caller,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/20260101/{}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc",
        caller.access_key_id, caller.region
    );
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{}.amazonaws.com", caller.region))
        .header("authorization", authorization);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
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
        content_length,
    }
}

async fn put_profile(
    router: &MultiServiceRouter,
    caller: &Caller,
    name: &str,
    request: Value,
) -> Answer {
    call(
        router,
        caller,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(request),
    )
    .await
}

async fn get_profile(router: &MultiServiceRouter, caller: &Caller, name: &str) -> Answer {
    call(
        router,
        caller,
        "GET",
        &format!("/signing-profiles/{name}"),
        None,
    )
    .await
}

async fn cancel_profile(router: &MultiServiceRouter, caller: &Caller, name: &str) -> Answer {
    call(
        router,
        caller,
        "DELETE",
        &format!("/signing-profiles/{name}"),
        None,
    )
    .await
}

async fn revoke_profile(
    router: &MultiServiceRouter,
    caller: &Caller,
    name: &str,
    version: &str,
) -> Answer {
    call(
        router,
        caller,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": version,
            "reason": "rotate-signing-key",
            "effectiveTime": 1_700_000_000,
        })),
    )
    .await
}

/// `ProfileVersion`'s `@length(10, 10)` and `^[a-zA-Z0-9]{10}$`, checked here
/// rather than through any crate helper so a wrong mint cannot make this pass.
fn matches_profile_version_pattern(value: &str) -> bool {
    value.len() == 10 && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A create that names no `signingMaterial` is served on every platform AWS
/// Signer issues the certificate for, and refused 400 on one whose profile holds
/// the caller's certificate — and a served create is a whole profile: its answer
/// carries the ARN, a shaped version and the version ARN, a read reports it, and
/// the cancel of the profile it created answers the empty 204 rather than a 404.
///
/// One row per platform the catalog publishes, which is both halves of the split.
#[tokio::test]
async fn a_create_without_signing_material_is_served_where_the_platform_needs_none() {
    let router = emulator();
    let caller = CALLERS[0];
    let mut served = 0usize;
    let mut refused = 0usize;

    for (ordinal, entry) in SIGNING_PLATFORM_CATALOG.iter().enumerate() {
        let case = entry.platform_id;
        let name = format!("No_Material_{ordinal}");
        let answer = put_profile(&router, &caller, &name, json!({ "platformId": case })).await;

        if entry.requires_signing_material {
            refused += 1;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a platform whose profile holds the caller's certificate must \
                 refuse a create that omits it, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                get_profile(&router, &caller, &name).await.status,
                404,
                "case {case:?}: a refused create must store no profile"
            );
            // The same create WITH the certificate is served, so the refusal is
            // about the certificate and not about the platform.
            let with_material = put_profile(
                &router,
                &caller,
                &name,
                json!({
                    "platformId": case,
                    "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                }),
            )
            .await;
            assert_eq!(
                with_material.status, 200,
                "case {case:?}: expected the create naming the certificate to be served, got {} {:?}",
                with_material.status, with_material.body
            );
            continue;
        }

        served += 1;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected PutSigningProfile 200 for a platform that needs no \
             signingMaterial, got {} {:?}",
            answer.status, answer.body
        );
        let expected_arn = format!(
            "arn:aws:signer:{}:{ACCOUNT_ID}:/signing-profiles/{name}",
            caller.region
        );
        assert_eq!(
            answer.str_member("arn"),
            expected_arn,
            "case {case:?}: expected arn {expected_arn}, got {:?}",
            answer.body["arn"]
        );
        let version = answer.str_member("profileVersion");
        assert!(
            matches_profile_version_pattern(&version),
            "case {case:?}: expected a 10-character alphanumeric profileVersion, got {version:?}"
        );
        assert_eq!(
            answer.str_member("profileVersionArn"),
            format!("{expected_arn}/{version}"),
            "case {case:?}: expected the version ARN to be the profile ARN and the version, got {:?}",
            answer.body["profileVersionArn"]
        );
        assert_eq!(
            answer.body["tags"]["resourceArn"],
            json!(expected_arn),
            "case {case:?}: expected the created profile's identity to carry its ARN, got {:?}",
            answer.body["tags"]
        );

        // The profile the create stored is the one a read reports…
        let read_back = get_profile(&router, &caller, &name).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: expected the created profile to be readable, got {} {:?}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.body["platformId"],
            json!(case),
            "case {case:?}: expected the stored platformId, got {:?}",
            read_back.body["platformId"]
        );
        assert!(
            read_back.body["signingMaterial"].is_null(),
            "case {case:?}: expected no signingMaterial on a profile created without it, got {:?}",
            read_back.body["signingMaterial"]
        );
        assert_eq!(
            read_back.str_member("status"),
            "Active",
            "case {case:?}: expected a created profile to be Active, got {:?}",
            read_back.body["status"]
        );

        // … and the one the cancel of that name finds.
        let canceled = cancel_profile(&router, &caller, &name).await;
        assert_eq!(
            canceled.status, NO_CONTENT_STATUS,
            "case {case:?}: expected CancelSigningProfile {NO_CONTENT_STATUS} for the profile the \
             create stored, got {} {:?}",
            canceled.status, canceled.body
        );
        assert_eq!(
            get_profile(&router, &caller, &name).await.str_member("status"),
            "Canceled",
            "case {case:?}: expected the cancel to be recorded"
        );
    }

    // The rows are meant to span both verdicts; a catalog that published only
    // one kind would make half the assertions above vacuous.
    assert!(
        served > 0 && refused > 0,
        "expected the published catalog to carry platforms of both kinds, got {served} that need \
         no signing material and {refused} that do"
    );
}

/// Every answer of this service whose modelled output carries no members comes
/// back `204` with an empty body that STATES its length.
///
/// One row per operation the model binds to `Unit`, each driven through the whole
/// request walk; the other half of that class — the tagging writers, whose
/// response structures declare no members — is covered by the suite that tables
/// the whole class.
#[tokio::test]
async fn an_answer_with_no_body_states_its_length() {
    let router = emulator();
    let caller = CALLERS[0];

    // A profile to cancel, one to revoke, and a job whose signature is revoked.
    let cancelable = put_profile(
        &router,
        &caller,
        "Framing_Cancel",
        json!({ "platformId": "AWSLambda-SHA384-ECDSA" }),
    )
    .await;
    assert_eq!(
        cancelable.status, 200,
        "seeding the cancelable profile: {:?}",
        cancelable.body
    );
    let revocable = put_profile(
        &router,
        &caller,
        "Framing_Revoke",
        json!({ "platformId": "AWSLambda-SHA384-ECDSA" }),
    )
    .await;
    assert_eq!(
        revocable.status, 200,
        "seeding the revocable profile: {:?}",
        revocable.body
    );
    let revocable_version = revocable.str_member("profileVersion");
    let started = call(
        &router,
        &caller,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
            "destination": { "s3": { "bucketName": "dst", "prefix": "signed/" } },
            "profileName": "Framing_Cancel",
            "clientRequestToken": "tok-framing",
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "seeding the signing job: {:?}",
        started.body
    );
    let job_id = started.str_member("jobId");

    let rows: Vec<(&str, &str, String, Option<Value>, u16)> = vec![
        (
            "RevokeSigningProfile",
            "PUT",
            "/signing-profiles/Framing_Revoke/revoke".to_string(),
            Some(json!({
                "profileVersion": revocable_version,
                "reason": "rotate-signing-key",
                "effectiveTime": 1_700_000_000,
            })),
            NO_CONTENT_STATUS,
        ),
        (
            "RevokeSignature",
            "PUT",
            format!("/signing-jobs/{job_id}/revoke"),
            Some(json!({ "reason": "key-compromise" })),
            NO_CONTENT_STATUS,
        ),
        (
            "CancelSigningProfile",
            "DELETE",
            "/signing-profiles/Framing_Cancel".to_string(),
            None,
            NO_CONTENT_STATUS,
        ),
    ];

    for (case, method, uri, body, status) in rows {
        let answer = call(&router, &caller, method, &uri, body).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.raw_body.is_empty(),
            "case {case:?}: expected an empty body, got {:?}",
            String::from_utf8_lossy(&answer.raw_body)
        );
        assert_eq!(
            answer.content_length.as_deref(),
            Some(NO_CONTENT_LENGTH),
            "case {case:?}: expected the empty answer to state content-length \
             {NO_CONTENT_LENGTH}, got {:?}",
            answer.content_length
        );
    }
}

/// A PROBE of what a real socket delivers for the empty answers above, as
/// opposed to what the service states: the status, the absence of a body, and
/// that the answer is not chunked.
///
/// The framing the service states is not the last word on the wire: the HTTP/1.1
/// writer this emulator is served by removes `content-length` from a `204`
/// (RFC 9110 §15.3.5 forbids it there), and that writer is a dependency of the
/// vendored HTTP layer rather than anything this crate decides. So this pins what
/// the socket really delivers, while
/// [`an_answer_with_no_body_states_its_length`] pins the length the service
/// states, which the in-process front door serves unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_answer_on_the_wire_carries_the_status_and_no_body() {
    let router = emulator();
    let caller = CALLERS[0];
    let put = put_profile(
        &router,
        &caller,
        "Wire_Cancel",
        json!({ "platformId": "AWSLambda-SHA384-ECDSA" }),
    )
    .await;
    assert_eq!(put.status, 200, "seeding the profile: {:?}", put.body);

    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(router).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    let head = tokio::task::spawn_blocking(move || raw_cancel_request(port, &caller))
        .await
        .expect("the socket exchange runs to completion");

    assert!(
        head.starts_with(&format!("HTTP/1.1 {NO_CONTENT_STATUS}")),
        "case CancelSigningProfile: expected a {NO_CONTENT_STATUS} on the wire, got {head:?}"
    );
    let (head, body) = head
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {head:?}"));
    assert!(
        body.is_empty(),
        "case CancelSigningProfile: expected no body on the wire, got {body:?}"
    );
    assert!(
        !head.to_ascii_lowercase().contains("transfer-encoding"),
        "case CancelSigningProfile: expected an empty answer not to be chunked, got {head:?}"
    );
}

/// One `CancelSigningProfile` written straight onto a socket, with everything the
/// server answered read back as text.
///
/// `connection: close` so the server closes the socket after answering and the
/// read below terminates on EOF rather than on a parse of the framing this very
/// case is about.
fn raw_cancel_request(port: u16, caller: &Caller) -> String {
    use std::io::{Read, Write};

    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "DELETE /signing-profiles/Wire_Cancel HTTP/1.1\r\n\
         host: signer.{}.amazonaws.com\r\n\
         authorization: AWS4-HMAC-SHA256 Credential={}/20260101/{}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc\r\n\
         connection: close\r\n\
         \r\n",
        caller.region, caller.access_key_id, caller.region
    );
    stream
        .write_all(request.as_bytes())
        .expect("write the request");
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .expect("read the whole answer");
    String::from_utf8_lossy(&answer).to_string()
}

/// A revocation's `revokedBy` — and a job's `requestedBy` — is the IAM principal
/// the request's credentials resolve to: ONE identity, whatever access key id the
/// request presented, and the same one every later read reports.
///
/// One row per caller: the access key ids differ in value, in form (long-term,
/// session-shaped, the emulator's own prefix) and in the region they are scoped
/// to.
#[tokio::test]
async fn the_revoker_is_the_principal_the_credentials_resolve_to() {
    let router = emulator();
    let mut reported = BTreeSet::new();

    for caller in CALLERS {
        let case = caller.case;
        let name = "Revoked_By_Caller";
        let put = put_profile(
            &router,
            caller,
            name,
            json!({ "platformId": "AWSLambda-SHA384-ECDSA" }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: seeding the profile: {:?}",
            put.body
        );
        let version = put.str_member("profileVersion");

        let revoked = revoke_profile(&router, caller, name, &version).await;
        assert_eq!(
            revoked.status, NO_CONTENT_STATUS,
            "case {case:?}: expected RevokeSigningProfile {NO_CONTENT_STATUS}, got {} {:?}",
            revoked.status, revoked.body
        );

        let after = get_profile(&router, caller, name).await;
        let revoker = after.body["revocationRecord"]["revokedBy"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            revoker,
            caller_principal(),
            "case {case:?}: expected revokedBy to be the principal the credentials resolve to, \
             got {revoker:?}"
        );
        assert!(
            !revoker.contains(caller.access_key_id),
            "case {case:?}: expected revokedBy not to carry the access key id, got {revoker:?}"
        );
        reported.insert(revoker);

        // A second read reports the same revoker: the record is a read, not a
        // write.
        let again = get_profile(&router, caller, name).await;
        assert_eq!(
            again.body["revocationRecord"], after.body["revocationRecord"],
            "case {case:?}: expected the revocation record to be stable across reads, got {:?} \
             then {:?}",
            after.body["revocationRecord"], again.body["revocationRecord"]
        );

        // The same rule on the other two members that report a principal: the
        // job a caller started, and the revocation of its signature.
        let job_profile = "Job_By_Caller";
        let seeded = put_profile(
            &router,
            caller,
            job_profile,
            json!({ "platformId": "AWSLambda-SHA384-ECDSA" }),
        )
        .await;
        assert_eq!(
            seeded.status, 200,
            "case {case:?}: seeding the job's profile: {:?}",
            seeded.body
        );
        let started = call(
            &router,
            caller,
            "POST",
            "/signing-jobs",
            Some(json!({
                "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
                "destination": { "s3": { "bucketName": "dst", "prefix": "signed/" } },
                "profileName": job_profile,
                "clientRequestToken": format!("tok-{}", caller.access_key_id),
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: starting the signing job: {:?}",
            started.body
        );
        let job_id = started.str_member("jobId");
        let revoked_signature = call(
            &router,
            caller,
            "PUT",
            &format!("/signing-jobs/{job_id}/revoke"),
            Some(json!({ "reason": "key-compromise" })),
        )
        .await;
        assert_eq!(
            revoked_signature.status, NO_CONTENT_STATUS,
            "case {case:?}: expected RevokeSignature {NO_CONTENT_STATUS}, got {} {:?}",
            revoked_signature.status, revoked_signature.body
        );
        let described = call(&router, caller, "GET", &format!("/signing-jobs/{job_id}"), None).await;
        assert_eq!(
            described.str_member("requestedBy"),
            caller_principal(),
            "case {case:?}: expected requestedBy to be the principal the credentials resolve to, \
             got {:?}",
            described.body["requestedBy"]
        );
        assert_eq!(
            described.body["revocationRecord"]["revokedBy"],
            json!(caller_principal()),
            "case {case:?}: expected the signature's revokedBy to be the resolved principal, got \
             {:?}",
            described.body["revocationRecord"]
        );
    }

    // Every row reported the SAME principal: the identity does not vary with the
    // credential presented, so a value synthesized from one would show up here as
    // several.
    assert_eq!(
        reported.len(),
        1,
        "expected one resolved principal across every caller, got {reported:?}"
    );
}

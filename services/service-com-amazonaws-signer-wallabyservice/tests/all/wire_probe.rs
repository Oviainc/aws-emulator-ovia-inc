// SPDX-License-Identifier: CC0-1.0

//! What a Signer success looks like on the wire, split by whether the operation
//! has anything to report.
//!
//! The rule under test: an operation whose modelled output declares NO members
//! answers a successful call `204 No Content` with an empty body whose length is
//! stated, while an operation whose output declares members answers `200` with
//! the JSON document those members form. Which half an operation is in is read
//! off the model — the output shape is `smithy.api#Unit`, or a structure with an
//! empty member set — and never off the operation's name, its method or what the
//! call happened to change.
//!
//! Source of the split: `smithy_query(query="digest",
//! model_path="services/service-com-amazonaws-signer-wallabyservice/smithy-model/\
//! com-amazonaws-signer-wallabyservice.json")` for every operation's output
//! shape, plus `smithy_query(query="shape",
//! shape="TagResourceResponse,UntagResourceResponse")`, which gives both of them
//! `members: {}`. No operation of this service declares an `@httpResponseCode`,
//! so the status is NOT the protocol default the route resolves to.
//!
//! The rows differ along what the rule generalises over: every operation in the
//! no-content half (both the `Unit` outputs and the memberless structures), every
//! HTTP method they are bound to (`POST`, `PUT`, `DELETE`), what the call did to
//! the resource (a write that adds a key, a write that replaces one, a removal
//! that takes a key the resource holds and one it does not, a revoke of a profile
//! and of a signature, a cancel), and — on the other side of the split — the
//! readers and the writers of this service that DO report a document.
//!
//! One case drives the same `204` over a real socket, because what the service
//! states and what an HTTP/1.1 writer puts on the wire are not the same thing:
//! RFC 9110 §15.3.5 forbids `content-length` on a `204`, and the writer the
//! vendored listener is built on drops it there. So the socket case pins the
//! status and the absent body, and the in-process cases pin the length the
//! service itself states.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out rather than imported so a wrong account in the handlers fails
/// here.
const ACCOUNT_ID: &str = "000000000000";

/// A platform whose profiles need no `signingMaterial` and that publishes
/// revocation support, so one `PutSigningProfile` is enough to seed a row.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The status a success with nothing to report answers with.
const NO_CONTENT_STATUS: u16 = 204;

/// The length such an answer states.
const NO_CONTENT_LENGTH: &str = "0";

/// The status a success that reports a document answers with.
const DOCUMENT_STATUS: u16 = 200;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the raw bytes, the body as JSON and
/// the framing header this suite reads.
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

/// Percent-encode everything outside the unreserved set, so an ARN travels as
/// ONE path label.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    );
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
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

/// A profile of `name`, and the ARN this service stored for it.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": PLATFORM_ID })),
    )
    .await;
    assert_eq!(
        answer.status, DOCUMENT_STATUS,
        "seeding profile {name}: expected {DOCUMENT_STATUS}, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("arn")
}

/// A signing job on `profile`, and the job id this service minted for it.
async fn seed_job(router: &MultiServiceRouter, profile: &str, token: &str) -> String {
    let answer = call(
        router,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
            "destination": { "s3": { "bucketName": "dst", "prefix": "signed/" } },
            "profileName": profile,
            "clientRequestToken": token,
        })),
    )
    .await;
    assert_eq!(
        answer.status, DOCUMENT_STATUS,
        "seeding a job on {profile}: expected {DOCUMENT_STATUS}, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("jobId")
}

/// A tag map as the request body carries it.
fn tags_body(pairs: &[(&str, &str)]) -> Value {
    let map: Map<String, Value> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
        .collect();
    json!({ "tags": Value::Object(map) })
}

/// One call this suite makes, and which half of the split it belongs to.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every operation of this service whose modelled output declares no members
/// answers a success `204` with an empty body whose length it states — whatever
/// method it is bound to and whatever the call changed.
#[tokio::test]
async fn a_success_with_nothing_to_report_answers_no_content() {
    let router = emulator();

    // One profile per revoking row (a revoke and a cancel are terminal), one for
    // the tagging rows, and a job whose signature is revoked.
    let tagged = seed_profile(&router, "No_Content_Tagged").await;
    let revocable = seed_profile(&router, "No_Content_Revoke").await;
    let revocable_version = call(
        &router,
        "GET",
        "/signing-profiles/No_Content_Revoke",
        None,
    )
    .await
    .str_member("profileVersion");
    let cancelable = seed_profile(&router, "No_Content_Cancel").await;
    assert!(
        !cancelable.is_empty(),
        "seeding the cancelable profile must report an arn"
    );
    assert!(
        !revocable.is_empty(),
        "seeding the revocable profile must report an arn"
    );
    let signed = seed_profile(&router, "No_Content_Signature").await;
    assert!(
        !signed.is_empty(),
        "seeding the signed profile must report an arn"
    );
    let job_id = seed_job(&router, "No_Content_Signature", "tok-no-content").await;

    let rows = vec![
        Row {
            case: "TagResource writing one key the profile does not hold",
            method: "POST",
            uri: format!("/tags/{}", encode(&tagged)),
            body: Some(tags_body(&[("env", "prod")])),
        },
        Row {
            case: "TagResource replacing a key the profile already holds",
            method: "POST",
            uri: format!("/tags/{}", encode(&tagged)),
            body: Some(tags_body(&[("env", "staging")])),
        },
        Row {
            case: "TagResource writing several keys at once",
            method: "POST",
            uri: format!("/tags/{}", encode(&tagged)),
            body: Some(tags_body(&[("team", "signing"), ("owner", "platform")])),
        },
        Row {
            case: "UntagResource removing one key the profile holds",
            method: "DELETE",
            uri: format!("/tags/{}?tagKeys=env", encode(&tagged)),
            body: None,
        },
        Row {
            case: "UntagResource removing several keys at once",
            method: "DELETE",
            uri: format!("/tags/{}?tagKeys=team&tagKeys=owner", encode(&tagged)),
            body: None,
        },
        Row {
            case: "UntagResource naming a key the profile does not hold",
            method: "DELETE",
            uri: format!("/tags/{}?tagKeys=never-written", encode(&tagged)),
            body: None,
        },
        Row {
            case: "RevokeSigningProfile",
            method: "PUT",
            uri: format!("/signing-profiles/{}/revoke", "No_Content_Revoke"),
            body: Some(json!({
                "profileVersion": revocable_version,
                "reason": "rotate-signing-key",
                "effectiveTime": 1_700_000_000,
            })),
        },
        Row {
            case: "RevokeSignature",
            method: "PUT",
            uri: format!("/signing-jobs/{job_id}/revoke"),
            body: Some(json!({ "reason": "key-compromise" })),
        },
        Row {
            case: "CancelSigningProfile",
            method: "DELETE",
            uri: "/signing-profiles/No_Content_Cancel".to_string(),
            body: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = call(&router, row.method, &row.uri, row.body).await;
        assert_eq!(
            answer.status, NO_CONTENT_STATUS,
            "case {case:?}: expected HTTP {NO_CONTENT_STATUS}, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.raw_body.is_empty(),
            "case {case:?}: expected no body at all, got {:?}",
            String::from_utf8_lossy(&answer.raw_body)
        );
        assert_eq!(
            answer.content_length.as_deref(),
            Some(NO_CONTENT_LENGTH),
            "case {case:?}: expected the stated length {NO_CONTENT_LENGTH}, got {:?}",
            answer.content_length
        );
    }
}

/// The other half of the split: an operation whose modelled output declares
/// members answers `200` with the document those members form, so the no-content
/// status is not simply what every success of this service answers.
#[tokio::test]
async fn a_success_that_reports_a_document_answers_two_hundred() {
    let router = emulator();
    let arn = seed_profile(&router, "Document_Profile").await;
    let written = call(
        &router,
        "POST",
        &format!("/tags/{}", encode(&arn)),
        Some(tags_body(&[("env", "prod")])),
    )
    .await;
    assert_eq!(
        written.status, NO_CONTENT_STATUS,
        "seeding the tags: expected {NO_CONTENT_STATUS}, got {} {:?}",
        written.status, written.body
    );

    let rows = vec![
        Row {
            case: "PutSigningProfile",
            method: "PUT",
            // A name nothing has created yet: a signing-profile name is taken by
            // the create that first used it, so the create that answers a
            // document is one under a free name.
            uri: "/signing-profiles/Document_Profile_Created".to_string(),
            body: Some(json!({ "platformId": PLATFORM_ID })),
        },
        Row {
            case: "GetSigningProfile",
            method: "GET",
            uri: "/signing-profiles/Document_Profile".to_string(),
            body: None,
        },
        Row {
            case: "ListTagsForResource",
            method: "GET",
            uri: format!("/tags/{}", encode(&arn)),
            body: None,
        },
        Row {
            case: "ListSigningProfiles",
            method: "GET",
            uri: "/signing-profiles".to_string(),
            body: None,
        },
        Row {
            case: "AddProfilePermission",
            method: "POST",
            uri: "/signing-profiles/Document_Profile/permissions".to_string(),
            body: Some(json!({
                "statementId": "AllowRead",
                "action": "signer:GetSigningProfile",
                "principal": ACCOUNT_ID,
            })),
        },
        Row {
            case: "ListProfilePermissions",
            method: "GET",
            uri: "/signing-profiles/Document_Profile/permissions".to_string(),
            body: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = call(&router, row.method, &row.uri, row.body).await;
        assert_eq!(
            answer.status, DOCUMENT_STATUS,
            "case {case:?}: expected HTTP {DOCUMENT_STATUS}, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body.is_object(),
            "case {case:?}: expected a JSON document, got {:?}",
            String::from_utf8_lossy(&answer.raw_body)
        );
    }
}

/// The same no-content answer over a real socket: the status reaches the client
/// and the body is still empty.
///
/// The stated `content-length` does NOT survive here — RFC 9110 §15.3.5 forbids
/// it on a `204` and the HTTP/1.1 writer the vendored listener is built on drops
/// it — so this case pins what the socket really delivers, while
/// [`a_success_with_nothing_to_report_answers_no_content`] pins the length the
/// service states, which the in-process front door serves unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn the_no_content_answer_reaches_a_socket_client_with_no_body() {
    let router = emulator();
    let arn = seed_profile(&router, "Wire_No_Content").await;

    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(router).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    let target = format!("/tags/{}", encode(&arn));
    let head = tokio::task::spawn_blocking(move || raw_tag_request(port, &target))
        .await
        .expect("the socket exchange runs to completion");

    assert!(
        head.starts_with(&format!("HTTP/1.1 {NO_CONTENT_STATUS}")),
        "case TagResource on a socket: expected a {NO_CONTENT_STATUS}, got {head:?}"
    );
    let (_, body) = head
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {head:?}"));
    assert!(
        body.is_empty(),
        "case TagResource on a socket: expected no body, got {body:?}"
    );
    assert!(
        !head.to_ascii_lowercase().contains("transfer-encoding"),
        "case TagResource on a socket: expected the answer not to be chunked, got {head:?}"
    );
}

/// One `TagResource` written straight onto a socket, with everything the server
/// answered read back as text.
///
/// `connection: close` so the server closes the socket after answering and the
/// read below terminates on EOF rather than on a parse of the framing this very
/// case is about.
fn raw_tag_request(port: u16, target: &str) -> String {
    use std::io::{Read, Write};

    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let payload = json!({ "tags": { "env": "prod" } }).to_string();
    let request = format!(
        "POST {target} HTTP/1.1\r\n\
         host: signer.{REGION}.amazonaws.com\r\n\
         authorization: AWS4-HMAC-SHA256 \
         Credential={ACCESS_KEY_ID}/20260101/{REGION}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc\r\n\
         content-type: application/json\r\n\
         content-length: {}\r\n\
         connection: close\r\n\
         \r\n{payload}",
        payload.len()
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

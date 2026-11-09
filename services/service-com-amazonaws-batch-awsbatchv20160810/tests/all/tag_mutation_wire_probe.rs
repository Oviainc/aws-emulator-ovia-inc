// SPDX-License-Identifier: CC0-1.0

//! A PROBE of what a real socket delivers for a tag mutation answered with no content, as
//! opposed to what the service hands back in process (`tag_mutation_no_content.rs`, the
//! in-process half of the same rule).
//!
//! The framing a service states is not the last word on the wire: the HTTP writer this
//! emulator is served by owns the framing headers of a response that carries no body. What
//! is pinned here is what the socket really delivers for an accepted `TagResource` — the
//! status, an empty body, no chunked framing and no length header — so the boundary between
//! what this crate states and what reaches a client is a fact in the suite rather than a
//! claim in a comment.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_batch_awsbatchv20160810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header is how a request names the service and the region it is
/// addressed to, so every call carries one; the signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20240101/us-east-1/batch/aws4_request, SignedHeaders=host, \
     Signature=0000";

/// The router this probe serves, and the ARN of one compute environment in it to tag.
async fn router_with_a_compute_environment() -> (MultiServiceRouter, String) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));

    let create = http::Request::builder()
        .method("POST")
        .uri("/v1/createcomputeenvironment")
        .header("host", "batch.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            json!({"computeEnvironmentName": "wire-framing", "type": "UNMANAGED"}).to_string(),
        ))
        .expect("the test request is well formed");
    let response = router.dispatch(create).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "the fixture compute environment must be created, got {} {:?}",
        response.status().as_u16(),
        String::from_utf8_lossy(response.body())
    );
    let body: serde_json::Value =
        serde_json::from_slice(response.body()).expect("CreateComputeEnvironment answers JSON");
    let arn = body["computeEnvironmentArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty(),
        "CreateComputeEnvironment must mint an ARN, got {body}"
    );
    (router, arn)
}

/// RULE: on the wire an accepted tag mutation is the no-content status with no body — not
/// chunked, and with no length header, which the HTTP writer below this crate owns for this
/// status.
#[tokio::test(flavor = "multi_thread")]
async fn a_tag_mutation_on_the_wire_carries_the_status_and_no_body() {
    let (router, arn) = router_with_a_compute_environment().await;
    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(router).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    // The whole ARN is ONE path segment, so its `:` and `/` travel percent-encoded.
    let path = format!("/v1/tags/{}", arn.replace(':', "%3A").replace('/', "%2F"));
    let answer = tokio::task::spawn_blocking(move || raw_tag_request(port, &path))
        .await
        .expect("the socket exchange runs to completion");

    assert!(
        answer.starts_with("HTTP/1.1 204"),
        "case TagResource: expected a 204 on the wire, got {answer:?}"
    );
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {answer:?}"));
    assert!(
        body.is_empty(),
        "case TagResource: a no-content answer carries no body, got {body:?}"
    );
    let head = head.to_ascii_lowercase();
    assert!(
        !head.contains("transfer-encoding"),
        "case TagResource: an answer with no content is not chunked, got {head:?}"
    );
    assert!(
        !head.contains("content-length"),
        "case TagResource: a no-content answer states no length, got {head:?}"
    );
}

/// One `TagResource` request written straight onto a socket, with everything the server
/// answered read back as text.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the very framing this case is about.
fn raw_tag_request(port: u16, path: &str) -> String {
    use std::io::{Read, Write};

    let payload = json!({"tags": {"team": "batch"}}).to_string();
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         host: batch.us-east-1.amazonaws.com\r\n\
         authorization: {AUTHORIZATION}\r\n\
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

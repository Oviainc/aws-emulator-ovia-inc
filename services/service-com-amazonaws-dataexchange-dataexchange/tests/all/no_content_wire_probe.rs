// SPDX-License-Identifier: CC0-1.0

//! A PROBE of what a real socket delivers for an answer that carries no content, as
//! opposed to what this service STATES for it (`no_content_framing.rs`, the in-process
//! half of the same rule).
//!
//! The framing a service states is not the last word on the wire: the HTTP/1.1 writer this
//! emulator is served by removes `content-length` from a `204` (RFC 9110 §15.3.5 forbids a
//! body there), and that writer is a dependency of the vendored HTTP layer rather than
//! anything this crate decides. What is pinned here is what the socket really delivers for
//! such an answer — the status, the absence of a body, that the answer is not chunked, and
//! that the length this service states does not survive that writer — so the boundary
//! between the two is a fact in the suite rather than a claim in a comment. The in-process
//! front door (the inter-service callback path and the SDK connector, both of which serve
//! `dispatch`'s response unchanged) does deliver it, which is what
//! `no_content_framing.rs` pins.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_dataexchange_dataexchange::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header is how a request names the service and the region it is
/// addressed to, so every call carries one; the signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=TESTKEY/20240101/us-west-2/dataexchange/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The router this probe serves, and the ARN of one data set in it to tag.
async fn router_with_a_data_set() -> (MultiServiceRouter, String) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));

    let create = http::Request::builder()
        .method("POST")
        .uri("/v1/data-sets")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            json!({
                "AssetType": "S3_SNAPSHOT",
                "Description": "wire framing",
                "Name": "wire-framing",
            })
            .to_string(),
        ))
        .expect("the test request is well formed");
    let response = router.dispatch(create).await;
    assert_eq!(
        response.status().as_u16(),
        201,
        "the fixture data set must be created, got {} {:?}",
        response.status().as_u16(),
        String::from_utf8_lossy(response.body())
    );
    let body: serde_json::Value =
        serde_json::from_slice(response.body()).expect("CreateDataSet answers JSON");
    let arn = body["Arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !arn.is_empty(),
        "CreateDataSet must mint an Arn, got {body}"
    );
    (router, arn)
}

/// RULE: on the wire, a `TagResource` write is a 204 that carries no body and is not
/// chunked, and the `content-length: 0` this service states for it does not reach the
/// client — the HTTP/1.1 writer below this crate drops that header on a 204. Pinned so a
/// change in that writer shows up as a test failure rather than as a silent change in what
/// a client sees.
#[tokio::test(flavor = "multi_thread")]
async fn a_no_content_answer_on_the_wire_carries_the_status_and_no_body() {
    let (router, arn) = router_with_a_data_set().await;
    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(router).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    // The whole ARN is ONE path segment, so its `:` and `/` travel percent-encoded.
    let path = format!("/tags/{}", arn.replace(':', "%3A").replace('/', "%2F"));
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
        "case TagResource: a 204 carries no body, got {body:?}"
    );
    let head = head.to_ascii_lowercase();
    assert!(
        !head.contains("transfer-encoding"),
        "case TagResource: an answer with no content is not chunked, got {head:?}"
    );
    assert!(
        !head.contains("content-length"),
        "case TagResource: the HTTP/1.1 writer below this crate drops content-length from a \
         204, so the socket states none, got {head:?}"
    );
}

/// One `TagResource` request written straight onto a socket, with everything the server
/// answered read back as text.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the very framing this case is about.
fn raw_tag_request(port: u16, path: &str) -> String {
    use std::io::{Read, Write};

    let payload = json!({ "tags": { "team": "data" } }).to_string();
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         host: localhost\r\n\
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

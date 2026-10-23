// SPDX-License-Identifier: CC0-1.0

//! A PROBE of what a real socket answers for a tagging write, as opposed to what this
//! service states (`empty_response_framing.rs`, the in-process half of the same rule).
//!
//! The framing this service states is not the last word on the wire: the HTTP/1.1 writer
//! this emulator is served by removes `content-length` from a `204` (RFC 9110 §15.3.5
//! forbids it there), and that writer is a dependency of the vendored HTTP layer rather
//! than anything this crate decides. So what is pinned here is what the socket really
//! delivers — the status, the absence of a body, and that the answer is not chunked —
//! while `empty_response_framing.rs` pins the `content-length: 0` the service itself
//! states, which the in-process front door (the inter-service callback path and the SDK
//! connector, both of which serve `dispatch`'s response unchanged) delivers. The sibling
//! `schemas` emulator records the same boundary in
//! `services/service-com-amazonaws-schemas-schemas/tests/all/empty_response_framing.rs`.

use serde_json::json;

use crate::empty_response_framing::{label, router_with_a_stage, AUTHORIZATION};

/// RULE: on the wire, a tagging write is a 204 with no body and no chunked framing.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_answer_on_the_wire_carries_the_status_and_no_body() {
    let (router, arn) = router_with_a_stage().await;

    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(router).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    let path = format!("/tags/{}", label(&arn));
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
    assert!(
        !head.to_ascii_lowercase().contains("transfer-encoding"),
        "case TagResource: an empty answer is not chunked, got {head:?}"
    );
}

/// One `TagResource` request written straight onto a socket, with everything the server
/// answered read back as text.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the framing this very case is about.
fn raw_tag_request(port: u16, path: &str) -> String {
    use std::io::{Read, Write};

    let payload = json!({ "tags": { "owner": "behavioral" } }).to_string();
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

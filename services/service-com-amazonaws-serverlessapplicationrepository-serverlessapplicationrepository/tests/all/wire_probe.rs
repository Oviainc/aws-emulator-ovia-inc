// SPDX-License-Identifier: CC0-1.0

//! A PROBE of what a REAL SOCKET delivers for the answers this service frames itself, as
//! opposed to what it states in process (`empty_response_framing.rs` and
//! `artifact_document_plane.rs`, the in-process halves of the same two rules).
//!
//! Two answers of this service carry framing the handler decides rather than the body:
//!
//! * `DeleteApplication` is modelled with an empty (`Unit`) output at HTTP 204
//!   (`smithy_query(query="operation", operation="DeleteApplication")`), and the service
//!   states that answer's length (`content-length: 0`);
//! * a `HEAD` of a published artifact locator reports the stored document's length with no
//!   bytes, which is what the method means.
//!
//! The second is what this suite ASSERTS about the framing: a length the handler states
//! reaches a socket client. The first is asserted only as far as a socket can show it — the
//! status, that no body is carried, that the answer is not chunked — because the one header
//! is dropped below this crate: the HTTP/1.1 writer the vendored transport serves on
//! removes `content-length` from a `204`, which RFC 9110 §15.3.5 forbids there, and that
//! writer is a dependency of the vendored HTTP layer rather than anything this crate
//! decides. The `content-length: 0` the service itself states is pinned in
//! `empty_response_framing.rs`, on the response `dispatch` produces, which is what the
//! in-process front door (the inter-service callback path and the SDK connector, both of
//! which serve that response unchanged) delivers.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::EmulatedService;

use super::applications::path_label;

/// A SigV4 `Authorization` header is how a request names the service and the region it is
/// addressed to, so every control-plane call carries one; the signature itself is not under
/// test here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20240101/us-west-2/serverlessrepo/aws4_request, \
     SignedHeaders=host, Signature=deadbeef";

/// This service, mounted on its own router and served on a loopback port: the endpoint's
/// authority (which a minted locator is addressed to and signed over) and the router, so a
/// fixture can be set up through the same front door while the port is being served.
async fn served() -> (String, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here; the framing of the answers is.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));

    let (endpoint, router) = emulator_server::serve_on_ephemeral_port(router).await;
    let port = endpoint
        .rsplit(':')
        .next()
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));
    (format!("127.0.0.1:{port}"), router)
}

/// The port of an `host:port` authority.
fn port_of(authority: &str) -> u16 {
    authority
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an authority with a port, got {authority:?}"))
}

/// One control-plane call through the in-process front door, from a client that reached the
/// emulator at `authority` — so a locator the answer announces is addressed back there.
async fn call(
    router: &MultiServiceRouter,
    authority: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .header("host", authority)
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.body().to_vec();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// RULE: on the wire, the delete of an application is a 204 that carries no body and is not
/// chunked.
#[tokio::test(flavor = "multi_thread")]
async fn the_empty_answer_on_the_wire_carries_the_status_and_no_body() {
    let (authority, router) = served().await;

    let (status, created) = call(
        &router,
        &authority,
        "POST",
        "/applications",
        Some(json!({
            "name": "wire-probe-app",
            "author": "example-author",
            "description": "a demo app"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture application must be created, got {status} with {created}"
    );
    let application_id = created["applicationId"].as_str().unwrap_or_default().to_string();
    assert!(
        !application_id.is_empty(),
        "CreateApplication must mint an ApplicationId, got {created}"
    );

    let target = format!("/applications/{}", path_label(&application_id));
    let port = port_of(&authority);
    let answer = tokio::task::spawn_blocking(move || {
        raw_request(port, "DELETE", &target, &format!("authorization: {AUTHORIZATION}\r\n"))
    })
    .await
    .expect("the socket exchange runs to completion");

    assert!(
        answer.starts_with("HTTP/1.1 204"),
        "case DeleteApplication: expected a 204 on the wire, got {answer:?}"
    );
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {answer:?}"));
    assert!(
        body.is_empty(),
        "case DeleteApplication: a 204 carries no body, got {body:?}"
    );
    assert!(
        !head.to_ascii_lowercase().contains("transfer-encoding"),
        "case DeleteApplication: an empty answer is not chunked, got {head:?}"
    );
    // The bodyless answer is a real delete, not just a framing: the application it named is
    // gone for the next caller.
    let (status, read_back) = call(
        &router,
        &authority,
        "GET",
        &format!("/applications/{}", path_label(&application_id)),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case DeleteApplication: the deleted application must be gone, got {status} with \
         {read_back}"
    );
}

/// RULE: a length this service STATES for a document reaches a socket client — including on
/// the `HEAD` that reports a document's length without its bytes, where the length is the
/// handler's own statement rather than a count of what the message carries.
///
/// The rows are the two methods the document plane serves, which differ in exactly that:
/// the `GET` carries the document, the `HEAD` carries none of it, and both state the same
/// length — the stored document's.
#[tokio::test(flavor = "multi_thread")]
async fn a_stated_length_reaches_the_wire_where_the_status_allows_one() {
    let (authority, router) = served().await;

    let template = json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": { "Q": { "Type": "AWS::SQS::Queue" } }
    })
    .to_string();
    let (status, created) = call(
        &router,
        &authority,
        "POST",
        "/applications",
        Some(json!({
            "name": "wire-probe-artifact",
            "author": "example-author",
            "description": "a demo app",
            "semanticVersion": "1.0.0",
            "templateBody": template
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture publish must succeed, got {status} with {created}"
    );
    let locator = created["version"]["templateUrl"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        locator.contains('?'),
        "a published body is announced at a presigned locator, got {locator:?}"
    );
    // The locator's request TARGET: its path and the query its signature covers, with the
    // scheme and the authority removed, which is what goes on the request line.
    let target = match locator.split_once("://") {
        Some((_scheme, rest)) => rest[rest.find('/').unwrap_or(rest.len())..].to_string(),
        None => locator.clone(),
    };

    let port = port_of(&authority);
    // (case, method, whether the answer carries the document)
    let cases: Vec<(&str, &str, bool)> = vec![
        ("GET: the document itself", "GET", true),
        ("HEAD: the document's metadata only", "HEAD", false),
    ];
    for (case, method, carries_document) in &cases {
        let target = target.clone();
        let verb = (*method).to_string();
        let answer = tokio::task::spawn_blocking(move || raw_request(port, &verb, &target, ""))
            .await
            .expect("the socket exchange runs to completion");
        assert!(
            answer.starts_with("HTTP/1.1 200"),
            "case {case:?}: expected a 200 on the wire, got {answer:?}"
        );
        let (head, body) = answer
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("case {case:?}: a response head and body, got {answer:?}"));
        assert_eq!(
            body.is_empty(),
            !carries_document,
            "case {case:?}: expected the document present={carries_document}, got {body:?}"
        );
        if *carries_document {
            assert_eq!(
                body, template,
                "case {case:?}: the published document is served back, got {body:?}"
            );
        }
        assert_eq!(
            stated_length(head),
            Some(template.len().to_string()),
            "case {case:?}: the answer states the stored document's {} bytes, got {head:?}",
            template.len()
        );
    }
}

/// The `content-length` the response head states, or `None` when it states none.
fn stated_length(head: &str) -> Option<String> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().to_string())
}

/// One request written straight onto a socket, with everything the server answered read
/// back as text. `extra` carries any further request header lines, each `\r\n`-terminated.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the framing these cases are about.
fn raw_request(port: u16, method: &str, target: &str, extra: &str) -> String {
    use std::io::{Read, Write};

    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "{method} {target} HTTP/1.1\r\n\
         host: 127.0.0.1:{port}\r\n\
         {extra}\
         connection: close\r\n\
         \r\n"
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

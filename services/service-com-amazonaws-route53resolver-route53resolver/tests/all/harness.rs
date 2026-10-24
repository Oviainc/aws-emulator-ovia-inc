// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the service through.
//!
//! Each case goes over the protocol the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what a suite asserts is the HTTP
//! behaviour and not a handler called directly.
//!
//! It lives here, and not in one suite, so a second suite does not grow its own
//! copy of the dispatch, the error reading and the timestamp parsing.

use serde_json::Value;
use service_com_amazonaws_route53resolver_route53resolver::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for; it is where the ARNs the
/// service mints must be rooted.
pub const REGION: &str = "us-west-2";

/// The canonical placeholder account id every emulator ARN carries.
pub const ACCOUNT: &str = "000000000000";

/// A fresh emulator: one shared engine, this one service mounted on it.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return `(http status, parsed body)`.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Route53Resolver.{operation}"))
        // The SigV4 credential scope is what routes the request to this service
        // and what names the region its ARNs are minted in.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{REGION}/route53resolver/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=not-verified-in-this-test"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call an operation that must succeed, and return its body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} must succeed, got {status}: {parsed}");
    parsed
}

/// The `__type` of an error response, which carries the modelled error's shape
/// name on the awsJson protocols.
pub fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert an operation failed with `expected` as its modelled error.
pub async fn expect_error(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
    expected: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must answer HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(&parsed).contains(expected),
        "case {case:?}: {operation} must answer {expected}, got {parsed}"
    );
}

pub fn string_member(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The same object with `Status` dropped, so two reads can be compared without
/// the member the clock is allowed to move.
pub fn without_status(value: &Value) -> Value {
    let mut cloned = value.clone();
    if let Some(object) = cloned.as_object_mut() {
        object.remove("Status");
    }
    cloned
}

/// Parse a modelled `Rfc3339TimeString` (length 20..40) into a point in time, so
/// two timestamps are compared numerically rather than as text.
pub fn parse_time(case: &str, member: &str, raw: &str) -> aws_smithy_types::DateTime {
    assert!(
        raw.len() >= 20 && raw.len() <= 40,
        "case {case:?}: {member} must satisfy Rfc3339TimeString's length 20..40, got {raw:?}"
    );
    aws_smithy_types::DateTime::from_str(raw, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} must be an RFC 3339 time, got {raw:?}: {error}")
        })
}

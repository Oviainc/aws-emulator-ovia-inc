// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this test target shares: a router over this one
//! service, the awsJson1_1 call/assert helpers, and the change-token protocol a
//! mutating AWS WAF Classic call needs.
//!
//! Each call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: AWSWAF_Regional_20161128.<Op>`), so what a suite asserts is what an
//! SDK client sees: each response is built from the persisted record, not from the
//! request.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::EmulatedService;

/// A router serving this one service over `engine`, as `build_router` does in
/// production. Hold on to the same `engine` to inspect the persisted state a call
/// wrote.
pub fn router_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A router serving this one service over a fresh engine.
pub fn router() -> MultiServiceRouter {
    router_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The region the credential scope of [`call`] names.
pub const DEFAULT_TEST_REGION: &str = "us-east-1";

/// Call one operation, signed for [`DEFAULT_TEST_REGION`], and return
/// `(http status, parsed body)`.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, DEFAULT_TEST_REGION, operation, body).await
}

/// Call one operation with a SigV4 credential scope naming `region`, and return
/// `(http status, parsed body)` — the seam a suite needs to assert what a response
/// derives from the region the request was signed for.
pub async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        // The routing signal an SDK request carries: the SigV4 credential scope names
        // this service's signing name (`waf-regional`). Request authentication is off
        // for this router, so the signature itself is never checked.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20250818/{region}/waf-regional/\
                 aws4_request, SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000"
            ),
        )
        .header("host", format!("waf-regional.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSWAF_Regional_20161128.{operation}"),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Succeed-or-explain: the call must answer 200.
pub async fn ok_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} expected 200, got {status}: {parsed}"
    );
    parsed
}

/// The wire error code (`__type`) of an error body.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` member of an error body, however the protocol cased it.
pub fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Assert `operation` refused `request` with HTTP 400 and the named modelled error.
pub async fn expect_error(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
    expected: &str,
    label: &str,
) -> Value {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {label}: {operation} must answer 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains(expected),
        "case {label}: expected {expected}, got {} ({body})",
        error_code(&body)
    );
    body
}

/// The change token a mutating call must carry. Each one is consumed by exactly one
/// mutating call, so every caller asks for its own.
pub async fn change_token(router: &MultiServiceRouter) -> String {
    let body = ok_call(router, "GetChangeToken", &json!({})).await;
    body["ChangeToken"]
        .as_str()
        .unwrap_or_else(|| panic!("GetChangeToken returned no ChangeToken: {body}"))
        .to_string()
}

/// The reported status of `token`.
pub async fn token_status(router: &MultiServiceRouter, token: &str) -> String {
    let body = ok_call(
        router,
        "GetChangeTokenStatus",
        &json!({"ChangeToken": token}),
    )
    .await;
    body["ChangeTokenStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("GetChangeTokenStatus returned no status: {body}"))
        .to_string()
}

/// `ResourceId`/`ChangeToken` values AWS WAF Classic hands out are 36-character
/// UUID-shaped strings, and the model bounds both at `len 1..128, pattern \S`.
pub fn is_resource_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(groups.iter())
            .all(|(len, group)| group.len() == *len && group.chars().all(|c| c.is_ascii_hexdigit()))
        && (1..=128).contains(&id.len())
        && !id.chars().any(char::is_whitespace)
}

// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the service through: `awsJson1_0`,
//! `POST /` with `X-Amz-Target: GraniteServiceVersion20100801.<OperationName>` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! It lives here so each suite under `tests/all/` reaches the service exactly the way the
//! production router does, through ONE definition of the request walk.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The `X-Amz-Target` prefix this service's model declares.
pub const TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// One emulator with this service mounted, reached exactly the way the production router
/// reaches it.
pub fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's concern, not this service's; turning it off
    // keeps the suite independent of how the process was launched.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call one operation and return its HTTP status and parsed body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        // The SigV4 credential scope is what routes the request to the `monitoring`
        // signing name.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/monitoring/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one operation, requiring a 200, and return its body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with body {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), without any
/// namespace prefix.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

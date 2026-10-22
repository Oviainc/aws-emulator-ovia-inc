// SPDX-License-Identifier: CC0-1.0

//! The shared wire harness every suite in this crate drives requests through.
//!
//! Each request goes through the real restJson1 front door —
//! [`MultiServiceRouter::dispatch`] — so routing, URI/query binding, constraint
//! validation, the handler, and response serialization are all exercised. The
//! state lives in one shared engine for the whole router, so a value written by
//! one request is read back by the next.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The region the requests below are signed for, unless a case says otherwise.
/// It is deliberately NOT the emulator's fallback region, so an ARN that ignored
/// the request's credential scope would be visible.
pub const REGION: &str = "eu-west-2";

/// The account every emulated ARN carries.
pub fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// A router with just this service mounted on one shared engine.
pub fn scheduler_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests drive the wire, not SigV4: the Authorization header below is
    // only how a request names the service and its region.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(engine),
    );
    router
}

/// A request addressed to `scheduler` in `region`, the way a signed SDK call is.
pub fn request(
    method: &str,
    uri: &str,
    region: &str,
    body: Option<serde_json::Value>,
) -> http::Request<bytes::Bytes> {
    let credential = format!("AKIDTESTTESTTEST/20260101/{region}/scheduler/aws4_request");
    let body = body.map_or_else(bytes::Bytes::new, |json| {
        bytes::Bytes::from(json.to_string())
    });
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={credential}, SignedHeaders=host, Signature=abc123"
            ),
        )
        .header("content-type", "application/json")
        .body(body)
        .expect("a well-formed test request")
}

/// Dispatch and return `(status, parsed body)`. An empty body parses as `null`.
pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    call_in(router, method, uri, REGION, body).await
}

/// Dispatch a request signed for `region`, and return `(status, parsed body)`.
pub async fn call_in(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let response = router.dispatch(request(method, uri, region, body)).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// The wire error code restJson1 puts in `__type`, without its namespace.
pub fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("")
        .to_string()
}

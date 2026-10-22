// SPDX-License-Identifier: CC0-1.0

//! The restJson1 wire harness every MediaLive suite in this crate drives the
//! service through.
//!
//! One copy, so the suites cannot disagree about how a request reaches the
//! emulator: each test builds a real HTTP request and dispatches it through
//! `MultiServiceRouter`, so what is asserted is the emulator's wire behaviour,
//! not an internal function's return value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// The region [`call`] signs for, and so the region every ARN those requests
/// mint must name. A suite whose rule is scoped to a region signs for one of its
/// own with [`call_in_region`].
pub const DEFAULT_REGION: &str = "us-west-2";

/// A SigV4 credential scope naming `medialive`, which is how the router decides
/// this service claims the request, and `region`, which is the region the
/// request's resources live in.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/medialive/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// The account segment of every ARN a generated emulator emits
/// (`emulator_server::EMULATOR_ACCOUNT_ID`).
pub const ACCOUNT: &str = "000000000000";

/// A router carrying this service alone, on its own engine, so each test starts
/// from an empty store.
pub fn medialive() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These suites exercise service behaviour, not request signing.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_medialive_medialive::EmulatedService::new(engine),
    );
    router
}

/// Dispatch one request signed for [`DEFAULT_REGION`] and read back its status
/// and JSON body.
pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(router, DEFAULT_REGION, method, uri, body).await
}

/// Dispatch one request signed for `region` and read back its status and JSON
/// body.
pub async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("medialive.{region}.amazonaws.com"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The `message` member of a restJson1 error body (`jsonName: "message"` on
/// every one of this service's error shapes), as a client reads it. One copy for
/// every suite, beside [`error_code`].
pub fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The error code a restJson1 error response carries, as a client reads it.
pub fn error_code(status: u16, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map_or_else(|| format!("<no __type, status {status}>"), ToString::to_string)
}

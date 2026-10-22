// SPDX-License-Identifier: CC0-1.0

//! The restJson1 wire harness every MediaLive suite in this crate drives the
//! service through.
//!
//! One copy, so `channel_lifecycle` and `sdi_source_lifecycle` cannot disagree
//! about how a request reaches the emulator: each test builds a real HTTP
//! request and dispatches it through `MultiServiceRouter`, so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming `medialive`, which is how the router decides
/// this service claims the request, and `us-west-2`, which is the region every
/// ARN the request mints must name.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20240101/us-west-2/medialive/aws4_request, \
     SignedHeaders=host, Signature=abc";

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

/// Dispatch one request and read back its status and JSON body.
pub async fn call(
    router: &MultiServiceRouter,
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
        .header("host", "medialive.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION)
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

/// The error code a restJson1 error response carries, as a client reads it.
pub fn error_code(status: u16, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map_or_else(|| format!("<no __type, status {status}>"), ToString::to_string)
}

// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate shares.
//!
//! Every assertion goes over the wire: an awsJson1_0 `POST /` with
//! `X-Amz-Target: VerifiedPermissions.<Operation>`, dispatched through the same
//! `MultiServiceRouter` the emulator binary serves, over ONE shared engine — so a
//! write made by one request is only visible to the next if it really reached
//! state.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A router serving only this service, over a fresh shared engine.
pub fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The same router, plus the engine it was built on — for a suite that has to
/// read or seed state no served request can reach. `router()` delegates here so
/// there is ONE place that decides how the harness is wired.
pub fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_verifiedpermissions_verifiedpermissions::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Call one operation and return `(http status, parsed body)`.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("VerifiedPermissions.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20260101/us-east-1/verifiedpermissions/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a test request body always serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one operation, requiring success, and return the parsed body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} failed: {parsed}");
    parsed
}

/// Create a store with the given validation `mode` and return its
/// `policyStoreId`. The ONE store-creation helper this test crate has; every
/// suite reaches a store through it.
pub async fn store_with_mode(router: &MultiServiceRouter, mode: &str) -> String {
    let created = ok(
        router,
        "CreatePolicyStore",
        &json!({ "validationSettings": { "mode": mode } }),
    )
    .await;
    created["policyStoreId"]
        .as_str()
        .expect("policyStoreId is required on the CreatePolicyStore response")
        .to_string()
}

/// Create a store with validation mode `OFF` and return its `policyStoreId`.
pub async fn store_with_validation_off(router: &MultiServiceRouter) -> String {
    store_with_mode(router, "OFF").await
}

/// The error code an awsJson1_0 fault carries in `__type`, without its namespace.
pub fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

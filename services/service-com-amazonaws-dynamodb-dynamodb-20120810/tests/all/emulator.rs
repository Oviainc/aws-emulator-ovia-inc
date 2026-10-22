// SPDX-License-Identifier: CC0-1.0

//! The emulator every suite in this test target drives, and the helpers they share.
//!
//! One harness rather than one per suite: a suite that needs the router or a request takes
//! it from here (`use crate::emulator::{..}`) so the way this crate is driven over the wire
//! is described once. The suites that still carry their own copy are welcome to adopt it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region a request is signed for when a suite does not care which one it is. A
/// DynamoDB table name is unique within ONE region, which is the scope the store keys by.
pub const REGION: &str = "us-east-1";

/// One emulator holding only this service, over one shared engine.
pub fn emulator() -> MultiServiceRouter {
    emulator_and_engine().0
}

/// The same emulator, together with the shared engine it was built on — for a suite that
/// must reach this service's STORED state (what a handler reaches through `self.engine`)
/// and not only its wire surface.
pub fn emulator_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suites drive unsigned requests, so signature verification is off; the
    // `Authorization` header below is still what names the region, exactly as a signed
    // client's would.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one operation in `region` and return its HTTP status and parsed body.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/dynamodb/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// [`call_in`] in [`REGION`], for a suite that is not about regions.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, REGION, operation, body).await
}

/// [`call`] for an operation that must succeed, returning its parsed body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The `GlobalSecondaryIndexes` entry of a description by index name, or `Null` when
/// that description carries no index by that name.
pub fn index_named(description: &Value, name: &str) -> Value {
    description["GlobalSecondaryIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|index| index["IndexName"] == serde_json::json!(name))
        .unwrap_or(Value::Null)
}

/// The wire error code of a refusal: the shape name the `__type` field carries, which
/// is what an SDK client matches an error on.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

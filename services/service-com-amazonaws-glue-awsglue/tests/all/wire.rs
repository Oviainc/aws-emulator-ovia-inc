// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the emulator through.
//!
//! One copy, shared: a suite makes its calls the way a client does — `POST /` with
//! `X-Amz-Target: AWSGlue.<Operation>` and an `awsJson1_1` body — through the same
//! `MultiServiceRouter::dispatch` the production front door uses, so what a suite
//! asserts is HTTP behaviour and not a direct call into a handler.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// This emulator's account id, which is also the default `CatalogId`.
pub const ACCOUNT_ID: &str = "000000000000";

/// The access key id every request here is signed with. The router routes on the
/// SigV4 credential scope's service field (`glue`), and `CreatedBy` is derived from
/// the key id, so both are pinned by this one header.
pub const AKID: &str = "AKIDGLUETEST";

/// A SigV4 `Authorization` header scoped to `glue`: the routing signal the router
/// reads first. Authentication is off on the test router, so the signature is not
/// verified — only the scope is read.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AKID}/20260101/us-east-1/glue/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// An emulator with only this service mounted, on one shared engine, with request
/// authentication off (the test sends no real signature).
pub fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The same, on an engine the caller holds — so a test can seed the store directly
/// instead of driving thousands of requests to reach a capacity ceiling.
pub fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        engine,
    ));
    router
}

/// Make one awsJson1_1 call and return its HTTP status and parsed body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSGlue.{operation}"))
        .header("authorization", authorization())
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Make one call that must succeed, and return its body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// The awsJson error code of a failure body (`__type`), which may carry a namespace.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert that every member of `sent` reappears, unchanged, on `got`. Recurses into
/// nested structures so a member the server fills with its own default (an absent
/// `Compressed`, say) is not read as a mismatch.
pub fn assert_echoed(case: &str, sent: &Value, got: &Value) {
    let members = sent.as_object().expect("the sent value is a structure");
    for (member, value) in members {
        let actual = got.get(member);
        if value.is_object() {
            let Some(actual) = actual else {
                panic!("case {case}: member {member} is absent from the response {got}");
            };
            assert_echoed(case, value, actual);
        } else {
            assert_eq!(
                actual,
                Some(value),
                "case {case}: member {member} must round-trip; expected {value}, got {actual:?}"
            );
        }
    }
}

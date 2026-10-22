// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate shares: one emulator over one shared
//! engine, and a signed awsJson1_0 call into it.
//!
//! Extracted here so a new suite does not copy it: the suites differ in the rules they
//! assert, not in how they reach the service.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::EmulatedService;

/// The `X-Amz-Target` prefix every request to this awsJson1_0 service carries (see the
/// crate's `OPERATIONS.md`).
const TARGET_PREFIX: &str = "AWSKendraRerankingFrontendService.";

/// One emulator holding this service, over the single shared engine the router builds.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the modeled behaviour, not signature verification.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call `operation` with `body`, signed for `region`, and return its status and body.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20240101/{region}/kendra-ranking/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|_| {
            json!({ "unparsed": String::from_utf8_lossy(response.body()).to_string() })
        })
    };
    (status, parsed)
}

/// [`call_in`] for the emulator's default region.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, "us-east-1", operation, body).await
}

/// The wire error code of an error body (`__type`), without any namespace prefix.
pub fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or_default();
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// Create a plan named `name` and return its id.
pub async fn create_plan(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "CreateRescoreExecutionPlan", json!({ "Name": name })).await;
    assert_eq!(status, 200, "create {name}: unexpected answer {body}");
    body["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("create {name}: no Id in {body}"))
        .to_string()
}

// SPDX-License-Identifier: CC0-1.0

//! The one HTTP harness every suite in this test binary drives the service through.
//!
//! Each suite is a `mod` of `tests/all.rs`, so they share this module rather than each
//! bootstrapping its own router: a request here goes over the service's real wire
//! protocol — `awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSInsightsIndexService.<Operation>` — through the same
//! `MultiServiceRouter` the production router uses, so nothing a suite asserts reaches
//! past the HTTP surface a client sees.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_costexplorer_awsinsightsindexservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` service prefix from the model (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AWSInsightsIndexService";

/// One emulator, driven over HTTP.
pub struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub fn start() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests send unsigned requests, so signature verification is off; routing
        // still goes through the production `can_handle`/routing table.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` with `body`, returning the HTTP status and the decoded JSON.
    pub async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            // The routing key: the SigV4 credential scope names `ce`, this service's
            // signing name — the same signal an AWS SDK request carries. Verification is
            // off for these tests, so the signature itself is not read.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEMULATORTEST/20240101/us-east-1/ce/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0",
            )
            .header("host", "ce.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let decoded = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered {status} with a body that is not JSON: {error}: {}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, decoded)
    }

    /// Call `operation`, requiring a 200.
    pub async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, decoded) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must succeed, got {status}: {decoded}"
        );
        decoded
    }
}

/// The error code an awsJson error envelope carries, without its namespace.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` an error envelope carries, under either casing.
pub fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

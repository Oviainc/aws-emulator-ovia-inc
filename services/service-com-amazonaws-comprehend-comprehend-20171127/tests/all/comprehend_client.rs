// SPDX-License-Identifier: CC0-1.0

//! The one HTTP client the Comprehend suites drive the service through.
//!
//! Every suite goes over the wire the service actually serves — `awsJson1_1`,
//! `POST /` with `X-Amz-Target: Comprehend_20171127.<OperationName>` — through the
//! same `MultiServiceRouter` the emulator's front doors dispatch on, so a test
//! exercises routing, deserialization, constraint validation, the handler and
//! serialization end to end.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_comprehend_comprehend_20171127::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (OPERATIONS.md, "Wire protocol").
const TARGET_PREFIX: &str = "Comprehend_20171127.";

/// The region the test's SigV4 credential scope names. Deliberately not the
/// emulator's default, so an ARN that carries it proves the region came from the
/// request rather than from a constant.
pub const REGION: &str = "eu-west-1";

/// The account id every ARN a generated emulator mints carries.
pub const ACCOUNT_ID: &str = "000000000000";

/// One emulator holding one Comprehend service over one shared engine.
pub struct Comprehend {
    router: MultiServiceRouter,
}

impl Comprehend {
    /// A fresh emulator with an empty store.
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suites assert on service behaviour, not on signature verification, and
        // sign with a fixed scope so the region is deterministic.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` with `body`, returning the HTTP status and the parsed JSON
    /// response (`Value::Null` for an empty body).
    pub async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/comprehend/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "{operation}: response body is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, parsed)
    }

    /// Call `operation` expecting success, returning the parsed output.
    pub async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation}: expected 200, got {status}: {parsed}");
        parsed
    }
}

/// The wire error code of a failure response (`__type` on the awsJson protocols),
/// with any `namespace#` prefix stripped.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives the service through: the
//! production multi-service router with this one service mounted on an engine the
//! test also holds.
//!
//! It lives here rather than in one suite so a new suite reuses it instead of
//! copying it. Operation-specific helpers stay in the suite that needs them,
//! added to [`Emulator`] through an inherent `impl` block there.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_notifications_notifications::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service, which is what the router's
/// routing key is resolved from. Request authentication is off in these tests,
/// so no real signature is needed — only the scope that names `notifications`.
const CREDENTIAL_SCOPE: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/notifications/aws4_request, SignedHeaders=host, Signature=0";

/// The emulator under test: the production router with this one service mounted
/// on an engine the test also holds, so a test can read the store directly for
/// state no operation reports yet.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null`
    /// for an empty body).
    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let payload = match &body {
            Some(value) => {
                bytes::Bytes::from(serde_json::to_vec(value).expect("a serializable body"))
            }
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "notifications.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header("authorization", CREDENTIAL_SCOPE)
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body().clone();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {uri} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        (status, parsed)
    }
}

/// Percent-encode one URI path segment, the way an SDK encodes a non-greedy
/// `@httpLabel`: an arn's `:` and `/` must not be read as segment structure.
pub(crate) fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The error code of an error response, from the restJson1 `__type` member.
pub(crate) fn error_code(response: &Value) -> String {
    response["__type"].as_str().unwrap_or_default().to_string()
}

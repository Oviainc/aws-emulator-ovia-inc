// SPDX-License-Identifier: CC0-1.0

//! The WIRE harness every suite in this test target calls the emulator through.
//!
//! One place that knows how a request reaches this service — the restJson1 POST, the
//! SigV4 credential scope `MultiServiceRouter` routes on and the handlers read the
//! region from, and how an answer is parsed — so a suite below says only what it is
//! asserting. Declared in `tests/all.rs` like any other suite; it holds no tests of its
//! own.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suites sign for, so the ARNs they mint carry it.
pub const REGION: &str = "us-east-1";

/// Another region, so account-wide reach can be told apart from per-region state.
pub const OTHER_REGION: &str = "us-west-2";

/// The access key id the suites sign with unless they name another: a key neither STS nor
/// IAM holds a record for, which is the position AWS account root credentials are in — so
/// the authorization gate (`service::authorization`) does not bound it and a suite that is
/// not about WHO called reads the operation's own answer.
pub const DEFAULT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// A SigV4 credential scope naming this service and `region`, presenting `access_key_id`;
/// `MultiServiceRouter` routes on the scope's service name, the handlers read the region
/// from it, and the authorization gate resolves the principal from the key id.
pub fn authorization_as(access_key_id: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{region}/chatbot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One answer from the emulator: the status, the `x-amzn-errortype` header a modelled
/// failure carries, and the parsed body (`Value::Null` for an empty body, as a 204 has).
pub struct Answer {
    pub status: u16,
    pub error_type: Option<String>,
    pub body: Value,
}

impl Answer {
    /// The lowercase `message` member of an error body.
    pub fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message member>")
            .to_string()
    }

    /// The CAPITALISED `Message` member of an error body.
    ///
    /// Which spelling an error carries is the model's: `ResourceNotFoundException` declares
    /// `Message` while `ConflictException` declares `message` (`smithy_query`
    /// shape=ResourceNotFoundException,ConflictException), so a suite asks for the one its
    /// error class declares.
    pub fn message_capitalized(&self) -> String {
        self.body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or("<no Message member>")
            .to_string()
    }

    /// The error code an error body names (`__type`, which is how restJson1 conveys the
    /// error shape in the body) — the counterpart of [`Answer::message`].
    pub fn code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .to_string()
    }
}

/// POST `body` to `path` on `router`, signed for `region`.
pub async fn post(
    router: &MultiServiceRouter,
    region: &str,
    path: &str,
    body: &Value,
) -> Answer {
    post_as(router, DEFAULT_ACCESS_KEY_ID, region, path, body).await
}

/// POST `body` to `path` on `router`, signed for `region` with `access_key_id` — which is
/// what names the principal the request speaks for.
pub async fn post_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    region: &str,
    path: &str,
    body: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://chatbot.{region}.amazonaws.com{path}"))
        .header("host", format!("chatbot.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization_as(access_key_id, region))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{path}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// This service mounted alone over one shared engine — the same assembly the production
/// router performs, minus the other services. The engine is kept so a suite can reach a
/// state seam (workspace authorization) the model gives no operation for.
pub struct Emulator {
    pub router: MultiServiceRouter,
    pub engine: Arc<engine::InMemoryEngine>,
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(chatbot::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// POST `body` to `path`, signed for `region`.
    pub async fn call_in_region(&self, region: &str, path: &str, body: &Value) -> Answer {
        post(&self.router, region, path, body).await
    }

    /// POST `body` to `path`, signed for [`REGION`].
    pub async fn call(&self, path: &str, body: &Value) -> Answer {
        self.call_in_region(REGION, path, body).await
    }
}

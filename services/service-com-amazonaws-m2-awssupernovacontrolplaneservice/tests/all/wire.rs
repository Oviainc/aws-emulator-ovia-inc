// SPDX-License-Identifier: CC0-1.0

//! The one wire harness this crate's suites drive m2 through: a router over one shared
//! engine, one buffered restJson1 request, and the answer off the wire.
//!
//! Shared rather than per suite so two suites cannot read the same response differently —
//! one asserting on `x-amzn-errortype` and the other not — and so the request a test sends
//! is built in one place: the same front door a client uses
//! ([`MultiServiceRouter::dispatch`]), with the URI bindings, the refusal envelope and the
//! error-type header all exercised.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_m2_awssupernovacontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here signs for.
pub(crate) const REGION: &str = "eu-west-1";

/// A SigV4 `Authorization` header naming m2 in [`REGION`]: it is what routes the request to
/// this service. Signatures are not checked unless a test turns authentication on, so the
/// signature bytes are filler.
pub(crate) const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTM2EXAMPLE/20260101/eu-west-1/m2/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One answer off the wire: the status, the `x-amzn-errortype` header restJson1 resolves an
/// error shape from, and the parsed body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

impl Answer {
    /// The error code the body carries (`__type`), which must agree with the header.
    pub(crate) fn body_type(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
    }

    /// The `message` member, or a marker when the body carries none.
    pub(crate) fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }
}

/// A router serving ONE m2 service over ONE shared engine — the assembly `core/router`
/// performs, minus the other services. `authenticated` turns request authentication on.
pub(crate) fn router(authenticated: bool) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    router_over(engine, authenticated)
}

/// As [`router`], over an engine the caller already holds — so two routers can serve the
/// same store (a request denied by one, read back through the other).
pub(crate) fn router_over(
    engine: Arc<engine::InMemoryEngine>,
    authenticated: bool,
) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(authenticated);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request. `authorization` is the header value to send, if any — absent is
/// itself a case for the authentication rules, and the request still reaches m2 because the
/// `Host` header names its endpoint.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
    authorization: Option<&str>,
) -> Answer {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://m2.{REGION}.amazonaws.com{path_and_query}"))
        .header("host", format!("m2.{REGION}.amazonaws.com"));
    if let Some(value) = authorization {
        request = request.header("authorization", value);
    }
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let bytes = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("a test body always serializes"))
    });
    let response = router
        .dispatch(request.body(bytes).expect("a valid test request"))
        .await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<no x-amzn-errortype>")
        .to_string();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body: parsed,
    }
}

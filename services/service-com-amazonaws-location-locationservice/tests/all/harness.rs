// SPDX-License-Identifier: CC0-1.0

//! The one HTTP harness this crate's suites drive the emulator through.
//!
//! Every suite sends its requests with [`call`] so each call is a separate HTTP
//! request against one shared engine, dispatched by the production router: a value
//! that survives from one call to the next survived because it was PERSISTED, not
//! because a handler echoed its own input back.
//!
//! Declared from `tests/all.rs`, so a suite reaches it as `crate::harness`.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_location_locationservice::EmulatedService;

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
pub const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the tests' SigV4 credential scope names, and therefore the region the
/// generated ARNs must carry.
pub const REGION: &str = "us-west-2";

/// One emulator: a shared engine with this service mounted on the production
/// router, so the tests drive the same dispatch path the binary does.
pub fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, plus a handle on the engine, for the tests that assert a
/// bound on what the store holds rather than on a response.
pub fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests assert behaviour, not signatures; the emulator verifies signatures
    // itself and a service never checks them.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// What the emulator answered.
pub struct Answer {
    pub status: u16,
    pub error_type: Option<String>,
    pub body: String,
}

impl Answer {
    pub fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {:?}", self.body))
    }

    /// The wire error code a client branches on: the `x-amzn-errortype` header the
    /// restJson1 envelope carries, falling back to the body's `__type`.
    pub fn error_code(&self) -> String {
        let raw = self.error_type.clone().unwrap_or_else(|| {
            self.json()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        // `x-amzn-errortype` may be `shape`, `ns#shape` or `shape:url`.
        raw.rsplit('#')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// A refusal's `message` member (every error shape in this service models it
    /// with `jsonName "message"`).
    pub fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Send one request to the emulator, signed into the `geo` credential scope so the
/// router's `can_handle` claims it for this service.
pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let bytes = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("test body serializes")),
        None => bytes::Bytes::new(),
    };
    call_in_region(router, method, path, bytes, REGION).await
}

/// Send one request whose SigV4 credential scope names `region`.
pub async fn call_in_region(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: bytes::Bytes,
    region: &str,
) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/geo/aws4_request, \
         SignedHeaders=host, Signature=abc"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://geo.{region}.amazonaws.com{path}"))
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(body)
        .expect("test request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = String::from_utf8_lossy(response.body()).to_string();
    Answer {
        status,
        error_type,
        body,
    }
}

/// Epoch milliseconds of an ISO 8601 timestamp member (`timestampFormat: date-time`).
///
/// Every timestamp member of this model targets one shape and that shape declares
/// `date-time`, so the member is an ISO 8601 STRING on the wire and this fails on any
/// other rendering: the suites that compare instants pin the form too.
pub fn millis_of(value: &Value, member: &str) -> i64 {
    let found = value
        .get(member)
        .unwrap_or_else(|| panic!("{member} must be present in {value}"));
    let text = found
        .as_str()
        .unwrap_or_else(|| panic!("{member} must be an ISO 8601 string, got {found}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("{member} {text:?} must parse as ISO 8601: {e}"))
        .to_millis()
        .unwrap_or_else(|e| panic!("{member} {text:?} must fit in epoch millis: {e}"))
}

/// Epoch milliseconds of an ISO 8601 literal written in a test table.
pub fn iso_millis(iso: &str) -> i64 {
    DateTime::from_str(iso, Format::DateTime)
        .unwrap_or_else(|e| panic!("the table's timestamp {iso:?} must be ISO 8601: {e}"))
        .to_millis()
        .expect("epoch millis")
}

/// The wire form of a timestamp inside a JSON body.
///
/// Every timestamp member of this model targets one shape and that shape carries
/// `@timestampFormat("date-time")`, so a body timestamp is the ISO 8601 STRING the
/// tables are written in. Kept as a seam so a table reads as the API documents it
/// while the request still speaks the wire form.
pub fn wire_time(iso: &str) -> Value {
    json!(iso)
}

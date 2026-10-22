// SPDX-License-Identifier: CC0-1.0

//! Shared harness for the Performance Insights suites: a one-service emulator
//! driven over this service's real wire protocol (`awsJson1_1`, `POST /`,
//! `X-Amz-Target: PerformanceInsightsv20180227.<Operation>`), so a test exercises
//! the same deserialize -> validate -> handle -> serialize walk a client does.

#![allow(dead_code)]

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service answers (README.md / OPERATIONS.md).
const TARGET_PREFIX: &str = "PerformanceInsightsv20180227";

/// The region [`AUTHORIZATION`]'s credential scope names — and so the region the
/// ARNs this service mints for a request of this harness sit in. Keep the two in
/// step.
pub const REGION: &str = "us-west-2";

/// A SigV4 credential scope naming the `pi` signing name: that is the signal the
/// router resolves a service from, because an `X-Amz-Target` prefix names the
/// target (`PerformanceInsightsv20180227`), not the signing name. Request
/// authentication is switched off below, so the signature itself is never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/pi/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=00";

/// A fresh emulator holding only this service, over one shared engine.
pub fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the engine it was built on.
///
/// For a suite that must seed a store ANOTHER service writes — the shared
/// data-source directory the database control planes record a minted DB resource id
/// in — which a one-service emulator has no operation to write.
pub fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pi_performanceinsightsv20180227::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One answer off the wire: the HTTP status and the parsed JSON body.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub body: Value,
}

impl Answer {
    /// The wire error code (`__type`), without any namespace or shape prefix.
    /// Empty for a success.
    pub fn error_code(&self) -> String {
        let raw = self
            .body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
    }

    /// The error message, for a failure that carries one.
    pub fn error_message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member of a successful response body.
    pub fn member(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }
}

/// Call one operation and read the answer.
pub async fn call(emulator: &MultiServiceRouter, operation: &str, input: Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "pi.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(Bytes::from(input.to_string()))
        .expect("the harness builds a well-formed request");

    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&bytes).to_string())
        })
    };
    Answer { status, body }
}

/// Whether `id` satisfies the modelled `AnalysisReportId` pattern
/// `^report-[0-9a-f]{17}$`.
pub fn is_modelled_report_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("report-") else {
        return false;
    };
    suffix.len() == 17 && suffix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `token` satisfies the modelled `NextToken` pattern
/// `^[a-zA-Z0-9_=-]+$` and its `length` bound of 1..8192.
pub fn is_modelled_next_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 8192
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'=' || b == b'-')
}

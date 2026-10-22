// SPDX-License-Identifier: CC0-1.0

//! The wire harness this crate's suites share: one emulator with an empty
//! store, addressed through the SAME front door the production router is.
//!
//! Every case in every suite drives a `MultiServiceRouter` with this service
//! mounted, addressed by the `xray` endpoint host and speaking restJson1, so the
//! assertions are about HTTP responses rather than internal state.

#![allow(dead_code)]

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// One emulator with an empty store.
pub struct Xray {
    router: MultiServiceRouter,
}

/// The region every call lands in unless a case names another, which is also
/// the region the service falls back to when a request names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// What one call answered.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    /// restJson1 resolves an error shape from `x-amzn-errortype`.
    pub error_type: Option<String>,
    pub body: Value,
}

impl Answer {
    /// A member of the response body, or `Value::Null`.
    pub fn get(&self, member: &str) -> &Value {
        self.body.get(member).unwrap_or(&Value::Null)
    }

    /// The refusal message, whichever casing rendered it: a framework refusal
    /// renders `message`, a modelled error shape its member's own `Message`.
    pub fn message(&self) -> Option<&str> {
        self.get("message")
            .as_str()
            .or_else(|| self.get("Message").as_str())
    }
}

impl Xray {
    pub fn new() -> Self {
        Self::with_engine().0
    }

    /// One emulator and the engine it was mounted on, for a test that has to
    /// seed state no operation in the API writes.
    pub fn with_engine() -> (Self, Arc<engine::InMemoryEngine>) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
            Arc::clone(&engine),
        ));
        // These tests exercise handlers, not SigV4; the signing name still has to
        // route, which the `Host` header below does.
        router.set_request_authentication(false);
        (Self { router }, engine)
    }

    pub async fn call(&self, path: &str, body: &Value) -> Answer {
        self.call_in_region(DEFAULT_REGION, path, body).await
    }

    /// The same call, signed for `region`.
    ///
    /// The region a handler works in comes from the request's SigV4 credential
    /// scope, so this is how a case addresses another region: the
    /// `Authorization` header carries the scope even though signature
    /// verification is off.
    pub async fn call_in_region(&self, region: &str, path: &str, body: &Value) -> Answer {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://xray.{region}.amazonaws.com{path}"))
            .header("host", format!("xray.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20240101/{region}/xray/aws4_request, \
                     SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(::bytes::Bytes::from(body.to_string()))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Answer {
            status,
            error_type,
            body,
        }
    }

    pub async fn put_segments(&self, documents: &[String]) -> Answer {
        self.call("/TraceSegments", &json!({ "TraceSegmentDocuments": documents }))
            .await
    }

    pub async fn summaries(&self, start: f64, end: f64, token: Option<&str>) -> Answer {
        let mut body = json!({ "StartTime": start, "EndTime": end });
        if let Some(token) = token {
            body["NextToken"] = json!(token);
        }
        self.call("/TraceSummaries", &body).await
    }

    /// `GetTraceSummaries` over `[start, end)` with a filter expression.
    pub async fn filtered(&self, start: f64, end: f64, filter: &str) -> Answer {
        self.call(
            "/TraceSummaries",
            &json!({ "StartTime": start, "EndTime": end, "FilterExpression": filter }),
        )
        .await
    }

    pub async fn batch_get(&self, ids: &[String], token: Option<&str>) -> Answer {
        let mut body = json!({ "TraceIds": ids });
        if let Some(token) = token {
            body["NextToken"] = json!(token);
        }
        self.call("/Traces", &body).await
    }

    pub async fn get_groups(&self, token: Option<&str>) -> Answer {
        let mut body = json!({});
        if let Some(token) = token {
            body["NextToken"] = json!(token);
        }
        self.call("/Groups", &body).await
    }

    /// `GetIndexingRules`, with `NextToken` sent only when `token` is `Some`.
    pub async fn get_rules(&self, token: Option<&str>) -> Answer {
        let mut body = json!({});
        if let Some(token) = token {
            body["NextToken"] = json!(token);
        }
        self.call("/GetIndexingRules", &body).await
    }

    /// `UpdateIndexingRule` with a probabilistic configuration.
    pub async fn update_rule(&self, name: &str, percentage: f64) -> Answer {
        self.call(
            "/UpdateIndexingRule",
            &json!({
                "Name": name,
                "Rule": { "Probabilistic": { "DesiredSamplingPercentage": percentage } },
            }),
        )
        .await
    }
}

/// Floating-point seconds compare: the values travel as JSON doubles.
pub fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-6
}

/// A 35-character X-Ray trace id (version, 8 hex, 24 hex), distinct per `n`.
///
/// The widths are fixed, so the id stays inside `TraceId`'s modelled `@length`
/// 1..35 for every `n`, and sorting the ids orders them by `n` — which is what
/// the pagination cases rely on.
pub fn trace_id(n: u32) -> String {
    format!("1-{n:08x}-{n:024x}")
}

/// The JSON array member of `value`, or an empty slice.
pub fn array<'a>(value: &'a Value, member: &str) -> &'a [Value] {
    value
        .get(member)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// A root segment document with the given times, in progress when `end` is
/// absent.
pub fn root_document(id: &str, trace: &str, name: &str, start: f64, end: Option<f64>) -> Value {
    let mut document = json!({
        "trace_id": trace,
        "id": id,
        "name": name,
        "start_time": start,
    });
    match end {
        Some(end) => document["end_time"] = json!(end),
        None => document["in_progress"] = json!(true),
    }
    document
}

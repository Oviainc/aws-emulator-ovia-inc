// SPDX-License-Identifier: CC0-1.0

//! The one HTTP harness this crate's suites share: a router mounting the service,
//! and the request helpers each operation is driven through.
//!
//! Every case in every suite goes through here, so what the suites assert is the
//! emulator's wire behaviour — a `restJson1` request through the
//! `MultiServiceRouter` into the service's own handlers — never a handler called
//! directly. It lives in one module so the two suites cannot drift into two
//! different ideas of what a request looks like.

use serde_json::{json, Value};
use service_com_amazonaws_uxc_awsaccountuxsetting::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A SigV4 `Authorization` header whose credential scope names this service, which
/// is the routing key `MultiServiceRouter` dispatches on. The signature itself is
/// never checked here: request authentication is off below.
const SIGV4_UXC: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/uxc/aws4_request, SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A router mounting one freshly-stated copy of the service.
pub fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Explicit, so the suite does not depend on the process-wide auth switch.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Percent-encode a query-parameter value: a page token is base64, whose `+`, `/`
/// and `=` would otherwise be read as query syntax (or as a space).
pub fn encode_query_value(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Send one request and return `(status, error-type, body)`. `error-type` is the
/// `x-amzn-errortype` header restJson1 resolves an error shape from.
pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let raw = body
        .map(|b| serde_json::to_vec(&b).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "uxc.us-east-1.amazonaws.com")
        .header("authorization", SIGV4_UXC)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(raw))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, error_type, value)
}

pub async fn get_customizations(router: &MultiServiceRouter) -> (u16, String, Value) {
    call(router, "GET", "/v1/account-customizations", None).await
}

pub async fn update_customizations(
    router: &MultiServiceRouter,
    body: Value,
) -> (u16, String, Value) {
    call(router, "PATCH", "/v1/account-customizations", Some(body)).await
}

/// The `ListServices` request URI for one page request.
pub fn list_services_uri(max_results: Option<i64>, next_token: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={}", encode_query_value(token)));
    }
    if query.is_empty() {
        "/v1/services".to_string()
    } else {
        format!("/v1/services?{}", query.join("&"))
    }
}

/// One page of `ListServices`: `(services, nextToken)`.
pub async fn list_page(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let uri = list_services_uri(max_results, next_token);

    let (status, _, body) = call(router, "GET", &uri, None).await;
    assert_eq!(status, 200, "ListServices {uri}: expected 200, got {status} with body {body}");
    let services = body
        .get("services")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListServices {uri}: no `services` array in {body}"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("ListServices {uri}: non-string service in {body}"))
                .to_string()
        })
        .collect();
    let next = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (services, next)
}

/// The whole catalog, as the single page an unsized request returns.
pub async fn whole_catalog(router: &MultiServiceRouter) -> Vec<String> {
    let (services, next) = list_page(router, None, None).await;
    assert!(
        next.is_none(),
        "an unsized page holding the whole catalog must not carry a nextToken, got {next:?}"
    );
    services
}

/// A service identifier legal under the model's `Service` member pattern
/// `^[a-z0-9]+(-[a-z0-9]+)*$` (`smithy_query(query="shape", shape="ServiceList")`)
/// and NOT a member of the catalog `ListServices` pages over, whose entries are
/// all real service identifiers.
pub fn generated_service_id(n: usize) -> String {
    format!("svc-{n}")
}

/// A Region code legal under the model's `Region` member pattern
/// `^[a-z]{2}(-[a-z]{1,10}){1,2}-[1-9]$` (`smithy_query(query="shape", shape="RegionsList")`).
pub fn generated_region(n: usize) -> String {
    let first = char::from(b'a' + (n / 26) as u8);
    let second = char::from(b'a' + (n % 26) as u8);
    format!("eu-{first}{second}-1")
}

pub fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| json!(v)).collect())
}

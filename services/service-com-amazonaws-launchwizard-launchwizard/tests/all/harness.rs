// SPDX-License-Identifier: CC0-1.0

//! The shared HTTP harness every suite in this crate drives the service through.
//!
//! Extracted from `deployment_lifecycle.rs` so a second suite reaches the same
//! router, the same signing region and the same pagination walker rather than
//! copying them: one dispatch path means two suites cannot disagree about how a
//! request reaches the service.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these suites sign with, so an ARN built from the request's SigV4
/// credential scope is observably that region rather than a default.
pub const REGION: &str = "eu-west-1";

/// One response, reduced to what the assertions read.
pub struct Reply {
    pub status: u16,
    /// The wire error code, from `x-amzn-errortype` or the body's `__type`.
    pub error_code: Option<String>,
    pub body: Value,
}

/// A router carrying only this service, over one shared engine — the same dispatch
/// path the production emulator uses.
pub struct Harness {
    router: MultiServiceRouter,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are the emulator's own concern and are not what these suites are
        // about; turn the check off for this router only.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// `POST path` with a JSON body — the shape most of this service's operations
    /// take.
    pub async fn call(&self, path: &str, body: &Value) -> Reply {
        self.send("POST", path, Some(body)).await
    }

    /// Dispatch one request with the given HTTP method and request target, so a suite
    /// can reach an operation whose model binds it to something other than a POST with
    /// a JSON payload (a `DELETE` with `@httpQuery` members, say). `body` is `None` for
    /// an operation that carries no payload at all, which is what a `GET`/`DELETE`
    /// binding sends.
    pub async fn send(&self, method: &str, target: &str, body: Option<&Value>) -> Reply {
        let payload = match body {
            Some(body) => serde_json::to_vec(body).expect("a test request body serializes"),
            None => Vec::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://launchwizard.{REGION}.amazonaws.com{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/launchwizard/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a test request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(short_code);
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        let error_code = header_code.or_else(|| {
            parsed
                .get("__type")
                .and_then(Value::as_str)
                .map(short_code)
        });
        Reply {
            status,
            error_code,
            body: parsed,
        }
    }

    /// Call `path`, requiring HTTP 200, and hand back the response body.
    pub async fn ok(&self, case: &str, path: &str, body: Value) -> Value {
        self.expect_ok(case, "POST", path, Some(&body)).await
    }

    /// [`Harness::ok`] for a call that names its own method and may carry no body.
    pub async fn expect_ok(
        &self,
        case: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Value {
        let reply = self.send(method, path, body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 from {method} {path}, got {} with body {}",
            reply.status, reply.body
        );
        reply.body
    }

    /// Call `path`, requiring the given HTTP status and wire error code.
    pub async fn refused(
        &self,
        case: &str,
        path: &str,
        body: Value,
        status: u16,
        code: &str,
    ) -> Value {
        self.expect_refused(case, "POST", path, Some(&body), status, code)
            .await
    }

    /// [`Harness::refused`] for a call that names its own method and may carry no
    /// body.
    pub async fn expect_refused(
        &self,
        case: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
        status: u16,
        code: &str,
    ) -> Value {
        let reply = self.send(method, path, body).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status} from {method} {path}, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some(code),
            "case {case:?}: expected error code {code:?}, got {:?} with body {}",
            reply.error_code,
            reply.body
        );
        reply.body
    }
}

/// The short name of a wire error code (`com.example#Boom` -> `Boom`).
pub fn short_code(raw: &str) -> String {
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// Every `workloadName` in the seeded catalogue, in the order `ListWorkloads` pages
/// them.
pub async fn catalogue(harness: &Harness) -> Vec<String> {
    let body = harness.ok("catalogue", "/listWorkloads", json!({})).await;
    body["workloads"]
        .as_array()
        .expect("ListWorkloads must report a workloads list")
        .iter()
        .map(|workload| {
            workload["workloadName"]
                .as_str()
                .expect("a catalogue entry must carry workloadName")
                .to_string()
        })
        .collect()
}

/// Walk every page of a paginated operation at `page_size`, returning the items in
/// order. Asserts the token contract: a token only when items remain, and resumption
/// exactly where the previous page ended.
pub async fn walk_pages(
    harness: &Harness,
    case: &str,
    path: &str,
    base: &Value,
    items_member: &str,
    page_size: usize,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request: Map<String, Value> = base
            .as_object()
            .cloned()
            .expect("a paginated request body is an object");
        request.insert("maxResults".to_string(), json!(page_size));
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let body = harness.ok(case, path, Value::Object(request)).await;
        let page = body[items_member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: {path} reported no {items_member}: {body}"))
            .clone();
        assert!(
            page.len() <= page_size,
            "case {case:?}: page {pages} of {path} held {} items, more than maxResults {page_size}",
            page.len()
        );
        collected.extend(page.iter().cloned());
        pages += 1;
        assert!(
            pages < 64,
            "case {case:?}: {path} did not terminate after {pages} pages"
        );
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => {
                assert_eq!(
                    page.len(),
                    page_size,
                    "case {case:?}: {path} handed back a nextToken after a short page of {} items",
                    page.len()
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    collected
}

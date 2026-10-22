// SPDX-License-Identifier: CC0-1.0

//! The shared harness every suite in this crate drives the service through.
//!
//! One in-process emulator behind a real `restJson1` request/response walk: a test
//! asserts on the HTTP answer only, never by reaching into `crate::state`, so a handler
//! that answered from a hardcoded value instead of from the store fails. Every suite
//! (`asset_lifecycle`, `asset_types`, `domain_project_lifecycle`, `glossary_lifecycle`,
//! `update_project`) reuses this one instead of copying it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The span `crate::state::LIFECYCLE_SETTLE_MS` gives a domain lifecycle step, plus
/// slack, so a test that waits for a settled domain is not racing the clock.
pub(crate) const SETTLED: std::time::Duration = std::time::Duration::from_millis(600);

/// One emulator: a single shared engine behind the DataZone service, driven over HTTP.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

/// What a call answered: the HTTP status, the parsed body (`Value::Null` for an empty
/// one) and the wire error code when the answer was a failure.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
    pub(crate) error_code: String,
}

impl Answer {
    /// A member of the response body as a string, or `"<absent>"` so an assertion
    /// message says which member was missing.
    pub(crate) fn s(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
            .to_string()
    }
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_datazone_datazone::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Send one request. The `Authorization` header carries a SigV4 credential scope,
    /// which is both how the router picks this service and where the handlers read the
    /// caller's region and principal from.
    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map(|b| serde_json::to_vec(&b).expect("a JSON body serializes"));
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "datazone.us-west-2.amazonaws.com")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLETESTKEY/20261021/us-west-2/datazone/aws4_request, SignedHeaders=host, Signature=0",
            );
        if payload.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request
            .body(bytes::Bytes::from(payload.unwrap_or_default()))
            .expect("a valid request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if error_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            error_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    /// Create a domain and wait until it has settled to `AVAILABLE`, which is the
    /// precondition every project case needs.
    pub(crate) async fn available_domain(&self, name: &str) -> String {
        let created = self
            .call("POST", "/v2/domains", Some(json!({ "name": name })))
            .await;
        assert_eq!(
            created.status, 201,
            "creating domain {name:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        tokio::time::sleep(SETTLED).await;
        created.s("id")
    }

    /// Create a project in a settled domain — the precondition every asset case needs.
    pub(crate) async fn project(&self, domain_id: &str, name: &str) -> String {
        let created = self
            .call(
                "POST",
                &format!("/v2/domains/{domain_id}/projects"),
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "creating project {name:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        created.s("id")
    }
}

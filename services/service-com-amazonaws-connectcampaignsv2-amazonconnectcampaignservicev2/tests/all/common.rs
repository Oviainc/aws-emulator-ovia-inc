// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives its requests through.
//!
//! One copy of the routing header, the router under test and the request/response
//! plumbing, so a suite adds cases rather than another variant of the harness. The
//! router is the one the production entrypoint serves, so each case that goes
//! through [`call`] exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to this service's signing name, which is
/// what routes a request to it, and to `region` — the region its ARNs are rooted in.
pub fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/connect-campaigns/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    )
}

/// The router with only this service mounted, over a fresh engine, so each case
/// starts from an empty store.
pub fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// [`router`], plus the engine it was built over — for a case that reads the store
/// the handlers wrote to.
pub fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the core's concern, not this service's: these cases
    // assert handler behaviour, and the header above is a routing signal only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Drive one request and return its status and parsed JSON body (`Value::Null` for
/// an empty body). `body` of `Value::Null` sends no body at all.
pub async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(serde_json::to_vec(body).expect("a test body always serializes"))
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The error code a failure response carries (restJson1 puts it in `__type`).
pub fn error_code(response: &Value) -> &str {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

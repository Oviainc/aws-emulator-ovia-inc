// SPDX-License-Identifier: CC0-1.0

//! The request/response harness the workload-identity suites of this crate drive the
//! service with.
//!
//! One copy, so two suites cannot drift apart over how a request is signed, how a
//! response is read, what ARN an identity of this account carries or how a missing
//! identity is worded: a suite adds only the cases that are its own.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
pub const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
pub const ACCOUNT: &str = "000000000000";

/// The modelled bounds of `WorkloadIdentityNameType` (len 3..255, pattern
/// `^[A-Za-z0-9_.-]+$`), from `smithy_query operation=CreateWorkloadIdentity`.
pub const NAME_MIN_LENGTH: usize = 3;
pub const NAME_MAX_LENGTH: usize = 255;

/// `ListWorkloadIdentities`' modelled `maxResults` maximum (range 1..20), from
/// `smithy_query operation=ListWorkloadIdentities`.
pub const MAX_RESULTS_MAX: i64 = 20;

/// A router carrying only this service, over one shared engine.
pub fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on and
/// what the handlers read the ARN region from.
pub fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// Drive one request through the router the way the emulator does, and read its status and
/// body. A response carrying no body reads as [`Value::Null`].
pub async fn post(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let payload = Bytes::from(serde_json::to_vec(body).expect("the request body serializes"));
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com")
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

pub async fn create_identity(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    post(router, "/identities/CreateWorkloadIdentity", body).await
}

pub async fn get_identity(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    post(
        router,
        "/identities/GetWorkloadIdentity",
        &json!({"name": name}),
    )
    .await
}

pub async fn update_identity(
    router: &MultiServiceRouter,
    name: &str,
    urls: Option<&Vec<&str>>,
) -> (u16, Value) {
    let mut body = json!({"name": name});
    if let Some(urls) = urls {
        body["allowedResourceOauth2ReturnUrls"] = json!(urls);
    }
    post(router, "/identities/UpdateWorkloadIdentity", &body).await
}

pub async fn delete_identity(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    post(
        router,
        "/identities/DeleteWorkloadIdentity",
        &json!({"name": name}),
    )
    .await
}

pub async fn list_identities(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({});
    if let Some(max) = max_results {
        body["maxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        body["nextToken"] = json!(token);
    }
    post(router, "/identities/ListWorkloadIdentities", &body).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

pub fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `workloadIdentities` of a listing response.
pub fn listed(body: &Value) -> Vec<Value> {
    body.get("workloadIdentities")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `name` of every entry of a listing response, in the order reported.
pub fn listed_names(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|entry| text(entry, "name"))
        .collect()
}

/// The ARN an identity of this account carries: the token vault's `default`
/// workload-identity directory, ending in the identity's own name.
pub fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:workload-identity-directory/default/\
         workload-identity/{name}"
    )
}

/// What the read and update routes word a missing identity as.
pub fn not_found_by_id(name: &str) -> String {
    format!("WorkloadIdentity with id {name} not found.")
}

/// What the delete route words a missing identity as.
pub fn not_found_by_name(name: &str) -> String {
    format!("WorkloadIdentity with name {name} not found.")
}

/// Assert `status`/`body` is the modelled 404 `ResourceNotFoundException` carrying
/// `message` — the one place the suites agree what a miss looks like on the wire.
pub fn assert_not_found(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        (status, error_code(body), text(body, "message")),
        (
            404,
            "ResourceNotFoundException".to_string(),
            message.to_string()
        ),
        "case {case:?}: expected HTTP 404 ResourceNotFoundException {message:?}, got {status} {} \
         ({body})",
        error_code(body)
    );
}

/// Whether an answer carries no member at all: `DeleteWorkloadIdentity`'s output is an
/// empty structure, so a success holds nothing whichever envelope it renders in (no body,
/// or an empty object).
pub fn carries_no_member(body: &Value) -> bool {
    match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// An epoch-seconds instant off a response member. The prelude `Timestamp` carries no
/// `@timestampFormat`, so on this service's restJson1 wire each instant is a NUMBER.
pub fn instant(case: &str, body: &Value, member: &str) -> f64 {
    body[member].as_f64().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {member} as epoch-seconds, got {}",
            body[member]
        )
    })
}

/// How a stored `allowedResourceOauth2ReturnUrls` reads back on the wire: the list the
/// caller supplied, in that order, or no member at all when the identity carries none.
pub fn expected_url_list(urls: Option<&Vec<&str>>) -> Value {
    urls.map_or(Value::Null, |urls| json!(urls))
}

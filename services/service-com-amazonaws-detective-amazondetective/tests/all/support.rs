// SPDX-License-Identifier: CC0-1.0

//! The one HTTP harness every suite in this crate's integration test shares.
//!
//! Each suite drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what a
//! suite asserts is what a signed AWS SDK request observes. The pieces that are not
//! about any one rule live here: building a signed request, dispatching it, reading an
//! error code out of the answer, and the two request fragments (`CreateGraph`, one
//! `Accounts` entry) every suite needs before it can test anything.

#![allow(dead_code)]

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::ACCOUNT_ID_LEN;
use smithy_server_runtime::MultiServiceRouter;

/// The region most requests sign for; the ARNs they mint carry exactly this one.
pub const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and every
/// administrator field carries.
pub const ACCOUNT: &str = "000000000000";

/// One emulator with an empty store, mounted exactly as the router mounts it.
pub fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// As [`emulator`], and also the shared engine it was built from — for a suite that
/// has to read the stored record an operation wrote through an operation that is still
/// scaffolded.
pub fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_detective_amazondetective::EmulatedService::new(engine.clone()),
    );
    (router, engine)
}

/// A request signed for `detective` in `region`: the signal the router routes on and
/// the handlers read the region out of. A `Value::Null` body is sent as no body at
/// all, which is what a `GET` carries.
pub fn request_with(
    method: &str,
    region: &str,
    uri: &str,
    body: &Value,
) -> http::Request<bytes::Bytes> {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("serialize the request body")
    };
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("api.detective.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDETECTIVETEST/20260101/{region}/detective/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request")
}

/// A `POST` request, which every operation of this service but the two tag readers
/// uses.
pub fn request(region: &str, uri: &str, body: &Value) -> http::Request<bytes::Bytes> {
    request_with("POST", region, uri, body)
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub async fn call_with(
    router: &MultiServiceRouter,
    method: &str,
    region: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let response = router.dispatch(request_with(method, region, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// One `POST` call in `region`.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    call_with(router, "POST", region, uri, body).await
}

/// One `POST` call in [`REGION`].
pub async fn call(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    call_in(router, REGION, uri, body).await
}

/// The modelled error shape a failed call reported.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.detective#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` a failed call reported, however the envelope spells it.
pub fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// `CreateGraph` in `region`, which must succeed; answers the minted ARN.
pub async fn create_graph_ok(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> String {
    let (status, created) = call_in(router, region, "/graph", body).await;
    assert_eq!(
        status, 200,
        "create graph {body}: expected 200, got {status} {created:?}"
    );
    created
        .get("GraphArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("create graph {body}: response carried no GraphArn, got {created:?}")
        })
        .to_string()
}

/// An ISO8601 `date-time`, as the modelled `Timestamp` members render
/// (`2021-08-18T16:35:56.284Z`).
pub fn assert_iso8601(case: &str, value: Option<&Value>) -> String {
    let rendered = value
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an ISO8601 timestamp, got {value:?}"));
    assert!(
        rendered.ends_with('Z') && rendered.contains('T') && rendered.len() >= 20,
        "case {case:?}: expected an ISO8601 date-time, got {rendered:?}"
    );
    rendered.to_string()
}

/// A 12-digit account id (the modelled `AccountId`: length 12..12, `^[0-9]+$`).
pub fn account_id(n: u64) -> String {
    let id = format!("{:012}", 100_000_000_000u64 + n);
    assert_eq!(id.len(), ACCOUNT_ID_LEN, "generated account id {id:?}");
    id
}

/// One account entry of a `CreateMembers` request.
pub fn account(id: &str) -> Value {
    json!({ "AccountId": id, "EmailAddress": format!("member-{id}@example.com") })
}

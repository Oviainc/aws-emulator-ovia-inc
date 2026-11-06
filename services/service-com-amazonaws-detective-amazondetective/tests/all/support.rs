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

use serde_json::{json, Map, Value};
use service_com_amazonaws_detective_amazondetective::state::{ACCOUNT_ID_LEN, GRAPH_ID_LEN};
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

/// One `ListTagsForResource` call on `graph_arn` in `region`: status and parsed answer,
/// with nothing asserted — for a suite that is testing a refusal.
pub async fn list_tags(
    router: &MultiServiceRouter,
    region: &str,
    graph_arn: &str,
) -> (u16, Value) {
    call_with(
        router,
        "GET",
        region,
        &format!("/tags/{graph_arn}"),
        &Value::Null,
    )
    .await
}

/// The tag map `ListTagsForResource` reports for `graph_arn` in `region`, labelled with
/// the table row that asked for it — the one reader every tag suite in this crate checks
/// a write through. An untagged graph reports an EMPTY map, so the member has to be
/// present either way.
pub async fn tags_of_case(
    router: &MultiServiceRouter,
    region: &str,
    graph_arn: &str,
    case: &str,
) -> Map<String, Value> {
    let (status, body) = list_tags(router, region, graph_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({graph_arn}) expected 200, got {status} {body:?}"
    );
    body.get("Tags")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListTagsForResource({graph_arn}) carried no Tags, got {body:?}")
        })
}

/// As [`tags_of_case`], for a caller with no table row to name.
pub async fn tags_of(
    router: &MultiServiceRouter,
    region: &str,
    graph_arn: &str,
) -> Map<String, Value> {
    tags_of_case(router, region, graph_arn, "ListTagsForResource").await
}

/// Owned key/value pairs from a literal table row.
pub fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Owned keys from a literal table row.
pub fn keys(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|key| (*key).to_string()).collect()
}

/// Pairs as a JSON tag map — the shape `ListTagsForResource` reports and the shape the
/// `Tags` member of `CreateGraph`/`TagResource` carries.
pub fn tag_map(entries: &[(String, String)]) -> Map<String, Value> {
    entries
        .iter()
        .map(|(key, value)| (key.clone(), json!(value)))
        .collect()
}

/// A `GraphArn` built from its parts, satisfying the modelled pattern
/// `^arn:aws[-\w]{0,10}?:detective:[-\w]{2,20}?:\d{12}?:graph:[abcdef\d]{32}?$`
/// (`smithy_query(query="shape",shape="GraphArn")`) — for a suite that needs an ARN the
/// store never held.
pub fn graph_arn_of(partition: &str, region: &str, account: &str, id: &str) -> String {
    assert_eq!(
        id.len(),
        GRAPH_ID_LEN,
        "a graph id is the modelled pattern's {GRAPH_ID_LEN} hex characters"
    );
    format!("arn:{partition}:detective:{region}:{account}:graph:{id}")
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

/// The epoch-millisecond INSTANT a rendered ISO8601 `date-time` names, read back through
/// the SDK's own parser.
///
/// The ONE place a suite turns a reported timestamp into something comparable, because a
/// rendering carries its fractional seconds only when they are non-zero and with trailing
/// zeros trimmed (`…:11Z`, `…:11.1Z`, `…:11.284Z`): two renderings therefore do NOT
/// compare chronologically as text — `Z` sorts after every digit, and `…:11.1Z` (100 ms)
/// sorts after `…:11.12Z` (120 ms). Which of two reported instants came first is a
/// question about the instants, so every case that asks it reads them here.
pub fn millis_of_iso8601(case: &str, rendered: &str) -> i64 {
    let instant = ::aws_smithy_types::DateTime::from_str(
        rendered,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: {rendered:?} is not a date-time timestamp: {error}")
    });
    instant.to_millis().unwrap_or_else(|error| {
        panic!("case {case:?}: {rendered:?} does not convert to millis: {error}")
    })
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

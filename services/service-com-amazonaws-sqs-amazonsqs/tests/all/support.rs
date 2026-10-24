// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate shares: one emulator over one engine,
//! and the helpers that drive it (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use.
//!
//! It lives here rather than in each suite so a second suite does not re-derive the
//! request shape, the account id or the url and ARN forms.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the request's SigV4 credential scope names; it is the region every
/// expected ARN carries.
pub const REGION: &str = "us-east-1";

/// The authority the request arrives on, and therefore the endpoint every minted
/// queue url is built from.
pub const HOST: &str = "sqs.us-east-1.amazonaws.com";

/// A SigV4-shaped `Authorization` header: its credential scope is what routes the
/// request to this service (signing name `sqs`) and names the region. Request
/// authentication is off in these tests, so the signature itself is never checked.
pub const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/sqs/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000";

/// One emulator holding only this service, over one shared engine.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The account id every generated emulator presents.
pub fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The url a queue named `name` must be reachable at: `<endpoint>/<account>/<name>`.
pub fn expected_url(name: &str) -> String {
    format!("https://{HOST}/{}/{name}", account())
}

/// The ARN a queue named `name` must report.
pub fn expected_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{}:{name}", account())
}

/// Send one operation and return its HTTP status and parsed body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", HOST)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Send one operation that must succeed.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code of a refusal: awsJson1_0 carries it as the body's `__type`.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A JSON object of strings as a sorted map, so two attribute maps compare directly.
pub fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|value| (name.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Create a queue and return the raw reply.
pub async fn create(router: &MultiServiceRouter, name: &str, attributes: &Value) -> (u16, Value) {
    let mut body = json!({"QueueName": name});
    if attributes.as_object().is_some_and(|map| !map.is_empty()) {
        body["Attributes"] = attributes.clone();
    }
    call(router, "CreateQueue", &body).await
}

/// Create a queue that must succeed and return its url.
pub async fn create_ok(router: &MultiServiceRouter, name: &str, attributes: &Value) -> String {
    let (status, parsed) = create(router, name, attributes).await;
    assert_eq!(
        status, 200,
        "case create {name} with {attributes}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case create {name}: no QueueUrl in {parsed}"))
        .to_string()
}

/// The `Messages` of a receive reply, as a list — empty when the member is absent,
/// which is how a reply that hands over nothing answers.
pub fn messages(reply: &Value) -> Vec<Value> {
    reply
        .get("Messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every attribute the queue at `url` has a value for (`AttributeNames: ["All"]`).
pub async fn all_attributes(
    router: &MultiServiceRouter,
    url: &str,
) -> BTreeMap<String, String> {
    let reported = ok(
        router,
        "GetQueueAttributes",
        json!({"QueueUrl": url, "AttributeNames": ["All"]}),
    )
    .await;
    as_map(&reported["Attributes"])
}

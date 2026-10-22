// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the emulator through.
//!
//! One `POST /` with `X-Amz-Target: AmazonEC2ContainerServiceV20141113.<Operation>` and
//! an `awsJson1_1` body, dispatched by the multi-service router. Nothing here reaches
//! into the state struct, so a response that is not computed from persisted state cannot
//! pass.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in these suites signs for. Deliberately NOT the emulator's
/// default, so an ARN that carries it proves the region came from the request's SigV4
/// credential scope.
pub(crate) const SIGNED_REGION: &str = "us-west-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// A router with just this service mounted on a fresh engine.
pub(crate) fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /ecs/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
pub(crate) async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 and the named modelled error code.
pub(crate) async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected a {expected_code} from {operation}, got {parsed}"
    );
}

/// A call that must be refused with HTTP 400, returning the modelled error code (the
/// short shape name the `__type` carries) and the `message` the refusal reports — the
/// two things a client keys on.
pub(crate) async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let code = parsed["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = parsed["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed}"))
        .to_string();
    (code, message)
}

/// A required string member of a response body.
pub(crate) fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// An integer member whose modelled default is `0`, so an absent member reads as zero.
pub(crate) fn count(body: &Value, member: &str, case: &str) -> i64 {
    match &body[member] {
        Value::Null => 0,
        value => value
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected an integer {member}, got {body}")),
    }
}

/// A list member of a response body, absent reading as empty.
pub(crate) fn list(body: &Value, member: &str, case: &str) -> Vec<Value> {
    match &body[member] {
        Value::Null => Vec::new(),
        Value::Array(items) => items.clone(),
        other => panic!("case {case:?}: expected a list {member}, got {other}"),
    }
}

/// An EC2 instance identity document, as the agent reads it from
/// `http://169.254.169.254/latest/dynamic/instance-identity/document/`. It carries the
/// `accountId` and the `instanceId` a registration must supply.
pub(crate) fn identity_document(instance_id: &str) -> String {
    json!({
        "accountId": ACCOUNT,
        "instanceId": instance_id,
        "region": SIGNED_REGION,
        "instanceType": "m5.large",
        "availabilityZone": format!("{SIGNED_REGION}a"),
    })
    .to_string()
}

/// Create a cluster and return its ARN.
pub(crate) async fn create_cluster(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> String {
    let body = ok(router, "CreateCluster", json!({ "clusterName": name }), case).await;
    string(&body["cluster"], "clusterArn", case)
}

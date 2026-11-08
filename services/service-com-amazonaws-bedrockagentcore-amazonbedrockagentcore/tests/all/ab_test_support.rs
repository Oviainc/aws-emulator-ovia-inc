// SPDX-License-Identifier: CC0-1.0

//! What every A/B-test suite in this crate's test target needs, in ONE place.
//!
//! `tests/all.rs` is the test target's crate root, so each suite is a sibling module of
//! this one and reaches it as `use super::ab_test_support::*;`. The emulator, the request
//! sender, the four routes of the family, the JSON readers and the fixture ARNs live here
//! rather than once per suite, so two suites cannot drift apart on the shape of a request
//! or on what a variant looks like.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// A gateway whose traffic a test splits, in the form `GatewayArn` publishes as its
/// pattern (source: smithy_query query="operation" operation="CreateABTest").
pub(crate) const GATEWAY_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:gateway/demo-gateway-0a1b2c3d4e";
/// A second gateway, so a case can show the gateway is reported from the record rather
/// than from one fixed value.
pub(crate) const OTHER_GATEWAY_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:gateway/other-gateway-1b2c3d4e5f";
/// The role the test runs under, in the form `RoleArn` publishes as its pattern.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/AgentCoreABTest";
/// A second role, for the update that replaces the first.
pub(crate) const OTHER_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/AgentCoreABTestV2";
/// An online evaluation configuration, in the form `OnlineEvaluationConfigArn` publishes
/// as its pattern (source: smithy_query query="shape"
/// shape="ABTestEvaluationConfig").
pub(crate) const EVALUATION_CONFIG_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:online-evaluation-config/quality-0a1b2c3d4e";
/// A second evaluation configuration, for the per-variant member of that union.
pub(crate) const TREATMENT_EVALUATION_CONFIG_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:online-evaluation-config/latency-1b2c3d4e5f";
/// A configuration bundle a variant can be configured from, in the form
/// `ConfigurationBundleRef` publishes as its patterns (source: smithy_query
/// query="shape" shape="ConfigurationBundleRef").
pub(crate) const BUNDLE_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:configuration-bundle/bundle-0a1b2c3d4e";
/// A bundle version, which that shape publishes as a UUID pattern.
pub(crate) const BUNDLE_VERSION: &str = "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";
/// A `clientToken` of the modelled length (33..256) and pattern.
pub(crate) const CLIENT_TOKEN: &str = "abtestclienttoken000000000000000001";
/// A second `clientToken`, so a case can tell a replay from a fresh call.
pub(crate) const OTHER_CLIENT_TOKEN: &str = "abtestclienttoken000000000000000002";

/// The emulator, with request authentication off so a test signs nothing, plus the
/// shared engine so a case can age a stored test's clock without sleeping.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://bedrock-agentcore.us-east-1.amazonaws.com{path}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// `CreateABTest`: `POST /ab-tests`.
pub(crate) async fn create_ab_test(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    send(router, "POST", "/ab-tests", Some(body)).await
}

/// `ListABTests`: `GET /ab-tests` with `maxResults` / `nextToken` as query parameters.
pub(crate) async fn list_ab_tests(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        "/ab-tests".to_string()
    } else {
        format!("/ab-tests?{query}")
    };
    send(router, "GET", &path, None).await
}

/// `GetABTest`: `GET /ab-tests/{abTestId}`. The `@httpLabel` needs no escaping —
/// `ABTestId` admits only letters, digits, `-` and `_`.
pub(crate) async fn get_ab_test(router: &MultiServiceRouter, ab_test_id: &str) -> (u16, Value) {
    send(router, "GET", &format!("/ab-tests/{ab_test_id}"), None).await
}

/// `UpdateABTest`: `PUT /ab-tests/{abTestId}`.
pub(crate) async fn update_ab_test(
    router: &MultiServiceRouter,
    ab_test_id: &str,
    body: Value,
) -> (u16, Value) {
    send(
        router,
        "PUT",
        &format!("/ab-tests/{ab_test_id}"),
        Some(body),
    )
    .await
}

/// `DeleteABTest`: `DELETE /ab-tests/{abTestId}`.
pub(crate) async fn delete_ab_test(router: &MultiServiceRouter, ab_test_id: &str) -> (u16, Value) {
    send(router, "DELETE", &format!("/ab-tests/{ab_test_id}"), None).await
}

/// One member of a JSON object, or `Value::Null` when it is absent.
pub(crate) fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

/// One member of a JSON object as a string, or `""`.
pub(crate) fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    member(body, name).as_str().unwrap_or("")
}

/// The wire error code of a refusal (`__type` on `restJson1`).
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// One variant, as `Variant` models it: the name, its share of the traffic and the
/// configuration traffic routed to it uses (source: smithy_query query="shape"
/// shape="Variant,VariantConfiguration").
pub(crate) fn variant(name: &str, weight: i64, configuration: Value) -> Value {
    json!({"name": name, "weight": weight, "variantConfiguration": configuration})
}

/// A variant configured from a gateway target.
pub(crate) fn target_configuration(target: &str) -> Value {
    json!({"target": {"name": target}})
}

/// A variant configured from a version of a configuration bundle.
pub(crate) fn bundle_configuration() -> Value {
    json!({"configurationBundle": {"bundleArn": BUNDLE_ARN, "bundleVersion": BUNDLE_VERSION}})
}

/// The evaluation configuration naming one configuration PER variant, the other member
/// of the `ABTestEvaluationConfig` union.
pub(crate) fn per_variant_evaluation_config() -> Value {
    json!({"perVariantOnlineEvaluationConfig": [
        {"name": "C", "onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
        {"name": "T1", "onlineEvaluationConfigArn": TREATMENT_EVALUATION_CONFIG_ARN},
    ]})
}

/// The smallest `CreateABTest` request the model admits: every `@required` member and
/// nothing else, with an even split between the two variants.
pub(crate) fn minimal_request(name: &str) -> Value {
    json!({
        "name": name,
        "gatewayArn": GATEWAY_ARN,
        "roleArn": ROLE_ARN,
        "variants": [
            variant("C", 50, target_configuration("control")),
            variant("T1", 50, target_configuration("treatment")),
        ],
        "evaluationConfig": {"onlineEvaluationConfigArn": EVALUATION_CONFIG_ARN},
    })
}

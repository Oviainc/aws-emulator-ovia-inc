// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate shares.
//!
//! Every assertion goes over the wire: an awsJson1_0 `POST /` with
//! `X-Amz-Target: VerifiedPermissions.<Operation>`, dispatched through the same
//! `MultiServiceRouter` the emulator binary serves, over ONE shared engine — so a
//! write made by one request is only visible to the next if it really reached
//! state.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request this harness sends is SIGNED for. The emulator reads
/// it out of the SigV4 credential scope, and a minted regional ARN names it, so
/// the header below and a suite's ARN expectation share this one constant rather
/// than repeating a literal that could drift.
pub const SIGNING_REGION: &str = "us-east-1";

/// A router serving only this service, over a fresh shared engine.
pub fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// A router serving this service AND the Amazon Cognito user-pool service, over one
/// shared engine.
///
/// An identity source references a user pool, and an authorization call that takes its
/// principal from a token that pool issued verifies the token against the key set the
/// pool publishes — so a suite about such a call needs the pool to really exist, with
/// really signed tokens. Mounting both services on one engine is how this emulator serves
/// them in the router its binary builds, so the suite exercises the same seam.
pub fn router_with_user_pools() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (mut router, engine) = router_with_engine();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// The same router, plus the engine it was built on — for a suite that has to
/// read or seed state no served request can reach. `router()` delegates here so
/// there is ONE place that decides how the harness is wired.
pub fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_verifiedpermissions_verifiedpermissions::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Call one operation and return `(http status, parsed body)`.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("VerifiedPermissions.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAEXAMPLE/20260101/{SIGNING_REGION}/verifiedpermissions/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a test request body always serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one operation, requiring success, and return the parsed body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} failed: {parsed}");
    parsed
}

/// Create a store with the given validation `mode` and return its
/// `policyStoreId`. The ONE store-creation helper this test crate has; every
/// suite reaches a store through it.
pub async fn store_with_mode(router: &MultiServiceRouter, mode: &str) -> String {
    let created = ok(
        router,
        "CreatePolicyStore",
        &json!({ "validationSettings": { "mode": mode } }),
    )
    .await;
    created["policyStoreId"]
        .as_str()
        .expect("policyStoreId is required on the CreatePolicyStore response")
        .to_string()
}

/// Create a store with validation mode `OFF` and return its `policyStoreId`.
pub async fn store_with_validation_off(router: &MultiServiceRouter) -> String {
    store_with_mode(router, "OFF").await
}

/// Call one Amazon Cognito user-pool operation, requiring success, and return the parsed
/// body. That service speaks awsJson1_1 under its own target prefix and is reached over
/// the SAME router, so a pool a suite builds here is the pool this service's identity
/// sources reference.
pub async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{SIGNING_REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAEXAMPLE/20260101/{SIGNING_REGION}/cognito-idp/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a test request body always serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(status, 200, "{operation} failed: {parsed}");
    parsed
}

/// A JSON object as an ordered `String -> String` map, so two tag sets compare
/// regardless of wire order. An absent member (`Null`) and `{}` both map to the
/// empty map, so a suite that cares about the DIFFERENCE between those two
/// asserts on the raw member instead.
pub fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// The tag set `ListTagsForResource` reports for `arn`, as an ordered map. The
/// ONE tag-reading helper this test crate has; every suite reaches a persisted
/// tag set through it.
pub async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let body = ok(
        router,
        "ListTagsForResource",
        &json!({ "resourceArn": arn }),
    )
    .await;
    as_map(&body["tags"])
}

/// The error code an awsJson1_0 fault carries in `__type`, without its namespace.
pub fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A Cognito user pool ARN in the one account this emulator presents
/// (`engine::emulator_account_id`), for `region` and `pool`. The ONE pool-ARN
/// helper this test crate has, so every suite shapes a pool ARN the same way.
pub fn user_pool_arn(region: &str, pool: &str) -> String {
    format!(
        "arn:aws:cognito-idp:{region}:{}:userpool/{pool}",
        engine::emulator_account_id()
    )
}

/// The `message` an awsJson1_0 fault carries. The modelled member is `message`;
/// `Message` is accepted too, so an assertion is about the TEXT rather than
/// about which spelling the protocol layer emitted.
pub fn fault_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

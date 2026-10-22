// SPDX-License-Identifier: CC0-1.0

//! The wire harness both suites in this target share.
//!
//! Every call goes through the same `MultiServiceRouter::dispatch` the emulator's front
//! door uses: `POST /` with `X-Amz-Target: ComputeOptimizerService.<Operation>` and an
//! `awsJson1_0` body. Nothing here reaches into the state struct, so what a suite built on
//! it proves is what a client sees — and one definition of the dispatch shape means two
//! suites cannot drift apart on the header set or the routing credential.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account that owns every resource the suites name. A `Scope` of `AccountId` must be
/// a 12-digit account id (the model's `Scope$value` doc), and an account-level preference
/// applies to the resources of that account.
pub(crate) const ACCOUNT: &str = "111122223333";

/// An EC2 instance ARN — one of the two ARN forms the preference API accepts.
pub(crate) fn instance_arn(instance_id: &str) -> String {
    format!("arn:aws:ec2:us-east-1:{ACCOUNT}:instance/{instance_id}")
}

/// An Auto Scaling group ARN — the other accepted form.
pub(crate) fn auto_scaling_group_arn(name: &str) -> String {
    format!(
        "arn:aws:autoscaling:us-east-1:{ACCOUNT}:autoScalingGroup:\
         12345678-1234-1234-1234-123456789012:autoScalingGroupName/{name}"
    )
}

/// One emulator: one engine, one shared store, reached only over HTTP.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return the HTTP status and the parsed JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("ComputeOptimizerService.{operation}"))
        // The SigV4 credential scope is what routes a request to its service.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/compute-optimizer/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=not-verified",
        )
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error response.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// Opt the account in, which every preference operation requires.
pub(crate) async fn opt_in(router: &MultiServiceRouter) {
    let (status, body) = call(
        router,
        "UpdateEnrollmentStatus",
        json!({"status": "Active", "includeMemberAccounts": false}),
    )
    .await;
    assert_eq!(status, 200, "opting in: expected 200, got {status}: {body}");
}

/// Write one preference, asserting it was accepted.
pub(crate) async fn put_preference(router: &MultiServiceRouter, request: &Value) {
    let (status, body) = call(router, "PutRecommendationPreferences", request.clone()).await;
    assert_eq!(
        status, 200,
        "put {request}: expected 200, got {status}: {body}"
    );
    assert_eq!(
        body,
        json!({}),
        "put {request}: PutRecommendationPreferences has an empty output, got {body}"
    );
}

/// Read the preference records stored for `resource_type`, optionally at one scope.
pub(crate) async fn get_preferences(
    router: &MultiServiceRouter,
    resource_type: &str,
    scope: Option<&Value>,
) -> Value {
    let mut request = json!({"resourceType": resource_type});
    if let Some(scope) = scope {
        request["scope"] = scope.clone();
    }
    let (status, body) = call(router, "GetRecommendationPreferences", request.clone()).await;
    assert_eq!(
        status, 200,
        "get {request}: expected 200, got {status}: {body}"
    );
    body
}

/// The effective preferences in force for one resource.
pub(crate) async fn effective_for(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    let (status, body) = call(
        router,
        "GetEffectiveRecommendationPreferences",
        json!({"resourceArn": resource_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "effective preferences for {resource_arn}: expected 200, got {status}: {body}"
    );
    body
}

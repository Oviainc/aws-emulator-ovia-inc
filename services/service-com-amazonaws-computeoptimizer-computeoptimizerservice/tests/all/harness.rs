// SPDX-License-Identifier: CC0-1.0

//! The wire harness the suites in this target share.
//!
//! Every call goes through the same `MultiServiceRouter::dispatch` the emulator's front
//! door uses: `POST /` with `X-Amz-Target: <Prefix>.<Operation>` and an `awsJson` body.
//! Nothing here reaches into the state struct, so what a suite built on it proves is what a
//! client sees — and one definition of the dispatch shape means no two suites can drift
//! apart on the header set or the routing credential.
//!
//! Compute Optimizer does not stand alone: an Amazon ECS service recommendation is computed
//! over the ECS service the ECS emulator holds and the `AWS/ECS` utilization the CloudWatch
//! emulator holds, so [`emulator_with_analysed_services`] mounts all three on ONE engine —
//! exactly what `core/router` assembles.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::RESOURCE_TYPE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    RESOURCE_TYPE_AUTO_SCALING_GROUP, SCOPE_ACCOUNT_ID, SCOPE_RESOURCE_ARN,
};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The one account this service answers for, and so the account that owns every resource
/// the suites name: this emulator presents exactly ONE standalone account
/// (`engine::emulator_account_id`) and records no organization, so a request naming another
/// account's resource — in `accountIds`, in a resource ARN, or in an `AccountId` scope — is
/// refused rather than answered. A `Scope` of `AccountId` must be a 12-digit account id
/// (the model's `Scope$value` doc), and an account-level preference applies to the resources
/// of that account.
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

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

/// The region every request in these suites signs for; every ARN the ECS emulator mints
/// carries it, because the SigV4 credential scope is what names a request's region.
pub(crate) const REGION: &str = "us-east-1";

/// One emulator: one engine, one shared store, reached only over HTTP.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the engine it was built on — for a suite that has to RECORD
/// analysis output (an idle resource recommendation, say) that no operation of this API
/// creates, and then read it back over the wire like any client.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Compute Optimizer, Amazon ECS and CloudWatch over ONE engine: the emulator a suite needs
/// to drive a recommendation that is computed over another service's resources and the
/// utilization published for them.
pub(crate) fn emulator_with_analysed_services() -> MultiServiceRouter {
    emulator_with_analysed_services_and_engine().0
}

/// The same emulator, with the engine it was built on — for a suite that has to seed a
/// CloudWatch series of the AWS-reserved namespace space ([`seed_cloudwatch_series`]), which
/// no client call can write.
pub(crate) fn emulator_with_analysed_services_and_engine(
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Write datapoints straight into the CloudWatch metric store, the way the AWS service that
/// OWNS the metric writes them.
///
/// A client cannot publish into the AWS-reserved namespace space: `PutMetricData` refuses a
/// `Namespace` beginning with `AWS/` ("To avoid conflicts with Amazon Web Services service
/// namespaces, you should not specify a namespace that begins with AWS/"), and the
/// utilization Compute Optimizer analyses is exactly such a series (`AWS/ECS`), authored by
/// the service that owns the resource. So a suite seeds it here and reads the recommendation
/// computed from it over the wire, like any client.
pub(crate) fn seed_cloudwatch_series(
    engine: &Arc<engine::InMemoryEngine>,
    namespace: &str,
    metric_name: &str,
    dimensions: &[(&str, &str)],
    unit: Option<&str>,
    at_secs: f64,
    values: &[f64],
) {
    use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

    let key = cloudwatch::MetricKey {
        namespace: namespace.to_string(),
        metric_name: metric_name.to_string(),
        dimensions: dimensions
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect(),
    };
    let data: Vec<cloudwatch::StoredDatum> = values
        .iter()
        .map(|value| cloudwatch::StoredDatum {
            timestamp_secs: at_secs,
            sample_count: 1.0,
            sum: *value,
            minimum: *value,
            maximum: *value,
            unit: unit.map(ToString::to_string),
            // The default `StorageResolution` of a published datum.
            storage_resolution: 60,
        })
        .collect();
    engine
        .state::<cloudwatch::ComAmazonawsCloudwatchGraniteserviceversion20100801State>()
        .lock()
        .append_datapoints(key, data)
        .expect("the metric store has room for the seeded series");
}

/// Call one Compute Optimizer operation and return the HTTP status and the parsed JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The same call at the endpoint of `region` — the region a client's SigV4 credential scope
/// names — for a suite that asserts what a regional endpoint reports.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        region,
        "compute-optimizer",
        "application/x-amz-json-1.0",
        &format!("ComputeOptimizerService.{operation}"),
        body,
    )
    .await
}

/// Call one Amazon ECS operation (`awsJson1_1`).
pub(crate) async fn call_ecs(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        REGION,
        "ecs",
        "application/x-amz-json-1.1",
        &format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        body,
    )
    .await
}

/// Dispatch one request, returning the HTTP status and the parsed JSON body.
async fn dispatch(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", content_type)
        .header("x-amz-target", target)
        // The SigV4 credential scope is what routes a request to its service, and the
        // region it names is the regional endpoint the request addressed.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=not-verified"
            ),
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

/// Every value of the model's `ResourceType` enum — the type a preference request's
/// `resourceType` member carries — taken from the crate's generated value set rather than
/// copied, so a resource type the model adds is covered the day it appears
/// (`RESOURCE_TYPE_VALUES`: AuroraDBClusterStorage | AutoScalingGroup | EbsVolume |
/// Ec2Instance | EcsService | Idle | LambdaFunction | License | NotApplicable |
/// RdsDBInstance).
pub(crate) const RESOURCE_TYPES: &[&str] = RESOURCE_TYPE_VALUES;

/// The scope a preference for `resource_type` may be written at.
///
/// An Auto Scaling group preference "can only be set at the resource level by specifying a
/// scope name of `ResourceArn`" (`PutRecommendationPreferences$scope`); every other
/// resource type takes the account-level scope, whose "value must be a 12-digit Amazon Web
/// Services account ID" (`Scope$value`). Stated once here so no suite that walks the whole
/// enum can drift from another on it.
pub(crate) fn scope_for(resource_type: &str) -> Value {
    if resource_type == RESOURCE_TYPE_AUTO_SCALING_GROUP {
        json!({"name": SCOPE_RESOURCE_ARN, "value": auto_scaling_group_arn("batch-tier")})
    } else {
        json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT})
    }
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

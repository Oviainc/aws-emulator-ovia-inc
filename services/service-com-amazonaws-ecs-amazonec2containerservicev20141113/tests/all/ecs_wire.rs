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

/// One call, returning the HTTP status and the parsed body — for a suite whose table
/// spans requests that are ANSWERED and requests that are REFUSED, where which of the
/// two a row earns is itself what the row is about.
pub(crate) async fn answered(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    call(router, operation, body).await
}

/// A call that must be refused with HTTP 400 and the named modelled error code,
/// returning its error body so a caller can assert the message too.
pub(crate) async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) -> Value {
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
    parsed
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

/// The signature of an EC2 instance identity document, as the agent reads it from
/// `http://169.254.169.254/latest/dynamic/instance-identity/signature/`: base64 text,
/// which a registration presents BESIDE the document — the document alone identifies no
/// machine. 350 characters, because the signature of a document under a 2048-bit key is
/// 256 bytes and base64 spells those in 344. Nothing here signs anything: what a
/// signature is OF cannot be checked without the key that produced it.
pub(crate) fn identity_document_signature() -> String {
    "ZXhhbXBsZQ".repeat(35)
}

/// Register one container instance into `cluster` through the agent's own operation and
/// return its ARN.
pub(crate) async fn register_instance(
    router: &MultiServiceRouter,
    cluster: &str,
    instance_id: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterContainerInstance",
        json!({
            "cluster": cluster,
            "instanceIdentityDocument": identity_document(instance_id),
            "instanceIdentityDocumentSignature": identity_document_signature(),
        }),
        case,
    )
    .await;
    string(&body["containerInstance"], "containerInstanceArn", case)
}

/// The principal every caller of this emulator authenticates as, and so the one a "who
/// created this" member names: the identity `sts:GetCallerIdentity` publishes
/// (`arn:aws:iam::<account>:user/emulator`).
pub(crate) fn caller_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// An integer member that must REACH the wire, as `Some(value)`, and `None` for a member
/// the response left out — so an assertion on a member whose expected value is its
/// modelled default cannot pass against a projection that never wrote it.
pub(crate) fn present_count(body: &Value, member: &str, case: &str) -> Option<i64> {
    match &body[member] {
        Value::Null => None,
        value => Some(
            value
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: expected an integer {member}, got {body}")),
        ),
    }
}

/// The `networkConfiguration` every launch of an `awsvpc` task definition has to carry:
/// an `awsvpc` task gets its own elastic network interface, so `CreateService`,
/// `UpdateService` and `RunTask` must say which subnets and security groups it takes
/// (`AwsVpcConfiguration$subnets`/`$securityGroups`). The ids are EC2-shaped but never
/// resolved: ECS stores the configuration it is given.
pub(crate) fn awsvpc_network_configuration() -> Value {
    json!({
        "awsvpcConfiguration": {
            "subnets": ["subnet-0123456789abcdef0"],
            "securityGroups": ["sg-0123456789abcdef0"],
        }
    })
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

/// The EC2 instance id the shared capacity helper registers. One per cluster is enough:
/// a container instance is registered INTO a cluster, so the same instance id in another
/// cluster is another registration.
const CAPACITY_INSTANCE_ID: &str = "i-0c0ffee0c0ffee0c0";

/// Create a cluster and register ONE container instance into it, so a task placed with
/// a launch type whose capacity is registered to the cluster has somewhere to run —
/// `RunTask` refuses a cluster with none ("The EC2 launch type runs your tasks on Amazon
/// EC2 instances registered to your cluster"). Returns the cluster's ARN.
pub(crate) async fn cluster_with_capacity(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> String {
    let arn = create_cluster(router, name, case).await;
    register_capacity(router, name, case).await;
    arn
}

/// Register ONE container instance into an EXISTING cluster, for a suite that created
/// the cluster itself. Returns the container instance's ARN.
pub(crate) async fn register_capacity(
    router: &MultiServiceRouter,
    cluster: &str,
    case: &str,
) -> String {
    register_instance(router, cluster, CAPACITY_INSTANCE_ID, case).await
}

/// Register one revision of `family` with a single container and return its ARN. The
/// suites that need a plain revision share this one; a suite that needs a revision of a
/// particular SHAPE (Fargate-compatible, a named container) builds its own.
pub(crate) async fn register_definition(
    router: &MultiServiceRouter,
    family: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [
                crate::task_lifecycle::container("web", "public.ecr.aws/nginx/nginx:latest"),
            ],
        }),
        case,
    )
    .await;
    string(&body["taskDefinition"], "taskDefinitionArn", case)
}

/// A router carrying this service AND the Cloud Map (service discovery) service over ONE
/// shared engine — the two stores every cross-service registry rule reads across, mounted
/// the way the production router mounts them.
pub(crate) fn emulator_with_cloud_map() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_servicediscovery_route53autonaming_v20170314::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One `awsJson1_1` call to the Cloud Map service, returning its parsed body. A Cloud Map
/// resource an ECS rule reads is created through CLOUD MAP's own API, never written into a
/// store by hand.
pub(crate) async fn cloud_map(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("Route53AutoNaming_v20170314.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /servicediscovery/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected Cloud Map {operation} to answer 200, got {status} with \
         {parsed}"
    );
    parsed
}

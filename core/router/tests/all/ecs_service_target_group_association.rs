// SPDX-License-Identifier: CC0-1.0

//! An ECS service's TARGET GROUP must be one a load balancer forwards to, across the two
//! services that span.
//!
//! RULE: a service registers its tasks as TARGETS of the groups its `loadBalancers` name,
//! and only a listener on a load balancer can send traffic to a group — so a group no load
//! balancer forwards to can never reach the tasks, and `CreateService` refuses the binding
//! with `InvalidParameterException` (HTTP 400) and creates nothing. The association is
//! LIVE: the same group, named in the identical request, is refused before a listener
//! forwards to it and accepted after, so ECS reads it from Elastic Load Balancing v2 at
//! request time rather than inferring it from the ARN.
//!
//! The association comes from the ELBv2 store, which is why this suite lives here: the
//! target group, the load balancer and the listener are created through the real ELBv2
//! operations on the same engine that serves ECS — including the CLASSIC front door the
//! production router mounts beside them, so the two Elastic Load Balancing models share
//! one endpoint here as they do there — and what ECS reads is what ELBv2 persisted rather
//! than a copy of it.
//!
//! The rows differ along how many groups the request binds and which of them are attached:
//! one orphan group, one attached group, and a request mixing the two.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element reader this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{form_encode, opt_element};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The API version an awsQuery request naming the ELBv2 model carries.
const ELB_VERSION: &str = "2015-12-01";

/// The ECS cluster and task-definition family every case uses.
const CLUSTER: &str = "bound";
const FAMILY: &str = "boundfamily";

/// The container the task definition declares, and the port it declares on it: a binding
/// may only name a container and port the revision has.
const CONTAINER: &str = "app";
const CONTAINER_PORT: i64 = 80;

/// The subnet the services of this suite are placed in, in the account's own Amazon EC2
/// store: an `awsvpc` placement is resolved against those records — the interface the task
/// is given is created IN the subnet it names — so a service placed in one needs the
/// account to hold it. Written through the fixture every suite naming a subnet literally
/// shares ([`crate::ec2_network::register_network`]).
const SUBNET: &str = "subnet-0123456789abcdef0";

/// The second subnet the load balancers of this suite are spread over.
const SECOND_SUBNET: &str = "subnet-0123456789abcdef1";

/// The VPC both subnets are in, and the one the target groups below are created in.
const VPC: &str = "vpc-0123456789abcdef0";

/// One emulator carrying ECS and both Elastic Load Balancing models over ONE shared
/// engine, exactly as the production router builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::ec2_network::register_network(
        &engine,
        VPC,
        &[SUBNET, SECOND_SUBNET],
        &format!("{REGION}a"),
    );
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to the ELBv2 model, answered with its XML body. Addressed by the
/// routing key that names the ELBv2 model itself, so this suite's own setup does not
/// depend on which front door claims the shared signing name — the point under test is
/// what ECS's in-process read sees, not what this test's calls reach.
async fn elbv2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={ELB_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("elasticloadbalancingv2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}\
                 /elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action}: expected Elastic Load Balancing to answer 200, got {status} with {body}"
    );
    body
}

/// One awsJson1_1 call to ECS, answered with the HTTP status and the parsed body.
async fn ecs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
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
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{REGION}\
                 /ecs/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(Bytes::from(
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

/// Create a target group with no listener forwarding to it and answer its ARN. Its
/// `LoadBalancerArns` is asserted empty, which is the precondition the rule is about.
async fn orphan_target_group(router: &MultiServiceRouter, name: &str) -> String {
    let created = elbv2(
        router,
        "CreateTargetGroup",
        &[
            ("Name", name),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("TargetType", "ip"),
            ("VpcId", VPC),
        ],
    )
    .await;
    let arn = opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {name}: no TargetGroupArn in {created}"));
    let described = elbv2(
        router,
        "DescribeTargetGroups",
        &[("TargetGroupArns.member.1", &arn)],
    )
    .await;
    assert!(
        opt_element(&described, "LoadBalancerArns")
            .unwrap_or_default()
            .is_empty(),
        "case {name}: a group with no listener forwarding to it has no load balancer, \
         got {described}"
    );
    arn
}

/// Attach `target_group` to a new load balancer by creating a listener that forwards to
/// it, and assert ELBv2 then reports the association.
async fn attach(router: &MultiServiceRouter, load_balancer_name: &str, target_group: &str) {
    let created = elbv2(
        router,
        "CreateLoadBalancer",
        &[
            ("Name", load_balancer_name),
            ("Subnets.member.1", SUBNET),
            ("Subnets.member.2", SECOND_SUBNET),
        ],
    )
    .await;
    let load_balancer = opt_element(&created, "LoadBalancerArn")
        .unwrap_or_else(|| panic!("case {load_balancer_name}: no LoadBalancerArn in {created}"));
    elbv2(
        router,
        "CreateListener",
        &[
            ("LoadBalancerArn", &load_balancer),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("DefaultActions.member.1.Type", "forward"),
            ("DefaultActions.member.1.TargetGroupArn", target_group),
        ],
    )
    .await;
    let described = elbv2(
        router,
        "DescribeTargetGroups",
        &[("TargetGroupArns.member.1", target_group)],
    )
    .await;
    assert!(
        opt_element(&described, "LoadBalancerArns")
            .unwrap_or_default()
            .contains(&load_balancer),
        "case {load_balancer_name}: ELBv2 must report the association the listener made, \
         got {described}"
    );
}

/// The `loadBalancers` binding onto one target group.
fn binding(target_group: &str) -> Value {
    json!({
        "targetGroupArn": target_group,
        "containerName": CONTAINER,
        "containerPort": CONTAINER_PORT,
    })
}

/// Create the cluster and the revision every case binds a service to.
async fn seed_ecs(router: &MultiServiceRouter) {
    let (status, body) = ecs(router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    assert_eq!(status, 200, "setup: CreateCluster answered {status} with {body}");
    let (status, body) = ecs(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": "public.ecr.aws/nginx/nginx:latest",
                "memory": 128,
                "portMappings": [{ "containerPort": CONTAINER_PORT }],
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: RegisterTaskDefinition answered {status} with {body}"
    );
}

/// One `CreateService` naming `bindings`, answered with the status and body.
async fn create_service(
    router: &MultiServiceRouter,
    service_name: &str,
    bindings: Vec<Value>,
) -> (u16, Value) {
    ecs(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [SUBNET],
                    "securityGroups": ["sg-0123456789abcdef0"],
                }
            },
            "loadBalancers": bindings,
        }),
    )
    .await
}

/// Whether ECS holds a service of this name in the cluster.
async fn service_exists(router: &MultiServiceRouter, service_name: &str) -> bool {
    let (status, body) = ecs(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [service_name] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeServices answered {status} with {body}"
    );
    body["services"]
        .as_array()
        .is_some_and(|services| !services.is_empty())
}

#[tokio::test]
async fn a_service_is_refused_on_an_unattached_target_group_and_accepted_once_a_listener_forwards() {
    let router = emulator();
    seed_ecs(&router).await;

    let orphan = orphan_target_group(&router, "ecs-orphan").await;
    let second = orphan_target_group(&router, "ecs-second").await;

    // Before any listener: the binding is refused, the refusal names the group, and
    // nothing is created.
    let case = "one unattached target group";
    let (status, body) = create_service(&router, "refused", vec![binding(&orphan)]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected CreateService to be refused, got {status} with {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|wire| wire.contains("InvalidParameterException")),
        "case {case:?}: expected an InvalidParameterException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(
            format!(
                "The target group with targetGroupArn {orphan} does not have an \
                 associated load balancer."
            )
            .as_str()
        ),
        "case {case:?}: unexpected message in {body}"
    );
    assert!(
        !service_exists(&router, "refused").await,
        "case {case:?}: a refused create must store no service"
    );

    // One group attached, the other still orphaned: a request mixing them is refused on
    // the one that is not attached, so the check is per BINDING and not per request.
    attach(&router, "ecs-alb", &orphan).await;
    let case = "an attached group beside an unattached one";
    let (status, body) = create_service(
        &router,
        "mixed",
        vec![binding(&orphan), binding(&second)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected CreateService to be refused, got {status} with {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains(&second)),
        "case {case:?}: expected the refusal to name the unattached group, got {body}"
    );
    assert!(
        !service_exists(&router, "mixed").await,
        "case {case:?}: a refused create must store no service"
    );

    // The IDENTICAL request that was refused above is now accepted, because a listener
    // forwards to the group: the association is read at request time.
    let case = "the same group once a listener forwards to it";
    let (status, body) = create_service(&router, "refused", vec![binding(&orphan)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateService to be accepted, got {status} with {body}"
    );
    assert_eq!(
        body["service"]["loadBalancers"],
        json!([binding(&orphan)]),
        "case {case:?}: the stored service must report the binding it was created with, \
         got {body}"
    );
    assert!(
        service_exists(&router, "refused").await,
        "case {case:?}: the accepted service must be stored"
    );

    // And once the second group is attached too, the mixed request is accepted whole.
    attach(&router, "ecs-alb-two", &second).await;
    let case = "both groups attached";
    let (status, body) = create_service(
        &router,
        "mixed",
        vec![binding(&orphan), binding(&second)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateService to be accepted, got {status} with {body}"
    );
    assert_eq!(
        body["service"]["loadBalancers"],
        json!([binding(&orphan), binding(&second)]),
        "case {case:?}: both bindings must round-trip, got {body}"
    );
}

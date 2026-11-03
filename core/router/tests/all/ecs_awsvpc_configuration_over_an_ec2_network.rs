// SPDX-License-Identifier: CC0-1.0

//! An ECS service placed in EC2's OWN subnets reads its `awsvpcConfiguration` back WHOLE.
//!
//! RULE: Amazon ECS stores the `awsvpcConfiguration` a launch supplies with the optional
//! members it fills in already filled in, and every projection of that stored record
//! reports the same complete structure — `securityGroups` is a LIST whether or not the
//! request named one (a request that names none leaves the interface on "the default
//! security group for the VPC", whose id belongs to the VPC rather than to this
//! configuration, so none is invented in its place), and `assignPublicIp` carries the
//! value its own member doc gives it: "When you use `create-service` or `update-service`,
//! the default is `DISABLED`". A client can therefore diff the read-back against what it
//! asked for, or re-apply it unchanged, without a member disappearing between the two.
//!
//! This suite lives beside the router because the network the configuration names is
//! EC2's: the VPC, the subnets and the security groups below are minted by the real EC2
//! operations on the SAME engine that serves ECS, so every id in an assertion is one EC2
//! handed out rather than one this suite made up. The rule itself is tabled, over more
//! shapes of configuration and over `CreateTaskSet` too, in the ECS crate's
//! `tests/all/awsvpc_configuration_store.rs`.
//!
//! The refusal leg is asserted beside it, because the two are one behaviour: an `awsvpc`
//! revision has its own elastic network interface, so a launch that says nothing about
//! where that interface goes is refused outright, and only an accepted launch has a stored
//! configuration to normalize.
//!
//! Member and default source: `smithy_query(query="shape", shape="AwsVpcConfiguration")` —
//! `subnets` is required, `securityGroups` is an optional list, and `assignPublicIp`
//! carries the `create-service`/`update-service` default quoted above.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The ECS cluster and task-definition family every case here uses.
const CLUSTER: &str = "awsvpcnetwork";
const FAMILY: &str = "awsvpcnetworkfamily";

/// The Fargate-capable `awsvpc` revision the row that asks for a PUBLIC IP runs: a public
/// address is attached to the task's own elastic network interface, which only Fargate
/// does, so that row cannot be placed on the cluster's EC2 capacity.
const FARGATE_FAMILY: &str = "awsvpcnetworkfargatefamily";

/// The image the family's one container starts from; irrelevant to the rule.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// `AwsVpcConfiguration$assignPublicIp` as ECS materialises it for a service that leaves
/// it out — the member's own doc: "When you use `create-service` or `update-service`, the
/// default is `DISABLED`."
const DEFAULT_ASSIGN_PUBLIC_IP: &str = "DISABLED";

/// The refusal an `awsvpc` launch that carries no `networkConfiguration` earns: the
/// condition names no resource, so the message is fixed text.
const NETWORK_CONFIGURATION_REQUIRED: &str =
    "Network Configuration must be provided when networkMode 'awsvpc' is specified.";

/// One emulator carrying EC2 and ECS over ONE shared engine, as the production router
/// mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is
/// what routes the request here; the signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One ec2Query action that must succeed, answered with its XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("ec2"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {text}"
    );
    text
}

/// One awsJson1_1 call to ECS, answered with the HTTP status and the parsed body.
async fn ecs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ecs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        )
        .header("authorization", authorization("ecs"))
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

/// One ECS call that must answer 200, as its body.
async fn ecs_ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = ecs(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// The network every case runs over: two subnets and two security groups of one VPC, all
/// minted by EC2 itself.
struct Network {
    subnets: Vec<String>,
    security_groups: Vec<String>,
}

/// Create that network through EC2's own operations.
async fn ec2_network(router: &MultiServiceRouter) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", "10.80.0.0/16")]).await,
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (cidr, zone) in [
        ("10.80.1.0/24", "us-west-2a"),
        ("10.80.2.0/24", "us-west-2b"),
    ] {
        subnets.push(element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", cidr),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        ));
    }
    let mut security_groups = Vec::new();
    for name in ["ecs-tasks-one", "ecs-tasks-two"] {
        security_groups.push(element(
            &ec2(
                router,
                "CreateSecurityGroup",
                &[
                    ("VpcId", vpc.as_str()),
                    ("GroupName", name),
                    ("GroupDescription", "the tasks of an awsvpc service"),
                ],
            )
            .await,
            "groupId",
        ));
    }
    Network {
        subnets,
        security_groups,
    }
}

/// Create the cluster and register the revisions every launch below runs: the `awsvpc`
/// revision validated against the EC2 launch type, and the Fargate-capable one (task-level
/// `cpu` and `memory`) the public-IP row needs.
async fn seed_ecs(router: &MultiServiceRouter) {
    ecs_ok(router, "CreateCluster", json!({ "clusterName": CLUSTER }), "setup").await;
    ecs_ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["EC2"],
            "containerDefinitions": [{
                "name": "web",
                "image": IMAGE,
                "memory": 128,
            }],
        }),
        "setup",
    )
    .await;
    ecs_ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": FARGATE_FAMILY,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": "web",
                "image": IMAGE,
                "memory": 128,
            }],
        }),
        "setup",
    )
    .await;
}

/// One row of the table: the configuration the request carries, built out of the EC2 ids,
/// and what every read of the stored configuration must then report.
struct ConfigurationCase {
    name: &'static str,
    /// How many of EC2's subnets the request names (`subnets` is required).
    subnets: usize,
    /// How many of EC2's security groups it names, or `None` when it omits the member.
    security_groups: Option<usize>,
    /// The `assignPublicIp` it names, or `None` when it omits the member.
    assign_public_ip: Option<&'static str>,
    /// The `assignPublicIp` every read must report.
    expected_assign_public_ip: &'static str,
    /// The launch type this row places its tasks with — the row that asks for a public IP
    /// on Fargate, every other row on the cluster's own EC2 capacity.
    launch_type: &'static str,
}

impl ConfigurationCase {
    /// The revision this row runs: the one validated against the launch type it places with.
    fn family(&self) -> &'static str {
        if self.launch_type == "FARGATE" {
            FARGATE_FAMILY
        } else {
            FAMILY
        }
    }

    /// The subnets this row names, in order.
    fn named_subnets(&self, network: &Network) -> Vec<String> {
        network.subnets[..self.subnets].to_vec()
    }

    /// The security groups every read must report: the ones the request named, and an
    /// empty list both when it named none and when it named an empty list.
    fn expected_security_groups(&self, network: &Network) -> Vec<String> {
        network.security_groups[..self.security_groups.unwrap_or(0)].to_vec()
    }

    /// The `networkConfiguration` this row sends, carrying only the members it names.
    fn request(&self, network: &Network) -> Value {
        let mut awsvpc = json!({ "subnets": self.named_subnets(network) });
        if self.security_groups.is_some() {
            awsvpc["securityGroups"] = json!(self.expected_security_groups(network));
        }
        if let Some(assign) = self.assign_public_ip {
            awsvpc["assignPublicIp"] = json!(assign);
        }
        json!({ "awsvpcConfiguration": awsvpc })
    }
}

/// The rows: they differ along how many of EC2's subnets the configuration names, whether
/// it names `securityGroups` at all (omitted, an explicitly empty list, one group, two),
/// whether it names `assignPublicIp` and — because only Fargate attaches a public address
/// to the task's own interface — which launch type the row places with.
fn cases() -> Vec<ConfigurationCase> {
    vec![
        ConfigurationCase {
            name: "one subnet, neither optional member",
            subnets: 1,
            security_groups: None,
            assign_public_ip: None,
            expected_assign_public_ip: DEFAULT_ASSIGN_PUBLIC_IP,
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "two subnets, neither optional member",
            subnets: 2,
            security_groups: None,
            assign_public_ip: None,
            expected_assign_public_ip: DEFAULT_ASSIGN_PUBLIC_IP,
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "an explicitly empty security-group list",
            subnets: 1,
            security_groups: Some(0),
            assign_public_ip: None,
            expected_assign_public_ip: DEFAULT_ASSIGN_PUBLIC_IP,
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "one of EC2's security groups",
            subnets: 1,
            security_groups: Some(1),
            assign_public_ip: None,
            expected_assign_public_ip: DEFAULT_ASSIGN_PUBLIC_IP,
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "both of EC2's security groups across both subnets",
            subnets: 2,
            security_groups: Some(2),
            assign_public_ip: None,
            expected_assign_public_ip: DEFAULT_ASSIGN_PUBLIC_IP,
            launch_type: "EC2",
        },
        ConfigurationCase {
            name: "no security groups, assignPublicIp ENABLED on Fargate",
            subnets: 1,
            security_groups: None,
            assign_public_ip: Some("ENABLED"),
            expected_assign_public_ip: "ENABLED",
            launch_type: "FARGATE",
        },
        ConfigurationCase {
            name: "no security groups, the default assignPublicIp named explicitly",
            subnets: 1,
            security_groups: None,
            assign_public_ip: Some(DEFAULT_ASSIGN_PUBLIC_IP),
            expected_assign_public_ip: DEFAULT_ASSIGN_PUBLIC_IP,
            launch_type: "EC2",
        },
    ]
}

/// The list member of a JSON view, an absent member reading as `None` so an assertion can
/// tell "the member is not there" from "the member is an empty list".
fn reported_list(view: &Value, member: &str) -> Option<Vec<String>> {
    match &view[member] {
        Value::Null => None,
        Value::Array(items) => Some(
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect(),
        ),
        _ => None,
    }
}

/// The `PRIMARY` deployment of a service view.
fn primary_deployment(service_view: &Value, case: &str) -> Value {
    service_view["deployments"]
        .as_array()
        .and_then(|deployments| {
            deployments
                .iter()
                .find(|deployment| deployment["status"].as_str() == Some("PRIMARY"))
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a PRIMARY deployment, got {service_view}")
        })
}

/// Assert that `view` — any projection carrying a `networkConfiguration` — reports the
/// configuration `test` asked for, with the members ECS fills in materialised.
fn assert_stored_configuration(
    view: &Value,
    source: &str,
    test: &ConfigurationCase,
    network: &Network,
) {
    let case = test.name;
    let configured = &view["networkConfiguration"]["awsvpcConfiguration"];
    assert!(
        configured.is_object(),
        "case {case:?}: {source} reports the stored awsvpcConfiguration, got {view}"
    );
    assert_eq!(
        reported_list(configured, "subnets"),
        Some(test.named_subnets(network)),
        "case {case:?}: {source} expected EC2's subnets to round-trip in order, got \
         {configured}"
    );
    // The optional member is a LIST on the wire whether or not the request named one:
    // absent is not the same answer as empty to a client that dereferences it.
    assert_eq!(
        reported_list(configured, "securityGroups"),
        Some(test.expected_security_groups(network)),
        "case {case:?}: {source} expected securityGroups {:?} as a materialised list, \
         got {configured}",
        test.expected_security_groups(network)
    );
    assert_eq!(
        configured["assignPublicIp"].as_str(),
        Some(test.expected_assign_public_ip),
        "case {case:?}: {source} expected assignPublicIp {:?}, got {configured}",
        test.expected_assign_public_ip
    );
}

#[tokio::test]
async fn a_service_in_ec2_subnets_reads_its_awsvpc_configuration_back_whole() {
    let router = emulator();
    let network = ec2_network(&router).await;
    seed_ecs(&router).await;

    for (index, test) in cases().iter().enumerate() {
        let case = test.name;
        let name = format!("awsvpcsvc{index}");
        let created = ecs_ok(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": name,
                "taskDefinition": test.family(),
                "desiredCount": 0,
                "launchType": test.launch_type,
                "networkConfiguration": test.request(&network),
            }),
            case,
        )
        .await;
        let created = created["service"].clone();
        assert_eq!(
            created["serviceName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: the create reports the service it was asked for, got {created}"
        );

        // The create's own answer, the deployment the rollout is, and the later read of
        // the same service are projections of ONE stored configuration, so all of them
        // report it the same way.
        assert_stored_configuration(&created, "CreateService", test, &network);
        assert_stored_configuration(
            &primary_deployment(&created, case),
            "the PRIMARY deployment of CreateService",
            test,
            &network,
        );

        let read = ecs_ok(
            &router,
            "DescribeServices",
            json!({ "cluster": CLUSTER, "services": [name] }),
            case,
        )
        .await;
        let read = read["services"]
            .as_array()
            .and_then(|services| services.first())
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected the service to be described"));
        assert_stored_configuration(&read, "DescribeServices", test, &network);
        assert_stored_configuration(
            &primary_deployment(&read, case),
            "the PRIMARY deployment of DescribeServices",
            test,
            &network,
        );
    }
}

#[tokio::test]
async fn an_awsvpc_launch_that_says_nothing_about_its_interface_is_refused() {
    let router = emulator();
    seed_ecs(&router).await;

    // Both operations that launch the revision are refused the same way, because the rule
    // is a property of the revision they run rather than of either operation.
    let launches: Vec<(&str, Value)> = vec![
        (
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": "nonetwork",
                "taskDefinition": FAMILY,
                "desiredCount": 0,
                "launchType": "EC2",
            }),
        ),
        (
            "RunTask",
            json!({
                "cluster": CLUSTER,
                "taskDefinition": FAMILY,
                "launchType": "EC2",
            }),
        ),
    ];

    for (operation, request) in launches {
        let case = format!("{operation} with no networkConfiguration");
        let (status, body) = ecs(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the launch to be refused, got {status} with {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|wire| wire.contains("InvalidParameterException")),
            "case {case:?}: expected an InvalidParameterException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(NETWORK_CONFIGURATION_REQUIRED),
            "case {case:?}: unexpected refusal message in {body}"
        );
    }

    // And nothing was created: the refused service is not in the store.
    let read = ecs_ok(
        &router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": ["nonetwork"] }),
        "the refused service",
    )
    .await;
    assert!(
        read["services"]
            .as_array()
            .is_none_or(|services| services.is_empty()),
        "case \"the refused service\": a refused create must store no service, got {read}"
    );
}

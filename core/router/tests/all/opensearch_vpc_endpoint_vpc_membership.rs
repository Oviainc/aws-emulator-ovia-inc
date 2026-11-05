// SPDX-License-Identifier: CC0-1.0

//! A service-managed OpenSearch VPC endpoint stays in the VPC it was created in.
//!
//! RULE: an endpoint's `VPCDerivedInfo$VPCId` is DERIVED from the subnets it was created
//! in ("Amazon VPC generates this value when you create a VPC") and is a member no call
//! sets, so the VPC an endpoint lives in is fixed at creation: `UpdateVpcEndpoint` serves
//! a change whose subnets are subnets of that VPC, and refuses one naming a subnet of
//! another VPC with the modelled `ValidationException` (HTTP 400), leaving the endpoint's
//! stored subnets, security groups and status exactly as they were.
//!
//! A subnet is Amazon EC2's resource, so which VPC one is in is EC2's to say: this suite
//! lives beside the router because every VPC and subnet below is minted by the real EC2
//! operations on the SAME engine that serves OpenSearch Service, so every id in a request
//! is one EC2 handed out and every refusal is grounded in what EC2 actually answered.
//!
//! The rows differ along what the rule generalises over: how many subnets the change
//! names, which of them is the foreign one and in which position, whether the change also
//! names security groups, and whether it names subnets at all — and every refusal is
//! followed by an accepted change, so a refused call is shown to leave the operation
//! working.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The API version every path of the OpenSearch Service control plane is prefixed with.
const PREFIX: &str = "/2021-01-01";

/// Long enough for the span the clock owes a VPC endpoint's create or update to have
/// fallen due, so the endpoint is idle again and the next change is served.
fn settle_wait() -> Duration {
    service_com_amazonaws_opensearch_amazonopensearchservice::state::PROVISIONING
        + Duration::from_millis(250)
}

/// One emulator carrying Amazon EC2 and Amazon OpenSearch Service over ONE shared
/// engine, as the production router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(engine),
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

/// One answer: its HTTP status and its body as JSON (`Value::Null` when it is not JSON).
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type` of the restJson1 error envelope), or `""` on
    /// success.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The error envelope's message, or `""` when there is none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Call one OpenSearch Service operation over its restJson1 wire.
async fn opensearch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("es.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("es"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    Reply { status, body }
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

/// One VPC with one subnet per zone named, minted by EC2 itself: `(vpc id, subnet ids)`.
async fn vpc_with_subnets(
    router: &MultiServiceRouter,
    cidr_prefix: &str,
    zones: &[&str],
) -> (String, Vec<String>) {
    let vpc = element(
        &ec2(
            router,
            "CreateVpc",
            &[("CidrBlock", &format!("{cidr_prefix}.0.0/16"))],
        )
        .await,
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (index, zone) in zones.iter().enumerate() {
        subnets.push(element(
            &ec2(
                router,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", &format!("{cidr_prefix}.{}.0/24", index + 1)),
                    ("AvailabilityZone", zone),
                ],
            )
            .await,
            "subnetId",
        ));
    }
    (vpc, subnets)
}

/// A VPC-provisioned domain, answering with its ARN — what a `CreateVpcEndpoint`
/// addresses the domain by.
async fn vpc_domain(router: &MultiServiceRouter, name: &str, subnet: &str) -> String {
    let created = opensearch(
        router,
        "POST",
        &format!("{PREFIX}/opensearch/domain"),
        Some(json!({
            "DomainName": name,
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
            "VPCOptions": {
                "SubnetIds": [subnet],
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: CreateDomain({name:?}) must succeed, got {} with {}",
        created.status, created.body
    );
    created.body["DomainStatus"]["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the domain reported no ARN: {}", created.body))
        .to_owned()
}

/// `CreateVpcEndpoint` for `domain_arn` in `subnets`, answering with the endpoint.
async fn create_endpoint(
    router: &MultiServiceRouter,
    domain_arn: &str,
    subnets: &[String],
    security_groups: &[&str],
) -> Value {
    let created = opensearch(
        router,
        "POST",
        &format!("{PREFIX}/opensearch/vpcEndpoints"),
        Some(json!({
            "DomainArn": domain_arn,
            "VpcOptions": {
                "SubnetIds": subnets,
                "SecurityGroupIds": security_groups,
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: CreateVpcEndpoint must succeed, got {} with {}",
        created.status, created.body
    );
    created.body["VpcEndpoint"].clone()
}

/// `UpdateVpcEndpoint` with the `VpcOptions` a row names.
async fn update_endpoint(router: &MultiServiceRouter, id: &str, vpc_options: &Value) -> Reply {
    opensearch(
        router,
        "POST",
        &format!("{PREFIX}/opensearch/vpcEndpoints/update"),
        Some(json!({ "VpcEndpointId": id, "VpcOptions": vpc_options })),
    )
    .await
}

/// The endpoint `DescribeVpcEndpoints` reports for `id`.
async fn read_endpoint(router: &MultiServiceRouter, id: &str) -> Value {
    let read = opensearch(
        router,
        "POST",
        &format!("{PREFIX}/opensearch/vpcEndpoints/describe"),
        Some(json!({ "VpcEndpointIds": [id] })),
    )
    .await;
    assert_eq!(
        read.status, 200,
        "reading endpoint {id:?}: expected 200, got {} with {}",
        read.status, read.body
    );
    read.body["VpcEndpoints"][0].clone()
}

/// One change to apply: how its `VpcOptions` differ from the other rows', and whether the
/// endpoint's own VPC admits it.
struct Row {
    case: &'static str,
    /// The subnets the change names, as `(own VPC, index)` pairs: `true` means a subnet
    /// of the endpoint's own VPC. `None` names no subnets at all.
    named: Option<&'static [(bool, usize)]>,
    /// The security groups the change names, if any.
    security_groups: &'static [&'static str],
    /// Whether the change is served.
    accepted: bool,
}

/// Every row, in the order it is driven: each refusal is followed by an accepted change.
const ROWS: &[Row] = &[
    Row {
        case: "one subnet of the endpoint's own VPC",
        named: Some(&[(true, 1)]),
        security_groups: &[],
        accepted: true,
    },
    Row {
        case: "a subnet of another VPC",
        named: Some(&[(false, 0)]),
        security_groups: &[],
        accepted: false,
    },
    Row {
        case: "two subnets of the own VPC, one per zone",
        named: Some(&[(true, 0), (true, 2)]),
        security_groups: &["sg-0aaa0000000000002"],
        accepted: true,
    },
    Row {
        case: "one own subnet and one of another VPC",
        named: Some(&[(true, 0), (false, 0)]),
        security_groups: &[],
        accepted: false,
    },
    Row {
        case: "the subnet of another VPC named first",
        named: Some(&[(false, 1), (true, 1)]),
        security_groups: &["sg-0aaa0000000000003"],
        accepted: false,
    },
    Row {
        case: "two subnets of another VPC",
        named: Some(&[(false, 0), (false, 1)]),
        security_groups: &[],
        accepted: false,
    },
    Row {
        case: "three subnets of the own VPC",
        named: Some(&[(true, 0), (true, 1), (true, 2)]),
        security_groups: &[],
        accepted: true,
    },
    Row {
        case: "security groups alone, naming no subnet",
        named: None,
        security_groups: &["sg-0aaa0000000000004"],
        accepted: true,
    },
    Row {
        case: "three subnets of the own VPC with the foreign one last",
        named: Some(&[(true, 0), (true, 1), (false, 0)]),
        security_groups: &[],
        accepted: false,
    },
    Row {
        case: "one subnet of the own VPC again",
        named: Some(&[(true, 2)]),
        security_groups: &[],
        accepted: true,
    },
];

/// The `SubnetIds` and `SecurityGroupIds` an endpoint reports, as a pair of sorted lists.
fn placement_of(endpoint: &Value) -> (Vec<String>, Vec<String>) {
    let member = |name: &str| {
        let mut values: Vec<String> = endpoint["VpcOptions"][name]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        values.sort();
        values
    };
    (member("SubnetIds"), member("SecurityGroupIds"))
}

#[tokio::test]
async fn a_vpc_endpoint_cannot_be_moved_into_another_vpc() {
    let router = emulator();

    // The two VPCs every row runs against, created through EC2 itself so every id below
    // is one EC2 minted.
    let (own_vpc, own_subnets) = vpc_with_subnets(
        &router,
        "10.120",
        &["us-west-2a", "us-west-2b", "us-west-2c"],
    )
    .await;
    let (other_vpc, other_subnets) =
        vpc_with_subnets(&router, "10.121", &["us-west-2a", "us-west-2b"]).await;
    assert_ne!(
        own_vpc, other_vpc,
        "the two VPCs must be distinct, got {own_vpc} and {other_vpc}"
    );

    let domain_arn = vpc_domain(&router, "endpoint-vpc-membership", &own_subnets[0]).await;
    let created = create_endpoint(
        &router,
        &domain_arn,
        &own_subnets[..1],
        &["sg-0aaa0000000000001"],
    )
    .await;
    let id = created["VpcEndpointId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the create reported no VpcEndpointId: {created}"))
        .to_owned();

    // The endpoint reports the network EC2 holds for the subnet it was created in — the
    // VPC and the zone are read off that answer, which is what fixes the VPC below.
    assert_eq!(
        created["VpcOptions"]["VPCId"].as_str(),
        Some(own_vpc.as_str()),
        "the endpoint reports the VPC its subnet is in, got {created}"
    );
    assert_eq!(
        created["VpcOptions"]["AvailabilityZones"],
        json!(["us-west-2a"]),
        "the endpoint reports the zone its subnet is in, got {created}"
    );

    // The create's own span falls due before the first change is made.
    tokio::time::sleep(settle_wait()).await;

    for row in ROWS {
        let case = row.case;
        let named: Option<Vec<String>> = row.named.map(|named| {
            named
                .iter()
                .map(|(own, index)| {
                    if *own {
                        own_subnets[*index].clone()
                    } else {
                        other_subnets[*index].clone()
                    }
                })
                .collect()
        });
        let mut vpc_options = json!({});
        if let Some(subnets) = named.as_ref() {
            vpc_options["SubnetIds"] = json!(subnets);
        }
        if !row.security_groups.is_empty() {
            vpc_options["SecurityGroupIds"] = json!(row.security_groups);
        }

        let before = read_endpoint(&router, &id).await;
        let answer = update_endpoint(&router, &id, &vpc_options).await;
        if row.accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the change to be served, got {} with {}",
                answer.status, answer.body
            );
            let applied = &answer.body["VpcEndpoint"];
            if let Some(subnets) = named.as_ref() {
                let (reported, _) = placement_of(applied);
                let mut expected = subnets.clone();
                expected.sort();
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the endpoint to carry {expected:?}, got {applied}"
                );
            }
            // The VPC the endpoint is in never changes, whichever of its subnets it is
            // moved between.
            assert_eq!(
                applied["VpcOptions"]["VPCId"].as_str(),
                Some(own_vpc.as_str()),
                "case {case:?}: the endpoint stays in its own VPC, got {applied}"
            );
            // The change's span falls due before the next row's.
            tokio::time::sleep(settle_wait()).await;
        } else {
            let foreign = named
                .as_ref()
                .and_then(|subnets| {
                    subnets
                        .iter()
                        .find(|subnet| other_subnets.contains(subnet))
                        .cloned()
                })
                .unwrap_or_else(|| panic!("case {case:?}: the row names a foreign subnet"));
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, "ValidationException"),
                "case {case:?}: expected a 400 ValidationException, got {} with {}",
                answer.status,
                answer.body
            );
            assert!(
                answer.message().contains(&foreign) && answer.message().contains(&own_vpc),
                "case {case:?}: expected the refusal to name the subnet {foreign:?} and the \
                 endpoint's VPC {own_vpc:?}, got {}",
                answer.body
            );

            // A refused change leaves the endpoint exactly as it was — its subnets, its
            // security groups and the status it was resting in.
            let after = read_endpoint(&router, &id).await;
            assert_eq!(
                placement_of(&after),
                placement_of(&before),
                "case {case:?}: a refused change leaves the placement standing, got {after}"
            );
            assert_eq!(
                after["Status"], before["Status"],
                "case {case:?}: a refused change starts no modification, got {after}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack shaped like a CDK L2 VPC — one route table, one subnet
//! association and one default route per subnet, with the Elastic IP and NAT gateway the
//! private subnets egress through — deploys into Amazon EC2, so the route tables, their
//! subnet associations, their routes and the NAT gateways are all readable there.
//!
//! The rule under test: **a stack's declared resource of a type whose owning service this
//! emulator serves is CREATED in that service through that service's own API, so what the
//! owning service's reads report for the deployed estate is what the template declared** —
//! every `AWS::EC2::RouteTable` is a table `DescribeRouteTables` reports for the VPC beside
//! its implicit main table, every `AWS::EC2::SubnetRouteTableAssociation` is the one
//! explicit association `DescribeRouteTables` filtered by `association.subnet-id` reports
//! for that subnet, every `AWS::EC2::Route` is a route in that table, and every
//! `AWS::EC2::NatGateway` is a gateway `DescribeNatGateways` reports and whose state
//! settles.
//!
//! The rows differ along the COUNT the rule generalises over — how many Availability Zones
//! the VPC spans, hence how many subnets, route tables, associations, routes and NAT
//! gateways the template declares — and along whether the private subnets egress through a
//! NAT gateway at all (the isolated row declares none, as a CDK `PRIVATE_ISOLATED` subnet
//! group does).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use bytes::Bytes;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{
    block_of, elements, form_encode, items_of, members_of, opt_element, outputs_of,
};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The VPC block every row is declared with, and the destination every default route is
/// declared for.
const VPC_CIDR: &str = "10.0.0.0/16";
const ALL_IPV4: &str = "0.0.0.0/0";

/// How long a poll of a stack's status waits between reads, and how many times it reads.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_ATTEMPTS: usize = 60;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One form-encoded call on a query wire, as its HTTP status and body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon EC2 operation over its ec2Query wire.
async fn ec2(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "ec2", "2016-11-15", action, members).await
}

/// Read a stack until it reports a status no longer in progress, and answer that read's body.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        tokio::time::sleep(POLL_INTERVAL).await;
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        described = body;
        if !opt_element(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    described
}

/// Delete a stack and read it until it is no longer reported in progress: the answer is the
/// status it ended on, which is empty once the stack is no longer described at all — the
/// terminal answer for a deletion.
async fn tear_down(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let (status, body) = cloudformation(router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    let mut reported = String::new();
    for _ in 0..POLL_ATTEMPTS {
        tokio::time::sleep(POLL_INTERVAL).await;
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        // A deleted stack is no longer described, which is terminal for a deletion.
        if status != 200 {
            return String::new();
        }
        reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            break;
        }
    }
    reported
}

/// What the stack's own events say about every resource that failed.
async fn failures(router: &MultiServiceRouter, stack_name: &str) -> String {
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    members_of(&events)
        .into_iter()
        .filter(|member| member.contains("_FAILED"))
        .map(|member| {
            format!(
                "{:?} {:?} {:?}",
                opt_element(member, "LogicalResourceId").unwrap_or_default(),
                opt_element(member, "ResourceStatus").unwrap_or_default(),
                opt_element(member, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Deploy a stack and assert it settled at `CREATE_COMPLETE`, quoting what failed when it
/// did not. Answers the `DescribeStacks` body that reported it.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &Value,
) -> String {
    let template = template.to_string();
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let described = poll_to_terminal(router, case, stack_name).await;
    let reported = opt_element(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        reported, "CREATE_COMPLETE",
        "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {reported:?}; its \
         failed resources: {}",
        failures(router, stack_name).await
    );
    described
}

/// One row of the table: how many Availability Zones the VPC spans, and whether its private
/// subnets egress through a NAT gateway.
struct Row {
    case: &'static str,
    zones: usize,
    /// How many NAT gateways the template declares: one per zone for the CDK default, one
    /// for the `natGateways=1` override, none for a `PRIVATE_ISOLATED` subnet group.
    nat_gateways: usize,
}

const ROWS: &[Row] = &[
    Row {
        case: "one zone, one NAT gateway",
        zones: 1,
        nat_gateways: 1,
    },
    Row {
        case: "two zones, one NAT gateway per zone",
        zones: 2,
        nat_gateways: 2,
    },
    Row {
        case: "two zones, one NAT gateway for both",
        zones: 2,
        nat_gateways: 1,
    },
    Row {
        case: "three zones, isolated private subnets with no egress route",
        zones: 3,
        nat_gateways: 0,
    },
];

/// The tag set CDK puts on each resource of a VPC: its path in the construct tree.
fn name_tag(value: &str) -> Value {
    json!([{ "Key": "Name", "Value": value }])
}

/// The template a CDK L2 `ec2.Vpc` synthesises, written out: the VPC, its internet gateway
/// and the attachment that puts it on the VPC, then per zone a public and a private subnet,
/// each with its OWN route table, the association that puts the subnet on it and the default
/// route out of it — the public one through the internet gateway, the private one through a
/// NAT gateway, which is itself declared in the public subnet with the Elastic IP it
/// consumes.
fn vpc_template(row: &Row) -> Value {
    let mut resources = Map::new();
    resources.insert(
        "Vpc".to_string(),
        json!({
            "Type": "AWS::EC2::VPC",
            "Properties": {
                "CidrBlock": VPC_CIDR,
                "EnableDnsHostnames": true,
                "EnableDnsSupport": true,
                "InstanceTenancy": "default",
                "Tags": name_tag("stack/Vpc"),
            },
        }),
    );
    resources.insert(
        "VpcIGW".to_string(),
        json!({
            "Type": "AWS::EC2::InternetGateway",
            "Properties": { "Tags": name_tag("stack/Vpc") },
        }),
    );
    resources.insert(
        "VpcVPCGW".to_string(),
        json!({
            "Type": "AWS::EC2::VPCGatewayAttachment",
            "Properties": {
                "VpcId": { "Ref": "Vpc" },
                "InternetGatewayId": { "Ref": "VpcIGW" },
            },
        }),
    );
    let mut outputs = Map::new();
    outputs.insert("VpcId".to_string(), json!({ "Value": { "Ref": "Vpc" } }));
    for zone in 0..row.zones {
        let index = zone + 1;
        // The two subnets of the zone, each a quarter of the VPC's block apart, as CDK
        // carves one /18 per subnet of a two-group, three-zone VPC.
        for (group, public) in [("Public", true), ("Private", false)] {
            let subnet = format!("{group}Subnet{index}");
            let table = format!("{subnet}RouteTable");
            let association = format!("{subnet}RouteTableAssociation");
            let route = format!("{subnet}DefaultRoute");
            let block = format!(
                "10.0.{}.0/20",
                16 * (zone * 2 + usize::from(!public))
            );
            let mut properties = json!({
                "VpcId": { "Ref": "Vpc" },
                "AvailabilityZone": format!("{REGION}{}", ["a", "b", "c"][zone]),
                "CidrBlock": block,
                "Tags": name_tag(&format!("stack/Vpc/{subnet}")),
            });
            if public {
                if let Some(members) = properties.as_object_mut() {
                    members.insert("MapPublicIpOnLaunch".to_string(), json!(true));
                }
            }
            resources.insert(
                subnet.clone(),
                json!({ "Type": "AWS::EC2::Subnet", "Properties": properties }),
            );
            resources.insert(
                table.clone(),
                json!({
                    "Type": "AWS::EC2::RouteTable",
                    "Properties": {
                        "VpcId": { "Ref": "Vpc" },
                        "Tags": name_tag(&format!("stack/Vpc/{subnet}")),
                    },
                }),
            );
            resources.insert(
                association.clone(),
                json!({
                    "Type": "AWS::EC2::SubnetRouteTableAssociation",
                    "Properties": {
                        "RouteTableId": { "Ref": table },
                        "SubnetId": { "Ref": subnet },
                    },
                }),
            );
            outputs.insert(
                format!("{subnet}Id"),
                json!({ "Value": { "Ref": subnet } }),
            );
            outputs.insert(
                format!("{table}Id"),
                json!({ "Value": { "Ref": table } }),
            );
            if public {
                resources.insert(
                    route,
                    json!({
                        "Type": "AWS::EC2::Route",
                        "DependsOn": ["VpcVPCGW"],
                        "Properties": {
                            "RouteTableId": { "Ref": table },
                            "DestinationCidrBlock": ALL_IPV4,
                            "GatewayId": { "Ref": "VpcIGW" },
                        },
                    }),
                );
                // The Elastic IP and the NAT gateway of this zone, where the row declares
                // one for it: CDK puts the gateway in the public subnet of the zone.
                if zone < row.nat_gateways {
                    let eip = format!("{subnet}EIP");
                    let nat = format!("{subnet}NATGateway");
                    resources.insert(
                        eip.clone(),
                        json!({
                            "Type": "AWS::EC2::EIP",
                            "Properties": {
                                "Domain": "vpc",
                                "Tags": name_tag(&format!("stack/Vpc/{subnet}")),
                            },
                        }),
                    );
                    resources.insert(
                        nat.clone(),
                        json!({
                            "Type": "AWS::EC2::NatGateway",
                            "DependsOn": [association],
                            "Properties": {
                                "SubnetId": { "Ref": subnet },
                                "AllocationId": { "Fn::GetAtt": [eip, "AllocationId"] },
                                "Tags": name_tag(&format!("stack/Vpc/{subnet}")),
                            },
                        }),
                    );
                    outputs.insert(
                        format!("{nat}Id"),
                        json!({ "Value": { "Ref": nat } }),
                    );
                }
            } else if row.nat_gateways > 0 {
                // The private subnet egresses through the NAT gateway of its own zone where
                // the row declares one per zone, and through the single one otherwise.
                let serving = zone.min(row.nat_gateways - 1) + 1;
                let nat = format!("PublicSubnet{serving}NATGateway");
                resources.insert(
                    route,
                    json!({
                        "Type": "AWS::EC2::Route",
                        "Properties": {
                            "RouteTableId": { "Ref": table },
                            "DestinationCidrBlock": ALL_IPV4,
                            "NatGatewayId": { "Ref": nat },
                        },
                    }),
                );
            }
        }
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": Value::Object(resources),
        "Outputs": Value::Object(outputs),
    })
}

/// Every route table, subnet association, route and NAT gateway a CDK-shaped VPC stack
/// declares is one Amazon EC2 really holds: the VPC carries one explicit table per subnet
/// beside its implicit main table, each subnet's own table is the ONE explicitly associated
/// with it, each table carries the default route its template declared, and every NAT
/// gateway settles in a state `DescribeNatGateways` reports.
#[tokio::test]
async fn a_cdk_shaped_vpc_stack_deploys_its_route_tables_and_nat_gateways() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let stack_name = format!("cdk-vpc-{}-{}", row.zones, row.nat_gateways);
        let described = deploy(&router, case, &stack_name, &vpc_template(row)).await;
        let outputs = outputs_of(&described);
        let vpc_id = outputs.get("VpcId").cloned().unwrap_or_default();

        // Every subnet of the VPC: two per zone.
        let subnets: Vec<String> = (1..=row.zones)
            .flat_map(|index| {
                [
                    format!("PublicSubnet{index}Id"),
                    format!("PrivateSubnet{index}Id"),
                ]
            })
            .map(|key| outputs.get(&key).cloned().unwrap_or_default())
            .collect();

        // One table per subnet PLUS the implicit main table the VPC always carries.
        let (status, tables) = ec2(
            &router,
            "DescribeRouteTables",
            &[("Filter.1.Name", "vpc-id"), ("Filter.1.Value.1", &vpc_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeRouteTables, got {status} with body \
             {tables}"
        );
        // Each table's own id, as the DISTINCT set of them the answer carries: the id is
        // written both on the table and on each association reported with it.
        let reported: BTreeSet<String> = elements(&tables, "routeTableId").into_iter().collect();
        let expected = 2 * row.zones + 1;
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected the VPC to carry {expected} route tables (one per subnet \
             plus its implicit main table), got {reported:?}"
        );

        // Exactly one EXPLICITLY associated table per subnet, and it is the table the
        // template declared for that subnet.
        for (index, subnet) in subnets.iter().enumerate() {
            let group = if index % 2 == 0 { "Public" } else { "Private" };
            let key = format!("{group}Subnet{}RouteTableId", index / 2 + 1);
            let declared = outputs.get(&key).cloned().unwrap_or_default();
            let (status, answered) = ec2(
                &router,
                "DescribeRouteTables",
                &[
                    ("Filter.1.Name", "association.subnet-id"),
                    ("Filter.1.Value.1", subnet),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeRouteTables, got {status} with \
                 body {answered}"
            );
            let associated: BTreeSet<String> =
                elements(&answered, "routeTableId").into_iter().collect();
            assert_eq!(
                associated,
                BTreeSet::from([declared.clone()]),
                "case {case:?}: expected the subnet {subnet} to have exactly one explicitly \
                 associated route table ({declared:?}), got {associated:?}"
            );
        }

        // Each table carries the default route its template declared: the public tables out
        // through the internet gateway, the private ones through a NAT gateway where the row
        // declares one, and no route out of the VPC at all where it declares none.
        for index in 1..=row.zones {
            for group in ["Public", "Private"] {
                let table_id = outputs
                    .get(&format!("{group}Subnet{index}RouteTableId"))
                    .cloned()
                    .unwrap_or_default();
                let (_, answered) = ec2(
                    &router,
                    "DescribeRouteTables",
                    &[("RouteTableId.1", &table_id)],
                )
                .await;
                let routes = block_of(&answered, "routeSet").unwrap_or_default();
                let default: Vec<&str> = items_of(routes)
                    .into_iter()
                    .filter(|route| {
                        opt_element(route, "destinationCidrBlock").as_deref() == Some(ALL_IPV4)
                    })
                    .collect();
                let wanted = usize::from(group == "Public" || row.nat_gateways > 0);
                assert_eq!(
                    default.len(),
                    wanted,
                    "case {case:?}: expected {group} subnet {index}'s table to carry {wanted} \
                     default route(s), got {default:?} from {answered}"
                );
                let Some(route) = default.first() else {
                    continue;
                };
                let target = if group == "Public" {
                    opt_element(route, "gatewayId")
                } else {
                    opt_element(route, "natGatewayId")
                };
                let prefix = if group == "Public" { "igw-" } else { "nat-" };
                let target = target.unwrap_or_default();
                assert!(
                    target.starts_with(prefix),
                    "case {case:?}: expected {group} subnet {index}'s default route to target a \
                     {prefix}… gateway, got {target:?} from {answered}"
                );
            }
        }

        // Every NAT gateway the stack published is one Amazon EC2 holds, and it settles in a
        // state a client can act on rather than staying pending for ever.
        let declared_nats: BTreeSet<String> = (1..=row.nat_gateways)
            .map(|index| {
                outputs
                    .get(&format!("PublicSubnet{index}NATGatewayId"))
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            declared_nats.len(),
            row.nat_gateways,
            "case {case:?}: expected the stack to publish {} distinct NAT gateway id(s), got \
             {declared_nats:?}",
            row.nat_gateways
        );
        for nat in &declared_nats {
            let mut state = String::new();
            for _ in 0..POLL_ATTEMPTS {
                let (status, answered) =
                    ec2(&router, "DescribeNatGateways", &[("NatGatewayId.1", nat)]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected Amazon EC2 to hold the NAT gateway {nat}, got HTTP \
                     {status} with body {answered}"
                );
                state = opt_element(&answered, "state").unwrap_or_default();
                if state == "available" {
                    break;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            assert_eq!(
                state, "available",
                "case {case:?}: expected the NAT gateway {nat} to settle available, got \
                 {state:?}"
            );
        }

        // The other half of provisioning: the stack's deletion takes what it created away in
        // the service that owns it, and it is not reported complete while Amazon EC2 is still
        // tearing a NAT gateway down.
        let reported = tear_down(&router, case, &stack_name).await;
        assert!(
            reported.is_empty() || reported == "DELETE_COMPLETE",
            "case {case:?}: expected the stack's deletion to complete, got {reported:?}; its \
             failed resources: {}",
            failures(&router, &stack_name).await
        );
        for nat in &declared_nats {
            let (status, answered) =
                ec2(&router, "DescribeNatGateways", &[("NatGatewayId.1", nat)]).await;
            let state = if status == 200 {
                opt_element(&answered, "state").unwrap_or_default()
            } else {
                String::new()
            };
            assert!(
                state.is_empty() || state == "deleted",
                "case {case:?}: expected the deleted NAT gateway {nat} to be gone in Amazon \
                 EC2, got {state:?} from {answered}"
            );
        }
        // Every route table the stack created goes with it: the id it published resolves to
        // nothing afterwards.
        for index in 1..=row.zones {
            for group in ["Public", "Private"] {
                let table_id = outputs
                    .get(&format!("{group}Subnet{index}RouteTableId"))
                    .cloned()
                    .unwrap_or_default();
                let (status, answered) = ec2(
                    &router,
                    "DescribeRouteTables",
                    &[("RouteTableId.1", &table_id)],
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the deleted route table {table_id} to be refused, \
                     got HTTP {status} with body {answered}"
                );
                let code = opt_element(&answered, "Code").unwrap_or_default();
                assert_eq!(
                    code, "InvalidRouteTableID.NotFound",
                    "case {case:?}: expected the deleted {table_id} to be refused with \
                     InvalidRouteTableID.NotFound, got {code:?} from {answered}"
                );
            }
        }
    }
}

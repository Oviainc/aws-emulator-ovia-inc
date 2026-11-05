// SPDX-License-Identifier: CC0-1.0

//! A deployed network stack's route table, its subnet association and its gateway VPC
//! endpoint are REAL Amazon EC2 resources, so the network the stack published actually
//! routes.
//!
//! The rule under test: **a stack's declared resource of a type whose owning service this
//! emulator serves is CREATED in that service, so the physical id the stack publishes for it
//! — the value a `Ref`, an `Fn::GetAtt` or an `Outputs` entry carries — is the identifier that
//! service minted, the resource is readable through that service's own `Describe*`, and the
//! stack's deletion takes it away there.** A resource the stack only RECORDED would publish an
//! identifier of the right shape that resolves to nothing: a subnet would go on using its
//! VPC's main route table, and a gateway endpoint's route would be in no table at all, so the
//! deployed network would report success and route nothing.
//!
//! The first test's rows differ along the resource TYPE — hence the identifier prefix Amazon
//! EC2 publishes for it and the `Describe*` that resolves it — over the resources a
//! deploy-then-operate network stack declares beside the VPC and the subnet: the route table,
//! the association that puts the subnet on it, and the S3 gateway endpoint reached through it.
//! It then reads the one consequence those three have together, which is what the stack exists
//! for: the subnet's route table carries exactly one route out of the VPC, and it is the
//! endpoint's.
//!
//! The second test's rows differ along the COUNT the rule generalises over: a gateway endpoint
//! declared with one, two and three route tables puts its route in every table it names and in
//! no table it does not.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{
    block_of, elements, form_encode, items_of, opt_element, outputs_of, physical_ids_of,
};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The CIDR block every VPC here is declared with, and the subnet block inside it.
const VPC_CIDR: &str = "10.42.0.0/16";
const SUBNET_CIDR: &str = "10.42.1.0/24";

/// The zone the subnet is declared in.
const ZONE: &str = "us-east-1a";

/// The endpoint service the gateway endpoint is declared for: Amazon S3 of this Region, named
/// the way a gateway endpoint's service name is spelled (`com.amazonaws.<region>.<service>`).
const S3_SERVICE_NAME: &str = "com.amazonaws.us-east-1.s3";

/// How many hexadecimal characters an Amazon EC2 resource identifier carries after its prefix:
/// the current (long) id form is 17 (`rtb-0123456789abcdef0`).
const EC2_ID_HEX_DIGITS: usize = 17;

/// How long a poll of a stack's status waits between reads, and how many times it reads: a
/// stack reaches its terminal status once the services that own its resources answer about
/// them, which only a read asks them, so a client polls — which is what this is.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLL_ATTEMPTS: usize = 60;

/// The target of the route every route table comes with for its own VPC, which is NOT a route
/// out of it ("every route table contains a local route for communication within the VPC",
/// whose target the API reports as `local`).
const LOCAL_TARGET: &str = "local";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // Amazon EC2, which OWNS every type these templates declare: the stack creates each one
    // through this service's own API, so it is mounted beside CloudFormation on the one engine.
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request and what each handler reads its Region from. The signature is never checked:
/// this router runs with request authentication off.
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

/// Poll a stack until it reports a terminal status, and answer the `DescribeStacks` body that
/// reported it.
async fn await_terminal_status(
    router: &MultiServiceRouter,
    stack_name: &str,
    wanted: &str,
) -> String {
    let mut last = String::new();
    for _ in 0..POLL_ATTEMPTS {
        let (status, described) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        last = described;
        // A deleted stack is no longer described at all, which is the terminal answer for a
        // deletion: the poll stops on it rather than waiting the window out.
        if status != 200 {
            break;
        }
        let reported = opt_element(&last, "StackStatus").unwrap_or_default();
        if reported == wanted || reported.ends_with("_FAILED") {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    last
}

/// One row of the identifier table: the logical id a resource is declared under, the output
/// the stack publishes its `Ref` as, the identifier prefix Amazon EC2 publishes for the type,
/// and how that identifier is resolved in Amazon EC2 — the `Describe*` action, the parameter
/// that names the resource, the answer member carrying the id, and the error code that
/// service refuses an id it holds nothing for with.
///
/// Source: each type's "Return values" section in the AWS CloudFormation resource and property
/// type reference, Amazon EC2's own identifier forms (`vpc-`, `subnet-`, `rtb-`, `vpce-`,
/// `rtbassoc-`) and that service's own `Describe*` members
/// (`smithy_query(query="operation",operation="DescribeRouteTables")`).
struct IdentityRow {
    logical_id: &'static str,
    output_key: &'static str,
    prefix: &'static str,
    /// `None` for a resource no `Describe*` of its own reports: a subnet route table
    /// association is reported ON the route table it is with, which this suite reads there.
    resolution: Option<Resolution>,
}

/// How one identifier is resolved in Amazon EC2.
struct Resolution {
    action: &'static str,
    parameter: &'static str,
    member: &'static str,
    not_found: &'static str,
}

const IDENTITY_ROWS: &[IdentityRow] = &[
    IdentityRow {
        logical_id: "Vpc",
        output_key: "VpcId",
        prefix: "vpc",
        resolution: Some(Resolution {
            action: "DescribeVpcs",
            parameter: "VpcId.1",
            member: "vpcId",
            not_found: "InvalidVpcID.NotFound",
        }),
    },
    IdentityRow {
        logical_id: "Subnet",
        output_key: "SubnetId",
        prefix: "subnet",
        resolution: Some(Resolution {
            action: "DescribeSubnets",
            parameter: "SubnetId.1",
            member: "subnetId",
            not_found: "InvalidSubnetID.NotFound",
        }),
    },
    IdentityRow {
        logical_id: "RouteTable",
        output_key: "RouteTableId",
        prefix: "rtb",
        resolution: Some(Resolution {
            action: "DescribeRouteTables",
            parameter: "RouteTableId.1",
            member: "routeTableId",
            not_found: "InvalidRouteTableID.NotFound",
        }),
    },
    IdentityRow {
        logical_id: "S3Endpoint",
        output_key: "EndpointId",
        prefix: "vpce",
        resolution: Some(Resolution {
            action: "DescribeVpcEndpoints",
            parameter: "VpcEndpointId.1",
            member: "vpcEndpointId",
            not_found: "InvalidVpcEndpointId.NotFound",
        }),
    },
    IdentityRow {
        logical_id: "SubnetAssociation",
        output_key: "AssociationId",
        prefix: "rtbassoc",
        resolution: None,
    },
];

/// Whether `value` is an Amazon EC2 identifier of the published form: `<prefix>-` plus
/// [`EC2_ID_HEX_DIGITS`] lowercase hexadecimal characters.
fn is_ec2_identifier(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(&format!("{prefix}-"))
        .is_some_and(|digits| {
            digits.len() == EC2_ID_HEX_DIGITS
                && digits
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

/// The network stack's template: the VPC, the subnet, the route table the subnet is
/// associated with, and the S3 gateway endpoint that route table reaches S3 through — the
/// shape a client deploys a private network for a managed cluster with. Every output is a
/// `Ref` to one of them, which is what a later workflow hands to the service it operates.
fn network_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a private network reaching Amazon S3 through a gateway endpoint",
        "Resources": {
            "Vpc": {
                "Type": "AWS::EC2::VPC",
                "Properties": {
                    "CidrBlock": VPC_CIDR,
                    "EnableDnsSupport": true,
                    "EnableDnsHostnames": true,
                },
            },
            "Subnet": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "CidrBlock": SUBNET_CIDR,
                    "AvailabilityZone": ZONE,
                },
            },
            "RouteTable": {
                "Type": "AWS::EC2::RouteTable",
                "Properties": { "VpcId": { "Ref": "Vpc" } },
            },
            "SubnetAssociation": {
                "Type": "AWS::EC2::SubnetRouteTableAssociation",
                "Properties": {
                    "SubnetId": { "Ref": "Subnet" },
                    "RouteTableId": { "Ref": "RouteTable" },
                },
            },
            "S3Endpoint": {
                "Type": "AWS::EC2::VPCEndpoint",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "ServiceName": S3_SERVICE_NAME,
                    "VpcEndpointType": "Gateway",
                    "RouteTableIds": [{ "Ref": "RouteTable" }],
                },
            },
        },
        "Outputs": {
            "VpcId": { "Value": { "Ref": "Vpc" } },
            "SubnetId": { "Value": { "Ref": "Subnet" } },
            "RouteTableId": { "Value": { "Ref": "RouteTable" } },
            "AssociationId": { "Value": { "Ref": "SubnetAssociation" } },
            "EndpointId": { "Value": { "Ref": "S3Endpoint" } },
        },
    })
    .to_string()
}

/// Every resource a network stack declares reports the identifier Amazon EC2 minted for it,
/// resolves there through that service's own `Describe*`, and is taken away there when the
/// stack is deleted — and the network the stack deployed routes out of the VPC exactly once,
/// through its gateway endpoint.
#[tokio::test]
async fn a_declared_network_stack_resolves_every_resource_in_amazon_ec2() {
    let router = build_router();
    let stack_name = "network-stack-resources";
    let template = network_template();

    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the network stack\": expected HTTP 200 from CreateStack, got {status} with body \
         {body}"
    );
    let described = await_terminal_status(&router, stack_name, "CREATE_COMPLETE").await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case \"the network stack\": expected the stack to reach CREATE_COMPLETE, got {described}"
    );

    let outputs = outputs_of(&described);
    let (_, resources) = cloudformation(
        &router,
        "DescribeStackResources",
        &[("StackName", stack_name)],
    )
    .await;
    let reported = physical_ids_of(&resources);

    let mut identifiers: BTreeMap<&str, String> = BTreeMap::new();
    for row in IDENTITY_ROWS {
        let case = row.logical_id;
        let published = outputs.get(row.output_key).cloned().unwrap_or_default();
        assert!(
            is_ec2_identifier(&published, row.prefix),
            "case {case:?}: expected the stack output to carry \
             {prefix}-<17 hexadecimal characters>, got {published:?}",
            prefix = row.prefix
        );
        assert_eq!(
            reported.get(row.logical_id).map(String::as_str),
            Some(published.as_str()),
            "case {case:?}: expected the stack to report the physical id its output carries \
             ({published:?}), got {:?}",
            reported.get(row.logical_id)
        );
        // The resource really exists in the service that owns it: its own read resolves the
        // id the stack published, and answers that same id back.
        if let Some(resolution) = &row.resolution {
            let (status, answered) = ec2(
                &router,
                resolution.action,
                &[(resolution.parameter, &published)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected Amazon EC2 to hold {published}, got HTTP {status} from \
                 {action} with body {answered}",
                action = resolution.action
            );
            let resolved = elements(&answered, resolution.member);
            assert!(
                resolved.contains(&published),
                "case {case:?}: expected {action} to report {published:?}, got {resolved:?}",
                action = resolution.action
            );
        }
        identifiers.insert(row.logical_id, published);
    }

    // The association is reported ON the route table it is with, which is where a client
    // reads which table a subnet uses: the id the stack published names it, and it names the
    // subnet the template associated.
    let route_table_id = identifiers
        .get("RouteTable")
        .cloned()
        .unwrap_or_default();
    let (_, table) = ec2(
        &router,
        "DescribeRouteTables",
        &[("RouteTableId.1", &route_table_id)],
    )
    .await;
    let associations = block_of(&table, "associationSet").unwrap_or_default();
    let association = items_of(associations)
        .into_iter()
        .find(|item| {
            opt_element(item, "routeTableAssociationId").as_deref()
                == identifiers.get("SubnetAssociation").map(String::as_str)
        })
        .unwrap_or_default();
    assert_eq!(
        opt_element(association, "subnetId"),
        identifiers.get("Subnet").cloned(),
        "case \"the subnet association\": expected the table to report the association with \
         {:?}, got {table}",
        identifiers.get("Subnet")
    );

    // The one thing the stack exists for: the subnet's table routes out of the VPC exactly
    // once, and that route is the gateway endpoint's — a prefix-list destination whose target
    // is the endpoint the stack created.
    let routes = block_of(&table, "routeSet").unwrap_or_default();
    let out_of_the_vpc: Vec<&str> = items_of(routes)
        .into_iter()
        .filter(|route| opt_element(route, "gatewayId").as_deref() != Some(LOCAL_TARGET))
        .collect();
    assert_eq!(
        out_of_the_vpc.len(),
        1,
        "case \"the endpoint route\": expected exactly one route out of the VPC, got \
         {out_of_the_vpc:?} from {table}"
    );
    let endpoint_route = out_of_the_vpc.first().copied().unwrap_or_default();
    assert_eq!(
        opt_element(endpoint_route, "gatewayId"),
        identifiers.get("S3Endpoint").cloned(),
        "case \"the endpoint route\": expected the route to target the endpoint {:?}, got \
         {endpoint_route}",
        identifiers.get("S3Endpoint")
    );
    // The destination is the SERVICE's prefix list, which Amazon EC2 resolves to the service
    // the endpoint was declared for — so the route is to Amazon S3 rather than to an id of the
    // right shape.
    let prefix_list_id = opt_element(endpoint_route, "destinationPrefixListId").unwrap_or_default();
    let (status, prefix_lists) = ec2(
        &router,
        "DescribePrefixLists",
        &[("PrefixListId.1", &prefix_list_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the endpoint route\": expected Amazon EC2 to publish the prefix list \
         {prefix_list_id:?}, got HTTP {status} with body {prefix_lists}"
    );
    assert_eq!(
        opt_element(&prefix_lists, "prefixListName").as_deref(),
        Some(S3_SERVICE_NAME),
        "case \"the endpoint route\": expected the route's prefix list to be {S3_SERVICE_NAME}'s, \
         got {prefix_lists}"
    );
    // The local route is still there: a table reaches its own VPC as well as the endpoint.
    let local: Vec<&str> = items_of(routes)
        .into_iter()
        .filter(|route| opt_element(route, "gatewayId").as_deref() == Some(LOCAL_TARGET))
        .collect();
    assert_eq!(
        local.len(),
        1,
        "case \"the local route\": expected the table to keep the local route of its VPC, got \
         {local:?} from {table}"
    );

    // The stack's deletion removes what it created, in the service that owns it: every
    // resource is refused there afterwards, which is the other half of provisioning it.
    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case \"the network stack\": expected HTTP 200 from DeleteStack, got {status} with body \
         {body}"
    );
    await_terminal_status(&router, stack_name, "DELETE_COMPLETE").await;
    for row in IDENTITY_ROWS {
        let Some(resolution) = &row.resolution else {
            continue;
        };
        let case = row.logical_id;
        let id = identifiers.get(row.logical_id).cloned().unwrap_or_default();
        let (status, answered) = ec2(&router, resolution.action, &[(resolution.parameter, &id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {action} on the deleted {id} to be refused, got HTTP \
             {status} with body {answered}",
            action = resolution.action
        );
        let code = opt_element(&answered, "Code").unwrap_or_default();
        assert_eq!(
            code, resolution.not_found,
            "case {case:?}: expected the deleted {id} to be refused with {expected}, got \
             {code:?} from {answered}",
            expected = resolution.not_found
        );
    }
}

/// The template of the count row: three route tables of one VPC, with the gateway endpoint
/// declared for the first `named` of them.
fn endpoint_template(named: usize) -> String {
    let tables = ["TableOne", "TableTwo", "TableThree"];
    let mut resources = json!({
        "Vpc": {
            "Type": "AWS::EC2::VPC",
            "Properties": { "CidrBlock": VPC_CIDR },
        },
        "S3Endpoint": {
            "Type": "AWS::EC2::VPCEndpoint",
            "Properties": {
                "VpcId": { "Ref": "Vpc" },
                "ServiceName": S3_SERVICE_NAME,
                "VpcEndpointType": "Gateway",
                "RouteTableIds": tables
                    .iter()
                    .take(named)
                    .map(|table| json!({ "Ref": table }))
                    .collect::<Vec<Value>>(),
            },
        },
    });
    let mut outputs = json!({ "EndpointId": { "Value": { "Ref": "S3Endpoint" } } });
    for table in tables {
        if let Some(members) = resources.as_object_mut() {
            members.insert(
                table.to_string(),
                json!({
                    "Type": "AWS::EC2::RouteTable",
                    "Properties": { "VpcId": { "Ref": "Vpc" } },
                }),
            );
        }
        if let Some(members) = outputs.as_object_mut() {
            members.insert(table.to_string(), json!({ "Value": { "Ref": table } }));
        }
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": resources,
        "Outputs": outputs,
    })
    .to_string()
}

/// A gateway endpoint's route lands in EVERY route table its declaration names, and in no
/// table it does not — whether it names one of the stack's tables, two of them or all three.
#[tokio::test]
async fn a_gateway_endpoints_route_lands_in_every_route_table_it_names() {
    let router = build_router();
    let tables = ["TableOne", "TableTwo", "TableThree"];

    for named in 1..=tables.len() {
        let case = format!("{named} of {} route tables named", tables.len());
        let stack_name = format!("endpoint-route-tables-{named}");
        let template = endpoint_template(named);
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        let described = await_terminal_status(&router, &stack_name, "CREATE_COMPLETE").await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );
        let outputs = outputs_of(&described);
        let endpoint_id = outputs.get("EndpointId").cloned().unwrap_or_default();

        for (index, table) in tables.iter().enumerate() {
            let route_table_id = outputs.get(*table).cloned().unwrap_or_default();
            let (status, answered) = ec2(
                &router,
                "DescribeRouteTables",
                &[("RouteTableId.1", &route_table_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected Amazon EC2 to hold {route_table_id}, got HTTP {status} \
                 with body {answered}"
            );
            let routes = block_of(&answered, "routeSet").unwrap_or_default();
            let through_the_endpoint: Vec<&str> = items_of(routes)
                .into_iter()
                .filter(|route| opt_element(route, "gatewayId").as_deref() == Some(&endpoint_id))
                .collect();
            let expected = usize::from(index < named);
            assert_eq!(
                through_the_endpoint.len(),
                expected,
                "case {case:?}: expected {table} to carry {expected} route(s) through the \
                 endpoint {endpoint_id}, got {through_the_endpoint:?} from {answered}"
            );
            if expected == 1 {
                let route = through_the_endpoint.first().copied().unwrap_or_default();
                let destination =
                    opt_element(route, "destinationPrefixListId").unwrap_or_default();
                assert!(
                    destination.starts_with("pl-"),
                    "case {case:?}: expected {table}'s endpoint route to match the service's \
                     prefix list, got {destination:?} from {answered}"
                );
            }
        }
    }
}

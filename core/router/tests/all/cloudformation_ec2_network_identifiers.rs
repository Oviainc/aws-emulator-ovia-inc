// SPDX-License-Identifier: CC0-1.0

//! The physical resource id a stack reports for a declared Amazon EC2 VPC-network resource
//! is the IDENTIFIER that type's published `Ref` return value names, not a name.
//!
//! The rule under test: **for a resource type whose "Return values" section documents that
//! `Ref` returns an identifier the owning service assigns, the physical resource id a stack
//! reports — and therefore the value a `Ref` or an `Fn::GetAtt` to that resource resolves
//! to anywhere in the template — is one of that service's own identifier form, never the
//! `<stack name>-<logical id>-<unique id>` name CloudFormation generates for a resource it
//! has to name.** Amazon EC2 takes no name on any of these creates, so a reference to such
//! a resource that carried a generated name would hand the next service a value its API
//! refuses.
//!
//! The rows of the first table differ along what the rule generalises over: the resource
//! TYPE, hence the identifier prefix EC2 publishes for it and the `Fn::GetAtt` attribute
//! that type names the identifier with (a narrower `VpcId`/`SubnetId`/`GroupId`, or the
//! plain `Id` the types that publish no narrower name carry). Each row also covers the
//! invariants the rule carries: one value everywhere the stack reports that resource's
//! identity, two resources of one type in one stack keeping distinct ids, and the id
//! surviving a second read.
//!
//! The second test is the end-to-end consequence this suite exists for, and the one that
//! needs two crates to agree (CloudFormation makes the calls; Route 53 Resolver serves
//! them): one stack that declares the VPC, the subnets and the security group a Route 53
//! Resolver OUTBOUND endpoint is built on, the forwarding rule through that endpoint and
//! the rule's VPC association. Route 53 Resolver bounds a subnet id at 32 characters and
//! takes a VPC id, so that stack only completes when the EC2 resources it declares resolve
//! to EC2 identifiers.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's own clock-owed span and for Route 53 Resolver's provisioning
/// span to come due, so the stack reports its outputs and the association reports the
/// status the clock owed it.
const PAST_PROVISIONING_SPANS: Duration = Duration::from_millis(900);

/// How many hexadecimal characters an Amazon EC2 resource identifier carries after its
/// prefix: the current (long) id form is 17 (`vpc-0123456789abcdef0`).
const EC2_ID_HEX_DIGITS: usize = 17;

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // Amazon EC2, which OWNS the types these rows declare: the two this emulator creates
    // there (`AWS::EC2::VPC` and `AWS::EC2::Subnet`) are created by the stack through this
    // service's own API, so it is mounted beside CloudFormation on the one engine.
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_route53resolver_route53resolver::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Route 53 Resolver operation over its awsJson1_1 wire, signed for `region`.
async fn route53resolver(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("route53resolver.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Route53Resolver.{operation}"))
        .header("authorization", authorization("route53resolver", region))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// A `DescribeStackResources` answer as the logical-id -> physical-id map a client reads it
/// as. Each `member` carries one of each, in document order.
fn physical_ids_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "LogicalResourceId")
        .into_iter()
        .zip(elements(xml, "PhysicalResourceId"))
        .collect()
}

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

/// One row of the identifier table: an Amazon EC2 VPC-network resource type, the identifier
/// prefix that service publishes for it, the `Fn::GetAtt` attribute the type names that
/// identifier with, the properties a declaration of it carries (none for the types that
/// require nothing) and the resources its declaration has to be deployed beside.
///
/// `properties` takes the index of the resource inside the row's template (0 and 1, for the
/// two the row declares), so a type whose owning service refuses two resources configured
/// identically — two subnets cannot share a CIDR block inside one VPC — declares a
/// different configuration for each.
///
/// Source: each type's "Return values" section in the AWS CloudFormation resource and
/// property type reference, and Amazon EC2's own identifier forms — `vpc-`, `subnet-`,
/// `sg-`, `igw-`, `rtb-`, `nat-`, `acl-`, `eni-`, `vpce-`, `dopt-`, `vgw-`.
struct IdentityRow {
    resource_type: &'static str,
    prefix: &'static str,
    identifier_attribute: &'static str,
    properties: fn(usize) -> Value,
    /// Further `Resources` members the row's two resources are declared beside, as the
    /// object they are written into the template: a subnet is created IN a VPC, so the row
    /// for it declares the VPC its `VpcId` references.
    supporting: fn() -> Value,
}

/// The CIDR block the rows that declare one use, and the subnet block inside it.
const VPC_CIDR: &str = "10.77.0.0/16";

/// No further resources: what every row whose resource is created on its own is deployed
/// beside.
fn nothing_else() -> Value {
    json!({})
}

/// The VPC a row whose resource is created INSIDE one declares it beside: the types this
/// emulator creates in Amazon EC2 are created in the VPC their declaration names, so that VPC
/// has to be a resource of the same stack rather than an id nothing holds.
fn supporting_vpc() -> Value {
    json!({ "Vpc": { "Type": "AWS::EC2::VPC", "Properties": { "CidrBlock": VPC_CIDR } } })
}

const IDENTITY_ROWS: &[IdentityRow] = &[
    IdentityRow {
        resource_type: "AWS::EC2::VPC",
        prefix: "vpc",
        identifier_attribute: "VpcId",
        properties: |_| json!({ "CidrBlock": VPC_CIDR }),
        supporting: nothing_else,
    },
    IdentityRow {
        resource_type: "AWS::EC2::Subnet",
        prefix: "subnet",
        identifier_attribute: "SubnetId",
        // A subnet's block has to sit inside its VPC's and must not overlap another subnet's
        // in the same VPC, so each of the row's two subnets carves out its own /24.
        properties: |index| {
            json!({
                "VpcId": { "Ref": "Vpc" },
                "CidrBlock": format!("10.77.{}.0/24", index + 1),
            })
        },
        supporting: supporting_vpc,
    },
    IdentityRow {
        resource_type: "AWS::EC2::SecurityGroup",
        prefix: "sg",
        identifier_attribute: "GroupId",
        properties: |_| json!({ "GroupDescription": "the identifier table group" }),
        supporting: nothing_else,
    },
    IdentityRow {
        resource_type: "AWS::EC2::InternetGateway",
        prefix: "igw",
        identifier_attribute: "InternetGatewayId",
        properties: |_| json!({}),
        supporting: nothing_else,
    },
    IdentityRow {
        resource_type: "AWS::EC2::RouteTable",
        prefix: "rtb",
        identifier_attribute: "RouteTableId",
        properties: |_| json!({ "VpcId": { "Ref": "Vpc" } }),
        supporting: supporting_vpc,
    },
    IdentityRow {
        resource_type: "AWS::EC2::NatGateway",
        prefix: "nat",
        identifier_attribute: "NatGatewayId",
        properties: |_| json!({ "SubnetId": "subnet-0a1b2c3d4e5f60718" }),
        supporting: nothing_else,
    },
    IdentityRow {
        resource_type: "AWS::EC2::DHCPOptions",
        prefix: "dopt",
        identifier_attribute: "DhcpOptionsId",
        properties: |_| json!({ "DomainName": "identifiers.test" }),
        supporting: nothing_else,
    },
    IdentityRow {
        resource_type: "AWS::EC2::VPNGateway",
        prefix: "vgw",
        identifier_attribute: "VPNGatewayId",
        properties: |_| json!({ "Type": "ipsec.1" }),
        supporting: nothing_else,
    },
    // The three types whose published return values name the identifier `Id` rather than
    // with a type-specific name, which is the other half of what the rule generalises over.
    IdentityRow {
        resource_type: "AWS::EC2::NetworkAcl",
        prefix: "acl",
        identifier_attribute: "Id",
        properties: |_| json!({ "VpcId": "vpc-0a1b2c3d4e5f60718" }),
        supporting: nothing_else,
    },
    IdentityRow {
        resource_type: "AWS::EC2::NetworkInterface",
        prefix: "eni",
        identifier_attribute: "Id",
        properties: |_| json!({ "SubnetId": "subnet-0a1b2c3d4e5f60718" }),
        supporting: nothing_else,
    },
    IdentityRow {
        resource_type: "AWS::EC2::VPCEndpoint",
        prefix: "vpce",
        identifier_attribute: "Id",
        properties: |_| json!({
            "VpcId": { "Ref": "Vpc" },
            "ServiceName": "com.amazonaws.us-east-1.s3",
        }),
        supporting: supporting_vpc,
    },
];

/// Every Amazon EC2 VPC-network type a stack declares reports the identifier its `Ref`
/// return value publishes: the prefix EC2 mints it under followed by 17 hexadecimal
/// characters, the SAME value from `Ref`, from the type's own identifier attribute and from
/// `DescribeStackResources`, distinct between two resources of one type, and unchanged on a
/// second read.
#[tokio::test]
async fn a_declared_ec2_network_resource_reports_the_identifier_its_type_publishes() {
    let router = build_router();
    let region = "us-east-1";

    for (index, row) in IDENTITY_ROWS.iter().enumerate() {
        let case = row.resource_type;
        let stack_name = format!("ec2-identity-{index}");
        // TWO resources of the type, so the rule's "distinct resources keep distinct
        // identifiers" half is covered by every row rather than by one special case — beside
        // whatever resources the row's own declaration has to sit next to.
        let mut resources = json!({
            "First": { "Type": case, "Properties": (row.properties)(0) },
            "Second": { "Type": case, "Properties": (row.properties)(1) },
        });
        if let (Some(resources), Some(supporting)) =
            (resources.as_object_mut(), (row.supporting)().as_object())
        {
            for (logical_id, declaration) in supporting {
                resources.insert(logical_id.clone(), declaration.clone());
            }
        }
        let template = json!({
            "Resources": resources,
            "Outputs": {
                "FirstRef": { "Value": { "Ref": "First" } },
                "FirstAttribute": {
                    "Value": { "Fn::GetAtt": ["First", row.identifier_attribute] }
                },
                "SecondRef": { "Value": { "Ref": "Second" } },
            }
        })
        .to_string();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_PROVISIONING_SPANS).await;

        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let outputs = outputs_of(&described);
        let first = outputs.get("FirstRef").cloned().unwrap_or_default();
        let second = outputs.get("SecondRef").cloned().unwrap_or_default();
        assert!(
            is_ec2_identifier(&first, row.prefix),
            "case {case:?}: expected Ref to carry {}-<17 hexadecimal characters>, got {first:?}",
            row.prefix
        );
        assert!(
            is_ec2_identifier(&second, row.prefix),
            "case {case:?}: expected the second resource's Ref to carry \
             {}-<17 hexadecimal characters>, got {second:?}",
            row.prefix
        );
        assert_ne!(
            first, second,
            "case {case:?}: expected two resources of one type to carry distinct \
             identifiers, got {first:?} twice"
        );
        assert_eq!(
            outputs.get("FirstAttribute").map(String::as_str),
            Some(first.as_str()),
            "case {case:?}: expected Fn::GetAtt of {} to carry the id Ref answered \
             ({first:?}), got {:?}",
            row.identifier_attribute,
            outputs.get("FirstAttribute")
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let reported = physical_ids_of(&resources);
        assert_eq!(
            reported.get("First").map(String::as_str),
            Some(first.as_str()),
            "case {case:?}: expected the stack to report the physical id Ref answered \
             ({first:?}), got {:?}",
            reported.get("First")
        );
        assert_eq!(
            reported.get("Second").map(String::as_str),
            Some(second.as_str()),
            "case {case:?}: expected the stack to report {second:?} for the second \
             resource, got {:?}",
            reported.get("Second")
        );

        // A read does not change what a later caller sees: the identifiers are the same on
        // the second read as on the first.
        let (_, again) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            physical_ids_of(&again),
            reported,
            "case {case:?}: expected a second read to report the same identifiers, got \
             {again}"
        );
    }
}

/// The template the end-to-end case deploys: the VPC, two subnets and the security group a
/// Route 53 Resolver OUTBOUND endpoint needs, the FORWARD rule through that endpoint, and
/// the rule's association with that VPC — every Resolver property referencing an EC2
/// resource of the same stack.
fn forwarding_stack_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a Resolver forwarding rule over the network the same stack declares",
        "Resources": {
            "Vpc": {
                "Type": "AWS::EC2::VPC",
                "Properties": { "CidrBlock": "10.81.0.0/16" }
            },
            "SubnetA": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "CidrBlock": "10.81.1.0/24",
                    "AvailabilityZone": { "Fn::Select": [0, { "Fn::GetAZs": "" }] }
                }
            },
            "SubnetB": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "CidrBlock": "10.81.2.0/24",
                    "AvailabilityZone": { "Fn::Select": [1, { "Fn::GetAZs": "" }] }
                }
            },
            "Sg": {
                "Type": "AWS::EC2::SecurityGroup",
                "Properties": {
                    "GroupDescription": "outbound DNS",
                    "VpcId": { "Ref": "Vpc" }
                }
            },
            "OutboundEndpoint": {
                "Type": "AWS::Route53Resolver::ResolverEndpoint",
                "Properties": {
                    "Direction": "OUTBOUND",
                    "Name": "cfn-forward-endpoint",
                    "IpAddresses": [
                        { "SubnetId": { "Ref": "SubnetA" } },
                        { "SubnetId": { "Ref": "SubnetB" } }
                    ],
                    "SecurityGroupIds": [{ "Ref": "Sg" }]
                }
            },
            "Rule": {
                "Type": "AWS::Route53Resolver::ResolverRule",
                "Properties": {
                    "RuleType": "FORWARD",
                    "DomainName": "cfn-forward.test.",
                    "Name": "cfn-forward-rule",
                    "ResolverEndpointId": { "Ref": "OutboundEndpoint" },
                    "TargetIps": [{ "Ip": "10.254.254.254", "Port": 53 }]
                }
            },
            "RuleAssociation": {
                "Type": "AWS::Route53Resolver::ResolverRuleAssociation",
                "Properties": {
                    "ResolverRuleId": { "Ref": "Rule" },
                    "VPCId": { "Ref": "Vpc" }
                }
            }
        },
        "Outputs": {
            "RuleId": { "Value": { "Ref": "Rule" } },
            "AssociationId": { "Value": { "Ref": "RuleAssociation" } },
            "VpcId": { "Value": { "Ref": "Vpc" } },
            "SubnetAId": { "Value": { "Ref": "SubnetA" } },
            "EndpointId": { "Value": { "Ref": "OutboundEndpoint" } }
        }
    })
    .to_string()
}

/// A stack that declares its own VPC, subnets and security group beside a Route 53 Resolver
/// outbound endpoint, forwarding rule and rule association CREATES all of them: the EC2
/// resources resolve to EC2 identifiers, so every Resolver create takes the values the
/// template wired together, and the association Route 53 Resolver holds names the rule and
/// the VPC the stack reports.
#[tokio::test]
async fn a_stack_forwards_through_the_network_it_declares_itself() {
    let case = "a Resolver forwarding rule over the stack's own VPC, subnets and group";
    let router = build_router();
    let region = "us-east-1";
    let stack_name = "ec2-resolver-forwarding";
    let template = forwarding_stack_template();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_PROVISIONING_SPANS).await;

    let (status, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {described}"
    );
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
    );

    let outputs = outputs_of(&described);
    for (key, prefix) in [
        ("VpcId", "vpc"),
        ("SubnetAId", "subnet"),
        ("RuleId", "rslvr-rr"),
        ("AssociationId", "rslvr-rrassoc"),
        ("EndpointId", "rslvr-out"),
    ] {
        let value = outputs.get(key).cloned().unwrap_or_default();
        assert!(
            value.starts_with(&format!("{prefix}-")),
            "case {case:?}: expected output {key:?} to carry a {prefix}- identifier, got \
             {value:?} in {outputs:?}"
        );
    }

    let rule_id = outputs.get("RuleId").cloned().unwrap_or_default();
    let association_id = outputs.get("AssociationId").cloned().unwrap_or_default();
    let vpc_id = outputs.get("VpcId").cloned().unwrap_or_default();

    // The association Route 53 Resolver holds under the id the stack published: it names the
    // rule and the VPC the template wired together, and the clock has made its status due.
    let (status, answered) = route53resolver(
        &router,
        region,
        "GetResolverRuleAssociation",
        &json!({ "ResolverRuleAssociationId": association_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetResolverRuleAssociation, got {status} with \
         body {answered}"
    );
    let association = answered
        .get("ResolverRuleAssociation")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        association.get("Status").and_then(Value::as_str),
        Some("COMPLETE"),
        "case {case:?}: expected the association to have settled at COMPLETE, got \
         {association}"
    );
    assert_eq!(
        association.get("ResolverRuleId").and_then(Value::as_str),
        Some(rule_id.as_str()),
        "case {case:?}: expected the association to name the rule {rule_id:?}, got \
         {association}"
    );
    assert_eq!(
        association.get("VPCId").and_then(Value::as_str),
        Some(vpc_id.as_str()),
        "case {case:?}: expected the association to name the VPC {vpc_id:?}, got {association}"
    );

    // The rule itself: the forwarding target the template declared, through the endpoint it
    // declared.
    let (status, answered) = route53resolver(
        &router,
        region,
        "GetResolverRule",
        &json!({ "ResolverRuleId": rule_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetResolverRule, got {status} with body \
         {answered}"
    );
    let rule = answered.get("ResolverRule").cloned().unwrap_or(Value::Null);
    assert_eq!(
        rule.get("RuleType").and_then(Value::as_str),
        Some("FORWARD"),
        "case {case:?}: expected the declared RuleType, got {rule}"
    );
    assert_eq!(
        rule.get("DomainName").and_then(Value::as_str),
        Some("cfn-forward.test."),
        "case {case:?}: expected the declared DomainName, got {rule}"
    );
    assert_eq!(
        rule.get("ResolverEndpointId").and_then(Value::as_str),
        outputs.get("EndpointId").map(String::as_str),
        "case {case:?}: expected the rule to route through the declared endpoint, got {rule}"
    );
    let targets = rule
        .get("TargetIps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        targets.len(),
        1,
        "case {case:?}: expected the one declared target address, got {rule}"
    );
    assert_eq!(
        (
            targets[0].get("Ip").and_then(Value::as_str),
            targets[0].get("Port").and_then(Value::as_i64),
        ),
        (Some("10.254.254.254"), Some(53)),
        "case {case:?}: expected the declared target address, got {rule}"
    );

    // Deleting the stack removes both Resolver resources there.
    let (status, body) =
        cloudformation(&router, region, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
    for (operation, member, id) in [
        ("GetResolverRule", "ResolverRuleId", rule_id.clone()),
        (
            "GetResolverRuleAssociation",
            "ResolverRuleAssociationId",
            association_id.clone(),
        ),
    ] {
        let (status, answered) =
            route53resolver(&router, region, operation, &json!({ member: id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to refuse the deleted {id:?}, got {status} \
             with body {answered}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares `AWS::Route53Resolver::ResolverEndpoint`,
//! `::ResolverRule` and `::ResolverRuleAssociation` really creates them in Route 53
//! Resolver, and the identifiers the stack hands back are the ones that service answers to.
//!
//! The rule under test: **a stack's physical resource id is the identifier the service that
//! owns the resource assigned — for a type whose identifier that service MINTS, the one its
//! create answered with — and a reference to such a resource from another resource of the
//! same template resolves to that same identifier, so `Ref`, `Fn::GetAtt` and the owning
//! service's own read all resolve the resource the template configured.** Two crates have
//! to agree for that to hold (CloudFormation makes the calls; Route 53 Resolver serves them
//! and mints the ids), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: the rule type (`FORWARD` through
//! an outbound endpoint, `SYSTEM` with no endpoint at all, `DELEGATE` with a delegation
//! record), whether the template declares an endpoint and an association, how many target
//! addresses a forwarding rule carries and whether they name a port, whether the domain
//! name is written fully qualified or not, whether the VPC and subnets are literal ids or
//! `Ref`s to `AWS::EC2::*` resources the stack declares itself, whether the template is
//! JSON or YAML, and which intrinsic each output reads a resource with (`Ref`, `Fn::GetAtt`
//! of the conventional `Id` and of the type's own `<Type>Id`, `Fn::Sub`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the three identifiers themselves, which are
//! Route 53 Resolver's to mint: what is checked about those is that ONE value appears
//! everywhere the stack reports a resource's identity, that it has the form that service
//! mints, and that that service's own `GetResolverEndpoint` / `GetResolverRule` /
//! `GetResolverRuleAssociation` resolve it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};

/// Long enough for the stack's own clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) and
/// for Route 53 Resolver's provisioning span (`PROVISIONING_DELAY`, 500 ms) to come due, so
/// the stack reports the template it deployed and the resources report their settled
/// statuses.
const PAST_PROVISIONING_SPANS: Duration = Duration::from_millis(900);

/// The prefixes Route 53 Resolver mints its three identifiers under: an outbound endpoint
/// is `rslvr-out-…`, a rule `rslvr-rr-…` and a rule association `rslvr-rrassoc-…`.
const ENDPOINT_ID_PREFIX: &str = "rslvr-out-";
const RULE_ID_PREFIX: &str = "rslvr-rr-";
const ASSOCIATION_ID_PREFIX: &str = "rslvr-rrassoc-";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request either
/// way (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // The network the literal-id rows name is the caller's own: it exists before any stack
    // is deployed, and no stack here declares it.
    register_literal_network(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // Amazon EC2, for the `AWS::EC2::VPC` a row declares: a stack creates one in the
    // service that owns it, so that service has to be mounted beside CloudFormation.
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_route53resolver_route53resolver::EmulatedService::new(engine),
    );
    router
}

/// Put the VPC and the two subnets the literal-id rows name into the account's own
/// Amazon EC2 store, through the fixture every suite naming a subnet literally shares
/// ([`crate::ec2_network::register_network`]).
///
/// A Resolver endpoint's IP addresses are placed IN subnets of the caller's VPC, which
/// Route 53 Resolver resolves against that store, so a stack that names those subnets
/// literally is a stack deployed into a network the account already holds — the rows that
/// declare `AWS::EC2::*` resources themselves need none of this, because the stack creates
/// them through Amazon EC2's own API.
fn register_literal_network(engine: &engine::InMemoryEngine) {
    crate::ec2_network::register_network(
        engine,
        LITERAL_VPC_ID,
        &LITERAL_SUBNET_IDS,
        "us-east-1a",
    );
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`,
/// which is what routes the request and what each handler reads its Region from. The
/// signature is never checked: this router runs with request authentication off.
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

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
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

/// One target address a forwarding rule must report: the IP and the port.
type ExpectedTarget = (&'static str, i64);

/// One output whose value is COMPUTED from a minted identifier: the key, and the value the
/// rule says it must carry once Route 53 Resolver has minted that id.
type DerivedOutput = (&'static str, fn(&str) -> String);

/// One row of the table: a template, and what the rule says the Resolver resources it
/// declares must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is also the Region the resources are created
    /// in and the one their ARNs carry.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The logical id of the declared rule, and of the endpoint and association where the
    /// row declares them.
    rule_logical_id: &'static str,
    endpoint_logical_id: Option<&'static str>,
    association_logical_id: Option<&'static str>,
    /// The `RuleType` `GetResolverRule` must report.
    expected_rule_type: &'static str,
    /// The `DomainName` it must report: the fully-qualified form of what the template
    /// declared, whichever form the template wrote.
    expected_domain_name: Option<&'static str>,
    /// The `DelegationRecord` it must report.
    expected_delegation_record: Option<&'static str>,
    /// The target addresses it must report, in the order the template declares them. A
    /// target that names no port forwards to the well-known DNS port.
    expected_targets: &'static [ExpectedTarget],
    /// The `VPCId` the association must report, which the template decides.
    expected_vpc_id: Option<&'static str>,
    /// The `Name` the association must report: the one its declaration asks for, verbatim.
    /// A name that merely CONTAINS the rule's own (`<rule name>-association`) is a value of
    /// its own, not a reference to the rule, so nothing re-points it.
    expected_association_name: Option<&'static str>,
    /// The output keys whose value must be the rule's id, each written with a different
    /// intrinsic over the same resource.
    rule_id_outputs: &'static [&'static str],
    /// The same for the endpoint's and the association's ids.
    endpoint_id_outputs: &'static [&'static str],
    association_id_outputs: &'static [&'static str],
    /// Outputs whose value is COMPUTED from the rule's id: the key, and what the rule says
    /// the output must carry once Route 53 Resolver has minted that id.
    derived_outputs: &'static [DerivedOutput],
}

/// The VPC a row names literally, in the form Amazon EC2 ids take.
const LITERAL_VPC_ID: &str = "vpc-0a1b2c3d4e5f60718";

/// The subnets a row names literally, in the form Amazon EC2 ids take.
///
/// `AWS::Route53Resolver::ResolverEndpoint` places its IP addresses in subnets of the
/// caller's VPC, and Route 53 Resolver resolves every `SubnetId` against the account's own
/// Amazon EC2 store, so the account holds these two subnets of [`LITERAL_VPC_ID`] before
/// any stack naming them literally is deployed ([`register_literal_network`]).
const LITERAL_SUBNET_IDS: [&str; 2] = ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"];

const ROWS: &[Row] = &[
    Row {
        case: "a FORWARD rule through an outbound endpoint, associated, JSON, literal ids",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an outbound endpoint, a forwarding rule and its association",
                "Resources": {
                    "Endpoint": {
                        "Type": "AWS::Route53Resolver::ResolverEndpoint",
                        "Properties": {
                            "Direction": "OUTBOUND",
                            "Name": "outbound-endpoint",
                            "SecurityGroupIds": ["sg-0123456789abcdef0"],
                            "IpAddresses": [
                                { "SubnetId": "subnet-0123456789abcdef0" },
                                { "SubnetId": "subnet-0123456789abcdef1" }
                            ],
                            "Tags": [{ "Key": "Owner", "Value": "networking" }]
                        }
                    },
                    "Rule": {
                        "Type": "AWS::Route53Resolver::ResolverRule",
                        "Properties": {
                            "DomainName": "forward.example.test.",
                            "Name": "forward-rule",
                            "RuleType": "FORWARD",
                            "ResolverEndpointId": { "Ref": "Endpoint" },
                            "TargetIps": [
                                { "Ip": "10.254.254.254", "Port": "53" },
                                { "Ip": "10.254.254.253" }
                            ]
                        }
                    },
                    "RuleAssociation": {
                        "Type": "AWS::Route53Resolver::ResolverRuleAssociation",
                        "Properties": {
                            "Name": "forward-rule-association",
                            "ResolverRuleId": { "Ref": "Rule" },
                            "VPCId": LITERAL_VPC_ID
                        }
                    }
                },
                "Outputs": {
                    "EndpointId": { "Value": { "Ref": "Endpoint" } },
                    "RuleId": { "Value": { "Ref": "Rule" } },
                    "RuleIdAttribute": { "Value": { "Fn::GetAtt": ["Rule", "ResolverRuleId"] } },
                    "AssociationId": { "Value": { "Ref": "RuleAssociation" } },
                    "AssociationIdAttribute": {
                        "Value": { "Fn::GetAtt": ["RuleAssociation", "Id"] }
                    },
                    "RuleArn": {
                        "Value": {
                            "Fn::Sub": [
                                "arn:aws:route53resolver:${AWS::Region}:${AWS::AccountId}:resolver-rule/${Id}",
                                { "Id": { "Ref": "Rule" } }
                            ]
                        }
                    }
                }
            })
            .to_string()
        },
        rule_logical_id: "Rule",
        endpoint_logical_id: Some("Endpoint"),
        association_logical_id: Some("RuleAssociation"),
        expected_rule_type: "FORWARD",
        expected_domain_name: Some("forward.example.test."),
        expected_delegation_record: None,
        // The second target names no port, so it forwards to the well-known DNS port
        // (RFC 1035 §4.2), which is not the template's value to decide.
        expected_targets: &[("10.254.254.254", 53), ("10.254.254.253", 53)],
        expected_vpc_id: Some(LITERAL_VPC_ID),
        expected_association_name: Some("forward-rule-association"),
        rule_id_outputs: &["RuleId", "RuleIdAttribute"],
        endpoint_id_outputs: &["EndpointId"],
        association_id_outputs: &["AssociationId", "AssociationIdAttribute"],
        derived_outputs: &[(
            "RuleArn",
            |id| format!("arn:aws:route53resolver:us-east-1:000000000000:resolver-rule/{id}"),
        )],
    },
    Row {
        case: "a SYSTEM rule with no endpoint, associated with the stack's own VPC, YAML",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Vpc:",
                "    Type: AWS::EC2::VPC",
                "    Properties:",
                "      CidrBlock: 10.0.0.0/16",
                "  Rule:",
                "    Type: AWS::Route53Resolver::ResolverRule",
                "    Properties:",
                "      DomainName: apex.example.test",
                "      Name: system-rule",
                "      RuleType: SYSTEM",
                "  RuleAssociation:",
                "    Type: AWS::Route53Resolver::ResolverRuleAssociation",
                "    Properties:",
                "      ResolverRuleId: !Ref Rule",
                "      VPCId: !Ref Vpc",
                "Outputs:",
                "  RuleId:",
                "    Value: !Ref Rule",
                "  AssociationId:",
                "    Value: !Ref RuleAssociation",
                "  AssociationIdAttribute:",
                "    Value: !GetAtt RuleAssociation.ResolverRuleAssociationId",
                "",
            ]
            .join("\n")
        },
        rule_logical_id: "Rule",
        endpoint_logical_id: None,
        association_logical_id: Some("RuleAssociation"),
        expected_rule_type: "SYSTEM",
        // Declared relative, reported fully qualified: Resolver selects on a
        // fully-qualified name whatever form the request wrote.
        expected_domain_name: Some("apex.example.test."),
        expected_delegation_record: None,
        expected_targets: &[],
        // The VPC is one the stack creates in Amazon EC2, so its `Ref` is the identifier
        // that service assigned — and that is what the association must carry, since it is
        // what the declaration resolved to. Compared against the stack's own record of it
        // rather than against a value written here.
        expected_vpc_id: None,
        expected_association_name: None,
        rule_id_outputs: &["RuleId"],
        endpoint_id_outputs: &[],
        association_id_outputs: &["AssociationId", "AssociationIdAttribute"],
        derived_outputs: &[],
    },
    Row {
        case: "a DELEGATE rule with a delegation record and no association, JSON",
        region: "ap-northeast-1",
        template: || {
            json!({
                "Resources": {
                    "Rule": {
                        "Type": "AWS::Route53Resolver::ResolverRule",
                        "Properties": {
                            "DomainName": "delegated.example.test",
                            "RuleType": "DELEGATE",
                            "DelegationRecord": "ns1.example.test."
                        }
                    }
                },
                "Outputs": {
                    "RuleId": { "Value": { "Ref": "Rule" } },
                    "RuleIdAttribute": { "Value": { "Fn::GetAtt": ["Rule", "Id"] } }
                }
            })
            .to_string()
        },
        rule_logical_id: "Rule",
        endpoint_logical_id: None,
        association_logical_id: None,
        expected_rule_type: "DELEGATE",
        expected_domain_name: Some("delegated.example.test."),
        expected_delegation_record: Some("ns1.example.test."),
        expected_targets: &[],
        expected_vpc_id: None,
        expected_association_name: None,
        rule_id_outputs: &["RuleId", "RuleIdAttribute"],
        endpoint_id_outputs: &[],
        association_id_outputs: &[],
        derived_outputs: &[],
    },
];

/// Every row: the ids the stack reports for its Route 53 Resolver resources are the ones
/// that service minted, each resource's own read resolves the resource the template
/// configured, and deleting the stack deletes all of them there.
#[tokio::test]
async fn a_stack_provisions_the_resolver_rules_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("resolver-stack-{index}");
        let template = (row.template)();

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

        // The stack reports the template it deployed once the create COMPLETES, and by then
        // the Resolver resources' own clock-owed transitions are due too.
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
        let physical_ids = physical_ids_of(&resources);
        let outputs = outputs_of(&described);

        let rule_id = minted_id(
            case,
            &physical_ids,
            row.rule_logical_id,
            RULE_ID_PREFIX,
            &resources,
        );
        for key in row.rule_id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&rule_id),
                "case {case:?}: expected output {key:?} to be the rule id {rule_id:?}, got {:?}",
                outputs.get(*key)
            );
        }
        for (key, expected) in row.derived_outputs {
            let expected = expected(&rule_id);
            assert_eq!(
                outputs.get(*key),
                Some(&expected),
                "case {case:?}: expected output {key:?} to be {expected:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // The endpoint the rule routes through, where the row declares one: its id is
        // Route 53 Resolver's, and the rule the stack created must name THAT id.
        let endpoint_id = row.endpoint_logical_id.map(|logical_id| {
            minted_id(
                case,
                &physical_ids,
                logical_id,
                ENDPOINT_ID_PREFIX,
                &resources,
            )
        });
        for key in row.endpoint_id_outputs {
            assert_eq!(
                outputs.get(*key),
                endpoint_id.as_ref(),
                "case {case:?}: expected output {key:?} to be the endpoint id {endpoint_id:?}, \
                 got {:?}",
                outputs.get(*key)
            );
        }
        if let Some(endpoint_id) = &endpoint_id {
            let (status, answer) = route53resolver(
                &router,
                region,
                "GetResolverEndpoint",
                &json!({ "ResolverEndpointId": endpoint_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetResolverEndpoint on the id the stack \
                 reported, got {status} with body {answer}"
            );
            let endpoint = answer.get("ResolverEndpoint").cloned().unwrap_or(Value::Null);
            assert_eq!(
                endpoint.get("Direction").and_then(Value::as_str),
                Some("OUTBOUND"),
                "case {case:?}: expected the declared Direction, got {endpoint}"
            );
            assert_eq!(
                endpoint.get("Status").and_then(Value::as_str),
                Some("OPERATIONAL"),
                "case {case:?}: expected the endpoint to have reached OPERATIONAL, got {endpoint}"
            );
            assert_eq!(
                endpoint.get("IpAddressCount").and_then(Value::as_i64),
                Some(2),
                "case {case:?}: expected the two declared IP addresses, got {endpoint}"
            );
        }

        // And Route 53 Resolver itself resolves the rule id to the rule the template
        // configured.
        let (status, answer) = route53resolver(
            &router,
            region,
            "GetResolverRule",
            &json!({ "ResolverRuleId": rule_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetResolverRule on the id the stack reported, \
             got {status} with body {answer}"
        );
        let rule = answer.get("ResolverRule").cloned().unwrap_or(Value::Null);
        assert_eq!(
            rule.get("RuleType").and_then(Value::as_str),
            Some(row.expected_rule_type),
            "case {case:?}: expected RuleType {:?}, got {rule}",
            row.expected_rule_type
        );
        assert_eq!(
            rule.get("DomainName").and_then(Value::as_str),
            row.expected_domain_name,
            "case {case:?}: expected DomainName {:?}, got {rule}",
            row.expected_domain_name
        );
        assert_eq!(
            rule.get("DelegationRecord").and_then(Value::as_str),
            row.expected_delegation_record,
            "case {case:?}: expected DelegationRecord {:?}, got {rule}",
            row.expected_delegation_record
        );
        assert_eq!(
            rule.get("Status").and_then(Value::as_str),
            Some("COMPLETE"),
            "case {case:?}: expected the rule to report COMPLETE, got {rule}"
        );
        assert_eq!(
            rule.get("ResolverEndpointId").and_then(Value::as_str),
            endpoint_id.as_deref(),
            "case {case:?}: expected the rule to route through the endpoint the stack created \
             ({endpoint_id:?}), got {rule}"
        );
        let reported_targets: Vec<(String, i64)> = rule
            .get("TargetIps")
            .and_then(Value::as_array)
            .map(|targets| {
                targets
                    .iter()
                    .map(|target| {
                        (
                            target
                                .get("Ip")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            target.get("Port").and_then(Value::as_i64).unwrap_or(-1),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected_targets: Vec<(String, i64)> = row
            .expected_targets
            .iter()
            .map(|(ip, port)| ((*ip).to_string(), *port))
            .collect();
        assert_eq!(
            reported_targets, expected_targets,
            "case {case:?}: expected the declared target addresses {expected_targets:?}, got \
             {rule}"
        );

        // The association, where the row declares one: its id is Route 53 Resolver's, it
        // names the rule the stack created, and it converges to COMPLETE.
        let association_id = row.association_logical_id.map(|logical_id| {
            minted_id(
                case,
                &physical_ids,
                logical_id,
                ASSOCIATION_ID_PREFIX,
                &resources,
            )
        });
        for key in row.association_id_outputs {
            assert_eq!(
                outputs.get(*key),
                association_id.as_ref(),
                "case {case:?}: expected output {key:?} to be the association id \
                 {association_id:?}, got {:?}",
                outputs.get(*key)
            );
        }
        if let Some(association_id) = &association_id {
            let (status, answer) = route53resolver(
                &router,
                region,
                "GetResolverRuleAssociation",
                &json!({ "ResolverRuleAssociationId": association_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetResolverRuleAssociation on the id the \
                 stack reported, got {status} with body {answer}"
            );
            let association = answer
                .get("ResolverRuleAssociation")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                association.get("ResolverRuleId").and_then(Value::as_str),
                Some(rule_id.as_str()),
                "case {case:?}: expected the association to name the rule {rule_id:?}, got \
                 {association}"
            );
            assert_eq!(
                association.get("Status").and_then(Value::as_str),
                Some("COMPLETE"),
                "case {case:?}: expected the association to have reached COMPLETE, got \
                 {association}"
            );
            assert_eq!(
                association.get("Name").and_then(Value::as_str),
                row.expected_association_name,
                "case {case:?}: expected the declared Name {:?}, got {association}",
                row.expected_association_name
            );
            // The VPC is whatever the declaration resolved to: a literal id where the row
            // writes one, and otherwise the id the stack itself reports for the VPC
            // resource it references.
            let expected_vpc = row
                .expected_vpc_id
                .map(ToString::to_string)
                .or_else(|| physical_ids.get("Vpc").cloned());
            assert_eq!(
                association.get("VPCId").and_then(Value::as_str),
                expected_vpc.as_deref(),
                "case {case:?}: expected the association to name the VPC {expected_vpc:?}, got \
                 {association}"
            );
        }

        // Deleting the stack deletes every Resolver resource it created.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        for (operation, member, id) in [
            Some(("GetResolverRule", "ResolverRuleId", rule_id.clone())),
            association_id.clone().map(|id| {
                (
                    "GetResolverRuleAssociation",
                    "ResolverRuleAssociationId",
                    id,
                )
            }),
            endpoint_id
                .clone()
                .map(|id| ("GetResolverEndpoint", "ResolverEndpointId", id)),
        ]
        .into_iter()
        .flatten()
        {
            let (status, gone) =
                route53resolver(&router, region, operation, &json!({ member: id })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from {operation} once the stack deleted the \
                 resource, got {status} with body {gone}"
            );
        }
    }
}

/// The physical resource id the stack reports for one declared resource, checked to be an
/// identifier Route 53 Resolver MINTED rather than one CloudFormation's naming rules
/// computed: the prefix that service uses for the resource, followed by lower-case
/// hexadecimal.
fn minted_id(
    case: &str,
    physical_ids: &BTreeMap<String, String>,
    logical_id: &str,
    prefix: &str,
    reported: &str,
) -> String {
    let id = physical_ids.get(logical_id).cloned().unwrap_or_else(|| {
        panic!("case {case:?}: no physical resource id for {logical_id:?} in {reported}")
    });
    let Some(suffix) = id.strip_prefix(prefix) else {
        panic!("case {case:?}: expected {logical_id:?} to report an id prefixed {prefix:?}, got {id:?}")
    };
    assert!(
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: expected {logical_id:?} to report {prefix:?} and lower-case hexadecimal, \
         got {id:?}"
    );
    id
}

/// Re-deploying the same template is a NO-OP: every Resolver resource keeps the identifier
/// its owning service minted, and the resource that NAMES another one is not read as
/// changed because the reference now resolves to the minted identifier.
///
/// The identity half of the rule: for a type whose id its owning service assigns there is
/// no id a deployment can recompute, so a declaration that still asks for the same resource
/// keeps the one in force — and a property of another resource naming it has to be resolved
/// against that same id, or an unchanged template would read as a changed configuration.
#[tokio::test]
async fn re_deploying_the_same_template_keeps_the_minted_ids() {
    let router = build_router();
    let case = "an update that re-declares the endpoint, the rule and the association";
    let region = "ca-central-1";
    let stack_name = "resolver-redeploy";
    // The SAME resources both times, with one literal output changed: CloudFormation
    // refuses an update whose template is byte-identical ("No updates are to be
    // performed."), so the marker is what makes this a deployment at all — and the
    // resources it re-declares are unchanged, which is what the rule is about.
    let template = |marker: &str| {
        let mut body: Value = serde_json::from_str(&(ROWS[0].template)())
            .expect("the row's template is JSON");
        body["Outputs"]["Marker"] = json!({ "Value": marker });
        body.to_string()
    };
    let created_template = template("created");
    let updated_template = template("updated");

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &created_template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {described}"
    );
    let created = outputs_of(&described);

    let (status, body) = cloudformation(
        &router,
        region,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", &updated_template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
    let (_, updated) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        opt_element(&updated, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the re-deploy to complete, got {updated}"
    );
    let after = outputs_of(&updated);
    for key in ["EndpointId", "RuleId", "AssociationId"] {
        assert_eq!(
            after.get(key),
            created.get(key),
            "case {case:?}: expected output {key:?} to keep the id the create reported \
             ({:?}), got {:?}",
            created.get(key),
            after.get(key)
        );
    }

    // And the resources those ids name are still the ones Route 53 Resolver holds.
    let rule_id = created
        .get("RuleId")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no RuleId output in {described}"));
    let (status, answer) = route53resolver(
        &router,
        region,
        "GetResolverRule",
        &json!({ "ResolverRuleId": rule_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the re-deployed rule to still resolve, got {status} with body \
         {answer}"
    );
}

/// A stack whose rule names an endpoint of the same template is NOT reported as created
/// when Route 53 Resolver refuses the rule: an INBOUND endpoint cannot route a rule's
/// queries, and that service's own refusal is what the stack carries.
///
/// The other half of the rule above — nothing is passed off as created when the owning
/// service refused to create it — and the case that proves the rule's reference really
/// reaches that service as the endpoint's minted id: the refusal is about the endpoint's
/// DIRECTION, which only a resolved id could have been read off.
#[tokio::test]
async fn a_rule_through_an_inbound_endpoint_fails_the_stack() {
    let router = build_router();
    let case = "a FORWARD rule routed through an INBOUND endpoint";
    let region = "us-west-2";
    let template = json!({
        "Resources": {
            "Endpoint": {
                "Type": "AWS::Route53Resolver::ResolverEndpoint",
                "Properties": {
                    "Direction": "INBOUND",
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                    "IpAddresses": [
                        { "SubnetId": "subnet-0123456789abcdef0" },
                        { "SubnetId": "subnet-0123456789abcdef1" }
                    ]
                }
            },
            "Rule": {
                "Type": "AWS::Route53Resolver::ResolverRule",
                "Properties": {
                    "DomainName": "refused.example.test.",
                    "RuleType": "FORWARD",
                    "ResolverEndpointId": { "Ref": "Endpoint" },
                    "TargetIps": [{ "Ip": "10.254.254.254" }]
                }
            }
        },
        "Outputs": { "RuleId": { "Value": { "Ref": "Rule" } } }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", "resolver-refused"), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_PROVISIONING_SPANS).await;

    let (_, described) = cloudformation(
        &router,
        region,
        "DescribeStacks",
        &[("StackName", "resolver-refused")],
    )
    .await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back, got {described}"
    );
    // The STACK's reason summarises which resource failed; Route 53 Resolver's own
    // refusal is reported on that resource's event.
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::CREATE, ["Rule"]),
        "case {case:?}: expected the stack reason to summarise what failed, got {reason:?}"
    );
    let (_, events) = cloudformation(
        &router,
        region,
        "DescribeStackEvents",
        &[("StackName", "resolver-refused")],
    )
    .await;
    assert!(
        elements(&events, "ResourceStatusReason")
            .iter()
            .any(|reported| reported.contains("INBOUND") || reported.contains("OUTBOUND")),
        "case {case:?}: expected the failing resource's event to carry Route 53 Resolver's \
         own refusal, got {events}"
    );

    // The rollback removed the endpoint the failed create had already made, so nothing of
    // the stack is left in Route 53 Resolver.
    let (status, listed) = route53resolver(
        &router,
        region,
        "ListResolverEndpoints",
        &json!({ "MaxResults": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListResolverEndpoints, got {status} with body \
         {listed}"
    );
    let endpoints = listed
        .get("ResolverEndpoints")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        endpoints, 0,
        "case {case:?}: expected the rolled-back stack to leave no endpoint behind, got {listed}"
    );
}

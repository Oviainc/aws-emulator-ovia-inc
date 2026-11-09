// SPDX-License-Identifier: CC0-1.0

//! Classic Elastic Load Balancing resolves the NETWORK a request names against Amazon
//! EC2, which owns it.
//!
//! RULE: a subnet and a security group are EC2's resources, so every one a Classic load
//! balancer request names is resolved against EC2: a value EC2 holds no such resource for
//! is refused with the error this operation models for it — `SubnetNotFound` for a subnet
//! and `InvalidSecurityGroup` for a security group, both HTTP 400 `text/xml` awsQuery
//! `ErrorResponse` envelopes, whose MESSAGE is the EC2 fault RELAYED WHOLE — the sentence
//! EC2 worded about the ids the request named, followed by the context of the call that
//! raised it (the service, the status and error code it answered with, and that call's own
//! request id) — rather than the documentation summary of the error shape — and the request
//! changes nothing, while what EC2 reports for
//! the resources it DOES hold is what the load balancer reports back: the zone of each
//! attached subnet, the VPC they belong to, and the name and owner of its source security
//! group.
//!
//! This suite lives beside the router because the resources are EC2's: the VPC, the
//! subnets and the security groups below are minted by the real EC2 operations on the SAME
//! engine that serves Elastic Load Balancing, so every id in an assertion is one EC2 handed
//! out rather than one this suite made up, and the refusals are grounded in what EC2
//! actually answered.
//!
//! The rows differ along the FORM of the value that names no resource (EC2's 17-hex and
//! 8-hex id forms, and a value that is no id at all), along the operation that names it
//! (`CreateLoadBalancer`, `AttachLoadBalancerToSubnets`,
//! `ApplySecurityGroupsToLoadBalancer`), and along how many resources the request names —
//! because the rule is about resolving them, not about one recorded request.
//!
//! Error source: `smithy_query(query="shape", shape="SubnetNotFoundException,
//! InvalidSecurityGroupException")` on the Classic model — `errorKind` client, `httpError`
//! 400, `awsQueryError` codes `SubnetNotFound` and `InvalidSecurityGroup`.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, opt_element};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The Classic Elastic Load Balancing API version a client sends in its awsQuery body.
const ELB_API_VERSION: &str = "2012-06-01";

/// The signing name both halves of the Elastic Load Balancing endpoint share, which is
/// what routes a Classic request to the Classic model.
const ELB_SIGNING_NAME: &str = "elasticloadbalancing";

/// One emulator carrying EC2 and Classic Elastic Load Balancing over ONE shared engine, as
/// the production router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            engine,
        ),
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

/// One query-protocol answer: its HTTP status and its XML body.
struct Answer {
    status: u16,
    body: String,
}

/// POST one query-protocol action, signed for `signing_name`.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(String, String)],
) -> Answer {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// One ec2Query action that must succeed, answered with its XML body.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let owned: Vec<(String, String)> = params
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    let answer = query(router, "ec2", EC2_API_VERSION, action, &owned).await;
    assert_eq!(
        answer.status, 200,
        "setup: EC2 {action} must succeed, got {} with {}",
        answer.status, answer.body
    );
    answer.body
}

/// One Classic Elastic Load Balancing action.
async fn elb(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> Answer {
    query(router, ELB_SIGNING_NAME, ELB_API_VERSION, action, params).await
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The network every case runs over: two subnets in different Availability Zones and two
/// security groups of one VPC, all minted by EC2 itself.
struct Network {
    vpc: String,
    subnets: Vec<String>,
    zones: Vec<String>,
    security_groups: Vec<String>,
    group_names: Vec<String>,
}

/// Create that network through EC2's own operations.
async fn ec2_network(router: &MultiServiceRouter) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", "10.90.0.0/16")]).await,
        "vpcId",
    );
    let mut subnets = Vec::new();
    let mut zones = Vec::new();
    for (cidr, zone) in [
        ("10.90.1.0/24", "us-west-2a"),
        ("10.90.2.0/24", "us-west-2b"),
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
        zones.push(zone.to_string());
    }
    let group_names: Vec<String> = ["elb-source-one", "elb-source-two"]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut security_groups = Vec::new();
    for name in &group_names {
        security_groups.push(element(
            &ec2(
                router,
                "CreateSecurityGroup",
                &[
                    ("VpcId", vpc.as_str()),
                    ("GroupName", name.as_str()),
                    ("GroupDescription", "a classic load balancer"),
                ],
            )
            .await,
            "groupId",
        ));
    }
    Network {
        vpc,
        subnets,
        zones,
        security_groups,
        group_names,
    }
}

/// What `DescribeLoadBalancers` reports for `name`, reduced to the members EC2's resources
/// decide.
struct Reported {
    subnets: Vec<String>,
    zones: Vec<String>,
    vpc: Option<String>,
    security_groups: Vec<String>,
    source_group_name: Option<String>,
    source_group_owner: Option<String>,
}

/// Describe `name` and read those members off the answer.
async fn reported(router: &MultiServiceRouter, name: &str) -> Reported {
    let answer = elb(
        router,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", name)],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a describe of {name} must answer 200, got {} with {}",
        answer.status, answer.body
    );
    // `SourceSecurityGroup` is the only element carrying `GroupName`/`OwnerAlias`, and the
    // reader answers the FIRST of a tag, so the members are read off the whole answer.
    Reported {
        subnets: elements(&answer.body, "Subnets")
            .first()
            .map(|list| elements(list, "member"))
            .unwrap_or_default(),
        zones: elements(&answer.body, "AvailabilityZones")
            .first()
            .map(|list| elements(list, "member"))
            .unwrap_or_default(),
        vpc: opt_element(&answer.body, "VPCId"),
        security_groups: elements(&answer.body, "SecurityGroups")
            .first()
            .map(|list| elements(list, "member"))
            .unwrap_or_default(),
        source_group_name: opt_element(&answer.body, "GroupName"),
        source_group_owner: opt_element(&answer.body, "OwnerAlias"),
    }
}

/// The subnet a load balancer starts in, and the group it starts with.
const LOAD_BALANCER: &str = "network-resolved-elb";

/// Values that name no resource of the account, whatever operation names them: EC2's
/// 17-hex id form, its older 8-hex form, and a value that is no id at all.
fn unknown_subnet_ids() -> Vec<(&'static str, String)> {
    vec![
        ("EC2's 17-hex id form", "subnet-0f1e2d3c4b5a69780".to_string()),
        ("EC2's older 8-hex id form", "subnet-1a2b3c4d".to_string()),
        ("a value that is no subnet id at all", "not-a-subnet".to_string()),
    ]
}

/// The same three forms for a security group id.
fn unknown_group_ids() -> Vec<(&'static str, String)> {
    vec![
        ("EC2's 17-hex id form", "sg-0f1e2d3c4b5a69780".to_string()),
        ("EC2's older 8-hex id form", "sg-1a2b3c4d".to_string()),
        ("a value that is no security group id at all", "not-a-group".to_string()),
    ]
}

/// One row of the refusal table: the case, the action, the form fields beside it, and the
/// error code the refusal answers under.
type RefusalRow = (String, &'static str, Vec<(String, String)>, &'static str);

/// The `<Code>` of an `<ErrorResponse>` envelope, or an empty string.
fn code(answer: &Answer) -> String {
    opt_element(&answer.body, "Code").unwrap_or_default()
}

/// The `<Message>` of a refusal envelope — EC2's or Elastic Load Balancing's, which carry
/// the same element — or an empty string.
fn message(answer: &Answer) -> String {
    opt_element(&answer.body, "Message").unwrap_or_default()
}

/// A load balancer reports what EC2 says about the network it is placed in, and every
/// request naming a resource EC2 does not hold is refused with the modelled error and
/// changes nothing.
#[tokio::test]
async fn classic_load_balancer_network_resources_resolve_against_ec2() {
    let router = emulator();
    let network = ec2_network(&router).await;

    // A create placed in EC2's own subnet, with EC2's own security group.
    let created = elb(
        &router,
        "CreateLoadBalancer",
        &[
            param("LoadBalancerName", LOAD_BALANCER),
            param("Listeners.member.1.Protocol", "HTTP"),
            param("Listeners.member.1.LoadBalancerPort", "80"),
            param("Listeners.member.1.InstancePort", "80"),
            param("Subnets.member.1", &network.subnets[0]),
            param("SecurityGroups.member.1", &network.security_groups[0]),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "a create naming EC2's own subnet must be accepted, got {} with {}",
        created.status, created.body
    );

    // Every member EC2's resources decide is reported from what EC2 answered: the subnet's
    // zone, the VPC it belongs to, and the name and owner of the source security group.
    let described = reported(&router, LOAD_BALANCER).await;
    assert_eq!(
        (
            described.subnets.clone(),
            described.zones.clone(),
            described.vpc.clone(),
            described.security_groups.clone(),
            described.source_group_name.clone(),
            described.source_group_owner.clone(),
        ),
        (
            vec![network.subnets[0].clone()],
            vec![network.zones[0].clone()],
            Some(network.vpc.clone()),
            vec![network.security_groups[0].clone()],
            Some(network.group_names[0].clone()),
            Some(engine::emulator_account_id().to_string()),
        ),
        "a load balancer must report what EC2 says about its own network"
    );

    // (case, action, the form fields, the expected error code)
    let mut rows: Vec<RefusalRow> = Vec::new();
    for (form, id) in unknown_subnet_ids() {
        rows.push((
            format!("a create placed in a subnet EC2 does not hold, in {form}"),
            "CreateLoadBalancer",
            vec![
                param("LoadBalancerName", "unknown-subnet-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("Subnets.member.1", &id),
            ],
            "SubnetNotFound",
        ));
        rows.push((
            format!("an attach naming a subnet EC2 does not hold, in {form}"),
            "AttachLoadBalancerToSubnets",
            vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("Subnets.member.1", &id),
            ],
            "SubnetNotFound",
        ));
        rows.push((
            format!("an attach naming one subnet EC2 holds beside one it does not, in {form}"),
            "AttachLoadBalancerToSubnets",
            vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("Subnets.member.1", &network.subnets[1]),
                param("Subnets.member.2", &id),
            ],
            "SubnetNotFound",
        ));
    }
    for (form, id) in unknown_group_ids() {
        rows.push((
            format!("a create naming a security group EC2 does not hold, in {form}"),
            "CreateLoadBalancer",
            vec![
                param("LoadBalancerName", "unknown-group-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("Subnets.member.1", &network.subnets[0]),
                param("SecurityGroups.member.1", &id),
            ],
            "InvalidSecurityGroup",
        ));
        rows.push((
            format!("an apply naming a security group EC2 does not hold, in {form}"),
            "ApplySecurityGroupsToLoadBalancer",
            vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("SecurityGroups.member.1", &id),
            ],
            "InvalidSecurityGroup",
        ));
        rows.push((
            format!("an apply naming one group EC2 holds beside one it does not, in {form}"),
            "ApplySecurityGroupsToLoadBalancer",
            vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("SecurityGroups.member.1", &network.security_groups[1]),
                param("SecurityGroups.member.2", &id),
            ],
            "InvalidSecurityGroup",
        ));
    }

    for (case, action, params, expected) in rows {
        let refused = elb(&router, action, &params).await;
        assert_eq!(
            (refused.status, code(&refused).as_str()),
            (400, expected),
            "case {case:?}: expected 400 {expected} from {action}, got {} with {}",
            refused.status,
            refused.body
        );
        assert!(
            refused.body.contains("<ErrorResponse"),
            "case {case:?}: an awsQuery client must get an <ErrorResponse> envelope, got {}",
            refused.body
        );

        // A refused request changed nothing: the load balancer is still placed in the one
        // subnet and governed by the one group the create gave it.
        let after = reported(&router, LOAD_BALANCER).await;
        assert_eq!(
            (
                after.subnets,
                after.security_groups,
                after.source_group_name
            ),
            (
                vec![network.subnets[0].clone()],
                vec![network.security_groups[0].clone()],
                Some(network.group_names[0].clone())
            ),
            "case {case:?}: a refused request must leave the load balancer as it was"
        );
        // And the load balancer a refused create named was never created.
        for name in ["unknown-subnet-elb", "unknown-group-elb"] {
            let described = elb(
                &router,
                "DescribeLoadBalancers",
                &[param("LoadBalancerNames.member.1", name)],
            )
            .await;
            assert_eq!(
                (described.status, code(&described).as_str()),
                (400, "LoadBalancerNotFound"),
                "case {case:?}: a refused create must leave no {name}, got {}",
                described.body
            );
        }
    }

    // The accepted side of the same rule: the resources EC2 DOES hold are attached and
    // associated, and what EC2 says about them is what the load balancer then reports.
    let attached = elb(
        &router,
        "AttachLoadBalancerToSubnets",
        &[
            param("LoadBalancerName", LOAD_BALANCER),
            param("Subnets.member.1", &network.subnets[1]),
        ],
    )
    .await;
    assert_eq!(
        attached.status, 200,
        "an attach naming EC2's own subnet must be accepted, got {} with {}",
        attached.status, attached.body
    );
    let applied = elb(
        &router,
        "ApplySecurityGroupsToLoadBalancer",
        &[
            param("LoadBalancerName", LOAD_BALANCER),
            param("SecurityGroups.member.1", &network.security_groups[1]),
        ],
    )
    .await;
    assert_eq!(
        applied.status, 200,
        "an apply naming EC2's own group must be accepted, got {} with {}",
        applied.status, applied.body
    );
    let after = reported(&router, LOAD_BALANCER).await;
    assert_eq!(
        (
            after.subnets,
            after.zones,
            after.security_groups,
            after.source_group_name,
            after.source_group_owner,
        ),
        (
            network.subnets.clone(),
            network.zones.clone(),
            vec![network.security_groups[1].clone()],
            Some(network.group_names[1].clone()),
            Some(engine::emulator_account_id().to_string()),
        ),
        "the accepted attach and apply must be what a later describe reports"
    );

    // A detach leaves the placement it can: one of the two subnets goes, and the request
    // that would take the last one is refused with the configuration error.
    let detached = elb(
        &router,
        "DetachLoadBalancerFromSubnets",
        &[
            param("LoadBalancerName", LOAD_BALANCER),
            param("Subnets.member.1", &network.subnets[1]),
        ],
    )
    .await;
    assert_eq!(
        detached.status, 200,
        "detaching one of two subnets must be accepted, got {} with {}",
        detached.status, detached.body
    );
    let refused = elb(
        &router,
        "DetachLoadBalancerFromSubnets",
        &[
            param("LoadBalancerName", LOAD_BALANCER),
            param("Subnets.member.1", &network.subnets[0]),
        ],
    )
    .await;
    assert_eq!(
        (refused.status, code(&refused).as_str()),
        (409, "InvalidConfigurationRequest"),
        "detaching the last subnet must be refused, got {} with {}",
        refused.status,
        refused.body
    );
    let after = reported(&router, LOAD_BALANCER).await;
    assert_eq!(
        (after.subnets, after.zones),
        (
            vec![network.subnets[0].clone()],
            vec![network.zones[0].clone()]
        ),
        "the refused detach must leave the last subnet attached"
    );
}

/// One case of the message table: the Classic request that is refused, the code it is
/// refused under, the EC2 read of the SAME ids, the value EC2 holds no resource for, and
/// the documentation summary of the modelled error shape.
struct MessageCase {
    case: String,
    action: &'static str,
    params: Vec<(String, String)>,
    expected_code: &'static str,
    ec2_action: &'static str,
    ec2_params: Vec<(String, String)>,
    unknown_id: String,
    shape_summary: &'static str,
    /// Whether this operation states the EC2 error CODE alone rather than the fault EC2
    /// raised: `CreateLoadBalancer`'s own recorded network refusal carries that code, so a
    /// recording pins its wording, while the two operations whose recordings do not assert
    /// the message relay the fault whole.
    states_ec2_code: bool,
}

/// `SubnetNotFoundException`'s own documentation
/// (`smithy_query(query="shape", shape="SubnetNotFoundException")`): the summary of the
/// error CLASS, which says nothing about the request and is therefore not a message.
const SUBNET_SHAPE_SUMMARY: &str = "One or more of the specified subnets do not exist.";

/// `InvalidSecurityGroupException`'s own documentation, read the same way and not a message
/// for the same reason.
const GROUP_SHAPE_SUMMARY: &str = "One or more of the specified security groups do not exist.";

/// `SubnetId.N` / `GroupId.N` form fields, as ec2Query flattens a list of ids.
fn indexed(prefix: &str, values: &[&str]) -> Vec<(String, String)> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| (format!("{prefix}.{}", index + 1), (*value).to_string()))
        .collect()
}

/// One error code answers every subnet a request names that EC2 cannot resolve — and one
/// answers every security group — so the MESSAGE is what tells the caller WHICH of the
/// values it sent was rejected and WHAT refused it: it is the EC2 fault relayed whole — the
/// sentence EC2 worded for that very id, and the context of the call that raised it — rather
/// than the documentation summary of the error shape, which describes the class and carries
/// nothing from the request.
///
/// Each row asserts the Classic message against what EC2 ITSELF answers for the same read,
/// through the same router — the sentence EC2 worded and the code it answered, or just that
/// code where the operation's own recorded refusal carries it — so what is pinned is that the
/// EC2 fault is relayed rather than a sentence this suite memorised. The rows differ
/// along the FORM of the value EC2 holds no resource for (its 17-hex and 8-hex id forms, and
/// a value that is no id at all), along the operation that names it, and along how many
/// resources the request names beside it.
#[tokio::test]
async fn a_network_refusal_relays_the_ec2_fault_for_the_id_the_request_named() {
    let router = emulator();
    let network = ec2_network(&router).await;
    let created = elb(
        &router,
        "CreateLoadBalancer",
        &[
            param("LoadBalancerName", LOAD_BALANCER),
            param("Listeners.member.1.Protocol", "HTTP"),
            param("Listeners.member.1.LoadBalancerPort", "80"),
            param("Listeners.member.1.InstancePort", "80"),
            param("Subnets.member.1", &network.subnets[0]),
            param("SecurityGroups.member.1", &network.security_groups[0]),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture must be created, got {} with {}",
        created.status, created.body
    );

    let mut cases: Vec<MessageCase> = Vec::new();
    for (form, id) in unknown_subnet_ids() {
        cases.push(MessageCase {
            case: format!("an attach naming only a subnet EC2 does not hold, in {form}"),
            action: "AttachLoadBalancerToSubnets",
            params: vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("Subnets.member.1", &id),
            ],
            expected_code: "SubnetNotFound",
            ec2_action: "DescribeSubnets",
            ec2_params: indexed("SubnetId", &[&id]),
            unknown_id: id.clone(),
            shape_summary: SUBNET_SHAPE_SUMMARY,
            states_ec2_code: false,
        });
        cases.push(MessageCase {
            case: format!("an attach naming a held subnet beside one EC2 does not hold, in {form}"),
            action: "AttachLoadBalancerToSubnets",
            params: vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("Subnets.member.1", &network.subnets[1]),
                param("Subnets.member.2", &id),
            ],
            expected_code: "SubnetNotFound",
            ec2_action: "DescribeSubnets",
            ec2_params: indexed("SubnetId", &[&network.subnets[1], &id]),
            unknown_id: id.clone(),
            shape_summary: SUBNET_SHAPE_SUMMARY,
            states_ec2_code: false,
        });
        cases.push(MessageCase {
            case: format!("a create placed in a subnet EC2 does not hold, in {form}"),
            action: "CreateLoadBalancer",
            params: vec![
                param("LoadBalancerName", "unknown-subnet-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("Subnets.member.1", &id),
            ],
            expected_code: "SubnetNotFound",
            ec2_action: "DescribeSubnets",
            ec2_params: indexed("SubnetId", &[&id]),
            unknown_id: id,
            shape_summary: SUBNET_SHAPE_SUMMARY,
            states_ec2_code: true,
        });
    }
    for (form, id) in unknown_group_ids() {
        cases.push(MessageCase {
            case: format!("an apply naming only a group EC2 does not hold, in {form}"),
            action: "ApplySecurityGroupsToLoadBalancer",
            params: vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("SecurityGroups.member.1", &id),
            ],
            expected_code: "InvalidSecurityGroup",
            ec2_action: "DescribeSecurityGroups",
            ec2_params: indexed("GroupId", &[&id]),
            unknown_id: id.clone(),
            shape_summary: GROUP_SHAPE_SUMMARY,
            states_ec2_code: false,
        });
        cases.push(MessageCase {
            case: format!("an apply naming a held group beside one EC2 does not hold, in {form}"),
            action: "ApplySecurityGroupsToLoadBalancer",
            params: vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("SecurityGroups.member.1", &network.security_groups[1]),
                param("SecurityGroups.member.2", &id),
            ],
            expected_code: "InvalidSecurityGroup",
            ec2_action: "DescribeSecurityGroups",
            ec2_params: indexed("GroupId", &[&network.security_groups[1], &id]),
            unknown_id: id.clone(),
            shape_summary: GROUP_SHAPE_SUMMARY,
            states_ec2_code: false,
        });
        cases.push(MessageCase {
            case: format!("a create naming a group EC2 does not hold, in {form}"),
            action: "CreateLoadBalancer",
            params: vec![
                param("LoadBalancerName", "unknown-group-elb"),
                param("Listeners.member.1.Protocol", "HTTP"),
                param("Listeners.member.1.LoadBalancerPort", "80"),
                param("Listeners.member.1.InstancePort", "80"),
                param("Subnets.member.1", &network.subnets[0]),
                param("SecurityGroups.member.1", &id),
            ],
            expected_code: "InvalidSecurityGroup",
            ec2_action: "DescribeSecurityGroups",
            ec2_params: indexed("GroupId", &[&id]),
            unknown_id: id,
            shape_summary: GROUP_SHAPE_SUMMARY,
            states_ec2_code: true,
        });
    }

    for case in cases {
        // What EC2 itself answers for the very read Elastic Load Balancing makes.
        let from_ec2 = query(
            &router,
            "ec2",
            EC2_API_VERSION,
            case.ec2_action,
            &case.ec2_params,
        )
        .await;
        assert_eq!(
            from_ec2.status, 400,
            "case {:?}: EC2 must refuse the read of {}, got {} with {}",
            case.case, case.unknown_id, from_ec2.status, from_ec2.body
        );
        let ec2_message = message(&from_ec2);
        assert!(
            ec2_message.contains(&case.unknown_id),
            "case {:?}: EC2's own refusal must name {}, got {ec2_message:?}",
            case.case,
            case.unknown_id
        );

        let refused = elb(&router, case.action, &case.params).await;
        assert_eq!(
            (refused.status, code(&refused).as_str()),
            (400, case.expected_code),
            "case {:?}: expected 400 {} from {}, got {} with {}",
            case.case,
            case.expected_code,
            case.action,
            refused.status,
            refused.body
        );
        // Either way the message states what EC2 answered: the fault relayed whole — its
        // sentence and the context of the call that raised it — or, where the operation's own
        // recording pins it, the code it answered.
        let stated = message(&refused);
        if case.states_ec2_code {
            assert_eq!(
                stated,
                code(&from_ec2),
                "case {:?}: expected the code EC2 answered, got {stated:?}",
                case.case
            );
        } else {
            let head = format!(
                "{ec2_message} (Service: {EC2_IN_A_RELAYED_FAULT}; Status Code: 400; \
                 Error Code: {}; Request ID: ",
                code(&from_ec2)
            );
            let relayed_request_id = stated
                .strip_prefix(&head)
                .and_then(|tail| tail.strip_suffix(RELAYED_FAULT_TAIL))
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected the EC2 fault relayed as \
                         {head:?}<request id>{RELAYED_FAULT_TAIL:?}, got {stated:?}",
                        case.case
                    )
                });
            assert!(
                is_request_id(relayed_request_id),
                "case {:?}: expected the relayed clause to carry a request id, got \
                 {relayed_request_id:?}",
                case.case
            );
            // The relayed id names the EC2 call, not this response: the envelope reports its
            // own, so a caller reading the two sees two calls rather than one id twice.
            assert_ne!(
                Some(relayed_request_id.to_string()),
                opt_element(&refused.body, "RequestId"),
                "case {:?}: the relayed clause must name the EC2 call, not this response",
                case.case
            );
        }
        assert_ne!(
            stated, case.shape_summary,
            "case {:?}: the message must state the request, not the error shape's summary",
            case.case
        );
    }
}

/// How EC2 names itself in the context clause of a fault relayed through another service's
/// API, as the SDK of the service that raised it renders it.
const EC2_IN_A_RELAYED_FAULT: &str = "AmazonEC2";

/// The close of that clause, after the raising call's own request id.
const RELAYED_FAULT_TAIL: &str = "; Proxy: null)";

/// Whether `id` has the 8-4-4-4-12 hexadecimal form every AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !group.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return false;
        }
    }
    groups.next().is_none()
}

/// The relayed clause names ONE EC2 call, so two refusals of the same request carry two
/// request ids: the id is minted per call rather than derived from the request, which is
/// what lets a caller find each attempt in EC2's own records.
#[tokio::test]
async fn each_relayed_fault_names_its_own_ec2_call() {
    let router = emulator();
    let network = ec2_network(&router).await;
    let created = elb(
        &router,
        "CreateLoadBalancer",
        &[
            param("LoadBalancerName", LOAD_BALANCER),
            param("Listeners.member.1.Protocol", "HTTP"),
            param("Listeners.member.1.LoadBalancerPort", "80"),
            param("Listeners.member.1.InstancePort", "80"),
            param("Subnets.member.1", &network.subnets[0]),
            param("SecurityGroups.member.1", &network.security_groups[0]),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture must be created, got {} with {}",
        created.status, created.body
    );

    for (case, action, params) in [
        (
            "an attach naming a subnet EC2 does not hold",
            "AttachLoadBalancerToSubnets",
            vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("Subnets.member.1", "subnet-0f1e2d3c4b5a69780"),
            ],
        ),
        (
            "an apply naming a group EC2 does not hold",
            "ApplySecurityGroupsToLoadBalancer",
            vec![
                param("LoadBalancerName", LOAD_BALANCER),
                param("SecurityGroups.member.1", "sg-0f1e2d3c4b5a69780"),
            ],
        ),
    ] {
        let first = message(&elb(&router, action, &params).await);
        let second = message(&elb(&router, action, &params).await);
        assert_ne!(
            first, second,
            "case {case:?}: two refusals of the same request must name two EC2 calls, got \
             {first:?} twice"
        );
        // Everything but that id is the same fault, stated the same way.
        let clause = |stated: &str| {
            stated
                .split_once("; Request ID: ")
                .map(|(head, _)| head.to_string())
                .unwrap_or_default()
        };
        assert_eq!(
            clause(&first),
            clause(&second),
            "case {case:?}: the fault itself must not change between two refusals"
        );
        assert!(
            !clause(&first).is_empty(),
            "case {case:?}: expected a relayed fault clause, got {first:?}"
        );
    }
}

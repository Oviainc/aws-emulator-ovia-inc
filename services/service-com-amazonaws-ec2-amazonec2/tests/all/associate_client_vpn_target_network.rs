// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateClientVpnTargetNetwork` and the describes that read its
//! writes back, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rule under test is that the operation ASSOCIATES ONE TARGET NETWORK WITH THE ENDPOINT
//! THE REQUEST NAMES — the subnet of a VPC-based endpoint, an Availability Zone of a Transit
//! Gateway-based one — answers it `associating` under a minted `AssociationId` (the clock
//! owes it `associated` a transition span later), moves an endpoint that was waiting for a
//! target network to `available`, pins that endpoint to the subnet's VPC, and adds the route
//! to the VPC's range the association brings with it; a network the endpoint already has, a
//! second subnet in one Availability Zone, a subnet of another VPC and a value this account
//! holds no subnet for are each refused with the code that says why, and write nothing.
//!
//! Every table row is a distinct input along what that generalises over: which subnet, which
//! Availability Zone and in which of its two spellings, which endpoint, whether the endpoint
//! names a VPC or a Transit Gateway, whether a client token is carried — and, for the
//! refusals, which part of the request cannot be served and in which form.

use std::time::Duration;

use ec2::ops::types::{ClientVpnEndpoint, TransitGatewayConfigurationDescribeEndpointStructure};
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, MAX_CLIENT_TOKEN_CHARS, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for. The Client VPN stores are per account rather
/// than per Region, but a zone is named per Region, so this is the Region whose zones a
/// Transit Gateway association names.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ASSOCIATE: &str = "AssociateClientVpnTargetNetwork";

/// The id prefix an association is minted under, as the AWS documentation's own association
/// ids read (`cvpn-assoc-0c9a06dc85b3e9bc1`).
const ASSOCIATION_ID_PREFIX: &str = "cvpn-assoc";

/// The range of the VPC the target networks under test are subnets of.
const VPC_CIDR: &str = "10.20.0.0/16";

/// The range of a SECOND VPC, whose subnets no endpoint pinned to the first may associate.
const OTHER_VPC_CIDR: &str = "10.30.0.0/16";

/// Long enough past [`TRANSITION_DELAY`] that the `associating` -> `associated` move the
/// clock owes an association is certainly due, with a margin for a slow machine.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(400)
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The form fields of one association: the endpoint, and whatever names the network.
fn associate_params(endpoint_id: &str, extra: &[Param]) -> Vec<Param> {
    let mut params = vec![param("ClientVpnEndpointId", endpoint_id)];
    params.extend(extra.iter().cloned());
    params
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// A subnet of `vpc_id` with `cidr_block` in the Availability Zone `zone`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
    zone: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[
            param("VpcId", vpc_id),
            param("CidrBlock", cidr_block),
            param("AvailabilityZone", zone),
        ],
    )
    .await;
    element(&body, "subnetId").unwrap_or_else(|| panic!("case {case:?}: no <subnetId> in {body}"))
}

/// Write one Client VPN endpoint, the way `CreateClientVpnEndpoint` would — `vpc_id` is the
/// VPC the request named, `None` for an endpoint created without one — and answer the
/// `ClientVpnEndpointId` it was minted with.
fn seed_endpoint(
    engine: &engine::InMemoryEngine,
    description: &str,
    vpc_id: Option<&str>,
    security_group_ids: &[&str],
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some(description.to_string()),
            client_cidr_block: Some("10.100.0.0/22".to_string()),
            vpc_id: vpc_id.map(str::to_string),
            security_group_ids: Some(
                security_group_ids
                    .iter()
                    .map(|group| (*group).to_string())
                    .collect(),
            ),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint")
}

/// Write one Client VPN endpoint that reaches its target networks through a Transit Gateway
/// rather than through a VPC, and answer its id.
fn seed_transit_gateway_endpoint(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some("a Transit Gateway-based endpoint".to_string()),
            client_cidr_block: Some("10.200.0.0/22".to_string()),
            transit_gateway_configuration: Some(
                TransitGatewayConfigurationDescribeEndpointStructure {
                    transit_gateway_id: Some("tgw-00000000000000001".to_string()),
                    transit_gateway_attachment_id: Some("tgw-attach-00000000000000001".to_string()),
                    ..Default::default()
                },
            ),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint")
}

/// How many target network associations the store holds, whichever endpoint they belong to —
/// what the "nothing was written" assertions read.
fn stored_associations(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .client_vpn_target_networks
        .len()
}

/// The `<status><code>` of the one association an `AssociateClientVpnTargetNetwork` response
/// carries, read through the document so the association's own status is never confused with
/// a member of the same name elsewhere in the envelope.
fn answered_status(case: &str, body: &str) -> String {
    let parsed = document(case, body);
    parsed
        .root_element()
        .children()
        .filter(roxmltree::Node::is_element)
        .find_map(|node| child_text(node, "code"))
        .unwrap_or_else(|| panic!("case {case:?}: no <status><code> in {body}"))
}

/// Every association `DescribeClientVpnTargetNetworks` reports for `endpoint_id`, as
/// (`targetNetworkId`, `vpcId`, `status/code`, the zone names it carries) — the members a
/// caller reads an association back by.
async fn described_associations(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
) -> Vec<(String, String, String, Vec<String>)> {
    let body = ok(
        case,
        router,
        "DescribeClientVpnTargetNetworks",
        &[param("ClientVpnEndpointId", endpoint_id)],
    )
    .await;
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "associationId").is_some())
        .map(|association| {
            let zones = child(association, "availabilityZoneSet")
                .map(|set| {
                    set.children()
                        .filter(roxmltree::Node::is_element)
                        .map(|item| item.text().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            (
                child_text(association, "targetNetworkId").unwrap_or_default(),
                child_text(association, "vpcId").unwrap_or_default(),
                child(association, "status")
                    .and_then(|status| child_text(status, "code"))
                    .unwrap_or_default(),
                zones,
            )
        })
        .collect()
}

/// The security groups the one association of `endpoint_id` carries, read through the
/// document so the association's own set is never confused with another list of the
/// envelope.
async fn described_security_groups(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeClientVpnTargetNetworks",
        &[param("ClientVpnEndpointId", endpoint_id)],
    )
    .await;
    let parsed = document(case, &body);
    let association = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "associationId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no association in {body}"));
    set_items(association, "securityGroups")
        .iter()
        .map(|item| item.text().unwrap_or_default().to_string())
        .collect()
}

/// The `status/code` and `vpcId` `DescribeClientVpnEndpoints` reports for `endpoint_id`.
async fn described_endpoint(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
) -> (String, String) {
    let body = ok(
        case,
        router,
        "DescribeClientVpnEndpoints",
        &indexed("ClientVpnEndpointId", &[endpoint_id]),
    )
    .await;
    let parsed = document(case, &body);
    let endpoint = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "clientVpnEndpointId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no endpoint in {body}"));
    (
        child(endpoint, "status")
            .and_then(|status| child_text(status, "code"))
            .unwrap_or_default(),
        child_text(endpoint, "vpcId").unwrap_or_default(),
    )
}

// ---------------------------------------------------------------------------
// What one association writes, and what reads it back
// ---------------------------------------------------------------------------

/// One row of the association table: the subnet to associate and the Availability Zone it
/// sits in.
struct SubnetRow {
    case: &'static str,
    cidr_block: &'static str,
    zone: &'static str,
}

/// The subnets the suite associates. One per Availability Zone of the Region — which is as
/// many as one endpoint may hold — and of three different sizes, so no row is the recorded
/// one renamed.
fn subnet_rows() -> Vec<SubnetRow> {
    vec![
        SubnetRow {
            case: "a /24 in the first zone",
            cidr_block: "10.20.1.0/24",
            zone: "us-east-1a",
        },
        SubnetRow {
            case: "a /20 in the second zone",
            cidr_block: "10.20.16.0/20",
            zone: "us-east-1b",
        },
        SubnetRow {
            case: "a /28 in the third zone",
            cidr_block: "10.20.32.0/28",
            zone: "us-east-1c",
        },
    ]
}

/// Every associated subnet is stored under a minted association id, answered `associating`,
/// reported by `DescribeClientVpnTargetNetworks` with the VPC it is in, and reaches
/// `associated` once the clock owes it. The endpoint, created without a VPC, is pinned to
/// that VPC and moves from `pending-associate` to `available`, and each association adds the
/// route to the VPC's range that `DescribeClientVpnRoutes` reports with the `associate`
/// origin.
#[tokio::test]
async fn every_associated_subnet_is_reported_for_its_endpoint_and_reaches_associated() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc = create_vpc("the VPC under test", &router, VPC_CIDR).await;
    let endpoint = seed_endpoint(&engine, "an endpoint created without a VPC", None, &[]);
    let other_endpoint = seed_endpoint(&engine, "another endpoint of the account", None, &[]);

    let (status, _) = described_endpoint("before any association", &router, &endpoint).await;
    assert_eq!(
        status, "pending-associate",
        "before any association: an endpoint with no target network waits in \
         pending-associate, got {status:?}"
    );

    let rows = subnet_rows();
    let mut subnets: Vec<(&SubnetRow, String, String)> = Vec::new();
    for row in &rows {
        let subnet = create_subnet(row.case, &router, &vpc, row.cidr_block, row.zone).await;
        let body = ok(
            row.case,
            &router,
            ASSOCIATE,
            &associate_params(&endpoint, &[param("SubnetId", &subnet)]),
        )
        .await;
        let association_id = element(&body, "associationId")
            .unwrap_or_else(|| panic!("case {:?}: no <associationId> in {body}", row.case));
        assert!(
            is_long_id(ASSOCIATION_ID_PREFIX, &association_id),
            "case {case:?}: expected a minted {ASSOCIATION_ID_PREFIX} id, got \
             {association_id:?}",
            case = row.case
        );
        let answered = answered_status(row.case, &body);
        assert_eq!(
            answered, "associating",
            "case {case:?}: a fresh association is answered associating, got {answered:?}",
            case = row.case
        );
        subnets.push((row, subnet, association_id));
    }

    // Each association ids one network: no two rows were answered the same id.
    for (row, _, association_id) in &subnets {
        let repeats = subnets
            .iter()
            .filter(|(_, _, other)| other == association_id)
            .count();
        assert_eq!(
            repeats, 1,
            "case {case:?}: the association id {association_id:?} was answered {repeats} times",
            case = row.case
        );
    }

    // The endpoint the associations were made on: pinned to the VPC its first target network
    // is in, and no longer waiting for one.
    let (status, reported_vpc) = described_endpoint("after associating", &router, &endpoint).await;
    assert_eq!(
        status, "available",
        "after associating: an endpoint with a target network is available, got {status:?}"
    );
    assert_eq!(
        reported_vpc, vpc,
        "after associating: expected the endpoint pinned to {vpc:?}, got {reported_vpc:?}"
    );

    // Every association is reported for the endpoint it was made on, with the VPC its subnet
    // is in, and for no other endpoint.
    let reported = described_associations("after associating", &router, &endpoint).await;
    let mut expected: Vec<(String, String, String, Vec<String>)> = subnets
        .iter()
        .map(|(_, subnet, _)| {
            (
                subnet.clone(),
                vpc.clone(),
                "associating".to_string(),
                Vec::new(),
            )
        })
        .collect();
    expected.sort();
    let mut sorted = reported.clone();
    sorted.sort();
    assert_eq!(
        sorted, expected,
        "after associating: expected every subnet reported in {vpc:?} as associating, got \
         {reported:?}"
    );
    let others = described_associations("the other endpoint", &router, &other_endpoint).await;
    assert!(
        others.is_empty(),
        "the other endpoint: an endpoint with no association reports none, got {others:?}"
    );

    // The route each association brings with it: the VPC's own range, through the subnet
    // that was associated, with the origin that says the association added it.
    let routes = ok(
        "the routes the associations added",
        &router,
        "DescribeClientVpnRoutes",
        &[param("ClientVpnEndpointId", &endpoint)],
    )
    .await;
    let mut targets = elements(&routes, "targetSubnet");
    targets.sort();
    let mut associated_subnets: Vec<String> = subnets
        .iter()
        .map(|(_, subnet, _)| subnet.clone())
        .collect();
    associated_subnets.sort();
    assert_eq!(
        targets, associated_subnets,
        "the routes the associations added: expected one route per associated subnet, got \
         {routes}"
    );
    let destinations = elements(&routes, "destinationCidr");
    assert!(
        destinations.iter().all(|destination| destination == VPC_CIDR),
        "the routes the associations added: every route goes to the VPC's range \
         {VPC_CIDR:?}, got {destinations:?}"
    );
    let origins = elements(&routes, "origin");
    assert!(
        origins.iter().all(|origin| origin == "associate"),
        "the routes the associations added: every route carries the associate origin, got \
         {origins:?}"
    );

    // The move the clock owes each association, which is due once the span has passed.
    tokio::time::sleep(past_the_transition()).await;
    let settled = described_associations("after the transition span", &router, &endpoint).await;
    assert!(
        settled
            .iter()
            .all(|(_, _, status, _)| status == "associated"),
        "after the transition span: every association has reached associated, got {settled:?}"
    );
}

/// The security groups an association carries: the endpoint's own when it has them, and the
/// default security group of the target network's VPC when it has none.
#[tokio::test]
async fn an_association_carries_the_endpoints_groups_or_the_vpcs_default_group() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc = create_vpc("the VPC under test", &router, VPC_CIDR).await;
    let subnet = create_subnet("the subnet under test", &router, &vpc, "10.20.1.0/24", "us-east-1a")
        .await;
    let other_subnet =
        create_subnet("a second subnet", &router, &vpc, "10.20.2.0/24", "us-east-1b").await;
    let default_group = engine
        .state::<Ec2State>()
        .lock()
        .security_group_id_for_name(&vpc, "default")
        .expect("a VPC comes with its default security group");

    // (case, the endpoint's own groups, the groups the association must carry, the subnet)
    let cases: [(&str, &[&str], Vec<String>, &str); 2] = [
        (
            "an endpoint with no security group of its own",
            &[],
            vec![default_group.clone()],
            subnet.as_str(),
        ),
        (
            "an endpoint with two security groups of its own",
            &["sg-00000000000000001", "sg-00000000000000002"],
            vec![
                "sg-00000000000000001".to_string(),
                "sg-00000000000000002".to_string(),
            ],
            other_subnet.as_str(),
        ),
    ];
    for (case, endpoint_groups, expected, subnet_id) in cases {
        let endpoint = seed_endpoint(&engine, case, Some(&vpc), endpoint_groups);
        ok(
            case,
            &router,
            ASSOCIATE,
            &associate_params(&endpoint, &[param("SubnetId", subnet_id)]),
        )
        .await;
        let reported = described_security_groups(case, &router, &endpoint).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the association to carry {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The Availability Zone association of a Transit Gateway-based endpoint
// ---------------------------------------------------------------------------

/// A Transit Gateway-based endpoint associates an Availability ZONE rather than a subnet,
/// named by either of its two spellings — and the same zone named the other way is the one it
/// already has, not a second target network.
#[tokio::test]
async fn a_transit_gateway_endpoint_associates_an_availability_zone_in_either_spelling() {
    let (router, engine) = super::ec2_query::emulator();

    // (case, the member naming the zone, its value, the zone name the association reports)
    let cases: [(&str, &str, &str, &str); 2] = [
        (
            "a zone named by its name",
            "AvailabilityZone",
            "us-east-1b",
            "us-east-1b",
        ),
        (
            "a zone named by its id",
            "AvailabilityZoneId",
            "use1-az3",
            "us-east-1c",
        ),
    ];
    for (case, member, value, zone_name) in cases {
        let endpoint = seed_transit_gateway_endpoint(&engine);
        let body = ok(
            case,
            &router,
            ASSOCIATE,
            &associate_params(&endpoint, &[param(member, value)]),
        )
        .await;
        let answered = answered_status(case, &body);
        assert_eq!(
            answered, "associating",
            "case {case:?}: a fresh association is answered associating, got {answered:?}"
        );
        let reported = described_associations(case, &router, &endpoint).await;
        assert_eq!(
            reported,
            vec![(
                String::new(),
                String::new(),
                "associating".to_string(),
                vec![zone_name.to_string()],
            )],
            "case {case:?}: expected the association to report the zone {zone_name:?} and no \
             subnet, got {reported:?}"
        );

        // The same zone, named by its OTHER spelling, is the target network this endpoint
        // already has.
        let (other_member, other_value) = if member == "AvailabilityZone" {
            ("AvailabilityZoneId", "use1-az2")
        } else {
            ("AvailabilityZone", "us-east-1c")
        };
        let (status, duplicate) = call(
            &router,
            ASSOCIATE,
            &associate_params(&endpoint, &[param(other_member, other_value)]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the same zone again is refused, got {status} with {duplicate}"
        );
        assert_eq!(
            error_code(&duplicate),
            "InvalidClientVpnDuplicateAssociation",
            "case {case:?}: expected the duplicate-association code, got {duplicate}"
        );
    }
}

// ---------------------------------------------------------------------------
// What an association refuses
// ---------------------------------------------------------------------------

/// One refusal row: the request's fields and the EC2 code it is answered with.
struct RefusalRow {
    case: &'static str,
    endpoint: Option<String>,
    fields: Vec<Param>,
    code: &'static str,
    status: u16,
}

/// Every unservable request is refused with its own EC2 code, and writes nothing — the store
/// holds exactly the one association the suite made before the table ran.
#[tokio::test]
async fn an_unservable_association_is_refused_with_its_own_code_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc = create_vpc("the VPC under test", &router, VPC_CIDR).await;
    let other_vpc = create_vpc("a second VPC", &router, OTHER_VPC_CIDR).await;
    let associated =
        create_subnet("the associated subnet", &router, &vpc, "10.20.1.0/24", "us-east-1a").await;
    let same_zone =
        create_subnet("a second subnet in that zone", &router, &vpc, "10.20.2.0/24", "us-east-1a")
            .await;
    let free = create_subnet("a free subnet", &router, &vpc, "10.20.3.0/24", "us-east-1b").await;
    let foreign = create_subnet(
        "a subnet of the second VPC",
        &router,
        &other_vpc,
        "10.30.1.0/24",
        "us-east-1c",
    )
    .await;
    let endpoint = seed_endpoint(&engine, "the endpoint under test", None, &[]);
    let transit_gateway_endpoint = seed_transit_gateway_endpoint(&engine);

    // The one association every row below must leave exactly as it is.
    ok(
        "the association the table is written against",
        &router,
        ASSOCIATE,
        &associate_params(&endpoint, &[param("SubnetId", &associated)]),
    )
    .await;
    let before = stored_associations(&engine);

    let rows = vec![
        // A value this account holds no subnet for, in each of the forms a caller reaches
        // for: this surface answers the one refusal for all of them.
        RefusalRow {
            case: "a subnet id of the right form that names nothing",
            endpoint: None,
            fields: vec![param("SubnetId", "subnet-0123456789abcdef0")],
            code: "InvalidSubnetID.Malformed",
            status: 400,
        },
        RefusalRow {
            case: "a value that is not a subnet id at all",
            endpoint: None,
            fields: vec![param("SubnetId", "not-a-subnet")],
            code: "InvalidSubnetID.Malformed",
            status: 400,
        },
        RefusalRow {
            case: "another resource's id in the SubnetId member",
            endpoint: None,
            fields: vec![param("SubnetId", &vpc)],
            code: "InvalidSubnetID.Malformed",
            status: 400,
        },
        RefusalRow {
            case: "the all-zero placeholder subnet id",
            endpoint: None,
            fields: vec![param("SubnetId", "subnet-00000000000000000")],
            code: "InvalidSubnetID.Malformed",
            status: 400,
        },
        // An endpoint id this account holds no endpoint for, with a subnet that would
        // otherwise have been associated.
        RefusalRow {
            case: "an endpoint id of the right form that names nothing",
            endpoint: Some("cvpn-endpoint-0123456789abcdef0".to_string()),
            fields: vec![param("SubnetId", &free)],
            code: "InvalidClientVpnEndpointId.NotFound",
            status: 400,
        },
        RefusalRow {
            case: "an empty endpoint id",
            endpoint: Some(String::new()),
            fields: vec![param("SubnetId", &free)],
            code: "InvalidClientVpnEndpointId.NotFound",
            status: 400,
        },
        // The request has to name exactly one network.
        RefusalRow {
            case: "no network at all on a VPC-based endpoint",
            endpoint: None,
            fields: Vec::new(),
            code: "MissingParameter",
            status: 400,
        },
        RefusalRow {
            case: "an empty subnet id, which is no subnet id",
            endpoint: None,
            fields: vec![param("SubnetId", "")],
            code: "MissingParameter",
            status: 400,
        },
        RefusalRow {
            case: "no network at all on a Transit Gateway-based endpoint",
            endpoint: Some(transit_gateway_endpoint.clone()),
            fields: Vec::new(),
            code: "MissingParameter",
            status: 400,
        },
        RefusalRow {
            case: "both a subnet and a zone name",
            endpoint: None,
            fields: vec![
                param("SubnetId", &free),
                param("AvailabilityZone", "us-east-1b"),
            ],
            code: "InvalidParameterCombination",
            status: 400,
        },
        RefusalRow {
            case: "both a subnet and a zone id",
            endpoint: None,
            fields: vec![
                param("SubnetId", &free),
                param("AvailabilityZoneId", "use1-az2"),
            ],
            code: "InvalidParameterCombination",
            status: 400,
        },
        RefusalRow {
            case: "both spellings of a zone",
            endpoint: Some(transit_gateway_endpoint.clone()),
            fields: vec![
                param("AvailabilityZone", "us-east-1b"),
                param("AvailabilityZoneId", "use1-az2"),
            ],
            code: "InvalidParameterCombination",
            status: 400,
        },
        // Each endpoint takes the network its own kind has.
        RefusalRow {
            case: "a zone on a VPC-based endpoint",
            endpoint: None,
            fields: vec![param("AvailabilityZone", "us-east-1b")],
            code: "InvalidParameterCombination",
            status: 400,
        },
        RefusalRow {
            case: "a subnet on a Transit Gateway-based endpoint",
            endpoint: Some(transit_gateway_endpoint.clone()),
            fields: vec![param("SubnetId", &free)],
            code: "InvalidParameterCombination",
            status: 400,
        },
        RefusalRow {
            case: "a zone the Region does not have",
            endpoint: Some(transit_gateway_endpoint.clone()),
            fields: vec![param("AvailabilityZone", "us-east-1z")],
            code: "InvalidParameterValue",
            status: 400,
        },
        RefusalRow {
            case: "a zone id the Region does not have",
            endpoint: Some(transit_gateway_endpoint.clone()),
            fields: vec![param("AvailabilityZoneId", "use1-az9")],
            code: "InvalidParameterValue",
            status: 400,
        },
        // The network itself cannot join this endpoint's associations.
        RefusalRow {
            case: "a subnet of another VPC than the endpoint's",
            endpoint: None,
            fields: vec![param("SubnetId", &foreign)],
            code: "InvalidParameterValue",
            status: 400,
        },
        RefusalRow {
            case: "a subnet that is already a target network of the endpoint",
            endpoint: None,
            fields: vec![param("SubnetId", &associated)],
            code: "InvalidClientVpnDuplicateAssociation",
            status: 400,
        },
        RefusalRow {
            case: "a second subnet in an Availability Zone the endpoint already has",
            endpoint: None,
            fields: vec![param("SubnetId", &same_zone)],
            code: "InvalidClientVpnSubnetId.DuplicateAz",
            status: 400,
        },
        // Request-shaped ceilings and the permission check.
        RefusalRow {
            case: "a client token past the bound this crate holds one to",
            endpoint: None,
            fields: vec![
                param("SubnetId", &free),
                param("ClientToken", &"t".repeat(MAX_CLIENT_TOKEN_CHARS + 1)),
            ],
            code: "InvalidParameterValue",
            status: 400,
        },
        RefusalRow {
            case: "a dry run that would otherwise have succeeded",
            endpoint: None,
            fields: vec![param("SubnetId", &free), param("DryRun", "true")],
            code: "DryRunOperation",
            status: 412,
        },
    ];

    for row in &rows {
        let named = row.endpoint.clone().unwrap_or_else(|| endpoint.clone());
        let (status, body) = call(&router, ASSOCIATE, &associate_params(&named, &row.fields)).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {expected}, got {status} with body {body}",
            case = row.case,
            expected = row.status,
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {code:?}, got {body}",
            case = row.case,
            code = row.code,
        );
        assert_eq!(
            stored_associations(&engine),
            before,
            "case {case:?}: a refused request must write nothing",
            case = row.case,
        );
    }
}

/// A refused subnet is named in the refusal, whatever the request sent — the message states
/// the value it could not resolve rather than one recorded value.
#[tokio::test]
async fn a_refused_subnet_is_named_in_the_refusal() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test", None, &[]);
    for requested in [
        "subnet-0123456789abcdef0",
        "subnet-00000000",
        "not-a-subnet",
        "vpc-00000000000000001",
    ] {
        let (status, body) = call(
            &router,
            ASSOCIATE,
            &associate_params(&endpoint, &[param("SubnetId", requested)]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {requested:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The subnet ID '{requested}' is malformed"),
            "case {requested:?}: the refusal must name the value the request sent, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// One `ClientToken` names the one association it made: a replay of the same request reports
/// that association and writes nothing, while the same token with another network is refused
/// rather than answered with an association the caller did not ask for.
#[tokio::test]
async fn a_replayed_client_token_reports_the_one_association_it_made() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc = create_vpc("the VPC under test", &router, VPC_CIDR).await;
    let first = create_subnet("the first subnet", &router, &vpc, "10.20.1.0/24", "us-east-1a").await;
    let second =
        create_subnet("the second subnet", &router, &vpc, "10.20.2.0/24", "us-east-1b").await;
    let endpoint = seed_endpoint(&engine, "the endpoint under test", Some(&vpc), &[]);
    let token = "a-token-of-this-suite";

    let body = ok(
        "the first call",
        &router,
        ASSOCIATE,
        &associate_params(
            &endpoint,
            &[param("SubnetId", &first), param("ClientToken", token)],
        ),
    )
    .await;
    let association_id =
        element(&body, "associationId").unwrap_or_else(|| panic!("no <associationId> in {body}"));
    let after_first = stored_associations(&engine);

    let replayed = ok(
        "the replay",
        &router,
        ASSOCIATE,
        &associate_params(
            &endpoint,
            &[param("SubnetId", &first), param("ClientToken", token)],
        ),
    )
    .await;
    assert_eq!(
        element(&replayed, "associationId").as_deref(),
        Some(association_id.as_str()),
        "the replay: expected the association {association_id:?} the token already made, got \
         {replayed}"
    );
    assert_eq!(
        stored_associations(&engine),
        after_first,
        "the replay: a replayed token must write nothing"
    );

    let (status, mismatched) = call(
        &router,
        ASSOCIATE,
        &associate_params(
            &endpoint,
            &[param("SubnetId", &second), param("ClientToken", token)],
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "the token with another subnet: expected HTTP 400, got {status} with {mismatched}"
    );
    assert_eq!(
        error_code(&mismatched),
        "IdempotentParameterMismatch",
        "the token with another subnet: expected the mismatch code, got {mismatched}"
    );
    assert_eq!(
        stored_associations(&engine),
        after_first,
        "the token with another subnet: a refused request must write nothing"
    );

    // Another token is another association: the second subnet joins the endpoint under it.
    ok(
        "a second token",
        &router,
        ASSOCIATE,
        &associate_params(
            &endpoint,
            &[
                param("SubnetId", &second),
                param("ClientToken", "another-token-of-this-suite"),
            ],
        ),
    )
    .await;
    assert_eq!(
        stored_associations(&engine),
        after_first + 1,
        "a second token: expected one further association"
    );
}

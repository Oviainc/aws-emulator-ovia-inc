// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for a VPC's PATH TO THE INTERNET — `CreateInternetGateway`,
//! `AttachInternetGateway`, `CreateRouteTable`, `CreateRoute` and `AssociateRouteTable`,
//! read back through `DescribeInternetGateways` and `DescribeRouteTables` — driven over the
//! wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: **an account builds that path out of resources each of which is
//! stored where its own describe reports it** — an internet gateway is created attached to
//! nothing and then attaches to exactly ONE VPC, a route table is created for one VPC
//! carrying the local route every table comes with for each of that VPC's IPv4 blocks, a
//! route points ONE destination at ONE target of that table's own VPC, and a subnet is
//! associated with at most one route table of its own VPC. Nothing on that path is reported
//! that was not stored, and a second claim on either side of an attachment or an
//! association is refused rather than added beside the one that holds.
//!
//! Every table row is a distinct input along what that rule generalises over — the
//! destination block's value, size and address family, how many host bits the request set,
//! the tag count, how many subnets an association covers, and which side of a pair a
//! request collides with — so any one recorded call is one row among many rather than the
//! specification.

use super::ec2_query::{element, elements, error_code, error_message, filters};
use super::wire::{param, Param};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
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

/// One element of a response body, or a panic naming the case.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    member(case, &body, "vpcId")
}

/// A subnet of `vpc_id` with `cidr_block`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    member(case, &body, "subnetId")
}

/// An internet gateway carrying `tags`, as its id.
async fn create_internet_gateway(
    case: &str,
    router: &MultiServiceRouter,
    tags: &[(&str, &str)],
) -> String {
    let mut params = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param("TagSpecification.1.ResourceType", "internet-gateway"));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Key"),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    let body = ok(case, router, "CreateInternetGateway", &params).await;
    member(case, &body, "internetGatewayId")
}

/// A route table of `vpc_id`, as its id.
async fn create_route_table(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateRouteTable",
        &[param("VpcId", vpc_id)],
    )
    .await;
    member(case, &body, "routeTableId")
}

/// The `DescribeRouteTables` body for the one table `route_table_id` names.
async fn describe_route_table(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
) -> String {
    ok(
        case,
        router,
        "DescribeRouteTables",
        &[param("RouteTableId.1", route_table_id)],
    )
    .await
}

/// The `DescribeInternetGateways` body for the one gateway `internet_gateway_id` names.
async fn describe_internet_gateway(
    case: &str,
    router: &MultiServiceRouter,
    internet_gateway_id: &str,
) -> String {
    ok(
        case,
        router,
        "DescribeInternetGateways",
        &[param("InternetGatewayId.1", internet_gateway_id)],
    )
    .await
}

/// One row of a table whose middle column is a list of `(name, value)` pairs — the tags a
/// create asks for, or the parameters a refusal row sends beside the resource it names.
/// Named so the row type is a type rather than a nest of tuples.
type PairsRow<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

/// Whether `value` is an EC2 long resource id of `prefix`.
fn is_long_id(prefix: &str, value: &str) -> bool {
    super::wire::is_long_id(prefix, value)
}

// ---------------------------------------------------------------------------
// CreateInternetGateway / AttachInternetGateway
// ---------------------------------------------------------------------------

/// The rule: a created internet gateway is stored attached to NOTHING ("after creating the
/// internet gateway, you attach it to a VPC using AttachInternetGateway" —
/// `CreateInternetGateway`), carries the tags its specification asked for, and reads back
/// through `DescribeInternetGateways`; attaching it records the one attachment, which that
/// describe then reports and its `attachment.*` filters select on.
///
/// The rows differ in how many tags the create carries — none, one, several — which is what
/// the tag payload generalises over.
#[tokio::test]
async fn an_internet_gateway_is_created_unattached_and_attaches_to_one_vpc() {
    let router = emulator();
    // (case, the tags the create asks for, the VPC's block)
    let cases: [PairsRow<'_>; 3] = [
        ("no tags", &[], "10.20.0.0/16"),
        ("one tag", &[("Name", "edge")], "10.21.0.0/16"),
        (
            "several tags",
            &[("Name", "edge"), ("team", "platform"), ("env", "test")],
            "10.22.0.0/20",
        ),
    ];

    for (case, tags, vpc_cidr) in cases {
        let internet_gateway_id = create_internet_gateway(case, &router, tags).await;
        assert!(
            is_long_id("igw", &internet_gateway_id),
            "case {case:?}: expected an igw-… id, got {internet_gateway_id:?}"
        );

        // Nothing is attached yet, so the describe reports the gateway with no attachment.
        let body = describe_internet_gateway(case, &router, &internet_gateway_id).await;
        assert!(
            elements(&body, "vpcId").is_empty(),
            "case {case:?}: expected no attachment before the attach, got {body}"
        );
        for (key, value) in tags {
            let keys = elements(&body, "key");
            let values = elements(&body, "value");
            assert!(
                keys.iter().any(|held| held == key) && values.iter().any(|held| held == value),
                "case {case:?}: expected the declared tag {key:?}={value:?}, got {body}"
            );
        }

        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        ok(
            case,
            &router,
            "AttachInternetGateway",
            &[
                param("InternetGatewayId", &internet_gateway_id),
                param("VpcId", &vpc_id),
            ],
        )
        .await;

        // The attachment is what the describe now reports: the VPC it was made to, in the
        // one state an internet gateway's attachment is reported in ("for an internet
        // gateway, the state is `available` when attached to a VPC" —
        // `InternetGatewayAttachment$State`).
        let body = describe_internet_gateway(case, &router, &internet_gateway_id).await;
        assert_eq!(
            elements(&body, "vpcId"),
            vec![vpc_id.clone()],
            "case {case:?}: expected the attachment to name {vpc_id:?}, got {body}"
        );
        assert_eq!(
            elements(&body, "state"),
            vec!["available".to_string()],
            "case {case:?}: expected the attachment reported available, got {body}"
        );

        // And the documented filters select on it.
        let filtered = ok(
            case,
            &router,
            "DescribeInternetGateways",
            &filters(&[
                ("attachment.vpc-id", &[&vpc_id]),
                ("attachment.state", &["available"]),
            ]),
        )
        .await;
        assert_eq!(
            elements(&filtered, "internetGatewayId"),
            vec![internet_gateway_id.clone()],
            "case {case:?}: expected the attachment filters to select the gateway, got \
             {filtered}"
        );
    }
}

/// The rule's refusal half: "you can attach only one internet gateway to a VPC at a time"
/// (Amazon VPC User Guide, "Connect to the internet using an internet gateway"), and a
/// gateway attaches one VPC — so a second claim on EITHER side is refused with EC2's
/// `Resource.AlreadyAssociated` and nothing is added beside the attachment that holds,
/// while an id naming no resource is that resource's own not-found refusal.
///
/// The rows differ in which side of the pair the request collides with and in which id
/// names nothing.
#[tokio::test]
async fn a_second_claim_on_either_side_of_an_internet_gateway_attachment_is_refused() {
    let router = emulator();
    let case = "the attachment that holds";
    let vpc_id = create_vpc(case, &router, "10.23.0.0/16").await;
    let other_vpc_id = create_vpc(case, &router, "10.24.0.0/16").await;
    let internet_gateway_id = create_internet_gateway(case, &router, &[]).await;
    let other_gateway_id = create_internet_gateway(case, &router, &[]).await;
    ok(
        case,
        &router,
        "AttachInternetGateway",
        &[
            param("InternetGatewayId", &internet_gateway_id),
            param("VpcId", &vpc_id),
        ],
    )
    .await;

    // (case, the gateway, the VPC, the error code, what the message has to name)
    let cases = [
        (
            "the same gateway again",
            internet_gateway_id.as_str(),
            vpc_id.as_str(),
            "Resource.AlreadyAssociated",
            internet_gateway_id.as_str(),
        ),
        (
            "the gateway attached elsewhere",
            internet_gateway_id.as_str(),
            other_vpc_id.as_str(),
            "Resource.AlreadyAssociated",
            vpc_id.as_str(),
        ),
        (
            "a second gateway for the VPC",
            other_gateway_id.as_str(),
            vpc_id.as_str(),
            "Resource.AlreadyAssociated",
            internet_gateway_id.as_str(),
        ),
        (
            "a gateway id naming nothing",
            "igw-00000000000000000",
            vpc_id.as_str(),
            "InvalidInternetGatewayID.NotFound",
            "igw-00000000000000000",
        ),
        (
            "a VPC id naming nothing",
            other_gateway_id.as_str(),
            "vpc-00000000000000000",
            "InvalidVpcID.NotFound",
            "vpc-00000000000000000",
        ),
    ];

    for (case, gateway, vpc, expected_code, named) in cases {
        let (status, body) = call(
            &router,
            "AttachInternetGateway",
            &[param("InternetGatewayId", gateway), param("VpcId", vpc)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            error_message(&body).contains(named),
            "case {case:?}: expected the message to name {named:?}, got {}",
            error_message(&body)
        );
    }

    // Nothing was added beside the attachment that holds.
    let body = describe_internet_gateway(case, &router, &internet_gateway_id).await;
    assert_eq!(
        elements(&body, "vpcId"),
        vec![vpc_id.clone()],
        "expected the one attachment to survive every refusal, got {body}"
    );
}

// ---------------------------------------------------------------------------
// CreateRouteTable
// ---------------------------------------------------------------------------

/// The rule: a route table is created FOR one VPC and comes with the local route every
/// table has ("every route table contains a local route for communication within the VPC",
/// whose target is `local` — Amazon VPC User Guide, "Route tables"), and that table reads
/// back through `DescribeRouteTables` with the routes it holds. The VPC's own MAIN table —
/// the one "your VPC automatically comes with" — is reported beside it, with the implicit
/// association `main` marks.
///
/// The rows differ in the VPC's block value and size, which is what the local route's
/// destination is read from.
#[tokio::test]
async fn a_route_table_is_created_for_its_vpc_with_the_local_route_it_comes_with() {
    let router = emulator();
    // (case, the VPC's block)
    let cases = [
        ("a /16", "10.30.0.0/16"),
        ("a /20", "10.31.0.0/20"),
        ("a /24 outside the private 10 block", "192.168.7.0/24"),
    ];

    for (case, vpc_cidr) in cases {
        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        let route_table_id = create_route_table(case, &router, &vpc_id).await;
        assert!(
            is_long_id("rtb", &route_table_id),
            "case {case:?}: expected an rtb-… id, got {route_table_id:?}"
        );

        let body = describe_route_table(case, &router, &route_table_id).await;
        assert_eq!(
            element(&body, "vpcId").as_deref(),
            Some(vpc_id.as_str()),
            "case {case:?}: expected the table to name its VPC, got {body}"
        );
        assert_eq!(
            elements(&body, "destinationCidrBlock"),
            vec![vpc_cidr.to_string()],
            "case {case:?}: expected the local route for {vpc_cidr}, got {body}"
        );
        assert_eq!(
            elements(&body, "gatewayId"),
            vec!["local".to_string()],
            "case {case:?}: expected the local route's target to be local, got {body}"
        );
        assert_eq!(
            elements(&body, "origin"),
            vec!["CreateRouteTable".to_string()],
            "case {case:?}: expected the local route's origin, got {body}"
        );
        assert_eq!(
            elements(&body, "state"),
            vec!["active".to_string()],
            "case {case:?}: expected the local route active, got {body}"
        );
        // A table a caller created is not the main one, so it carries no association at all
        // until one is made.
        assert!(
            elements(&body, "routeTableAssociationId").is_empty(),
            "case {case:?}: expected no association on a fresh table, got {body}"
        );

        // The VPC's own main table is there beside it, with the implicit association.
        let of_vpc = ok(
            case,
            &router,
            "DescribeRouteTables",
            &filters(&[("vpc-id", &[&vpc_id]), ("association.main", &["true"])]),
        )
        .await;
        let main_tables = elements(&of_vpc, "routeTableId");
        assert_eq!(
            main_tables.len(),
            2,
            "case {case:?}: expected the main table reported with its association's own \
             routeTableId, got {of_vpc}"
        );
        assert!(
            !main_tables.contains(&route_table_id),
            "case {case:?}: expected the created table not to be the main one, got {of_vpc}"
        );
    }

    // A VPC id naming no VPC is a refusal rather than a table in no VPC.
    let (status, body) = call(
        &router,
        "CreateRouteTable",
        &[param("VpcId", "vpc-00000000000000000")],
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} with body {body}");
    assert_eq!(
        error_code(&body),
        "InvalidVpcID.NotFound",
        "expected the VPC not-found refusal, got {body}"
    );
}

/// The same rule one address family along: "routes to IPv4 and IPv6 addresses within a VPC"
/// are both local routes (Amazon VPC User Guide, "Route tables"), so a table of a VPC that
/// has an IPv6 block associated comes with a local route for THAT block as well as for its
/// IPv4 one.
#[tokio::test]
async fn a_route_table_of_a_dual_stack_vpc_comes_with_a_local_route_per_address_family() {
    let router = emulator();
    let case = "an Amazon-provided IPv6 block";
    let body = ok(
        case,
        &router,
        "CreateVpc",
        &[
            param("CidrBlock", "10.70.0.0/16"),
            param("AmazonProvidedIpv6CidrBlock", "true"),
        ],
    )
    .await;
    let vpc_id = member(case, &body, "vpcId");
    let ipv6_cidr_block = member(case, &body, "ipv6CidrBlock");

    let route_table_id = create_route_table(case, &router, &vpc_id).await;
    let read = describe_route_table(case, &router, &route_table_id).await;
    assert_eq!(
        elements(&read, "destinationCidrBlock"),
        vec!["10.70.0.0/16".to_string()],
        "case {case:?}: expected the IPv4 local route, got {read}"
    );
    assert_eq!(
        elements(&read, "destinationIpv6CidrBlock"),
        vec![ipv6_cidr_block.clone()],
        "case {case:?}: expected a local route for the VPC's IPv6 block, got {read}"
    );
    // Both are the table's own local routes, so both carry that origin and the `local`
    // target — one per block rather than one route carrying two destinations.
    assert_eq!(
        elements(&read, "origin"),
        vec!["CreateRouteTable".to_string(), "CreateRouteTable".to_string()],
        "case {case:?}: expected both local routes to carry the create's origin, got {read}"
    );
    assert_eq!(
        elements(&read, "gatewayId"),
        vec!["local".to_string(), "local".to_string()],
        "case {case:?}: expected both local routes to target local, got {read}"
    );
}

// ---------------------------------------------------------------------------
// CreateRoute
// ---------------------------------------------------------------------------

/// The rule: a route matches ONE destination — "we modify the specified CIDR block to its
/// canonical form" (`CreateRouteRequest$DestinationCidrBlock`) — and points it at ONE
/// target of the route table's own VPC, and the route reads back through
/// `DescribeRouteTables`.
///
/// The rows differ in the destination's value, its size, how many host bits the request set
/// and its address family, which is what the canonical form generalises over.
#[tokio::test]
async fn create_route_stores_one_route_per_destination_against_an_attached_gateway() {
    let router = emulator();
    let case = "the VPC the routes are in";
    let vpc_id = create_vpc(case, &router, "10.40.0.0/16").await;
    let internet_gateway_id = create_internet_gateway(case, &router, &[]).await;
    ok(
        case,
        &router,
        "AttachInternetGateway",
        &[
            param("InternetGatewayId", &internet_gateway_id),
            param("VpcId", &vpc_id),
        ],
    )
    .await;

    // (case, the member the destination travels in, the requested value, its canonical form)
    let cases = [
        (
            "every IPv4 address",
            "DestinationCidrBlock",
            "0.0.0.0/0",
            "0.0.0.0/0",
        ),
        (
            "a /24",
            "DestinationCidrBlock",
            "192.0.2.0/24",
            "192.0.2.0/24",
        ),
        (
            "host bits set",
            "DestinationCidrBlock",
            "198.51.100.77/26",
            "198.51.100.64/26",
        ),
        (
            "a single address",
            "DestinationCidrBlock",
            "203.0.113.9/32",
            "203.0.113.9/32",
        ),
        (
            "every IPv6 address",
            "DestinationIpv6CidrBlock",
            "::/0",
            "::/0",
        ),
        (
            "an IPv6 block with host bits set",
            "DestinationIpv6CidrBlock",
            "2001:db8:1234::5/48",
            "2001:db8:1234::/48",
        ),
    ];

    for (case, member_name, requested, canonical) in cases {
        let route_table_id = create_route_table(case, &router, &vpc_id).await;
        let body = ok(
            case,
            &router,
            "CreateRoute",
            &[
                param("RouteTableId", &route_table_id),
                param(member_name, requested),
                param("GatewayId", &internet_gateway_id),
            ],
        )
        .await;
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected CreateRoute to answer true, got {body}"
        );

        let read = describe_route_table(case, &router, &route_table_id).await;
        let destination_element = if member_name == "DestinationCidrBlock" {
            "destinationCidrBlock"
        } else {
            "destinationIpv6CidrBlock"
        };
        assert!(
            elements(&read, destination_element).contains(&canonical.to_string()),
            "case {case:?}: expected the route for {canonical} in the table, got {read}"
        );
        assert!(
            elements(&read, "gatewayId").contains(&internet_gateway_id),
            "case {case:?}: expected the route's target to be the attached gateway, got \
             {read}"
        );
        assert!(
            elements(&read, "origin").contains(&"CreateRoute".to_string()),
            "case {case:?}: expected the route's origin to be CreateRoute, got {read}"
        );

        // One destination has one target, so the same destination again is refused rather
        // than replacing the route that holds.
        let (status, duplicate) = call(
            &router,
            "CreateRoute",
            &[
                param("RouteTableId", &route_table_id),
                param(member_name, requested),
                param("GatewayId", &internet_gateway_id),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the duplicate destination refused, got {status} with \
             body {duplicate}"
        );
        assert_eq!(
            error_code(&duplicate),
            "RouteAlreadyExists",
            "case {case:?}: expected RouteAlreadyExists, got {duplicate}"
        );
        assert!(
            error_message(&duplicate).contains(canonical),
            "case {case:?}: expected the refusal to name {canonical}, got {}",
            error_message(&duplicate)
        );
    }
}

/// The rule's refusal half for a route: a destination the request leaves out or states
/// twice names no route, a target naming no resource of the account is that resource's own
/// not-found refusal, and a gateway attached to ANOTHER VPC cannot carry this table's
/// traffic.
#[tokio::test]
async fn create_route_refuses_a_destination_or_a_target_it_cannot_resolve() {
    let router = emulator();
    let case = "the route table under test";
    let vpc_id = create_vpc(case, &router, "10.41.0.0/16").await;
    let route_table_id = create_route_table(case, &router, &vpc_id).await;
    let other_vpc_id = create_vpc(case, &router, "10.42.0.0/16").await;
    let foreign_gateway_id = create_internet_gateway(case, &router, &[]).await;
    ok(
        case,
        &router,
        "AttachInternetGateway",
        &[
            param("InternetGatewayId", &foreign_gateway_id),
            param("VpcId", &other_vpc_id),
        ],
    )
    .await;
    let unattached_gateway_id = create_internet_gateway(case, &router, &[]).await;

    // (case, the parameters beside the route table, the expected error code)
    let cases: [PairsRow<'_>; 6] = [
        (
            "no destination at all",
            &[("GatewayId", "")],
            "MissingParameter",
        ),
        (
            "no target at all",
            &[("DestinationCidrBlock", "0.0.0.0/0")],
            "MissingParameter",
        ),
        (
            "two destinations",
            &[
                ("DestinationCidrBlock", "0.0.0.0/0"),
                ("DestinationIpv6CidrBlock", "::/0"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a destination that is no CIDR block",
            &[("DestinationCidrBlock", "not-a-block")],
            "InvalidParameterValue",
        ),
        (
            "a gateway id naming nothing",
            &[
                ("DestinationCidrBlock", "0.0.0.0/0"),
                ("GatewayId", "igw-00000000000000000"),
            ],
            "InvalidGatewayID.NotFound",
        ),
        (
            "a gateway attached to another VPC",
            &[("DestinationCidrBlock", "0.0.0.0/0")],
            "InvalidParameterValue",
        ),
    ];

    for (index, (case, extra, expected_code)) in cases.into_iter().enumerate() {
        let mut params = vec![param("RouteTableId", &route_table_id)];
        for (name, value) in extra {
            if value.is_empty() {
                continue;
            }
            params.push(param(name, value));
        }
        // The last row's target is the gateway of the OTHER VPC, named here because its id
        // is only known at run time.
        if index == 5 {
            params.push(param("GatewayId", &foreign_gateway_id));
        }
        let (status, body) = call(&router, "CreateRoute", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // An unattached gateway is in the same position as one of another VPC: it carries no
    // traffic for this table's network.
    let (status, body) = call(
        &router,
        "CreateRoute",
        &[
            param("RouteTableId", &route_table_id),
            param("DestinationCidrBlock", "0.0.0.0/0"),
            param("GatewayId", &unattached_gateway_id),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected an unattached gateway refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "expected the different-networks refusal, got {body}"
    );

    // A route table id naming nothing is refused too.
    let (status, body) = call(
        &router,
        "CreateRoute",
        &[
            param("RouteTableId", "rtb-00000000000000000"),
            param("DestinationCidrBlock", "0.0.0.0/0"),
            param("GatewayId", &unattached_gateway_id),
        ],
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} with body {body}");
    assert_eq!(
        error_code(&body),
        "InvalidRouteTableID.NotFound",
        "expected the route table not-found refusal, got {body}"
    );

    // Nothing above stored a route: the table still holds only the local one.
    let read = describe_route_table(case, &router, &route_table_id).await;
    assert_eq!(
        elements(&read, "destinationCidrBlock"),
        vec!["10.41.0.0/16".to_string()],
        "expected only the local route after every refusal, got {read}"
    );
}

// ---------------------------------------------------------------------------
// AssociateRouteTable
// ---------------------------------------------------------------------------

/// The rule: a subnet is associated with a route table of ITS OWN VPC, the association is
/// reported on that table by `DescribeRouteTables` under the id the associate answered, and
/// "each subnet in your VPC must be associated with a route table" — with exactly one, so a
/// second association for the same subnet is refused rather than added beside it.
///
/// The rows differ in how many subnets one table covers ("a route table can be associated
/// with multiple subnets" — `AssociateRouteTable`), which is what the association set
/// generalises over.
#[tokio::test]
async fn associate_route_table_binds_each_subnet_to_one_table_of_its_own_vpc() {
    let router = emulator();
    // (case, the VPC's block, the subnet blocks one table is associated with)
    let cases: [(&str, &str, &[&str]); 3] = [
        ("one subnet", "10.50.0.0/16", &["10.50.1.0/24"]),
        (
            "two subnets",
            "10.51.0.0/16",
            &["10.51.1.0/24", "10.51.2.0/24"],
        ),
        (
            "three subnets",
            "10.52.0.0/16",
            &["10.52.1.0/24", "10.52.2.0/24", "10.52.3.0/24"],
        ),
    ];

    for (case, vpc_cidr, subnet_cidrs) in cases {
        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        let route_table_id = create_route_table(case, &router, &vpc_id).await;
        let mut associations = Vec::new();
        let mut subnet_ids = Vec::new();
        for subnet_cidr in subnet_cidrs {
            let subnet_id = create_subnet(case, &router, &vpc_id, subnet_cidr).await;
            let body = ok(
                case,
                &router,
                "AssociateRouteTable",
                &[
                    param("RouteTableId", &route_table_id),
                    param("SubnetId", &subnet_id),
                ],
            )
            .await;
            let association_id = member(case, &body, "associationId");
            assert!(
                is_long_id("rtbassoc", &association_id),
                "case {case:?}: expected an rtbassoc-… id, got {association_id:?}"
            );
            assert_eq!(
                element(&body, "state").as_deref(),
                Some("associated"),
                "case {case:?}: expected the association reported associated, got {body}"
            );
            associations.push(association_id);
            subnet_ids.push(subnet_id);
        }

        // Every association is on the table the associate named, under the id it answered.
        let read = describe_route_table(case, &router, &route_table_id).await;
        assert_eq!(
            elements(&read, "routeTableAssociationId"),
            associations,
            "case {case:?}: expected the table to report every association, got {read}"
        );
        assert_eq!(
            elements(&read, "subnetId"),
            subnet_ids,
            "case {case:?}: expected the table to report every associated subnet, got {read}"
        );

        // The documented filter selects the table by one of its associated subnets.
        let filtered = ok(
            case,
            &router,
            "DescribeRouteTables",
            &filters(&[("association.subnet-id", &[subnet_ids[0].as_str()])]),
        )
        .await;
        assert!(
            elements(&filtered, "routeTableId").contains(&route_table_id),
            "case {case:?}: expected the association filter to select the table, got \
             {filtered}"
        );

        // A second association for a subnet that already has one is refused.
        let second_table_id = create_route_table(case, &router, &vpc_id).await;
        let (status, body) = call(
            &router,
            "AssociateRouteTable",
            &[
                param("RouteTableId", &second_table_id),
                param("SubnetId", &subnet_ids[0]),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second association refused, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "Resource.AlreadyAssociated",
            "case {case:?}: expected Resource.AlreadyAssociated, got {body}"
        );
        assert!(
            error_message(&body).contains(&route_table_id),
            "case {case:?}: expected the refusal to name the table that holds it, got {}",
            error_message(&body)
        );
    }
}

/// The rule's cross-VPC half: "associates a subnet IN YOUR VPC … with a route table IN YOUR
/// VPC" — a route table routes for one VPC, so a subnet of another is refused, as is an id
/// naming no resource at all.
#[tokio::test]
async fn associate_route_table_refuses_a_subnet_of_another_vpc_or_an_id_naming_nothing() {
    let router = emulator();
    let case = "the two VPCs";
    let vpc_id = create_vpc(case, &router, "10.60.0.0/16").await;
    let other_vpc_id = create_vpc(case, &router, "10.61.0.0/16").await;
    let route_table_id = create_route_table(case, &router, &vpc_id).await;
    let foreign_subnet_id = create_subnet(case, &router, &other_vpc_id, "10.61.1.0/24").await;
    let subnet_id = create_subnet(case, &router, &vpc_id, "10.60.1.0/24").await;

    // (case, the table, the subnet, the expected error code)
    let cases = [
        (
            "a subnet of another VPC",
            route_table_id.as_str(),
            foreign_subnet_id.as_str(),
            "InvalidParameterValue",
        ),
        (
            "a route table id naming nothing",
            "rtb-00000000000000000",
            subnet_id.as_str(),
            "InvalidRouteTableID.NotFound",
        ),
        (
            "a subnet id naming nothing",
            route_table_id.as_str(),
            "subnet-00000000000000000",
            "InvalidSubnetID.NotFound",
        ),
    ];

    for (case, table, subnet, expected_code) in cases {
        let (status, body) = call(
            &router,
            "AssociateRouteTable",
            &[param("RouteTableId", table), param("SubnetId", subnet)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Nothing above associated anything: the table still carries no association.
    let read = describe_route_table(case, &router, &route_table_id).await;
    assert!(
        elements(&read, "routeTableAssociationId").is_empty(),
        "expected no association after every refusal, got {read}"
    );
}

// ---------------------------------------------------------------------------
// DisassociateRouteTable / DeleteRouteTable
// ---------------------------------------------------------------------------

/// The rule: the path comes apart the way it was built — "after you perform this action, the
/// subnet no longer uses the routes in the route table" (`DisassociateRouteTable`), and "you
/// must disassociate the route table from any subnets before you can delete it. You can't
/// delete the main route table" (`DeleteRouteTable`) — so a table a subnet still uses is
/// refused, a table nothing uses is removed, and its own describe then reports it for nobody.
///
/// The rows differ in how many subnets the table covers, which is what the set a delete has
/// to find empty generalises over.
#[tokio::test]
async fn a_route_table_is_deleted_once_every_subnet_is_disassociated_from_it() {
    let router = emulator();
    // (case, the VPC's block, the subnet blocks associated with the one table)
    let cases: [(&str, &str, &[&str]); 3] = [
        ("no subnet", "10.70.0.0/16", &[]),
        ("one subnet", "10.71.0.0/16", &["10.71.1.0/24"]),
        (
            "two subnets",
            "10.72.0.0/16",
            &["10.72.1.0/24", "10.72.2.0/24"],
        ),
    ];

    for (case, vpc_cidr, subnet_cidrs) in cases {
        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        let route_table_id = create_route_table(case, &router, &vpc_id).await;
        let mut associations = Vec::new();
        for subnet_cidr in subnet_cidrs {
            let subnet_id = create_subnet(case, &router, &vpc_id, subnet_cidr).await;
            let body = ok(
                case,
                &router,
                "AssociateRouteTable",
                &[
                    param("RouteTableId", &route_table_id),
                    param("SubnetId", &subnet_id),
                ],
            )
            .await;
            associations.push(member(case, &body, "associationId"));
        }

        // A table a subnet still uses has a dependency, so the delete is refused rather than
        // carried out under that subnet.
        if !associations.is_empty() {
            let (status, body) = call(
                &router,
                "DeleteRouteTable",
                &[param("RouteTableId", &route_table_id)],
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the delete of an associated table refused, got {status} \
                 with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "DependencyViolation",
                "case {case:?}: expected DependencyViolation, got {body}"
            );
        }

        for association_id in &associations {
            ok(
                case,
                &router,
                "DisassociateRouteTable",
                &[param("AssociationId", association_id)],
            )
            .await;
            // A disassociate is not idempotent: the association it took away names nothing
            // now, which is what a caller that disassociates twice has to be able to tell.
            let (status, body) = call(
                &router,
                "DisassociateRouteTable",
                &[param("AssociationId", association_id)],
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a second disassociate refused, got {status} with body \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidAssociationID.NotFound",
                "case {case:?}: expected InvalidAssociationID.NotFound, got {body}"
            );
        }

        // The table reports no association at all now, and the subnets go on using the VPC's
        // main table, which is still there.
        let read = describe_route_table(case, &router, &route_table_id).await;
        assert!(
            elements(&read, "routeTableAssociationId").is_empty(),
            "case {case:?}: expected no association left on the table, got {read}"
        );

        ok(
            case,
            &router,
            "DeleteRouteTable",
            &[param("RouteTableId", &route_table_id)],
        )
        .await;
        let (status, body) = call(
            &router,
            "DescribeRouteTables",
            &[param("RouteTableId.1", &route_table_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted table to be reported for nobody, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRouteTableID.NotFound",
            "case {case:?}: expected InvalidRouteTableID.NotFound, got {body}"
        );

        // "You can't delete the main route table": the table the VPC came with is refused,
        // and it is still the VPC's after the refusal.
        let main_table_id = {
            let read = ok(
                case,
                &router,
                "DescribeRouteTables",
                &filters(&[("vpc-id", &[vpc_id.as_str()])]),
            )
            .await;
            elements(&read, "routeTableId")
                .into_iter()
                .next()
                .unwrap_or_default()
        };
        let (status, body) = call(
            &router,
            "DeleteRouteTable",
            &[param("RouteTableId", &main_table_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete of the main table refused, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DependencyViolation",
            "case {case:?}: expected DependencyViolation for the main table, got {body}"
        );
        let read = describe_route_table(case, &router, &main_table_id).await;
        assert!(
            elements(&read, "routeTableId").contains(&main_table_id),
            "case {case:?}: expected the main table still held after the refusal, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateVpcEndpoint with route tables / DeleteVpcEndpoints
// ---------------------------------------------------------------------------

/// The rule: "we automatically add a route to the route table with a destination of the prefix
/// list for the service and a target of the gateway endpoint" (Amazon Web Services PrivateLink
/// User Guide, "Gateway endpoints") — so a gateway endpoint created for route tables of its own
/// VPC puts that one route in EVERY table it names and in no table it does not, and "when you
/// delete a gateway endpoint, we delete the endpoint routes in the route tables for the
/// endpoint" (`DeleteVpcEndpoints`) takes them away again.
///
/// The rows differ in how many of the VPC's tables the endpoint names, which is what the
/// association set generalises over.
#[tokio::test]
async fn a_gateway_endpoint_routes_every_table_it_names_through_itself() {
    let router = emulator();
    /// The endpoint service of this Region a gateway endpoint is created for, spelled the way
    /// a gateway endpoint's service name is (`com.amazonaws.<region>.<service>`), and the one
    /// `DescribePrefixLists` publishes a prefix list for.
    const SERVICE_NAME: &str = "com.amazonaws.us-east-1.s3";

    // (case, the VPC's block, how many of its three tables the endpoint names)
    let cases: [(&str, &str, usize); 3] = [
        ("one table", "10.80.0.0/16", 1),
        ("two tables", "10.81.0.0/16", 2),
        ("three tables", "10.82.0.0/16", 3),
    ];

    for (case, vpc_cidr, named) in cases {
        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        let tables = {
            let mut tables = Vec::new();
            for _ in 0..3 {
                tables.push(create_route_table(case, &router, &vpc_id).await);
            }
            tables
        };
        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", SERVICE_NAME),
            param("VpcEndpointType", "Gateway"),
        ];
        for (index, route_table_id) in tables.iter().take(named).enumerate() {
            params.push(param(&format!("RouteTableId.{}", index + 1), route_table_id));
        }
        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &body, "vpcEndpointId");
        // "(Gateway endpoint) The IDs of the route tables associated with the endpoint" — the
        // set the create answers with, read out of that member's own list.
        let reported_tables = elements(
            &element(&body, "routeTableIdSet")
                .unwrap_or_else(|| panic!("case {case:?}: no routeTableIdSet in {body}")),
            "item",
        );
        assert_eq!(
            reported_tables,
            tables[..named].to_vec(),
            "case {case:?}: expected the endpoint to report the tables it was created for, got \
             {body}"
        );

        for (index, route_table_id) in tables.iter().enumerate() {
            let read = describe_route_table(case, &router, route_table_id).await;
            let through_the_endpoint = elements(&read, "gatewayId")
                .into_iter()
                .filter(|target| target == &endpoint_id)
                .count();
            let expected = usize::from(index < named);
            assert_eq!(
                through_the_endpoint, expected,
                "case {case:?}: expected {expected} route(s) through {endpoint_id} in table \
                 {route_table_id}, got {read}"
            );
            // The destination is the SERVICE's prefix list, which that service publishes under
            // the endpoint service's own name.
            if expected == 1 {
                let prefix_list_id = element(&read, "destinationPrefixListId")
                    .unwrap_or_else(|| panic!("case {case:?}: no prefix list route in {read}"));
                let published = ok(
                    case,
                    &router,
                    "DescribePrefixLists",
                    &[param("PrefixListId.1", &prefix_list_id)],
                )
                .await;
                assert_eq!(
                    element(&published, "prefixListName").as_deref(),
                    Some(SERVICE_NAME),
                    "case {case:?}: expected the route's prefix list to be {SERVICE_NAME}'s, got \
                     {published}"
                );
            }
            // The local route every table comes with is untouched by the endpoint.
            assert!(
                elements(&read, "gatewayId").contains(&"local".to_string()),
                "case {case:?}: expected the table to keep its local route, got {read}"
            );
        }

        // The endpoint's deletion takes its routes with it, and the endpoint is then reported
        // for nobody.
        let deleted = ok(
            case,
            &router,
            "DeleteVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
        )
        .await;
        assert!(
            elements(&deleted, "code").is_empty(),
            "case {case:?}: expected the delete to report nothing unsuccessful, got {deleted}"
        );
        for route_table_id in &tables {
            let read = describe_route_table(case, &router, route_table_id).await;
            assert!(
                !elements(&read, "gatewayId").contains(&endpoint_id),
                "case {case:?}: expected the endpoint's route gone from {route_table_id}, got \
                 {read}"
            );
        }
        let (status, body) = call(
            &router,
            "DescribeVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted endpoint reported for nobody, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcEndpointId.NotFound",
            "case {case:?}: expected InvalidVpcEndpointId.NotFound, got {body}"
        );
        // A second delete of the same endpoint reports it as one it could not delete rather
        // than refusing the call: "information about the VPC endpoints that were not
        // successfully deleted".
        let again = ok(
            case,
            &router,
            "DeleteVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
        )
        .await;
        assert_eq!(
            elements(&again, "code"),
            vec!["InvalidVpcEndpointId.NotFound".to_string()],
            "case {case:?}: expected the second delete to report the endpoint unsuccessful, got \
             {again}"
        );
    }
}

/// The rule's own-VPC half: a gateway endpoint is only reached through route tables of ITS
/// VPC, so a table of another VPC — or an id naming no table at all — is refused, and the
/// endpoint is not created.
#[tokio::test]
async fn a_gateway_endpoint_refuses_a_route_table_outside_its_own_vpc() {
    let router = emulator();
    let case = "the two VPCs";
    let vpc_id = create_vpc(case, &router, "10.90.0.0/16").await;
    let other_vpc_id = create_vpc(case, &router, "10.91.0.0/16").await;
    let foreign_table_id = create_route_table(case, &router, &other_vpc_id).await;

    // (case, the route table the endpoint names, the expected error code)
    let cases = [
        (
            "a route table of another VPC",
            foreign_table_id.as_str(),
            "InvalidParameterValue",
        ),
        (
            "a route table id naming nothing",
            "rtb-00000000000000000",
            "InvalidRouteTableID.NotFound",
        ),
    ];

    for (case, route_table_id, expected_code) in cases {
        let (status, body) = call(
            &router,
            "CreateVpcEndpoint",
            &[
                param("VpcId", &vpc_id),
                param("ServiceName", "com.amazonaws.us-east-1.s3"),
                param("VpcEndpointType", "Gateway"),
                param("RouteTableId.1", route_table_id),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Nothing above created an endpoint: the VPC holds none.
    let read = ok(
        case,
        &router,
        "DescribeVpcEndpoints",
        &filters(&[("vpc-id", &[vpc_id.as_str()])]),
    )
    .await;
    assert!(
        element(&read, "vpcEndpointId").is_none(),
        "expected no endpoint after every refusal, got {read}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateSubnetCidrBlock`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that AN IPv6 CIDR BLOCK IS ASSOCIATED WITH A STORED SUBNET ONLY
//! AS A BLOCK OF ONE OF THE IPv6 BLOCKS ASSOCIATED WITH THAT SUBNET'S VPC, AT THE ONE
//! PUBLISHED SUBNET BLOCK SIZE, AT MOST ONE PER SUBNET AND OVERLAPPING NO OTHER SUBNET OF
//! THE VPC — answered in the non-terminal `associating` state the clock owes `associated`,
//! and reading back through `DescribeSubnets`; everything else is refused with the code
//! that says why and nothing is stored.
//!
//! Every table row is a distinct input along what that generalises over — where inside the
//! VPC's block the requested block sits, how it was written (case, compression, host bits
//! set), which family and size it is, where the VPC's own block came from, and which
//! subnet or VPC holds what already — so any one recorded call is one row among many
//! rather than the specification.

use std::net::Ipv6Addr;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, indexed};
use super::wire::{has_ipv6_association, ipv6_association, param, Param};

/// The Region every call here is signed for: the Region the default VPC is seeded for.
const REGION: &str = "us-east-1";

/// How long the `associating` -> `associated` move takes: the span the clock owes it
/// ([`ec2::state::TRANSITION_DELAY`]), plus a margin so a slow machine cannot read the
/// state before it is due.
const SETTLE_WAIT: Duration = Duration::from_millis(900);

/// An IPv6 address pool id, for the VPC whose IPv6 block is brought from the account's own
/// address space ("The ID of an IPv6 address pool from which to allocate the IPv6 CIDR
/// block" — `AssociateVpcCidrBlockRequest$Ipv6Pool`).
const IPV6_POOL: &str = "ipv6pool-ec2-0123456789abcdef0";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
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

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A VPC with `cidr_block` and an Amazon-provided IPv6 CIDR block, as its id and that
/// block.
async fn create_vpc_with_amazon_ipv6(
    case: &str,
    router: &MultiServiceRouter,
    cidr_block: &str,
) -> (String, String) {
    let body = ok(
        case,
        router,
        "CreateVpc",
        &[
            param("CidrBlock", cidr_block),
            param("AmazonProvidedIpv6CidrBlock", "true"),
        ],
    )
    .await;
    (
        member(case, &body, "vpcId"),
        member(case, &body, "ipv6CidrBlock"),
    )
}

/// A VPC with `cidr_block` and `ipv6_cidr_block` brought from [`IPV6_POOL`], as its id.
async fn create_vpc_with_byoip_ipv6(
    case: &str,
    router: &MultiServiceRouter,
    cidr_block: &str,
    ipv6_cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateVpc",
        &[
            param("CidrBlock", cidr_block),
            param("Ipv6CidrBlock", ipv6_cidr_block),
            param("Ipv6Pool", IPV6_POOL),
        ],
    )
    .await;
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

/// One `AssociateSubnetCidrBlock` call naming `subnet_id` and `ipv6_cidr_block`.
async fn associate(
    router: &MultiServiceRouter,
    subnet_id: &str,
    ipv6_cidr_block: &str,
) -> (u16, String) {
    call(
        router,
        "AssociateSubnetCidrBlock",
        &[
            param("SubnetId", subnet_id),
            param("Ipv6CidrBlock", ipv6_cidr_block),
        ],
    )
    .await
}

/// The `DescribeSubnets` body for the one subnet `subnet_id` names.
async fn describe_subnet(case: &str, router: &MultiServiceRouter, subnet_id: &str) -> String {
    ok(
        case,
        router,
        "DescribeSubnets",
        &indexed("SubnetId", &[subnet_id]),
    )
    .await
}

/// The `index`-th subnet-sized (`/64`) block inside `block`.
///
/// A subnet's IPv6 block is a block OF its VPC's, and `/64` blocks step by 2^64 addresses
/// — so the index lands in the lowest bit of the prefix. The arithmetic is the test's own,
/// written from the published subnet block size
/// ([`ec2::state::SUBNET_IPV6_PREFIX_LENGTH`]) rather than from the service's helpers.
fn nth_subnet_ipv6_block(block: &str, index: u64) -> String {
    let (address, _) = block
        .split_once('/')
        .unwrap_or_else(|| panic!("{block:?} is a CIDR block"));
    let base = address
        .parse::<Ipv6Addr>()
        .unwrap_or_else(|_| panic!("{address:?} is an IPv6 address"));
    let child = u128::from(base) + (u128::from(index) << 64);
    format!(
        "{child}/{size}",
        child = Ipv6Addr::from(child),
        size = ec2::state::SUBNET_IPV6_PREFIX_LENGTH,
    )
}

/// The rule: the block the request names is associated with the subnet it names, in
/// canonical form, in the non-terminal `associating` state, reported with where that
/// address space came from — and every member of the association reads back through
/// `DescribeSubnets`.
///
/// The rows differ in where inside the VPC's own block the requested block sits (first,
/// middle, last), in how the request spelled it (canonical, upper case, host bits set,
/// fully expanded), and in where the VPC's block came from (Amazon-provided, or brought
/// from the account's own pool) — which is what decides the association's reported source.
#[tokio::test]
async fn associate_subnet_cidr_block_stores_the_block_and_round_trips_through_describe_subnets() {
    let router = emulator();
    let (amazon_vpc, amazon_block) =
        create_vpc_with_amazon_ipv6("amazon-provided", &router, "10.100.0.0/16").await;
    let byoip_parent = "2001:db8:a::/56";
    let byoip_vpc =
        create_vpc_with_byoip_ipv6("byoip", &router, "10.101.0.0/16", byoip_parent).await;

    // (case, the VPC, its IPv4 block prefix, the request's block, the canonical form,
    //  the expected source)
    let cases: [(&str, &str, &str, String, String, &str); 7] = [
        (
            "the first /64 of an Amazon-provided block",
            &amazon_vpc,
            "10.100",
            nth_subnet_ipv6_block(&amazon_block, 0),
            nth_subnet_ipv6_block(&amazon_block, 0),
            ec2::state::IP_SOURCE_AMAZON,
        ),
        (
            "a /64 inside an Amazon-provided block",
            &amazon_vpc,
            "10.100",
            nth_subnet_ipv6_block(&amazon_block, 7),
            nth_subnet_ipv6_block(&amazon_block, 7),
            ec2::state::IP_SOURCE_AMAZON,
        ),
        (
            "the last /64 of an Amazon-provided /56",
            &amazon_vpc,
            "10.100",
            nth_subnet_ipv6_block(&amazon_block, 255),
            nth_subnet_ipv6_block(&amazon_block, 255),
            ec2::state::IP_SOURCE_AMAZON,
        ),
        (
            "a block from the account's own pool",
            &byoip_vpc,
            "10.101",
            "2001:db8:a:1::/64".to_string(),
            "2001:db8:a:1::/64".to_string(),
            ec2::state::IP_SOURCE_BYOIP,
        ),
        (
            "upper-case hex",
            &byoip_vpc,
            "10.101",
            "2001:DB8:A:2::/64".to_string(),
            "2001:db8:a:2::/64".to_string(),
            ec2::state::IP_SOURCE_BYOIP,
        ),
        (
            "host bits set",
            &byoip_vpc,
            "10.101",
            "2001:db8:a:3::dead:beef/64".to_string(),
            "2001:db8:a:3::/64".to_string(),
            ec2::state::IP_SOURCE_BYOIP,
        ),
        (
            "fully expanded",
            &byoip_vpc,
            "10.101",
            "2001:0db8:000a:0004:0000:0000:0000:0000/64".to_string(),
            "2001:db8:a:4::/64".to_string(),
            ec2::state::IP_SOURCE_BYOIP,
        ),
    ];

    for (index, (case, vpc_id, ipv4_prefix, requested, canonical, source)) in
        cases.into_iter().enumerate()
    {
        // One subnet per row: "you can only associate a single IPv6 CIDR block with your
        // subnet", so each row needs a subnet of its own.
        let subnet_id = create_subnet(
            case,
            &router,
            vpc_id,
            &format!("{ipv4_prefix}.{index}.0/24"),
        )
        .await;
        let body = ok(
            case,
            &router,
            "AssociateSubnetCidrBlock",
            &[
                param("SubnetId", &subnet_id),
                param("Ipv6CidrBlock", &requested),
            ],
        )
        .await;

        let reported_subnet = member(case, &body, "subnetId");
        assert_eq!(
            reported_subnet, subnet_id,
            "case {case:?}: expected the association on subnet {subnet_id:?}, \
             got {reported_subnet:?}"
        );
        let reported_block = member(case, &body, "ipv6CidrBlock");
        assert_eq!(
            reported_block, canonical,
            "case {case:?}: expected the canonical block {canonical:?}, got {reported_block:?}"
        );
        let association_id = member(case, &body, "associationId");
        assert!(
            association_id.starts_with("subnet-cidr-assoc-"),
            "case {case:?}: expected a subnet CIDR association id, got {association_id:?}"
        );
        let state = member(case, &body, "state");
        assert_eq!(
            state,
            ec2::state::CIDR_BLOCK_STATE_ASSOCIATING,
            "case {case:?}: expected an association the clock still owes `associated`, \
             got {state:?}"
        );
        let reported_source = member(case, &body, "ipSource");
        assert_eq!(
            reported_source, source,
            "case {case:?}: expected the source of the VPC block it was cut from \
             ({source:?}), got {reported_source:?}"
        );
        // "Public IPv6 addresses are those advertised on the internet from Amazon Web
        // Services" (`Ipv6AddressAttribute`) — the attribute of the VPC block this one is
        // a block of.
        let attribute = member(case, &body, "ipv6AddressAttribute");
        assert_eq!(
            attribute,
            ec2::state::IPV6_ADDRESS_PUBLIC,
            "case {case:?}: expected the attribute of the VPC block it was cut from, \
             got {attribute:?}"
        );

        // The round trip: the association the describe reports is the one just answered.
        // Read through the suite-wide association reader, so an element can only come from
        // the subnet's association and never from the subnet's own members.
        let described = describe_subnet(case, &router, &subnet_id).await;
        for (name, expected) in [
            ("associationId", association_id.as_str()),
            ("ipv6CidrBlock", canonical.as_str()),
            ("ipSource", source),
            ("ipv6AddressAttribute", ec2::state::IPV6_ADDRESS_PUBLIC),
        ] {
            let read_back = ipv6_association(case, &described, name);
            assert_eq!(
                read_back, expected,
                "case {case:?}: DescribeSubnets reported {name} {read_back:?}, \
                 AssociateSubnetCidrBlock answered {expected:?}"
            );
        }
    }
}

/// The rule: an `AssociateSubnetCidrBlock` the service cannot satisfy is refused with the
/// code that says why, and nothing is stored.
///
/// The rows are the ways a request can fail to name a usable block on a usable subnet: a
/// subnet the account does not hold, a block of the wrong size (each side of the one
/// published size, and well past it on each side — a size violation being the
/// `InvalidSubnet.Range` this crate words at one site, the same answer `CreateSubnet`
/// gives an IPv4 block outside its published size), a block that is not an IPv6 block at
/// all, no block, a block outside the VPC's own, a block overlapping another subnet's (in
/// each of the forms an overlap takes), and a subnet that already has one.
#[tokio::test]
async fn associate_subnet_cidr_block_refuses_a_block_it_cannot_associate() {
    let router = emulator();
    let vpc_id = create_vpc_with_byoip_ipv6(
        "refusals",
        &router,
        "10.110.0.0/16",
        "2001:db8:b::/56",
    )
    .await;
    // The subnet the refusals are aimed at, and a neighbour holding a block already.
    let subject = create_subnet("refusals", &router, &vpc_id, "10.110.1.0/24").await;
    let neighbour = create_subnet("refusals", &router, &vpc_id, "10.110.2.0/24").await;
    let held = create_subnet("refusals", &router, &vpc_id, "10.110.3.0/24").await;
    let (status, body) = associate(&router, &neighbour, "2001:db8:b:10::/64").await;
    assert_eq!(
        status, 200,
        "the suite's own neighbour fixture must be associated, got {status} with {body}"
    );
    let (status, body) = associate(&router, &held, "2001:db8:b:20::/64").await;
    assert_eq!(
        status, 200,
        "the suite's own held fixture must be associated, got {status} with {body}"
    );

    // (case, the request's fields, the EC2 error code, the HTTP status)
    let cases: [(&str, Vec<Param>, &str, u16); 12] = [
        (
            // The recorded instance this issue was filed from: a subnet id the account
            // holds no subnet for.
            "a subnet that does not exist",
            vec![
                param("SubnetId", "subnet-0123456789abcdef0"),
                param("Ipv6CidrBlock", "2001:db8:b:30::/64"),
            ],
            "InvalidSubnetID.NotFound",
            400,
        ),
        (
            "a subnet id that is not one at all",
            vec![
                param("SubnetId", "not-a-subnet"),
                param("Ipv6CidrBlock", "2001:db8:b:30::/64"),
            ],
            "InvalidSubnetID.NotFound",
            400,
        ),
        (
            "one bit larger than the published size",
            vec![
                param("SubnetId", &subject),
                param("Ipv6CidrBlock", "2001:db8:b:30::/63"),
            ],
            "InvalidSubnet.Range",
            400,
        ),
        (
            "one bit smaller than the published size",
            vec![
                param("SubnetId", &subject),
                param("Ipv6CidrBlock", "2001:db8:b:30::/65"),
            ],
            "InvalidSubnet.Range",
            400,
        ),
        (
            "the VPC's own block size",
            vec![
                param("SubnetId", &subject),
                param("Ipv6CidrBlock", "2001:db8:b::/56"),
            ],
            "InvalidSubnet.Range",
            400,
        ),
        (
            "a single address",
            vec![
                param("SubnetId", &subject),
                param("Ipv6CidrBlock", "2001:db8:b:30::1/128"),
            ],
            "InvalidSubnet.Range",
            400,
        ),
        (
            "an IPv4 block",
            vec![
                param("SubnetId", &subject),
                param("Ipv6CidrBlock", "10.110.4.0/24"),
            ],
            "InvalidParameterValue",
            400,
        ),
        (
            "not a CIDR block at all",
            vec![
                param("SubnetId", &subject),
                param("Ipv6CidrBlock", "2001:db8:b:30::"),
            ],
            "InvalidParameterValue",
            400,
        ),
        (
            "no block at all",
            vec![param("SubnetId", &subject)],
            "MissingParameter",
            400,
        ),
        (
            "a block outside the VPC's own",
            vec![
                param("SubnetId", &subject),
                param("Ipv6CidrBlock", "2001:db8:ffff:1::/64"),
            ],
            "InvalidSubnet.Range",
            400,
        ),
        (
            "a block just past the VPC's own",
            vec![
                param("SubnetId", &subject),
                param("Ipv6CidrBlock", "2001:db8:b:100::/64"),
            ],
            "InvalidSubnet.Range",
            400,
        ),
        (
            "the block another subnet of the VPC holds",
            vec![
                param("SubnetId", &subject),
                param("Ipv6CidrBlock", "2001:db8:b:10::/64"),
            ],
            "InvalidSubnet.Conflict",
            400,
        ),
    ];

    for (case, params, expected_code, expected_status) in cases {
        let (status, body) = call(&router, "AssociateSubnetCidrBlock", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} in {body}"
        );
    }

    // "You can only associate a single IPv6 CIDR block with your subnet"
    // (`AssociateSubnetCidrBlock`): the subnet holding one is at its limit, whichever
    // block the second request names.
    for second in ["2001:db8:b:21::/64", "2001:db8:b:22::/64"] {
        let (status, body) = associate(&router, &held, second).await;
        assert_eq!(
            status, 400,
            "case {second:?}: a second IPv6 block must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "CidrLimitExceeded",
            "case {second:?}: a second IPv6 block must be refused as a limit: {body}"
        );
    }

    // Allocating out of an IPAM pool is honestly unwritten (501), not the caller's error —
    // while a netmask length with no pool to cut it from is the caller's error.
    for pool_params in [
        vec![
            param("SubnetId", &subject),
            param("Ipv6IpamPoolId", "ipam-pool-0123456789abcdef0"),
        ],
        vec![
            param("SubnetId", &subject),
            param("Ipv6IpamPoolId", "ipam-pool-0123456789abcdef0"),
            param("Ipv6NetmaskLength", "64"),
        ],
    ] {
        let (status, body) = call(&router, "AssociateSubnetCidrBlock", &pool_params).await;
        assert_eq!(
            status, 501,
            "an IPAM-allocated block is not written yet and must answer 501, \
             got {status} with body {body}"
        );
    }
    let (status, body) = call(
        &router,
        "AssociateSubnetCidrBlock",
        &[
            param("SubnetId", &subject),
            param("Ipv6NetmaskLength", "64"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a netmask length with no pool must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterCombination",
        "a netmask length with no pool names an unusable combination: {body}"
    );

    // Nothing above was stored: the subject subnet still has no IPv6 block, and the
    // neighbour still has exactly the one it was given.
    let described = describe_subnet("refusals", &router, &subject).await;
    assert!(
        !has_ipv6_association(&described),
        "a refused AssociateSubnetCidrBlock must store nothing, got {described}"
    );
    let described = describe_subnet("refusals", &router, &neighbour).await;
    let block = ipv6_association("refusals", &described, "ipv6CidrBlock");
    assert_eq!(
        block, "2001:db8:b:10::/64",
        "the neighbour must keep exactly the block it was given, got {block:?}"
    );
}

/// The rule: the association is answered in the non-terminal `associating` state and the
/// clock owes it `associated`, so a caller reading straight after the call still sees
/// `associating`.
#[tokio::test]
async fn an_association_settles_to_associated_when_the_clock_owes_it() {
    let case = "the clock-owed move";
    let router = emulator();
    let vpc_id =
        create_vpc_with_byoip_ipv6(case, &router, "10.120.0.0/16", "2001:db8:c::/56").await;
    let subnet_id = create_subnet(case, &router, &vpc_id, "10.120.1.0/24").await;
    let body = ok(
        case,
        &router,
        "AssociateSubnetCidrBlock",
        &[
            param("SubnetId", &subnet_id),
            param("Ipv6CidrBlock", "2001:db8:c:1::/64"),
        ],
    )
    .await;
    let state = member(case, &body, "state");
    assert_eq!(
        state,
        ec2::state::CIDR_BLOCK_STATE_ASSOCIATING,
        "case {case:?}: expected the association still on its way, got {state:?}"
    );
    // A read straight after the call still sees the transient state.
    let described = describe_subnet(case, &router, &subnet_id).await;
    let state = ipv6_association(case, &described, "state");
    assert_eq!(
        state,
        ec2::state::CIDR_BLOCK_STATE_ASSOCIATING,
        "case {case:?}: expected the transient state to be visible, got {state:?}"
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let described = describe_subnet(case, &router, &subnet_id).await;
    let state = ipv6_association(case, &described, "state");
    assert_eq!(
        state,
        ec2::state::CIDR_BLOCK_STATE_ASSOCIATED,
        "case {case:?}: expected the clock-owed `associated`, got {state:?}"
    );
}

/// The rule's scope: the overlap a block is refused for is an overlap WITHIN the VPC the
/// subnet is in, so the same block under another VPC is not one.
#[tokio::test]
async fn the_same_block_under_another_vpc_is_not_a_conflict() {
    let case = "two VPCs, one block";
    let router = emulator();
    let block = "2001:db8:d:1::/64";
    let first_vpc =
        create_vpc_with_byoip_ipv6(case, &router, "10.130.0.0/16", "2001:db8:d::/56").await;
    let second_vpc =
        create_vpc_with_byoip_ipv6(case, &router, "10.131.0.0/16", "2001:db8:d::/56").await;
    let first = create_subnet(case, &router, &first_vpc, "10.130.1.0/24").await;
    let second = create_subnet(case, &router, &second_vpc, "10.131.1.0/24").await;

    for (subnet_id, vpc_id) in [(&first, &first_vpc), (&second, &second_vpc)] {
        let (status, body) = associate(&router, subnet_id, block).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {block} must be associable in VPC {vpc_id}, \
             got {status} with body {body}"
        );
        let reported = member(case, &body, "ipv6CidrBlock");
        assert_eq!(
            reported, block,
            "case {case:?}: expected {block:?} in VPC {vpc_id}, got {reported:?}"
        );
    }

    // And the overlap WITHIN a VPC is still refused: a second subnet of the first VPC
    // cannot take a block inside the one the first subnet holds.
    let sibling = create_subnet(case, &router, &first_vpc, "10.130.2.0/24").await;
    let (status, body) = associate(&router, &sibling, block).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a block another subnet of the VPC holds must be refused, \
         got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidSubnet.Conflict",
        "case {case:?}: expected the in-VPC overlap to be a conflict: {body}"
    );
}

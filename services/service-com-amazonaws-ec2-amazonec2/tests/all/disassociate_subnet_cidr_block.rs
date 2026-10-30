// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DisassociateSubnetCidrBlock`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that AN IPv6 CIDR BLOCK ASSOCIATION IS LET GO OF BY ITS OWN
//! ASSOCIATION ID, WHEREVER IN THE ACCOUNT'S SUBNETS IT IS HELD: the call answers that
//! association, and the subnet holding it, in the non-terminal `disassociating` state — so
//! the subnet stops holding the block at once and the record leaves the subnet at the
//! instant the clock owes — while nothing still holding an address out of the block may be
//! let go of, and an id the account holds no association for is refused with the code that
//! says why and nothing is written.
//!
//! Every table row is a distinct input along what that generalises over — which subnet of
//! which VPC holds the association, where inside the VPC's own range the block sits, how
//! the request that created it spelled it, where that address space came from, and what
//! kind of id a refused request named — so any one recorded call is one row among many
//! rather than the specification.

use std::net::Ipv6Addr;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{child, child_text, document, ipv6_association, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// How long the end of a disassociation takes: the span the clock owes it
/// ([`ec2::state::TRANSITION_DELAY`]), plus a margin so a slow machine cannot read the
/// subnet before the removal is due.
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

/// One element of a response body, or a panic naming the case.
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

/// The association id of the block `ipv6_cidr_block` associated with `subnet_id`, which has
/// to succeed.
async fn associated_id(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: &str,
    ipv6_cidr_block: &str,
) -> String {
    let (status, body) = associate(router, subnet_id, ipv6_cidr_block).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture block {ipv6_cidr_block} must be associated with \
         {subnet_id}, got {status} with body {body}"
    );
    member(case, &body, "associationId")
}

/// One `DisassociateSubnetCidrBlock` call naming `association_id`.
async fn disassociate(router: &MultiServiceRouter, association_id: &str) -> (u16, String) {
    call(
        router,
        "DisassociateSubnetCidrBlock",
        &[param("AssociationId", association_id)],
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

/// Every IPv6 CIDR block association `DescribeSubnets` reports for the one subnet in
/// `body`, as its id, its block and its own state.
///
/// Read through the XML tree, so a member of an association is never confused with one of
/// the subnet's own: the subnet carries a `<state>` too.
fn associations(case: &str, body: &str) -> Vec<(String, String, String)> {
    let parsed = document(case, body);
    let subnet = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "subnetId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no subnet (no <subnetId>) in {body}"));
    set_items(subnet, "ipv6CidrBlockAssociationSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "associationId").unwrap_or_default(),
                child_text(item, "ipv6CidrBlock").unwrap_or_default(),
                child(item, "ipv6CidrBlockState")
                    .and_then(|state| child_text(state, "state"))
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// The state `DescribeSubnets` reports for the association `association_id`.
fn association_state(case: &str, body: &str, association_id: &str) -> String {
    associations(case, body)
        .into_iter()
        .find(|(id, _, _)| id == association_id)
        .map(|(_, _, state)| state)
        .unwrap_or_else(|| panic!("case {case:?}: no association {association_id:?} in {body}"))
}

/// Whether an association in this state still HOLDS the block it names: one on its way in
/// or already in does, one on its way out or already out does not
/// (`SubnetCidrBlockStateCode`, as the suite's own reading of it rather than the service's).
fn holds_its_block(state: &str) -> bool {
    state == ec2::state::CIDR_BLOCK_STATE_ASSOCIATING
        || state == ec2::state::CIDR_BLOCK_STATE_ASSOCIATED
}

/// The blocks the one subnet in `body` still holds, in the order it reported them.
fn held_blocks(case: &str, body: &str) -> Vec<String> {
    associations(case, body)
        .into_iter()
        .filter(|(_, _, state)| holds_its_block(state))
        .map(|(_, block, _)| block)
        .collect()
}

/// The `index`-th subnet-sized (`/64`) block inside `block`.
///
/// A subnet's IPv6 block is a block OF its VPC's, and `/64` blocks step by 2^64 addresses —
/// so the index lands in the lowest bit of the prefix. The arithmetic is the test's own,
/// written from the published subnet block size ([`ec2::state::SUBNET_IPV6_PREFIX_LENGTH`])
/// rather than from the service's helpers.
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

/// The rule: the association the request names is let go of, whichever subnet of whichever
/// VPC holds it — answered with that subnet and with the association as stored, in the
/// non-terminal `disassociating` state, and reported that way by `DescribeSubnets`.
///
/// The rows differ in where inside the VPC's own block the block sits (first, middle,
/// last), in how the request that created it spelled it (canonical, upper case, host bits
/// set), and in where the VPC's block came from (Amazon-provided, or brought from the
/// account's own pool) — which is what decides the source the association is reported with.
#[tokio::test]
async fn disassociate_subnet_cidr_block_lets_go_of_the_association_it_names() {
    let router = emulator();
    let (amazon_vpc, amazon_block) =
        create_vpc_with_amazon_ipv6("amazon-provided", &router, "10.150.0.0/16").await;
    let byoip_vpc =
        create_vpc_with_byoip_ipv6("byoip", &router, "10.151.0.0/16", "2001:db8:f::/56").await;

    // (case, the VPC, its IPv4 block prefix, the request's block, its canonical form,
    //  the expected source)
    let cases: [(&str, &str, &str, String, String, &str); 6] = [
        (
            "the first /64 of an Amazon-provided block",
            &amazon_vpc,
            "10.150",
            nth_subnet_ipv6_block(&amazon_block, 0),
            nth_subnet_ipv6_block(&amazon_block, 0),
            ec2::state::IP_SOURCE_AMAZON,
        ),
        (
            "a /64 inside an Amazon-provided block",
            &amazon_vpc,
            "10.150",
            nth_subnet_ipv6_block(&amazon_block, 9),
            nth_subnet_ipv6_block(&amazon_block, 9),
            ec2::state::IP_SOURCE_AMAZON,
        ),
        (
            "the last /64 of an Amazon-provided /56",
            &amazon_vpc,
            "10.150",
            nth_subnet_ipv6_block(&amazon_block, 255),
            nth_subnet_ipv6_block(&amazon_block, 255),
            ec2::state::IP_SOURCE_AMAZON,
        ),
        (
            "a block from the account's own pool",
            &byoip_vpc,
            "10.151",
            "2001:db8:f:1::/64".to_string(),
            "2001:db8:f:1::/64".to_string(),
            ec2::state::IP_SOURCE_BYOIP,
        ),
        (
            "upper-case hex",
            &byoip_vpc,
            "10.151",
            "2001:DB8:F:2::/64".to_string(),
            "2001:db8:f:2::/64".to_string(),
            ec2::state::IP_SOURCE_BYOIP,
        ),
        (
            "host bits set",
            &byoip_vpc,
            "10.151",
            "2001:db8:f:3::dead:beef/64".to_string(),
            "2001:db8:f:3::/64".to_string(),
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
        let association_id = associated_id(case, &router, &subnet_id, &requested).await;

        let (status, body) = disassociate(&router, &association_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DisassociateSubnetCidrBlock, \
             got {status} with body {body}"
        );
        let reported_subnet = member(case, &body, "subnetId");
        assert_eq!(
            reported_subnet, subnet_id,
            "case {case:?}: expected the subnet that held the association ({subnet_id:?}), \
             got {reported_subnet:?}"
        );
        let reported_id = member(case, &body, "associationId");
        assert_eq!(
            reported_id, association_id,
            "case {case:?}: expected the association the request named ({association_id:?}), \
             got {reported_id:?}"
        );
        let reported_block = member(case, &body, "ipv6CidrBlock");
        assert_eq!(
            reported_block, canonical,
            "case {case:?}: expected the block as stored ({canonical:?}), got {reported_block:?}"
        );
        let state = member(case, &body, "state");
        assert_eq!(
            state,
            ec2::state::CIDR_BLOCK_STATE_DISASSOCIATING,
            "case {case:?}: expected a block the clock still owes the end of, got {state:?}"
        );
        let reported_source = member(case, &body, "ipSource");
        assert_eq!(
            reported_source, source,
            "case {case:?}: expected the source the association was stored with ({source:?}), \
             got {reported_source:?}"
        );
        let attribute = member(case, &body, "ipv6AddressAttribute");
        assert_eq!(
            attribute,
            ec2::state::IPV6_ADDRESS_PUBLIC,
            "case {case:?}: expected the attribute the association was stored with, \
             got {attribute:?}"
        );

        // What a LATER CALLER sees: the same association, in the same transient state.
        let described = describe_subnet(case, &router, &subnet_id).await;
        for (name, expected) in [
            ("associationId", association_id.as_str()),
            ("ipv6CidrBlock", canonical.as_str()),
            ("state", ec2::state::CIDR_BLOCK_STATE_DISASSOCIATING),
        ] {
            let read_back = ipv6_association(case, &described, name);
            assert_eq!(
                read_back, expected,
                "case {case:?}: DescribeSubnets reported {name} {read_back:?}, \
                 DisassociateSubnetCidrBlock answered {expected:?}"
            );
        }
    }
}

/// The rule: a block on its way out is no longer HELD, so the subnet may be given another
/// one at once; and the association reaches the terminal `disassociated` at the instant the
/// clock owes it, after which the block it named can be associated again as an association
/// of its own.
#[tokio::test]
async fn a_disassociating_block_is_no_longer_held_and_reaches_disassociated_when_it_falls_due() {
    let case = "the clock-owed end of a disassociation";
    let router = emulator();
    let vpc_id =
        create_vpc_with_byoip_ipv6(case, &router, "10.160.0.0/16", "2001:db8:10::/56").await;
    let subnet_id = create_subnet(case, &router, &vpc_id, "10.160.1.0/24").await;
    let first_block = "2001:db8:10:1::/64";
    let association_id = associated_id(case, &router, &subnet_id, first_block).await;

    let (status, body) = disassociate(&router, &association_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DisassociateSubnetCidrBlock, \
         got {status} with body {body}"
    );
    // A read straight after the call still sees the transient state, and the subnet already
    // holds nothing.
    let described = describe_subnet(case, &router, &subnet_id).await;
    let state = association_state(case, &described, &association_id);
    assert_eq!(
        state,
        ec2::state::CIDR_BLOCK_STATE_DISASSOCIATING,
        "case {case:?}: expected the transient state to be visible, got {state:?}"
    );
    let held = held_blocks(case, &described);
    assert!(
        held.is_empty(),
        "case {case:?}: expected the subnet to hold no block, got {held:?}"
    );

    // The subnet is at its limit no longer: "you can only associate a single IPv6 CIDR
    // block with your subnet" counts the block it HOLDS, and this one is on its way out.
    let second_block = "2001:db8:10:2::/64";
    let (status, body) = associate(&router, &subnet_id, second_block).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a subnet whose block is on its way out must take another, \
         got {status} with body {body}"
    );
    let second_id = member(case, &body, "associationId");
    assert_ne!(
        second_id, association_id,
        "case {case:?}: expected an association of its own for {second_block}, \
         got the first one's id {second_id:?}"
    );

    // And a second disassociation of the SAME association is the call it already is: the
    // same record, in the same state, rather than a second refusal.
    let (status, body) = disassociate(&router, &association_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: repeating the disassociation must answer the association as it \
         stands, got {status} with body {body}"
    );
    let state = member(case, &body, "state");
    assert_eq!(
        state,
        ec2::state::CIDR_BLOCK_STATE_DISASSOCIATING,
        "case {case:?}: expected the repeat to report the block still on its way out, \
         got {state:?}"
    );

    tokio::time::sleep(SETTLE_WAIT).await;

    // The clock-owed terminal state: the association the request named is `disassociated`,
    // and the only block the subnet holds is the second one.
    let described = describe_subnet(case, &router, &subnet_id).await;
    let state = association_state(case, &described, &association_id);
    assert_eq!(
        state,
        ec2::state::CIDR_BLOCK_STATE_DISASSOCIATED,
        "case {case:?}: expected the clock-owed `disassociated`, got {state:?}"
    );
    let held = held_blocks(case, &described);
    assert_eq!(
        held,
        vec![second_block.to_string()],
        "case {case:?}: expected the subnet to hold only {second_block:?}, got {held:?}"
    );
    // A call naming an association that has already finished answers it as it stands: its
    // state is terminal, so there is nothing left to start.
    let (status, body) = disassociate(&router, &association_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the finished association to be answered as it stands, \
         got {status} with body {body}"
    );
    let state = member(case, &body, "state");
    assert_eq!(
        state,
        ec2::state::CIDR_BLOCK_STATE_DISASSOCIATED,
        "case {case:?}: expected the terminal state to stand, got {state:?}"
    );

    // And the block the subnet let go of can be taken again, as a new association — once
    // the subnet is holding nothing again.
    let (status, body) = disassociate(&router, &second_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 letting go of {second_block}, \
         got {status} with body {body}"
    );
    let again = associated_id(case, &router, &subnet_id, first_block).await;
    assert_ne!(
        again, association_id,
        "case {case:?}: expected a fresh association id for {first_block}, got {again:?}"
    );
    let described = describe_subnet(case, &router, &subnet_id).await;
    let held = held_blocks(case, &described);
    assert_eq!(
        held,
        vec![first_block.to_string()],
        "case {case:?}: expected the subnet to hold {first_block:?} again, got {held:?}"
    );
}

/// The rule: an `AssociationId` the account holds no subnet IPv6 CIDR block association for
/// is refused with that resource's own `NotFound` code, naming the id the request named —
/// and nothing of the account's state moves.
///
/// The rows are the ways an id can name no association of this kind: one of the right form
/// that was never issued, the id of another resource of this service (a subnet, a VPC's own
/// CIDR association, a network interface), and a value that is no EC2 id at all.
#[tokio::test]
async fn an_association_id_naming_nothing_is_refused() {
    let case = "refusals";
    let router = emulator();
    let vpc_id =
        create_vpc_with_byoip_ipv6(case, &router, "10.170.0.0/16", "2001:db8:11::/56").await;
    let subnet_id = create_subnet(case, &router, &vpc_id, "10.170.1.0/24").await;
    let held_block = "2001:db8:11:1::/64";
    let association_id = associated_id(case, &router, &subnet_id, held_block).await;

    let unknown: [(&str, String); 5] = [
        (
            "an association id of the right form that was never issued",
            "subnet-cidr-assoc-0123456789abcdef0".to_string(),
        ),
        (
            "the id of the subnet that holds one",
            subnet_id.clone(),
        ),
        (
            "a VPC's own CIDR block association id",
            "vpc-cidr-assoc-0123456789abcdef0".to_string(),
        ),
        (
            "a network interface id",
            "eni-0123456789abcdef0".to_string(),
        ),
        ("no EC2 id at all", "not-an-association".to_string()),
    ];

    for (row, association) in unknown {
        let (status, body) = disassociate(&router, &association).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidSubnetCidrBlockAssociationID.NotFound",
            "case {row:?}: expected the association's own NotFound code, got {code:?} in {body}"
        );
        let message = error_message(&body);
        let expected =
            format!("The subnet CIDR block with association ID '{association}' does not exist");
        assert_eq!(
            message, expected,
            "case {row:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // Nothing above moved: the subnet still holds exactly the association it was given.
    let described = describe_subnet(case, &router, &subnet_id).await;
    let read_back = ipv6_association(case, &described, "associationId");
    assert_eq!(
        read_back, association_id,
        "case {case:?}: a refused disassociation must change nothing, got {read_back:?}"
    );
    let state = ipv6_association(case, &described, "state");
    assert!(
        state == ec2::state::CIDR_BLOCK_STATE_ASSOCIATING
            || state == ec2::state::CIDR_BLOCK_STATE_ASSOCIATED,
        "case {case:?}: a refused disassociation must leave the block held, got {state:?}"
    );
}

/// The rule's precondition: "you must detach or delete all gateways and resources that are
/// associated with the CIDR block before you can disassociate it"
/// (`DisassociateSubnetCidrBlock`) — while a network interface OF THAT SUBNET holds an
/// address or a delegated prefix out of the block, the association cannot be let go of;
/// once nothing does, it can.
///
/// The rows are the ways an interface holds part of the block: a named address, an address
/// the service chose, and a delegated prefix.
#[tokio::test]
async fn a_block_something_still_holds_an_address_out_of_cannot_be_disassociated() {
    let router = emulator();
    let vpc_id = create_vpc_with_byoip_ipv6(
        "dependencies",
        &router,
        "10.180.0.0/16",
        "2001:db8:12::/56",
    )
    .await;

    // (case, the subnet's block, the request fields that take part of it)
    let cases: [(&str, &str, Vec<Param>); 3] = [
        (
            "one named address out of the block",
            "2001:db8:12:1::/64",
            vec![param("Ipv6Addresses.1", "2001:db8:12:1::5")],
        ),
        (
            "an address the service chooses out of the block",
            "2001:db8:12:2::/64",
            vec![param("Ipv6AddressCount", "1")],
        ),
        (
            "a delegated prefix out of the block",
            "2001:db8:12:3::/64",
            vec![param("Ipv6PrefixCount", "1")],
        ),
    ];

    for (index, (case, subnet_block, assignment)) in cases.into_iter().enumerate() {
        let subnet_id = create_subnet(
            case,
            &router,
            &vpc_id,
            &format!("10.180.{index}.0/24"),
        )
        .await;
        let association_id = associated_id(case, &router, &subnet_id, subnet_block).await;
        let created = ok(
            case,
            &router,
            "CreateNetworkInterface",
            &[param("SubnetId", &subnet_id)],
        )
        .await;
        let interface_id = member(case, &created, "networkInterfaceId");
        let mut params = vec![param("NetworkInterfaceId", &interface_id)];
        params.extend(assignment);
        ok(case, &router, "AssignIpv6Addresses", &params).await;

        let (status, body) = disassociate(&router, &association_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a block still in use must not be let go of, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "DependencyViolation",
            "case {case:?}: expected the dependency refusal, got {code:?} in {body}"
        );
        let message = error_message(&body);
        let expected = format!(
            "The subnet CIDR block association '{association_id}' has dependencies and \
             cannot be disassociated."
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        // Nothing moved: the block is still held.
        let described = describe_subnet(case, &router, &subnet_id).await;
        let state = ipv6_association(case, &described, "state");
        assert!(
            state == ec2::state::CIDR_BLOCK_STATE_ASSOCIATING
                || state == ec2::state::CIDR_BLOCK_STATE_ASSOCIATED,
            "case {case:?}: a refused disassociation must leave the block held, got {state:?}"
        );

        // Once the interface holding part of the block is gone, the same call succeeds.
        ok(
            case,
            &router,
            "DeleteNetworkInterface",
            &[param("NetworkInterfaceId", &interface_id)],
        )
        .await;
        let (status, body) = disassociate(&router, &association_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: with nothing holding part of the block the association must be \
             let go of, got {status} with body {body}"
        );
        let state = member(case, &body, "state");
        assert_eq!(
            state,
            ec2::state::CIDR_BLOCK_STATE_DISASSOCIATING,
            "case {case:?}: expected the block on its way out, got {state:?}"
        );
    }

    // The scope of that precondition: an address is an address OF A SUBNET, so an interface
    // holding one out of the SAME block in another VPC's subnet is no dependency of this
    // subnet's association.
    let case = "the same block under another VPC, with an address assigned";
    let other_vpc = create_vpc_with_byoip_ipv6(case, &router, "10.181.0.0/16", "2001:db8:12::/56")
        .await;
    let shared_block = "2001:db8:12:9::/64";
    let subject = create_subnet(case, &router, &vpc_id, "10.180.9.0/24").await;
    let elsewhere = create_subnet(case, &router, &other_vpc, "10.181.9.0/24").await;
    let subject_association = associated_id(case, &router, &subject, shared_block).await;
    associated_id(case, &router, &elsewhere, shared_block).await;
    let created = ok(
        case,
        &router,
        "CreateNetworkInterface",
        &[param("SubnetId", &elsewhere)],
    )
    .await;
    let interface_id = member(case, &created, "networkInterfaceId");
    ok(
        case,
        &router,
        "AssignIpv6Addresses",
        &[
            param("NetworkInterfaceId", &interface_id),
            param("Ipv6AddressCount", "1"),
        ],
    )
    .await;
    let (status, body) = disassociate(&router, &subject_association).await;
    assert_eq!(
        status, 200,
        "case {case:?}: an address held in another subnet must not block this \
         disassociation, got {status} with body {body}"
    );
    let state = member(case, &body, "state");
    assert_eq!(
        state,
        ec2::state::CIDR_BLOCK_STATE_DISASSOCIATING,
        "case {case:?}: expected the block on its way out, got {state:?}"
    );
}

/// The rule's scope: the disassociation is of the ONE association the request names, so
/// another subnet's association — even one holding the same block under another VPC — is
/// left exactly as it was.
#[tokio::test]
async fn only_the_association_the_request_names_is_let_go_of() {
    let case = "one association among several";
    let router = emulator();
    let block = "2001:db8:13:1::/64";
    let first_vpc =
        create_vpc_with_byoip_ipv6(case, &router, "10.190.0.0/16", "2001:db8:13::/56").await;
    let second_vpc =
        create_vpc_with_byoip_ipv6(case, &router, "10.191.0.0/16", "2001:db8:13::/56").await;
    let subject = create_subnet(case, &router, &first_vpc, "10.190.1.0/24").await;
    let bystander = create_subnet(case, &router, &second_vpc, "10.191.1.0/24").await;
    let neighbour = create_subnet(case, &router, &first_vpc, "10.190.2.0/24").await;
    let subject_association = associated_id(case, &router, &subject, block).await;
    let bystander_association = associated_id(case, &router, &bystander, block).await;
    let neighbour_association =
        associated_id(case, &router, &neighbour, "2001:db8:13:2::/64").await;

    let (status, body) = disassociate(&router, &subject_association).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DisassociateSubnetCidrBlock, \
         got {status} with body {body}"
    );
    let reported_subnet = member(case, &body, "subnetId");
    assert_eq!(
        reported_subnet, subject,
        "case {case:?}: expected the subnet that held it ({subject:?}), got {reported_subnet:?}"
    );

    for (holder, association) in [
        (&bystander, &bystander_association),
        (&neighbour, &neighbour_association),
    ] {
        let described = describe_subnet(case, &router, holder).await;
        let read_back = ipv6_association(case, &described, "associationId");
        assert_eq!(
            read_back, *association,
            "case {case:?}: expected {holder:?} to keep {association:?}, got {read_back:?}"
        );
        let state = ipv6_association(case, &described, "state");
        assert!(
            state == ec2::state::CIDR_BLOCK_STATE_ASSOCIATING
                || state == ec2::state::CIDR_BLOCK_STATE_ASSOCIATED,
            "case {case:?}: expected {holder:?} to keep holding its block, got {state:?}"
        );
    }

    tokio::time::sleep(SETTLE_WAIT).await;

    // And after the clock-owed terminal state, the subject holds nothing while the others
    // still hold theirs.
    let described = describe_subnet(case, &router, &subject).await;
    let state = association_state(case, &described, &subject_association);
    assert_eq!(
        state,
        ec2::state::CIDR_BLOCK_STATE_DISASSOCIATED,
        "case {case:?}: expected the subject's association to have ended, got {state:?}"
    );
    let held = held_blocks(case, &described);
    assert!(
        held.is_empty(),
        "case {case:?}: expected the subject subnet to hold no IPv6 block, got {held:?}"
    );
    for (holder, expected) in [
        (&bystander, block),
        (&neighbour, "2001:db8:13:2::/64"),
    ] {
        let described = describe_subnet(case, &router, holder).await;
        let held = held_blocks(case, &described);
        assert_eq!(
            held,
            vec![expected.to_string()],
            "case {case:?}: expected {holder:?} to still hold {expected:?}, got {held:?}"
        );
    }
}

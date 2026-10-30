// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssignIpv6Addresses`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=AssignIpv6Addresses`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that AN ASSIGNMENT TAKES IPv6 ADDRESSES, AND IPv6 PREFIX
//! DELEGATION PREFIXES, OUT OF THE IPv6 CIDR BLOCK OF THE SUBNET THE NAMED NETWORK
//! INTERFACE SITS IN AND RECORDS THEM ON THAT INTERFACE: the request either names them or
//! says how many, each one lies inside the assignable part of that block, no two resources
//! of the account are handed the same address or overlapping ranges, and what a call
//! assigned reads back through `DescribeNetworkInterfaces`. Everything else is refused with
//! the code that says why, and nothing is recorded.
//!
//! Every table row is a distinct input along what that generalises over — named addresses
//! against a count of them, how many, how the address was spelled, where in the block it
//! sits, addresses against prefixes — so no single recorded call is the specification. The
//! values the SERVICE chooses are asserted as properties (inside the block, assignable,
//! distinct from everything already handed out) rather than as fixed text: which free
//! address it picks is its own choice.

use std::net::Ipv6Addr;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{child_text, document, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The IPv6 block the fixture VPC brings from the account's own address space ("The ID of an
/// IPv6 address pool from which to allocate the IPv6 CIDR block" —
/// `AssociateVpcCidrBlockRequest$Ipv6Pool`), so every block in this suite is a known value
/// rather than one the emulator minted.
const VPC_IPV6_BLOCK: &str = "2001:db8:e::/56";

/// The IPv6 address pool the fixture VPC's block is brought from.
const IPV6_POOL: &str = "ipv6pool-ec2-0123456789abcdef0";

/// The IPv6 block of the subnet the assignments come out of: a `/64` of [`VPC_IPV6_BLOCK`],
/// the one published subnet block size ([`ec2::state::SUBNET_IPV6_PREFIX_LENGTH`]).
const SUBNET_IPV6_BLOCK: &str = "2001:db8:e:1::/64";

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

/// The number an IPv6 address text names — the test's own arithmetic, so what the service
/// chose is judged against the block rather than against the service's own helpers.
fn address_number(case: &str, value: &str) -> u128 {
    u128::from(
        value
            .parse::<Ipv6Addr>()
            .unwrap_or_else(|_| panic!("case {case:?}: {value:?} must be an IPv6 address")),
    )
}

/// The first and last address of `block`, as numbers.
fn block_bounds(case: &str, block: &str) -> (u128, u128) {
    let (address, prefix_length) = block
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {block:?} must be a CIDR block"));
    let prefix_length: u32 = prefix_length
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: {block:?} must carry a prefix length"));
    let base = address_number(case, address);
    let size = 1u128 << (128 - prefix_length);
    (base, base + size - 1)
}

/// The fixtures every test here drives: the emulator, a subnet with an IPv6 CIDR block, and
/// one without — "the specified number of IPv6 addresses are assigned ... Amazon EC2
/// automatically selects the IPv6 addresses from the subnet range" (`Ipv6AddressCount`), so
/// a subnet with no IPv6 range is part of the rule too.
struct Fixtures {
    router: MultiServiceRouter,
    /// A subnet holding [`SUBNET_IPV6_BLOCK`].
    ipv6_subnet: String,
    /// A subnet of the same VPC with no IPv6 block at all.
    plain_subnet: String,
}

/// A VPC with an IPv6 block, a subnet holding a `/64` of it, and a subnet holding none.
async fn fixtures(case: &str) -> Fixtures {
    let (router, _engine) = super::ec2_query::emulator();
    let created = ok(
        case,
        &router,
        "CreateVpc",
        &[
            param("CidrBlock", "10.140.0.0/16"),
            param("Ipv6CidrBlock", VPC_IPV6_BLOCK),
            param("Ipv6Pool", IPV6_POOL),
        ],
    )
    .await;
    let vpc_id = member(case, &created, "vpcId");
    let ipv6_subnet = create_subnet(case, &router, &vpc_id, "10.140.1.0/24").await;
    let plain_subnet = create_subnet(case, &router, &vpc_id, "10.140.2.0/24").await;
    let associated = ok(
        case,
        &router,
        "AssociateSubnetCidrBlock",
        &[
            param("SubnetId", &ipv6_subnet),
            param("Ipv6CidrBlock", SUBNET_IPV6_BLOCK),
        ],
    )
    .await;
    let block = member(case, &associated, "ipv6CidrBlock");
    assert_eq!(
        block, SUBNET_IPV6_BLOCK,
        "case {case:?}: the fixture subnet must hold {SUBNET_IPV6_BLOCK}, got {block:?}"
    );
    Fixtures {
        router,
        ipv6_subnet,
        plain_subnet,
    }
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

/// A fresh network interface in `subnet_id`, as its id — so every row assigns to an
/// interface of its own and no row depends on another's assignments.
async fn create_interface(case: &str, router: &MultiServiceRouter, subnet_id: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateNetworkInterface",
        &[param("SubnetId", subnet_id)],
    )
    .await;
    member(case, &body, "networkInterfaceId")
}

/// The form fields of one `AssignIpv6Addresses` request: the addresses it names, or how many
/// it asks for, and the same for prefixes. A zero count is left out, since a request either
/// names or counts.
fn assign_params(
    network_interface_id: &str,
    named_addresses: &[&str],
    address_count: i32,
    named_prefixes: &[&str],
    prefix_count: i32,
) -> Vec<Param> {
    let mut params = vec![param("NetworkInterfaceId", network_interface_id)];
    params.extend(indexed("Ipv6Addresses", named_addresses));
    if address_count != 0 {
        params.push(param("Ipv6AddressCount", &address_count.to_string()));
    }
    // `Ipv6Prefixes` carries no `@ec2QueryName`, so ec2Query spells it with its
    // `@xmlName`: `Ipv6Prefix.N` (`smithy_query(query="operation",operation="AssignIpv6Addresses")`).
    params.extend(indexed("Ipv6Prefix", named_prefixes));
    if prefix_count != 0 {
        params.push(param("Ipv6PrefixCount", &prefix_count.to_string()));
    }
    params
}

/// What one `AssignIpv6Addresses` call asks for: the addresses it names, or how many it
/// wants the service to choose (the two the request may not carry together).
type Ask = (Vec<&'static str>, i32);

/// One `AssignIpv6Addresses` call asking for addresses only.
async fn assign_addresses(
    router: &MultiServiceRouter,
    network_interface_id: &str,
    named: &[&str],
    count: i32,
) -> (u16, String) {
    call(
        router,
        "AssignIpv6Addresses",
        &assign_params(network_interface_id, named, count, &[], 0),
    )
    .await
}

/// One `AssignIpv6Addresses` call asking for prefixes only.
async fn assign_prefixes(
    router: &MultiServiceRouter,
    network_interface_id: &str,
    named: &[&str],
    count: i32,
) -> (u16, String) {
    call(
        router,
        "AssignIpv6Addresses",
        &assign_params(network_interface_id, &[], 0, named, count),
    )
    .await
}

/// The text of every `<item>` of the response envelope's `set` member.
fn answered_set(case: &str, body: &str, set: &str) -> Vec<String> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), set)
        .into_iter()
        .map(|item| item.text().unwrap_or_default().to_string())
        .collect()
}

/// The addresses an `AssignIpv6Addresses` response says it assigned.
fn answered_addresses(case: &str, body: &str) -> Vec<String> {
    answered_set(case, body, "assignedIpv6Addresses")
}

/// The prefixes an `AssignIpv6Addresses` response says it assigned.
fn answered_prefixes(case: &str, body: &str) -> Vec<String> {
    answered_set(case, body, "assignedIpv6PrefixSet")
}

/// One member of every entry of the `set` the describe reports for `network_interface_id` —
/// what a LATER CALLER sees, read through the one implemented reader of these records.
async fn described_set(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
    set: &str,
    entry_member: &str,
) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeNetworkInterfaces",
        &indexed("NetworkInterfaceId", &[network_interface_id]),
    )
    .await;
    let parsed = document(case, &body);
    let interface = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "networkInterfaceId").as_deref() == Some(network_interface_id)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no {network_interface_id} in the describe body {body}")
        });
    set_items(interface, set)
        .into_iter()
        .filter_map(|item| child_text(item, entry_member))
        .collect()
}

/// The IPv6 addresses `DescribeNetworkInterfaces` reports for one interface.
async fn described_addresses(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
) -> Vec<String> {
    described_set(
        router,
        case,
        network_interface_id,
        "ipv6AddressesSet",
        "ipv6Address",
    )
    .await
}

/// The IPv6 prefixes `DescribeNetworkInterfaces` reports for one interface.
async fn described_prefixes(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
) -> Vec<String> {
    described_set(
        router,
        case,
        network_interface_id,
        "ipv6PrefixSet",
        "ipv6Prefix",
    )
    .await
}

/// Every address the assignment answered is one of the subnet's assignable addresses: inside
/// the block and above the ones AWS reserves at its bottom
/// ([`ec2::state::SUBNET_RESERVED_IPV6_ADDRESSES`]).
fn assert_assignable(case: &str, addresses: &[String]) {
    let (base, last) = block_bounds(case, SUBNET_IPV6_BLOCK);
    let lowest = base + ec2::state::SUBNET_RESERVED_IPV6_ADDRESSES;
    for address in addresses {
        let value = address_number(case, address);
        assert!(
            (lowest..=last).contains(&value),
            "case {case:?}: expected an assignable address of {SUBNET_IPV6_BLOCK}, \
             got {address:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What an assignment records
// ---------------------------------------------------------------------------

/// The rule: the addresses a request names, or the number of them it asks for, are taken out
/// of the subnet's IPv6 block and recorded on the interface — so the call answers exactly
/// what it assigned, in the canonical form of each address, and a later
/// `DescribeNetworkInterfaces` reports the same set.
///
/// The rows differ along what the rule generalises over: named addresses against a count,
/// how many of each, how the address was spelled (compressed, upper case, fully expanded),
/// and where in the block it sits (the lowest assignable address, the highest address of the
/// block, and addresses in between). No two rows may be handed the same address, which is
/// what pins that the records an interface holds ARE the in-use pool.
#[tokio::test]
async fn an_assignment_takes_addresses_out_of_the_subnet_block_and_round_trips() {
    let case = "assignment";
    let fixtures = fixtures(case).await;
    let router = &fixtures.router;
    let ceiling = i32::try_from(ec2::state::MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE)
        .expect("the per-interface ceiling fits an i32");

    // (case, the addresses named, the count asked for, the addresses expected — empty when
    //  the service chooses them, which is asserted as a property instead)
    // The named rows come first, so the rows that let the SERVICE choose are choosing out of
    // a block that already has addresses in it — which is what pins that it never hands out
    // one something else holds.
    let cases: Vec<(&str, Vec<&str>, i32, Vec<&str>)> = vec![
        (
            "the lowest assignable address of the block",
            vec!["2001:db8:e:1::4"],
            0,
            vec!["2001:db8:e:1::4"],
        ),
        (
            "the highest address of the block",
            vec!["2001:db8:e:1:ffff:ffff:ffff:ffff"],
            0,
            vec!["2001:db8:e:1:ffff:ffff:ffff:ffff"],
        ),
        (
            "one address the request names",
            vec!["2001:db8:e:1::80"],
            0,
            vec!["2001:db8:e:1::80"],
        ),
        (
            "three addresses the request names",
            vec!["2001:db8:e:1::90", "2001:db8:e:1::91", "2001:db8:e:1::9a"],
            0,
            vec!["2001:db8:e:1::90", "2001:db8:e:1::91", "2001:db8:e:1::9a"],
        ),
        (
            "an address in upper case",
            vec!["2001:DB8:E:1::A0"],
            0,
            vec!["2001:db8:e:1::a0"],
        ),
        (
            "a fully expanded address",
            vec!["2001:0db8:000e:0001:0000:0000:0000:00b0"],
            0,
            vec!["2001:db8:e:1::b0"],
        ),
        ("one address the service chooses", vec![], 1, vec![]),
        ("three addresses the service chooses", vec![], 3, vec![]),
        (
            // The published per-interface bound: "You can assign as many IPv6 addresses to a
            // network interface as you can assign private IPv4 addresses"
            // (`AssignIpv6Addresses`), which this crate holds at
            // `ec2::state::MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE`.
            "the per-interface maximum, all chosen",
            vec![],
            ceiling,
            vec![],
        ),
    ];

    // Every address this suite has handed out, so no row can be given one another row holds.
    let mut handed_out: Vec<String> = Vec::new();

    for (row, named, count, expected) in cases {
        let interface = create_interface(row, router, &fixtures.ipv6_subnet).await;
        let body = ok(
            row,
            router,
            "AssignIpv6Addresses",
            &assign_params(&interface, &named, count, &[], 0),
        )
        .await;

        let reported_interface = member(row, &body, "networkInterfaceId");
        assert_eq!(
            reported_interface, interface,
            "case {row:?}: expected the assignment on {interface:?}, got {reported_interface:?}"
        );
        let assigned = answered_addresses(row, &body);
        if expected.is_empty() {
            let wanted = usize::try_from(count).expect("a non-negative count");
            assert_eq!(
                assigned.len(),
                wanted,
                "case {row:?}: expected {wanted} assigned addresses, got {assigned:?}"
            );
        } else {
            assert_eq!(
                assigned, expected,
                "case {row:?}: expected the addresses {expected:?}, got {assigned:?}"
            );
        }
        assert_assignable(row, &assigned);
        for address in &assigned {
            assert!(
                !handed_out.contains(address),
                "case {row:?}: {address:?} was already assigned to another interface, \
                 and an IPv6 address is held by one interface at a time"
            );
            handed_out.push(address.clone());
        }

        // The round trip: what a later caller sees is what the call said it assigned.
        let described = described_addresses(router, row, &interface).await;
        assert_eq!(
            described, assigned,
            "case {row:?}: DescribeNetworkInterfaces reported {described:?}, the assignment \
             answered {assigned:?}"
        );
        // A request that asked for no prefixes assigned none, so nothing is reported for
        // them: "The IPv6 prefixes that are assigned to the network interface"
        // (`AssignedIpv6Prefixes`).
        let prefixes = answered_prefixes(row, &body);
        assert!(
            prefixes.is_empty(),
            "case {row:?}: a request that asked for no prefixes assigns none, got {prefixes:?}"
        );
    }
}

/// The rule's additive half: "The specified number of IPv6 addresses are assigned in addition
/// to the existing IPv6 addresses that are already assigned to the network interface"
/// (`Ipv6AddressCount`), and "Existing IPv6 addresses that were assigned to the network
/// interface before the request are not included" (`AssignedIpv6Addresses`) — so a second
/// call answers only what IT assigned while the interface holds the union.
///
/// The rows differ in how each of the two calls asked: a count then a count, a count then
/// named addresses, named then counted, and named then named.
#[tokio::test]
async fn a_second_assignment_adds_to_what_the_interface_already_holds() {
    let case = "additive";
    let fixtures = fixtures(case).await;
    let router = &fixtures.router;

    // (case, first ask, second ask) — each ask is (named addresses, count)
    let cases: [(&str, Ask, Ask); 4] = [
        ("a count then a count", (vec![], 2), (vec![], 1)),
        (
            "a count then named addresses",
            (vec![], 1),
            (vec!["2001:db8:e:1::50", "2001:db8:e:1::51"], 0),
        ),
        (
            "named addresses then a count",
            (vec!["2001:db8:e:1::60"], 0),
            (vec![], 3),
        ),
        (
            "named addresses then named addresses",
            (vec!["2001:db8:e:1::70"], 0),
            (vec!["2001:db8:e:1::71"], 0),
        ),
    ];

    for (row, (first_named, first_count), (second_named, second_count)) in cases {
        let interface = create_interface(row, router, &fixtures.ipv6_subnet).await;
        let first_body = ok(
            row,
            router,
            "AssignIpv6Addresses",
            &assign_params(&interface, &first_named, first_count, &[], 0),
        )
        .await;
        let first = answered_addresses(row, &first_body);
        let second_body = ok(
            row,
            router,
            "AssignIpv6Addresses",
            &assign_params(&interface, &second_named, second_count, &[], 0),
        )
        .await;
        let second = answered_addresses(row, &second_body);

        for address in &second {
            assert!(
                !first.contains(address),
                "case {row:?}: the second call answered {address:?}, which the first already \
                 assigned"
            );
        }
        let mut union = first.clone();
        union.extend(second.iter().cloned());
        let described = described_addresses(router, row, &interface).await;
        assert_eq!(
            described, union,
            "case {row:?}: the interface must hold every address both calls assigned \
             ({union:?}), got {described:?}"
        );
        assert_assignable(row, &described);
    }
}

// ---------------------------------------------------------------------------
// What an assignment refuses
// ---------------------------------------------------------------------------

/// The rule's refusals: a request that does not name a usable interface, does not say what to
/// assign, or names an address the subnet cannot give is refused with the code that says why,
/// and nothing is recorded on any interface.
///
/// The rows differ along the ways one request can fail: the form of the interface id (both
/// published lengths, a digit too few, another resource kind, text that is no id), the
/// combination of members (both halves of a pair, neither of them, a negative count, one past
/// the per-interface maximum), and the address named (not an IPv6 address, outside the block,
/// one AWS reserves at the bottom of the block, one another interface already holds, the same
/// one twice) — plus an interface whose subnet has no IPv6 block at all.
#[tokio::test]
async fn an_assignment_it_cannot_make_is_refused_and_records_nothing() {
    let case = "refusals";
    let fixtures = fixtures(case).await;
    let router = &fixtures.router;
    let subject = create_interface(case, router, &fixtures.ipv6_subnet).await;
    let plain = create_interface(case, router, &fixtures.plain_subnet).await;
    // One interface holding one address, so "an address something else holds" is a real row.
    let holder = create_interface(case, router, &fixtures.ipv6_subnet).await;
    let held = "2001:db8:e:1::100";
    let (status, body) = assign_addresses(router, &holder, &[held], 0).await;
    assert_eq!(
        status, 200,
        "the suite's own holder fixture must be assigned {held}, got {status} with {body}"
    );
    let over_ceiling = i32::try_from(ec2::state::MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE)
        .expect("the per-interface ceiling fits an i32")
        + 1;

    // (case, the request's fields, the EC2 error code)
    let cases: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "a value that is no interface id at all",
            assign_params("not-an-id", &[], 1, &[], 0),
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "one digit short of the long id form",
            assign_params("eni-0123456789abcdef", &[], 1, &[], 0),
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "another resource kind's id",
            assign_params("subnet-0123456789abcdef0", &[], 1, &[], 0),
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "a long id the account holds no interface for",
            assign_params("eni-0123456789abcdef0", &[], 1, &[], 0),
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            // The all-zero suffix is WELL FORMED for an interface id, so it is answered as
            // not found rather than as malformed.
            "the all-zero long id",
            assign_params("eni-00000000000000000", &[], 1, &[], 0),
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            "a short id the account holds no interface for",
            assign_params("eni-1a2b3c4d", &[], 1, &[], 0),
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            "both the addresses and a count of them",
            assign_params(&subject, &["2001:db8:e:1::200"], 2, &[], 0),
            "InvalidParameterCombination",
        ),
        (
            "both the prefixes and a count of them",
            assign_params(&subject, &[], 0, &["2001:db8:e:1:2::/80"], 2),
            "InvalidParameterCombination",
        ),
        (
            "neither the addresses nor a count",
            assign_params(&subject, &[], 0, &[], 0),
            "MissingParameter",
        ),
        (
            "a negative address count",
            assign_params(&subject, &[], -1, &[], 0),
            "InvalidParameterValue",
        ),
        (
            "a negative prefix count",
            assign_params(&subject, &[], 0, &[], -2),
            "InvalidParameterValue",
        ),
        (
            "one more than the per-interface maximum",
            assign_params(&subject, &[], over_ceiling, &[], 0),
            "InvalidParameterValue",
        ),
        (
            "an IPv4 address",
            assign_params(&subject, &["10.140.1.5"], 0, &[], 0),
            "InvalidParameterValue",
        ),
        (
            "a value that is no address at all",
            assign_params(&subject, &["not-an-address"], 0, &[], 0),
            "InvalidParameterValue",
        ),
        (
            "an address of another block of the VPC",
            assign_params(&subject, &["2001:db8:e:2::5"], 0, &[], 0),
            "InvalidParameterValue",
        ),
        (
            "an address outside the VPC altogether",
            assign_params(&subject, &["2001:db8:ffff:1::5"], 0, &[], 0),
            "InvalidParameterValue",
        ),
        (
            "the network address of the block",
            assign_params(&subject, &["2001:db8:e:1::"], 0, &[], 0),
            "InvalidParameterValue",
        ),
        (
            "the last address AWS reserves at the bottom of the block",
            assign_params(&subject, &["2001:db8:e:1::3"], 0, &[], 0),
            "InvalidParameterValue",
        ),
        (
            "an address another interface already holds",
            assign_params(&subject, &[held], 0, &[], 0),
            "InvalidIPAddress.InUse",
        ),
        (
            "the same address twice in one request",
            assign_params(
                &subject,
                &["2001:db8:e:1::300", "2001:db8:e:1::300"],
                0,
                &[],
                0,
            ),
            "InvalidIPAddress.InUse",
        ),
        (
            "one spelling of an address another interface holds, in another spelling",
            assign_params(&subject, &["2001:0DB8:000E:0001::0100"], 0, &[], 0),
            "InvalidIPAddress.InUse",
        ),
        (
            "an interface whose subnet has no IPv6 block",
            assign_params(&plain, &[], 1, &[], 0),
            "InvalidParameterValue",
        ),
    ];

    for (row, params, expected_code) in cases {
        let (status, body) = call(router, "AssignIpv6Addresses", &params).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {row:?}: expected error code {expected_code:?}, got {code:?} in {body}"
        );
    }

    // Nothing above was recorded: the subject and the plain interface still hold no IPv6
    // address, and the holder still holds exactly the one it was given.
    for (row, interface) in [("the subject", &subject), ("the plain interface", &plain)] {
        let described = described_addresses(router, row, interface).await;
        assert!(
            described.is_empty(),
            "case {row:?}: a refused assignment records nothing, got {described:?}"
        );
    }
    let described = described_addresses(router, "the holder", &holder).await;
    assert_eq!(
        described,
        vec![held.to_string()],
        "the holder must keep exactly the address it was given, got {described:?}"
    );

    // The wording of each of the two id refusals, which a client reads to tell a typo from a
    // resource it does not hold: the malformed one says the id is malformed, naming the
    // resource the way the not-found one names it.
    for (row, network_interface_id, expected_message) in [
        (
            "a malformed id",
            "not-an-id",
            "The interface ID 'not-an-id' is malformed".to_string(),
        ),
        (
            "a well-formed id naming nothing",
            "eni-0123456789abcdef0",
            "The networkInterface ID 'eni-0123456789abcdef0' does not exist".to_string(),
        ),
    ] {
        let (_, body) = assign_addresses(router, network_interface_id, &[], 1).await;
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {row:?}: expected the message {expected_message:?}, got {message:?}"
        );
    }

    // A request that omits the interface entirely never reaches the operation:
    // `NetworkInterfaceId` is required, so the boundary validator refuses it first.
    let (status, body) = call(
        router,
        "AssignIpv6Addresses",
        &[param("Ipv6AddressCount", "1")],
    )
    .await;
    assert_eq!(
        status, 400,
        "a request with no NetworkInterfaceId must be refused with HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "MalformedInput",
        "a request with no NetworkInterfaceId is refused by the boundary validator, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Prefix Delegation
// ---------------------------------------------------------------------------

/// The rule reaches the prefixes too: "You can optionally use Prefix Delegation on the
/// network interface. You must specify either the IPV6 Prefix Delegation prefixes, or the
/// IPv6 Prefix Delegation count" (`AssignIpv6Addresses`) — a delegated prefix is a range OF
/// the subnet's block, recorded on the interface and read back through the describe, and a
/// service-chosen one is a [`ec2::state::IPV6_PREFIX_DELEGATION_PREFIX_LENGTH`] block of it.
///
/// The rows differ in named prefixes against a count, how many, and how the prefix was
/// spelled (canonical, host bits set, upper case).
#[tokio::test]
async fn delegated_prefixes_come_out_of_the_subnet_block_and_round_trip() {
    let case = "prefixes";
    let fixtures = fixtures(case).await;
    let router = &fixtures.router;
    let (base, last) = block_bounds(case, SUBNET_IPV6_BLOCK);

    // (case, the prefixes named, the count asked for, the prefixes expected — empty when the
    //  service chooses them)
    let cases: [(&str, &[&str], i32, &[&str]); 5] = [
        ("one prefix the service chooses", &[], 1, &[]),
        ("two prefixes the service chooses", &[], 2, &[]),
        (
            // A `/80` of the subnet's `/64` differs from it in the fifth group of the
            // address: the delegation size is 16 bits narrower than the subnet block.
            "one prefix the request names",
            &["2001:db8:e:1:5::/80"],
            0,
            &["2001:db8:e:1:5::/80"],
        ),
        (
            "a prefix with host bits set",
            &["2001:db8:e:1:6::abcd/80"],
            0,
            &["2001:db8:e:1:6::/80"],
        ),
        (
            "two prefixes in upper case",
            &["2001:DB8:E:1:7::/80", "2001:DB8:E:1:8::/80"],
            0,
            &["2001:db8:e:1:7::/80", "2001:db8:e:1:8::/80"],
        ),
    ];

    let mut handed_out: Vec<String> = Vec::new();
    for (row, named, count, expected) in cases {
        let interface = create_interface(row, router, &fixtures.ipv6_subnet).await;
        let body = ok(
            row,
            router,
            "AssignIpv6Addresses",
            &assign_params(&interface, &[], 0, named, count),
        )
        .await;
        let assigned = answered_prefixes(row, &body);
        if expected.is_empty() {
            let wanted = usize::try_from(count).expect("a non-negative count");
            assert_eq!(
                assigned.len(),
                wanted,
                "case {row:?}: expected {wanted} delegated prefixes, got {assigned:?}"
            );
        } else {
            assert_eq!(
                assigned, expected,
                "case {row:?}: expected the prefixes {expected:?}, got {assigned:?}"
            );
        }
        for prefix in &assigned {
            let (prefix_base, prefix_last) = block_bounds(row, prefix);
            assert!(
                prefix_base >= base + ec2::state::SUBNET_RESERVED_IPV6_ADDRESSES
                    && prefix_last <= last,
                "case {row:?}: expected a prefix inside the assignable part of \
                 {SUBNET_IPV6_BLOCK}, got {prefix:?}"
            );
            assert!(
                !handed_out.contains(prefix),
                "case {row:?}: {prefix:?} is already delegated elsewhere, and a range is \
                 delegated to one interface at a time"
            );
            handed_out.push(prefix.clone());
        }
        // Each prefix the service chose is one of the published Prefix Delegation size.
        if expected.is_empty() {
            for prefix in &assigned {
                let size = prefix.split('/').next_back().unwrap_or_default();
                assert_eq!(
                    size,
                    ec2::state::IPV6_PREFIX_DELEGATION_PREFIX_LENGTH.to_string(),
                    "case {row:?}: a service-chosen prefix is a \
                     /{expected_size} of the subnet's block, got {prefix:?}",
                    expected_size = ec2::state::IPV6_PREFIX_DELEGATION_PREFIX_LENGTH,
                );
            }
        }

        let described = described_prefixes(router, row, &interface).await;
        assert_eq!(
            described, assigned,
            "case {row:?}: DescribeNetworkInterfaces reported {described:?}, the assignment \
             answered {assigned:?}"
        );
        let addresses = answered_addresses(row, &body);
        assert!(
            addresses.is_empty(),
            "case {row:?}: a request that asked for no addresses assigns none, \
             got {addresses:?}"
        );
    }
}

/// The rule's one pool: an address and a delegated range cannot be handed out twice, in
/// either order — so a prefix overlapping one already delegated, or covering an address an
/// interface holds, is refused, and so is an address inside a delegated prefix. A prefix
/// outside the assignable part of the subnet's block is refused too.
///
/// The rows differ in how the collision arises (the same prefix, a prefix containing it, a
/// prefix inside it, an address inside it) and in how a prefix can lie outside the range (the
/// bottom prefix, which holds the reserved addresses, the whole block, another block of the
/// VPC, and a value that is no prefix at all).
#[tokio::test]
async fn a_range_and_an_address_are_handed_out_once() {
    let case = "one pool";
    let fixtures = fixtures(case).await;
    let router = &fixtures.router;
    let subject = create_interface(case, router, &fixtures.ipv6_subnet).await;
    let holder = create_interface(case, router, &fixtures.ipv6_subnet).await;

    // The ranges and the address the collisions are measured against.
    let delegated = "2001:db8:e:1:5::/80";
    let (status, body) = assign_prefixes(router, &holder, &[delegated], 0).await;
    assert_eq!(
        status, 200,
        "the suite's own delegated fixture must be assigned, got {status} with {body}"
    );
    let held_address = "2001:db8:e:1:a::1";
    let (status, body) = assign_addresses(router, &holder, &[held_address], 0).await;
    assert_eq!(
        status, 200,
        "the suite's own held address must be assigned, got {status} with {body}"
    );

    // (case, the request's fields, the EC2 error code)
    let cases: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "the prefix another interface already holds",
            assign_params(&subject, &[], 0, &[delegated], 0),
            "InvalidIPAddress.InUse",
        ),
        (
            "a prefix inside one already delegated",
            assign_params(&subject, &[], 0, &["2001:db8:e:1:5::/96"], 0),
            "InvalidIPAddress.InUse",
        ),
        (
            "a prefix covering an address an interface holds",
            assign_params(&subject, &[], 0, &["2001:db8:e:1:a::/80"], 0),
            "InvalidIPAddress.InUse",
        ),
        (
            "an address inside a delegated prefix",
            assign_params(&subject, &["2001:db8:e:1:5::7"], 0, &[], 0),
            "InvalidIPAddress.InUse",
        ),
        (
            "the same prefix twice in one request",
            assign_params(
                &subject,
                &[],
                0,
                &["2001:db8:e:1:9::/80", "2001:db8:e:1:9::/80"],
                0,
            ),
            "InvalidIPAddress.InUse",
        ),
        (
            // The bottom prefix of the block holds the addresses AWS reserves there, so it
            // is not a range the service can delegate.
            "the bottom prefix of the block",
            assign_params(&subject, &[], 0, &["2001:db8:e:1::/80"], 0),
            "InvalidParameterValue",
        ),
        (
            "the subnet's whole block",
            assign_params(&subject, &[], 0, &[SUBNET_IPV6_BLOCK], 0),
            "InvalidParameterValue",
        ),
        (
            "a prefix of another block of the VPC",
            assign_params(&subject, &[], 0, &["2001:db8:e:2:5::/80"], 0),
            "InvalidParameterValue",
        ),
        (
            "a value that is no prefix at all",
            assign_params(&subject, &[], 0, &["2001:db8:e:1:5::"], 0),
            "InvalidParameterValue",
        ),
    ];

    for (row, params, expected_code) in cases {
        let (status, body) = call(router, "AssignIpv6Addresses", &params).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {row:?}: expected error code {expected_code:?}, got {code:?} in {body}"
        );
    }

    let described = described_prefixes(router, case, &subject).await;
    assert!(
        described.is_empty(),
        "a refused delegation records nothing, got {described:?}"
    );
    let described = described_addresses(router, case, &subject).await;
    assert!(
        described.is_empty(),
        "a refused assignment records nothing, got {described:?}"
    );
    let described = described_prefixes(router, case, &holder).await;
    assert_eq!(
        described,
        vec![delegated.to_string()],
        "the holder must keep exactly the prefix it was given, got {described:?}"
    );

    // And the two halves of one request are served together: a request may ask for addresses
    // AND prefixes, and both are recorded on the interface.
    let both = create_interface(case, router, &fixtures.ipv6_subnet).await;
    let body = ok(
        case,
        router,
        "AssignIpv6Addresses",
        &assign_params(&both, &[], 2, &[], 1),
    )
    .await;
    let addresses = answered_addresses(case, &body);
    let prefixes = answered_prefixes(case, &body);
    assert_eq!(
        addresses.len(),
        2,
        "a request asking for two addresses and a prefix assigns both, got {addresses:?}"
    );
    assert_eq!(
        prefixes.len(),
        1,
        "a request asking for two addresses and a prefix assigns both, got {prefixes:?}"
    );
    assert_assignable(case, &addresses);
    for prefix in &prefixes {
        let (prefix_base, prefix_last) = block_bounds(case, prefix);
        for address in &addresses {
            let value = address_number(case, address);
            assert!(
                !(prefix_base..=prefix_last).contains(&value),
                "a chosen prefix must not cover an address the same call chose: \
                 {prefix:?} against {address:?}"
            );
        }
    }
    let described = described_addresses(router, case, &both).await;
    assert_eq!(
        described, addresses,
        "the interface must hold the addresses the call assigned, got {described:?}"
    );
    let described = described_prefixes(router, case, &both).await;
    assert_eq!(
        described, prefixes,
        "the interface must hold the prefix the call delegated, got {described:?}"
    );
}

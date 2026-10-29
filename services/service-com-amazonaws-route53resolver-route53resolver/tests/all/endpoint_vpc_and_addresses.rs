// SPDX-License-Identifier: CC0-1.0

//! A Resolver endpoint's VPC and IP addresses are the account's own, not values
//! this service made up.
//!
//! Two rules are under test, both about members a client joins back to the
//! account's Amazon EC2 state:
//!
//! 1. `ResolverEndpoint$HostVPCId` is "The ID of the VPC that you want to create
//!    the Resolver endpoint in", and the request says which VPC that is only
//!    through its addresses' subnets — "The subnet ID uniquely identifies a VPC"
//!    (`CreateResolverEndpoint$IpAddresses`). So the reported id is the id of the
//!    VPC that OWNS those subnets in the account's inventory, whatever the subnet
//!    ids look like.
//! 2. "If you specify a subnet ID, Resolver chooses an IP address for you from
//!    the available IPs in the specified subnet": every address an endpoint holds
//!    carries one IPv4 address out of its subnet's own block
//!    (`IpAddressResponse$Ip`, length 7..36), and that address is the address of
//!    an elastic network interface of the same VPC — so Resolver and Amazon EC2
//!    report exactly the same addresses, and an address an endpoint gives up
//!    takes its interface with it.
//!
//! The rows differ along what the rules generalise over: how many addresses an
//! endpoint places (both modelled bounds of `IpAddresses`, 2..20, and sizes
//! between them), how many distinct subnets they sit in, which VPC those subnets
//! belong to (the account holds several), and whether the request names the
//! address or leaves it to the service. Every case goes over the wire the
//! service actually speaks, through the shared [`super::harness`].

use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    PROVISIONING_DELAY, SERVICE_PRINCIPAL,
};

use super::harness::{
    address_inside_block, emulator_on, ok, string_member, Emulator, NetworkInterfaceView,
};

/// Wait out the span the clock owes a freshly created endpoint, so the next read
/// sees its addresses `ATTACHED`.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

/// A fresh emulator a suite can also read the account's Amazon EC2 store
/// through.
fn emulator_with_ec2_state() -> Emulator {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The `ResolverEndpoint` of a create or get response.
fn endpoint(body: &Value) -> &Value {
    body.get("ResolverEndpoint")
        .unwrap_or_else(|| panic!("the response carries a ResolverEndpoint, got {body}"))
}

/// The `IpAddresses` of a `ListResolverEndpointIpAddresses` response.
async fn listed_addresses(router: &Emulator, resolver_endpoint_id: &str) -> Vec<Value> {
    ok(
        router,
        "ListResolverEndpointIpAddresses",
        json!({ "ResolverEndpointId": resolver_endpoint_id }),
    )
    .await
    .get("IpAddresses")
    .and_then(Value::as_array)
    .cloned()
    .unwrap_or_else(|| panic!("the read carries an IpAddresses array for {resolver_endpoint_id}"))
}

/// Create one OUTBOUND endpoint over `subnets`, placing `addresses` addresses
/// across them in turn, and answer its `ResolverEndpoint`.
async fn create_endpoint(
    router: &Emulator,
    creator: &str,
    subnets: &[String],
    addresses: usize,
    supplied_ips: &[String],
) -> Value {
    let ip_addresses: Vec<Value> = (0..addresses)
        .map(|index| {
            let mut address = json!({ "SubnetId": subnets[index % subnets.len()] });
            if let Some(ip) = supplied_ips.get(index) {
                address["Ip"] = json!(ip);
            }
            address
        })
        .collect();
    let created = ok(
        router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": creator,
            "Direction": "OUTBOUND",
            "SecurityGroupIds": ["sg-00000000000000001"],
            "IpAddresses": ip_addresses,
        }),
    )
    .await;
    endpoint(&created).clone()
}

/// The interfaces the account's Amazon EC2 store holds for the endpoint
/// `resolver_endpoint_id`, as `DescribeNetworkInterfaces` would report them.
fn interfaces_of(router: &Emulator, resolver_endpoint_id: &str) -> Vec<NetworkInterfaceView> {
    router
        .network_interfaces()
        .into_iter()
        .filter(|interface| interface.description.contains(resolver_endpoint_id))
        .collect()
}

/// One row of the `HostVPCId` table.
#[derive(Debug)]
struct VpcCase {
    case: &'static str,
    /// How many distinct subnets the endpoint's addresses sit in.
    subnets: usize,
    /// How many addresses the create places.
    addresses: usize,
    /// Whether the request names each address itself.
    supplies_ips: bool,
}

/// `HostVPCId` is the id of the VPC the account holds the endpoint's subnets in,
/// for every size and shape of address list — and the listing filter matches on
/// that very id.
#[tokio::test]
async fn host_vpc_id_is_the_vpc_that_owns_the_endpoints_subnets() {
    let router = emulator_with_ec2_state();
    let cases = [
        VpcCase {
            case: "the modelled minimum, one subnet each",
            subnets: 2,
            addresses: 2,
            supplies_ips: false,
        },
        VpcCase {
            case: "two addresses in ONE subnet",
            subnets: 1,
            addresses: 2,
            supplies_ips: false,
        },
        VpcCase {
            case: "five addresses across two subnets",
            subnets: 2,
            addresses: 5,
            supplies_ips: false,
        },
        VpcCase {
            case: "the modelled maximum, one subnet each",
            subnets: 20,
            addresses: 20,
            supplies_ips: false,
        },
        VpcCase {
            case: "three addresses the request names itself",
            subnets: 3,
            addresses: 3,
            supplies_ips: true,
        },
    ];

    // Each row gets its OWN VPC of the account, so an id read off the subnets
    // cannot be confused with an id read off anything else.
    let mut created: Vec<(String, String)> = Vec::new();
    for (row, case) in cases.iter().enumerate() {
        let vpc_id = router.hold_vpc();
        let subnets: Vec<String> = (0..case.subnets)
            .map(|index| format!("subnet-vpc-{row}-{index}"))
            .collect();
        router.hold_subnets_in(&vpc_id, &subnets);
        let supplied: Vec<String> = if case.supplies_ips {
            (0..case.addresses)
                .map(|index| {
                    let block = router
                        .cidr_block_of_subnet(&subnets[index % subnets.len()])
                        .expect("the subnet the account holds states a block");
                    let base = block
                        .split_once('/')
                        .map(|(base, _)| base.to_string())
                        .expect("the block states a prefix length");
                    let mut octets: Vec<&str> = base.split('.').collect();
                    octets.pop();
                    format!("{}.{}", octets.join("."), 20 + index)
                })
                .collect()
        } else {
            Vec::new()
        };

        let created_endpoint = create_endpoint(
            &router,
            &format!("req-vpc-{row}"),
            &subnets,
            case.addresses,
            &supplied,
        )
        .await;
        let id = string_member(&created_endpoint, "Id");
        let reported = string_member(&created_endpoint, "HostVPCId");

        // The requirement: the reported VPC is the VPC the account's own subnet
        // inventory names, read back from that inventory and not from the
        // response.
        let owning = router
            .vpc_of_subnet(&subnets[0])
            .expect("the account holds the subnet in a VPC");
        assert_eq!(
            reported, owning,
            "case {:?}: HostVPCId must be the VPC that owns the subnets, got {reported:?}",
            case.case
        );
        assert_eq!(
            reported, vpc_id,
            "case {:?}: HostVPCId must be the VPC the subnets were created in, got {reported:?}",
            case.case
        );

        // And the read-back reports the same VPC.
        let fetched = ok(
            &router,
            "GetResolverEndpoint",
            json!({ "ResolverEndpointId": id }),
        )
        .await;
        assert_eq!(
            string_member(endpoint(&fetched), "HostVPCId"),
            reported,
            "case {:?}: GetResolverEndpoint must report the stored HostVPCId, got {fetched}",
            case.case
        );

        created.push((id, reported));
    }

    // Every endpoint reported a DIFFERENT VPC, because each row's subnets are a
    // different VPC's: the id tracks the inventory and not the request.
    let distinct: BTreeSet<&String> = created.iter().map(|(_, vpc_id)| vpc_id).collect();
    assert_eq!(
        distinct.len(),
        created.len(),
        "each row's subnets belong to their own VPC, so each endpoint must report its own \
         HostVPCId, got {created:?}"
    );

    // Filtering the listing on a VPC the account really has finds exactly the
    // endpoint placed in it.
    for (id, vpc_id) in &created {
        let listed = ok(
            &router,
            "ListResolverEndpoints",
            json!({ "Filters": [{ "Name": "HostVPCId", "Values": [vpc_id] }] }),
        )
        .await;
        let matched: Vec<String> = listed
            .get("ResolverEndpoints")
            .and_then(Value::as_array)
            .map(|endpoints| {
                endpoints
                    .iter()
                    .map(|endpoint| string_member(endpoint, "Id"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            matched,
            vec![id.clone()],
            "filtering on HostVPCId {vpc_id:?} must find the endpoint placed in that VPC, \
             got {listed}"
        );
    }
}

/// One row of the address table.
#[derive(Debug)]
struct AddressCase {
    case: &'static str,
    subnets: usize,
    addresses: usize,
    /// How many of the addresses the request names itself; the rest are the
    /// service's to choose.
    supplied: usize,
}

/// Every address an endpoint holds carries an IPv4 address of its subnet, and
/// those addresses are exactly the addresses of the elastic network interfaces
/// the account's Amazon EC2 store holds for the endpoint.
#[tokio::test]
async fn an_endpoints_addresses_are_exactly_the_interfaces_amazon_ec2_holds() {
    let router = emulator_with_ec2_state();
    let cases = [
        AddressCase {
            case: "the modelled minimum, the service choosing both addresses",
            subnets: 2,
            addresses: 2,
            supplied: 0,
        },
        AddressCase {
            case: "three addresses in ONE subnet, all chosen by the service",
            subnets: 1,
            addresses: 3,
            supplied: 0,
        },
        AddressCase {
            case: "five addresses across two subnets, the first two named",
            subnets: 2,
            addresses: 5,
            supplied: 2,
        },
        AddressCase {
            case: "the modelled maximum, all chosen by the service",
            subnets: 20,
            addresses: 20,
            supplied: 0,
        },
    ];

    struct Placed {
        case: &'static str,
        id: String,
        vpc_id: String,
        subnets: Vec<String>,
        supplied: Vec<String>,
    }
    let mut placed: Vec<Placed> = Vec::new();

    for (row, case) in cases.iter().enumerate() {
        let vpc_id = router.hold_vpc();
        let subnets: Vec<String> = (0..case.subnets)
            .map(|index| format!("subnet-ips-{row}-{index}"))
            .collect();
        router.hold_subnets_in(&vpc_id, &subnets);
        let supplied: Vec<String> = (0..case.supplied)
            .map(|index| {
                let block = router
                    .cidr_block_of_subnet(&subnets[index % subnets.len()])
                    .expect("the subnet the account holds states a block");
                let base = block
                    .split_once('/')
                    .map(|(base, _)| base.to_string())
                    .expect("the block states a prefix length");
                let mut octets: Vec<&str> = base.split('.').collect();
                octets.pop();
                format!("{}.{}", octets.join("."), 40 + index)
            })
            .collect();

        let created_endpoint = create_endpoint(
            &router,
            &format!("req-ips-{row}"),
            &subnets,
            case.addresses,
            &supplied,
        )
        .await;
        placed.push(Placed {
            case: case.case,
            id: string_member(&created_endpoint, "Id"),
            vpc_id,
            subnets,
            supplied,
        });
    }

    // One wait for every row: the addresses reach `ATTACHED` after the span the
    // clock owes them, and that is the status the join below reads.
    settle().await;

    let mut every_address: Vec<String> = Vec::new();
    for row in &placed {
        let case = row.case;
        let addresses = listed_addresses(&router, &row.id).await;

        let mut reported: Vec<String> = Vec::new();
        for (index, address) in addresses.iter().enumerate() {
            let ip = string_member(address, "Ip");
            assert!(
                ip.parse::<Ipv4Addr>().is_ok(),
                "case {case:?}: address {index} must carry one IPv4 Ip, got {address}"
            );
            assert!(
                ip.len() >= 7 && ip.len() <= 36,
                "case {case:?}: Ip must satisfy the modelled length 7..36, got {ip:?}"
            );
            let subnet_id = string_member(address, "SubnetId");
            let block = router
                .cidr_block_of_subnet(&subnet_id)
                .unwrap_or_else(|| panic!("case {case:?}: the account holds subnet {subnet_id:?}"));
            assert!(
                address_inside_block(&block, &ip),
                "case {case:?}: {ip:?} must lie inside the subnet's block {block:?}, \
                 got {address}"
            );
            assert_eq!(
                string_member(address, "Status"),
                "ATTACHED",
                "case {case:?}: address {index} must be ATTACHED once the clock owes it, \
                 got {address}"
            );
            reported.push(ip);
        }

        // The addresses the request named are the addresses the endpoint holds.
        for supplied in &row.supplied {
            assert!(
                reported.contains(supplied),
                "case {case:?}: the address {supplied:?} the request named must be one the \
                 endpoint holds, got {reported:?}"
            );
        }

        // The join a client makes: Resolver's addresses and the interfaces
        // Amazon EC2 holds for the endpoint are the same set, in the same
        // subnets of the same VPC.
        let interfaces = interfaces_of(&router, &row.id);
        let from_ec2: BTreeSet<String> = interfaces
            .iter()
            .flat_map(|interface| interface.private_ips.clone())
            .collect();
        let from_resolver: BTreeSet<String> = reported.iter().cloned().collect();
        assert_eq!(
            from_resolver, from_ec2,
            "case {case:?}: the endpoint's addresses must be exactly the addresses Amazon EC2 \
             reports for its interfaces, got {interfaces:?}"
        );
        assert_eq!(
            from_resolver.len(),
            reported.len(),
            "case {case:?}: no two addresses of one endpoint may be the same, got {reported:?}"
        );
        for interface in &interfaces {
            assert_eq!(
                interface.vpc_id, row.vpc_id,
                "case {case:?}: the interface must sit in the endpoint's VPC, got {interface:?}"
            );
            assert!(
                row.subnets.contains(&interface.subnet_id),
                "case {case:?}: the interface must sit in a subnet the request named, \
                 got {interface:?}"
            );
            // Resolver created the interface to serve the endpoint, so the
            // inventory reports it as one requested and managed by that service
            // and not one the caller made for itself.
            assert!(
                interface.requester_managed,
                "case {case:?}: the interface must be reported as managed by the service that \
                 created it, got {interface:?}"
            );
            assert_eq!(
                interface.requester_id.as_deref(),
                Some(SERVICE_PRINCIPAL),
                "case {case:?}: the interface must name Resolver as its requester, \
                 got {interface:?}"
            );
        }
        every_address.extend(reported);
    }

    // No address was handed to two endpoints: each one comes out of the one pool
    // the account's Amazon EC2 state keeps.
    let distinct: BTreeSet<&String> = every_address.iter().collect();
    assert_eq!(
        distinct.len(),
        every_address.len(),
        "an address may be held by one interface only, got {every_address:?}"
    );
}

/// An address an endpoint gives up takes its interface with it, and deleting the
/// endpoint takes every interface it had.
#[tokio::test]
async fn an_address_an_endpoint_gives_up_takes_its_interface_with_it() {
    let router = emulator_with_ec2_state();
    let vpc_id = router.hold_vpc();
    let subnets: Vec<String> = (0..3).map(|index| format!("subnet-gone-{index}")).collect();
    router.hold_subnets_in(&vpc_id, &subnets);

    let created = create_endpoint(&router, "req-gone", &subnets, 3, &[]).await;
    let id = string_member(&created, "Id");
    assert_eq!(
        interfaces_of(&router, &id).len(),
        3,
        "a create places one interface per address"
    );

    // One more address, in a subnet of the same VPC, with the service choosing
    // the address: the added address is placed the same way.
    router.hold_subnets_in(&vpc_id, ["subnet-gone-added"]);
    let associated = ok(
        &router,
        "AssociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": id,
            "IpAddress": { "SubnetId": "subnet-gone-added" },
        }),
    )
    .await;
    assert_eq!(
        endpoint(&associated)
            .get("IpAddressCount")
            .and_then(Value::as_i64),
        Some(4),
        "the added address must be counted, got {associated}"
    );
    settle().await;

    let addresses = listed_addresses(&router, &id).await;
    let added = addresses
        .iter()
        .find(|address| string_member(address, "SubnetId") == "subnet-gone-added")
        .unwrap_or_else(|| panic!("the added address is listed, got {addresses:?}"));
    let added_ip = string_member(added, "Ip");
    let added_block = router
        .cidr_block_of_subnet("subnet-gone-added")
        .expect("the account holds the added subnet");
    assert!(
        address_inside_block(&added_block, &added_ip),
        "the added address must come out of its own subnet's block {added_block:?}, \
         got {added_ip:?}"
    );
    let interfaces: BTreeSet<String> = interfaces_of(&router, &id)
        .iter()
        .flat_map(|interface| interface.private_ips.clone())
        .collect();
    let listed: BTreeSet<String> = addresses
        .iter()
        .map(|address| string_member(address, "Ip"))
        .collect();
    assert_eq!(
        listed, interfaces,
        "after the addition the two services must still report the same addresses"
    );

    // Giving one address up takes its interface, and only its interface.
    ok(
        &router,
        "DisassociateResolverEndpointIpAddress",
        json!({
            "ResolverEndpointId": id,
            "IpAddress": { "Ip": added_ip, "SubnetId": "subnet-gone-added" },
        }),
    )
    .await;
    let after: BTreeSet<String> = interfaces_of(&router, &id)
        .iter()
        .flat_map(|interface| interface.private_ips.clone())
        .collect();
    assert!(
        !after.contains(&added_ip),
        "the interface holding the address the endpoint gave up must be gone, got {after:?}"
    );
    let remaining: BTreeSet<String> = listed_addresses(&router, &id)
        .await
        .iter()
        .map(|address| string_member(address, "Ip"))
        .collect();
    assert_eq!(
        remaining, after,
        "the addresses left on the endpoint must be exactly the interfaces left in Amazon EC2"
    );

    // And deleting the endpoint takes every interface it had.
    ok(
        &router,
        "DeleteResolverEndpoint",
        json!({ "ResolverEndpointId": id }),
    )
    .await;
    assert!(
        interfaces_of(&router, &id).is_empty(),
        "a deleted endpoint leaves no interface behind, got {:?}",
        interfaces_of(&router, &id)
    );
}

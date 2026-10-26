// SPDX-License-Identifier: CC0-1.0

//! Where an `ip` target's address falls relative to the target group's VPC, tested on the
//! rule itself rather than over the wire: the pure half of [`super::network`], which
//! decides from an address, the request's `AvailabilityZone` and what EC2 reported about
//! the VPC whether the registration is accepted and which zone it takes.
//!
//! The wire path these functions serve is tabled with EC2 mounted in
//! `core/router/tests/all/elbv2_network_resolution.rs`; here each clause of the rule is
//! driven directly, so a case that is awkward to stand a VPC up for — an address of a
//! subnet EC2 reports no zone for, an IPv6 group — is still covered.

use std::net::IpAddr;

use ipnet::IpNet;

use super::network::{
    address_in_no_subnet_message, address_not_allowed_message, availability_zone_required_message,
    parse_ip_target, resolve_ip_target, SubnetFacts,
};

/// The VPC every table below resolves against: one CIDR block and two subnets in two
/// zones, as EC2 would report them.
fn vpc() -> (Vec<IpNet>, Vec<SubnetFacts>) {
    let cidrs = vec!["10.80.0.0/16".parse().expect("a CIDR block")];
    let subnets = vec![
        SubnetFacts {
            subnet_id: "subnet-1".to_string(),
            vpc_id: Some("vpc-1".to_string()),
            availability_zone: Some("us-west-2a".to_string()),
            cidrs: vec!["10.80.1.0/24".parse().expect("a CIDR block")],
        },
        SubnetFacts {
            subnet_id: "subnet-2".to_string(),
            vpc_id: Some("vpc-1".to_string()),
            availability_zone: Some("us-west-2b".to_string()),
            cidrs: vec!["10.80.2.0/24".parse().expect("a CIDR block")],
        },
    ];
    (cidrs, subnets)
}

/// An address, as the rule takes it.
fn address(value: &str) -> IpAddr {
    value.parse().expect("an IP address")
}

/// One resolution as borrowed values, so a row can be compared against a literal.
fn resolved_as_refs(resolved: &Result<Option<String>, String>) -> Result<Option<&str>, &str> {
    match resolved {
        Ok(zone) => Ok(zone.as_deref()),
        Err(message) => Err(message.as_str()),
    }
}

#[test]
fn an_address_in_a_subnet_of_the_vpc_takes_that_subnets_zone() {
    let (cidrs, subnets) = vpc();
    // (case, address, the zone the request names, the zone the registration takes)
    let rows: [(&str, &str, Option<&str>, &str); 4] = [
        ("the first subnet", "10.80.1.5", None, "us-west-2a"),
        ("the second subnet", "10.80.2.5", None, "us-west-2b"),
        (
            "the first address of a subnet",
            "10.80.2.0",
            None,
            "us-west-2b",
        ),
        (
            "a request naming another zone does not move the target",
            "10.80.1.6",
            Some("us-west-2b"),
            "us-west-2a",
        ),
    ];
    for (case, value, requested, expected) in rows {
        let resolved = resolve_ip_target(address(value), requested, &cidrs, &subnets);
        assert_eq!(
            resolved_as_refs(&resolved),
            Ok(Some(expected)),
            "case {case:?}: expected the zone of the subnet holding {value}, got {resolved:?}"
        );
    }
}

#[test]
fn an_address_of_the_vpc_that_no_subnet_holds_is_refused() {
    let (cidrs, subnets) = vpc();
    // The zone plays no part: the address is in no zone the load balancer reaches either
    // way, so every row is refused for the same reason, and the refusal states that
    // constraint against the address the request carried — naming no VPC id, which the
    // request never carried.
    let rows: [(&str, &str, Option<&str>); 4] = [
        ("with no zone", "10.80.9.9", None),
        ("with a zone", "10.80.9.10", Some("us-west-2a")),
        ("with all zones", "10.80.9.11", Some("all")),
        // Between the two subnets rather than past them, and the first address of a block
        // the VPC holds: still no subnet of it.
        ("between the subnets", "10.80.3.0", None),
    ];
    for (case, value, requested) in rows {
        let resolved = resolve_ip_target(address(value), requested, &cidrs, &subnets);
        assert_eq!(
            resolved_as_refs(&resolved),
            Err(address_in_no_subnet_message(address(value)).as_str()),
            "case {case:?}: an address of the VPC that no subnet holds must be refused for \
             that, got {resolved:?}"
        );
    }
}

#[test]
fn each_refusal_class_states_its_own_constraint_against_the_address() {
    // The three sentences, written out rather than taken from the builders the rows above
    // call: those rows pin WHICH class fires for an address, and this one pins what each
    // class SAYS, so a rewording fails here instead of passing unnoticed. One address per
    // class, none of them a subnet address of the VPC.
    let (cidrs, subnets) = vpc();
    let rows: [(&str, &str, Option<&str>, &str); 3] = [
        (
            "an address of the VPC that no subnet holds",
            "10.80.9.9",
            None,
            "The IP address '10.80.9.9' is within the VPC, but is not within a VPC subnet \
             so it can't be registered",
        ),
        (
            "an address of no range an ip target may come from",
            "8.8.8.8",
            Some("all"),
            "The IP address '8.8.8.8' is not allowed, as it must be an address from a VPC \
             subnet, RFC1918, or RFC6598",
        ),
        (
            "an address outside the VPC with no zone to serve it",
            "192.168.5.6",
            None,
            "You must specify a valid Availability Zone or select 'all' for all enabled \
             Availability zones, since the IP address '192.168.5.6' is outside the VPC",
        ),
    ];
    for (case, value, requested, expected) in rows {
        let resolved = resolve_ip_target(address(value), requested, &cidrs, &subnets);
        assert_eq!(
            resolved_as_refs(&resolved),
            Err(expected),
            "case {case:?}: expected the refusal {expected:?}, got {resolved:?}"
        );
    }
}

#[test]
fn an_address_outside_the_vpc_is_judged_against_the_ranges_before_the_zone() {
    let (cidrs, subnets) = vpc();
    // Whether the address is one an `ip` target may carry at all does not depend on the
    // zone, so a publicly routable address is refused on its own terms WHETHER OR NOT the
    // request named one — and only an address the group could carry gets as far as being
    // asked for a zone.
    //
    // The ranges are `TargetType`'s own ("the RFC 1918 range (10.0.0.0/8, 172.16.0.0/12,
    // and 192.168.0.0/16), and the RFC 6598 range (100.64.0.0/10)"), one row each, plus
    // the IPv6 unique-local range and publicly routable addresses of two kinds.
    #[derive(Debug)]
    enum Want {
        /// Accepted, in the zone the request named.
        Zone(&'static str),
        /// Refused because the request named no zone for an address the group may carry.
        ZoneRequired,
        /// Refused because the address itself is not one an `ip` target may carry.
        NotRegistrable,
    }
    let rows: [(&str, &str, Option<&str>, Want); 11] = [
        (
            "an RFC 1918 address with the zone to serve it",
            "192.168.5.5",
            Some("us-west-2a"),
            Want::Zone("us-west-2a"),
        ),
        (
            "an RFC 1918 address of another range, with all zones",
            "172.16.4.4",
            Some("all"),
            Want::Zone("all"),
        ),
        (
            "an RFC 1918 address of the 10/8 range outside this VPC",
            "10.90.1.1",
            Some("us-west-2b"),
            Want::Zone("us-west-2b"),
        ),
        (
            "an RFC 6598 address",
            "100.64.3.3",
            Some("us-west-2a"),
            Want::Zone("us-west-2a"),
        ),
        (
            "an RFC 1918 address with no zone",
            "192.168.5.6",
            None,
            Want::ZoneRequired,
        ),
        (
            "a publicly routable address with a zone",
            "8.8.8.8",
            Some("us-west-2a"),
            Want::NotRegistrable,
        ),
        (
            "a publicly routable address with no zone",
            "8.8.4.4",
            None,
            Want::NotRegistrable,
        ),
        (
            "a publicly routable address of the documentation range, with no zone",
            "203.0.113.4",
            None,
            Want::NotRegistrable,
        ),
        (
            "a unique-local IPv6 address, which is not a publicly routable one",
            "fd00:1234::5",
            Some("us-west-2a"),
            Want::Zone("us-west-2a"),
        ),
        (
            "a publicly routable IPv6 address with a zone",
            "2001:4860:4860::8888",
            Some("us-west-2a"),
            Want::NotRegistrable,
        ),
        (
            "an RFC 6598 address with no zone",
            "100.64.3.4",
            None,
            Want::ZoneRequired,
        ),
    ];
    for (case, value, requested, want) in rows {
        let resolved = resolve_ip_target(address(value), requested, &cidrs, &subnets);
        match want {
            Want::Zone(zone) => assert_eq!(
                resolved_as_refs(&resolved),
                Ok(Some(zone)),
                "case {case:?}: expected the zone the request named, got {resolved:?}"
            ),
            // The class a registration earns is decided by the ADDRESS, so each refusal is
            // pinned to its own sentence: an address the group may carry is asked for the
            // zone that is to serve it, and one it may not is refused for being that
            // whether or not the request named a zone.
            Want::ZoneRequired => assert_eq!(
                resolved_as_refs(&resolved),
                Err(availability_zone_required_message(address(value)).as_str()),
                "case {case:?}: expected the Availability Zone to be required for this \
                 address, got {resolved:?}"
            ),
            Want::NotRegistrable => assert_eq!(
                resolved_as_refs(&resolved),
                Err(address_not_allowed_message(address(value)).as_str()),
                "case {case:?}: expected the address itself to be refused, got {resolved:?}"
            ),
        }
    }
}

#[test]
fn a_target_of_an_ip_group_is_an_address_of_the_groups_own_address_type() {
    // `TargetGroup$IpAddressType` decides which version a target may be: "the IP address
    // type. The default value is ipv4". A value of the other version, or no address at
    // all, names no target of the group.
    let rows: [(&str, &str, Option<&str>, bool); 8] = [
        ("an IPv4 address of an ipv4 group", "10.0.0.1", None, true),
        (
            "an IPv4 address of an explicitly ipv4 group",
            "10.0.0.2",
            Some("ipv4"),
            true,
        ),
        (
            "an IPv6 address of an ipv6 group",
            "fd00::1",
            Some("ipv6"),
            true,
        ),
        (
            "an IPv6 address of an ipv4 group",
            "fd00::2",
            Some("ipv4"),
            false,
        ),
        (
            "an IPv4 address of an ipv6 group",
            "10.0.0.3",
            Some("ipv6"),
            false,
        ),
        ("an instance id", "i-1111111111111111", None, false),
        ("a host name", "target.internal", None, false),
        ("an address with a port", "10.0.0.4:80", None, false),
    ];
    for (case, id, ip_address_type, accepted) in rows {
        let parsed = parse_ip_target(id, ip_address_type);
        assert_eq!(
            parsed.is_ok(),
            accepted,
            "case {case:?}: expected accepted={accepted}, got {parsed:?}"
        );
        if let Err(message) = parsed {
            assert!(
                message.contains(id),
                "case {case:?}: the refusal must name the value it refused, got {message:?}"
            );
        }
    }
}

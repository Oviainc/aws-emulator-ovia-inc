// SPDX-License-Identifier: CC0-1.0

//! What a replication subnet group reports ABOUT ITSELF: its own handle, and the IP
//! addressing protocols the subnets it holds support.
//!
//! The rules under test:
//!
//! 1. A group's handle is a DMS ARN over the `subgrp` resource type: the partition and
//!    namespace, the Region the request was signed for, the emulator's account, and the
//!    identifier the group is held under — which is the caller's folded to lower case
//!    ("This value is stored as a lowercase string"). It is a pure function of those, so
//!    re-creating a group under the same identifier reports the same ARN, and every
//!    reading of a group reports the one its create answered.
//! 2. `SupportedNetworkTypes` reports "the IP addressing protocol supported by the subnet
//!    group ... with values such as IPv4 only or Dual-stack that supports both IPv4 and
//!    IPv6 addressing. IPv6 only is not yet supported"
//!    (`ReplicationSubnetGroup$SupportedNetworkTypes`), which follows from the address
//!    families EC2 reports for the group's OWN subnets: `IPV4` when every subnet carries
//!    an IPv4 range, and `DUAL` as well when every subnet also holds an IPv6 range.
//!
//! The rows span what the rules generalise over: the case and alphabet of the identifier
//! and its length (the longest the member doc allows included), and how many of a group's
//! subnets carry an IPv6 range (none, some, all) over two and three subnets in two and
//! three zones. Nothing here reaches into either store: a subnet is dual-stack only
//! because EC2 associated an IPv6 block with it, and a group reports what DMS answered.

use std::net::Ipv6Addr;

use serde_json::{json, Value};
use service_com_amazonaws_databasemigrationservice_amazondmsv20160101::state::SUBNET_GROUP_IDENTIFIER_MAX_LEN;

use crate::migration_task_slice::{text, Emulator, ACCOUNT, REGION};
use crate::subnet_group_placement::{create_subnet, create_vpc, delete, Subnet};

/// The network type DMS reports for IPv4-only addressing. The value list lives on the one
/// member of this model that publishes it: "A value of IPV4 represents an instance profile
/// with IPv4 network type and only supports IPv4 addressing ... A value of DUAL represents
/// an instance profile with dual network type that supports IPv4 and IPv6 addressing"
/// (`CreateInstanceProfile$NetworkType`).
const IPV4: &str = "IPV4";

/// The network type DMS reports for subnets that carry both an IPv4 and an IPv6 range
/// (same member doc).
const DUAL: &str = "DUAL";

/// The IPv4 CIDR of the VPC every subnet below lives in. A group reports ONE VpcId, so
/// every row's subnets have to share a VPC; each subnet takes a /24 out of this /16.
const VPC_CIDR: &str = "10.40.0.0/16";

/// How many zones the fixture spreads subnets over, so a row can name two or three
/// subnets in as many zones.
const FIXTURE_ZONES: usize = 3;

/// The account's network: one VPC with an Amazon-provided IPv6 block, and one IPv4-only
/// and one dual-stack subnet in each of [`FIXTURE_ZONES`] zones.
struct Network {
    /// One IPv4-only subnet per zone, in zone order.
    ipv4_only: Vec<Subnet>,
    /// One dual-stack subnet per zone, in zone order: the same subnets with an IPv6 range
    /// of the VPC's associated with them.
    dual: Vec<Subnet>,
}

/// The `/64` block at `index` of the VPC's Amazon-provided `/56`, which is the size a
/// subnet's IPv6 block has ("an Amazon-provided IPv6 CIDR block with a /56 prefix length
/// for the VPC", `CreateVpcRequest$AmazonProvidedIpv6CidrBlock`; `AssociateSubnetCidrBlock`
/// takes the subnet's /64 out of it).
fn subnet_ipv6_block(vpc_block: &str, index: u16) -> String {
    let base: Ipv6Addr = vpc_block
        .split('/')
        .next()
        .unwrap_or_default()
        .parse()
        .unwrap_or_else(|error| {
            panic!("setup: EC2's VPC block {vpc_block:?} must be an IPv6 address: {error:?}")
        });
    let mut groups = base.segments();
    // The /56 fixes everything above the low byte of the fourth group, so that byte is
    // the subnet selector inside it.
    groups[3] |= index & 0xff;
    format!("{}/64", Ipv6Addr::from(groups))
}

/// Create the network the rows are built from, through EC2's own operations.
async fn create_network(ec2: &::aws_sdk_ec2::Client) -> Network {
    let zones: Vec<String> = ec2
        .describe_availability_zones()
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must report {REGION}'s zones: {error:?}"))
        .availability_zones()
        .iter()
        .filter_map(|zone| zone.zone_name().map(str::to_string))
        .collect();
    assert!(
        zones.len() >= FIXTURE_ZONES,
        "setup: expected {REGION} to have at least {FIXTURE_ZONES} zones, got {zones:?}"
    );

    let vpc = create_vpc(ec2, VPC_CIDR).await;
    // The IPv6 space a dual-stack subnet's range is cut out of: a subnet's block has to
    // sit inside one its VPC holds, so the VPC takes an Amazon-provided one first.
    let vpc_ipv6 = ec2
        .associate_vpc_cidr_block()
        .vpc_id(&vpc)
        .amazon_provided_ipv6_cidr_block(true)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("setup: EC2 must give {vpc} an Amazon-provided IPv6 block: {error:?}")
        })
        .ipv6_cidr_block_association()
        .and_then(|association| association.ipv6_cidr_block().map(str::to_string))
        .unwrap_or_else(|| panic!("setup: EC2 must report the IPv6 block it gave {vpc}"));

    let mut ipv4_only: Vec<Subnet> = Vec::with_capacity(FIXTURE_ZONES);
    let mut dual: Vec<Subnet> = Vec::with_capacity(FIXTURE_ZONES);
    for (index, zone) in zones.iter().take(FIXTURE_ZONES).enumerate() {
        ipv4_only.push(create_subnet(ec2, &vpc, &format!("10.40.{index}.0/24"), zone).await);

        let subnet = create_subnet(ec2, &vpc, &format!("10.40.{}.0/24", index + 10), zone).await;
        let block = subnet_ipv6_block(&vpc_ipv6, u16::try_from(index + 1).unwrap_or(1));
        ec2.associate_subnet_cidr_block()
            .subnet_id(&subnet.id)
            .ipv6_cidr_block(&block)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "setup: EC2 must associate {block} with the subnet {}: {error:?}",
                    subnet.id
                )
            });
        dual.push(subnet);
    }

    Network { ipv4_only, dual }
}

/// The `SupportedNetworkTypes` a group reports, or a marker naming their absence.
fn network_types(group: &Value) -> Vec<String> {
    match group.pointer("/SupportedNetworkTypes").and_then(Value::as_array) {
        Some(values) => values
            .iter()
            .map(|value| value.as_str().unwrap_or("<not a string>").to_string())
            .collect(),
        None => vec!["<missing>".to_string()],
    }
}

/// Create a group over `subnet_ids`, asserting a 200, and answer the group it reports.
async fn create_group(
    emulator: &Emulator,
    case: &str,
    identifier: &str,
    subnet_ids: &[String],
) -> Value {
    emulator
        .ok(
            case,
            "CreateReplicationSubnetGroup",
            &json!({
                "ReplicationSubnetGroupIdentifier": identifier,
                "ReplicationSubnetGroupDescription": format!("subnets for {case}"),
                "SubnetIds": subnet_ids,
            }),
        )
        .await
        .pointer("/ReplicationSubnetGroup")
        .cloned()
        .unwrap_or(Value::Null)
}

/// One identifier the ARN rule is asserted over: `(case, identifier)`.
///
/// The alphabet and the length are the ones the member doc publishes: "Must contain no
/// more than 255 alphanumeric characters, periods, underscores, or hyphens ... Example:
/// mySubnetgroup" (`CreateReplicationSubnetGroup$ReplicationSubnetGroupIdentifier`).
fn identifier_rows() -> Vec<(&'static str, String)> {
    vec![
        ("the example the member doc gives", "mySubnetgroup".to_string()),
        ("already lower case", "sng-handle".to_string()),
        ("periods and underscores", "sng.one_two-3".to_string()),
        ("upper case throughout", "SNG-HANDLE-2".to_string()),
        ("a single character", "a".to_string()),
        (
            "the longest identifier the doc allows",
            "s".repeat(SUBNET_GROUP_IDENTIFIER_MAX_LEN),
        ),
    ]
}

#[tokio::test]
async fn a_subnet_group_reports_a_subgrp_arn_over_its_region_account_and_identifier() {
    let (emulator, ec2, _iam) = Emulator::with_network();
    let network = create_network(&ec2).await;
    let subnet_ids: Vec<String> = network
        .ipv4_only
        .iter()
        .take(2)
        .map(|subnet| subnet.id.clone())
        .collect();

    for (case, identifier) in identifier_rows() {
        let group = create_group(&emulator, case, &identifier, &subnet_ids).await;
        let expected = format!(
            "arn:aws:dms:{REGION}:{ACCOUNT}:subgrp:{}",
            identifier.to_lowercase()
        );
        let reported = text(&group, "/ReplicationSubnetGroupArn");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the group's handle to be the subgrp ARN over the \
             Region, the account and the stored identifier, got {reported:?}"
        );

        // The ARN is derived, not minted per call: the group deleted and created again
        // under the same identifier carries the same handle.
        let (deleted, code) = delete(&emulator, &identifier).await;
        assert_eq!(
            deleted, 200,
            "case {case:?}: expected the group to be deletable, got {deleted} {code}"
        );
        let again = create_group(&emulator, case, &identifier, &subnet_ids).await;
        let re_reported = text(&again, "/ReplicationSubnetGroupArn");
        assert_eq!(
            re_reported, expected,
            "case {case:?}: expected a re-created group to carry the same handle, got \
             {re_reported:?}"
        );
        let (deleted, code) = delete(&emulator, &identifier).await;
        assert_eq!(
            deleted, 200,
            "case {case:?}: expected the re-created group to be deletable, got \
             {deleted} {code}"
        );
    }
}

/// One subnet list the network-type rule is asserted over.
struct Row {
    /// What this row varies.
    case: &'static str,
    /// The subnets the create names.
    named: Vec<String>,
    /// The protocols the group must report for them.
    expected: Vec<&'static str>,
}

/// Every row, built from the network EC2 answered for.
fn rows(network: &Network) -> Vec<Row> {
    let id = |subnet: &Subnet| subnet.id.clone();
    vec![
        Row {
            case: "two IPv4-only subnets in two zones",
            named: vec![id(&network.ipv4_only[0]), id(&network.ipv4_only[1])],
            expected: vec![IPV4],
        },
        Row {
            case: "three IPv4-only subnets in three zones",
            named: network.ipv4_only.iter().map(id).collect(),
            expected: vec![IPV4],
        },
        Row {
            case: "two dual-stack subnets in two zones",
            named: vec![id(&network.dual[0]), id(&network.dual[1])],
            expected: vec![IPV4, DUAL],
        },
        Row {
            case: "the same two dual-stack subnets, named in the other order",
            named: vec![id(&network.dual[1]), id(&network.dual[0])],
            expected: vec![IPV4, DUAL],
        },
        Row {
            case: "three dual-stack subnets in three zones",
            named: network.dual.iter().map(id).collect(),
            expected: vec![IPV4, DUAL],
        },
        Row {
            case: "one dual-stack and one IPv4-only subnet",
            named: vec![id(&network.dual[0]), id(&network.ipv4_only[1])],
            expected: vec![IPV4],
        },
        Row {
            case: "two dual-stack subnets and one IPv4-only",
            named: vec![
                id(&network.dual[0]),
                id(&network.dual[1]),
                id(&network.ipv4_only[2]),
            ],
            expected: vec![IPV4],
        },
    ]
}

#[tokio::test]
async fn supported_network_types_follow_the_address_families_of_a_group_subnets() {
    let (emulator, ec2, _iam) = Emulator::with_network();
    let network = create_network(&ec2).await;

    for (index, row) in rows(&network).into_iter().enumerate() {
        let identifier = format!("sng-network-{index}");
        let group = create_group(&emulator, row.case, &identifier, &row.named).await;
        let reported = network_types(&group);
        assert_eq!(
            reported, row.expected,
            "case {:?}: expected the protocols the group's subnets support, got \
             {reported:?}",
            row.case
        );
        let (deleted, code) = delete(&emulator, &identifier).await;
        assert_eq!(
            deleted, 200,
            "case {:?}: expected the group to be deletable, got {deleted} {code}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_group_reports_the_same_handle_and_protocols_wherever_it_is_read_back() {
    let (emulator, ec2, _iam) = Emulator::with_network();
    let network = create_network(&ec2).await;
    let subnet_ids: Vec<String> = network
        .dual
        .iter()
        .take(2)
        .map(|subnet| subnet.id.clone())
        .collect();

    let created = create_group(&emulator, "read back", "round-trip-handle", &subnet_ids).await;
    assert_eq!(
        network_types(&created),
        vec![IPV4, DUAL],
        "expected a group of dual-stack subnets to support both protocols, got {created}"
    );

    // The ROUND TRIP: a replication instance placed in the group reports the group, read
    // back through the implemented describe.
    emulator
        .ok(
            "read back",
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": "round-trip-handle-instance",
                "ReplicationInstanceClass": "dms.t3.medium",
                "ReplicationSubnetGroupIdentifier": "ROUND-TRIP-HANDLE",
            }),
        )
        .await;
    let described = emulator
        .ok("read back", "DescribeReplicationInstances", &json!({}))
        .await;
    let read_back = described
        .pointer("/ReplicationInstances/0/ReplicationSubnetGroup")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        text(&read_back, "/ReplicationSubnetGroupArn"),
        text(&created, "/ReplicationSubnetGroupArn"),
        "expected the group read back to carry the handle its create answered, got \
         {read_back}"
    );
    assert_eq!(
        network_types(&read_back),
        network_types(&created),
        "expected the group read back to report the same protocols, got {read_back}"
    );
}

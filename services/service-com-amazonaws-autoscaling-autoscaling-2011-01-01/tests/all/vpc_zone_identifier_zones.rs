// SPDX-License-Identifier: CC0-1.0

//! A group that names only `VPCZoneIdentifier` gets its Availability Zones from the
//! subnets it names.
//!
//! `VPCZoneIdentifier` is "a comma-separated list of subnet IDs for a virtual private
//! cloud (VPC) where instances in the Auto Scaling group can be created"
//! (`smithy_query(query="doc",shape="CreateAutoScalingGroupType",member="VPCZoneIdentifier")`),
//! and a subnet exists in exactly one Availability Zone. So the zones of a group are the
//! zones of its subnets, deduplicated — a fact of the account's own Amazon EC2 subnets,
//! not a value the caller has to supply alongside them — and the create is served, its
//! instances placed in those zones. A subnet the account does not hold has no zone to
//! place anything in, so naming one is refused.
//!
//! The table below spans what that rule generalises over: how many subnets are named, how
//! many distinct zones they are in, the order they are named in, repetition, spacing, and
//! whether the caller names zones as well. Each row is checked over the awsQuery wire
//! through one router over one shared engine, the way the emulator serves it.

use std::sync::Arc;

use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{
    new_engine, query, router_with, seed_subnets, tag_value, tag_values, SeededSubnet, REGION,
};

/// The launch source every group here is created from — the same launch template the
/// other suites use, because no account can create a launch configuration.
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The `<AvailabilityZones>` body a group with `zones` reports.
fn zones_xml(zones: &[String]) -> String {
    zones
        .iter()
        .map(|zone| format!("<member>{zone}</member>"))
        .collect()
}

/// Seed one subnet in the first Availability Zone of `region`, through EC2's own writers.
///
/// Used to put a subnet in a Region other than the one these requests are signed for: a
/// subnet is identified within its Region, so that subnet is not one this group could
/// launch into.
fn seed_subnet_in_region(engine: &Arc<engine::InMemoryEngine>, region: &str) -> SeededSubnet {
    let zones = ec2::availability_zones_in_region(region);
    let zone = zones
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the Region {region} must be one of EC2's own Regions"));
    let state = ec2::service_state(&**engine).expect("the EC2 store is mounted on this engine");
    let mut store = state.lock();
    let vpc_id = store.insert_vpc(
        ec2::DEFAULT_VPC_CIDR_BLOCK,
        false,
        ec2::DEFAULT_INSTANCE_TENANCY,
        ec2::VPC_STATE_AVAILABLE,
        region,
        Vec::new(),
    );
    let id = store.insert_subnet(ec2::SubnetSpec {
        vpc_id,
        cidr_block: ec2::DEFAULT_VPC_CIDR_BLOCK.to_string(),
        availability_zone: zone.zone_name.clone(),
        availability_zone_id: zone.zone_id,
        region: region.to_string(),
        state: ec2::SUBNET_STATE_AVAILABLE.to_string(),
        default_for_az: false,
        map_public_ip_on_launch: true,
        tags: Vec::new(),
    });
    SeededSubnet {
        id,
        zone: zone.zone_name,
    }
}

/// One row of the derivation table: which seeded subnets the request names (by index,
/// with the exact `VPCZoneIdentifier` text built from them), and which of their zones the
/// created group must report, in order.
struct DerivationCase {
    case: &'static str,
    /// The indexes into the seeded subnets, in the order `VPCZoneIdentifier` names them.
    subnets: Vec<usize>,
    /// Rendered between the ids, so a row can carry the spacing a caller wrote.
    separator: &'static str,
    /// The indexes whose zones the group must report, in order — the named subnets'
    /// zones, deduplicated.
    zones_of: Vec<usize>,
}

/// A group created with `VPCZoneIdentifier` and no `AvailabilityZones` is SERVED, and
/// reports the zones of the subnets it named.
#[tokio::test]
async fn zones_come_from_the_named_subnets() {
    let engine = new_engine();
    let router = router_with(engine.clone());
    // One more subnet than the Region has zones, so the last seeded subnet shares a zone
    // with the first: that is what the duplicate-zone rows below name.
    let zone_count = ec2::availability_zones_in_region(REGION).len();
    let subnets = seed_subnets(&engine, zone_count + 1);

    let cases = [
        DerivationCase {
            case: "one subnet: the group is in that subnet's zone",
            subnets: vec![0],
            separator: ",",
            zones_of: vec![0],
        },
        DerivationCase {
            case: "two subnets in different zones: both zones, in the order named",
            subnets: vec![0, 1],
            separator: ",",
            zones_of: vec![0, 1],
        },
        DerivationCase {
            case: "the same two subnets named in the other order: that order",
            subnets: vec![1, 0],
            separator: ",",
            zones_of: vec![1, 0],
        },
        DerivationCase {
            case: "three subnets, two of them in one zone: that zone reported once",
            subnets: vec![0, 1, zone_count],
            separator: ",",
            zones_of: vec![0, 1],
        },
        DerivationCase {
            case: "one subnet named twice: its zone reported once",
            subnets: vec![2, 2],
            separator: ",",
            zones_of: vec![2],
        },
        DerivationCase {
            case: "spaces around the commas: the ids are still the subnets named",
            subnets: vec![0, 1],
            separator: " , ",
            zones_of: vec![0, 1],
        },
        DerivationCase {
            case: "every zone of the Region, one subnet each",
            subnets: (0..zone_count).collect(),
            separator: ",",
            zones_of: (0..zone_count).collect(),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = format!("derived-{index}");
        let vpc_zone_identifier = case
            .subnets
            .iter()
            .map(|subnet| subnets[*subnet].id.as_str())
            .collect::<Vec<_>>()
            .join(case.separator);
        let expected_zones: Vec<String> = case
            .zones_of
            .iter()
            .map(|subnet| subnets[*subnet].zone.clone())
            .collect();

        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "1"),
                ("MaxSize", "4"),
                ("DesiredCapacity", "2"),
                ("VPCZoneIdentifier", vpc_zone_identifier.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateAutoScalingGroup expected 200, got {status}: {body}",
            case.case
        );

        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeAutoScalingGroups expected 200, got {status}: {body}",
            case.case
        );
        assert_eq!(
            tag_value(&body, "AvailabilityZones"),
            zones_xml(&expected_zones),
            "case {:?}: expected the zones of the named subnets {expected_zones:?}, got {body}",
            case.case
        );
        assert_eq!(
            tag_value(&body, "VPCZoneIdentifier"),
            vpc_zone_identifier,
            "case {:?}: the group reports the subnets it was given, got {body}",
            case.case
        );
        // The capacity the group asked for is placed, and every instance is in one of
        // the zones the subnets resolved to — the zone an instance runs in.
        let instance_zones = tag_values(&body, "AvailabilityZone");
        assert_eq!(
            instance_zones.len(),
            2,
            "case {:?}: expected the 2 instances the group asked for, got {body}",
            case.case
        );
        for zone in &instance_zones {
            assert!(
                expected_zones.contains(zone),
                "case {:?}: an instance in {zone}, which is not one of {expected_zones:?}: {body}",
                case.case
            );
        }
        assert_eq!(
            tag_value(&body, "DesiredCapacity"),
            "2",
            "case {:?}: the group holds the capacity it asked for, got {body}",
            case.case
        );
    }
}

/// One row of the refusal table: the `VPCZoneIdentifier` text, and the subnet id the
/// refusal must name.
struct UnknownSubnetCase {
    case: &'static str,
    /// Built from the seeded subnets plus the unknown id, so a row can place the unknown
    /// id first, last or alone.
    identifier: String,
    unknown: String,
}

/// A subnet the account cannot launch into — one it does not hold, or one it holds in
/// another Region — has no zone to place an instance in, so naming it is refused: with the
/// error code this service answers an unsatisfiable request with, naming the id, and
/// leaving no group behind.
#[tokio::test]
async fn an_unknown_subnet_is_refused() {
    let engine = new_engine();
    let router = router_with(engine.clone());
    let subnets = seed_subnets(&engine, 2);
    // A Region of EC2's own catalog other than the one these requests are signed for.
    let other_region = "us-east-1";
    assert_ne!(
        other_region, REGION,
        "the other-Region row needs a Region other than the signed one"
    );
    let elsewhere = seed_subnet_in_region(&engine, other_region);

    // Ids no `insert_subnet` minted: the long and the short modelled subnet-id forms, and
    // a value that is not a subnet id at all.
    let unknown_ids = [
        "subnet-00000000000000000",
        "subnet-0abc1234",
        "not-a-subnet-id",
    ];
    let mut cases: Vec<UnknownSubnetCase> = vec![
        UnknownSubnetCase {
            case: "the only subnet named is unknown",
            identifier: unknown_ids[0].to_string(),
            unknown: unknown_ids[0].to_string(),
        },
        UnknownSubnetCase {
            case: "an unknown subnet after one the account holds",
            identifier: format!("{},{}", subnets[0].id, unknown_ids[1]),
            unknown: unknown_ids[1].to_string(),
        },
        UnknownSubnetCase {
            case: "an unknown subnet before one the account holds",
            identifier: format!("{},{}", unknown_ids[2], subnets[1].id),
            unknown: unknown_ids[2].to_string(),
        },
        UnknownSubnetCase {
            case: "a subnet the account holds in another Region",
            identifier: elsewhere.id.clone(),
            unknown: elsewhere.id.clone(),
        },
        UnknownSubnetCase {
            case: "a subnet of this Region beside one of another Region",
            identifier: format!("{},{}", subnets[0].id, elsewhere.id),
            unknown: elsewhere.id.clone(),
        },
    ];
    cases.push(UnknownSubnetCase {
        case: "a trailing comma names no subnet, so nothing is placed anywhere",
        identifier: ",".to_string(),
        unknown: String::new(),
    });

    for (index, case) in cases.iter().enumerate() {
        let name = format!("unknown-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("VPCZoneIdentifier", case.identifier.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status}: {body}",
            case.case
        );
        assert!(
            body.contains("<Code>ValidationError</Code>"),
            "case {:?}: expected the ValidationError code, got {body}",
            case.case
        );
        if !case.unknown.is_empty() {
            assert!(
                body.contains(&case.unknown),
                "case {:?}: the refusal must name {}, got {body}",
                case.case,
                case.unknown
            );
        }

        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert!(
            tag_values(&body, "AutoScalingGroupName").is_empty(),
            "case {:?}: a refused create must leave no group behind, got {body}",
            case.case
        );
    }
}

/// The zones a request names are the group's zones: a request that supplies
/// `AvailabilityZones` reports those, whether or not it also names subnets, and an update
/// that names new subnets alone moves the group to the zones those subnets are in.
#[tokio::test]
async fn named_zones_win_and_an_update_moves_the_group() {
    let engine = new_engine();
    let router = router_with(engine.clone());
    let subnets = seed_subnets(&engine, 2);

    // Both members: the zones are the ones the caller named.
    let named_zone = subnets[1].zone.clone();
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "both-members"),
            LAUNCH_SOURCE,
            ("MinSize", "1"),
            ("MaxSize", "2"),
            ("AvailabilityZones.member.1", named_zone.as_str()),
            ("VPCZoneIdentifier", subnets[0].id.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "a create naming both members: got {status}: {body}");
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "both-members")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "AvailabilityZones"),
        zones_xml(std::slice::from_ref(&named_zone)),
        "the zones a request names are the group's zones, got {body}"
    );

    // An update that names only new subnets moves the group to their zones.
    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "both-members"),
            ("VPCZoneIdentifier", subnets[0].id.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "an update naming only subnets: got {status}: {body}");
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "both-members")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "AvailabilityZones"),
        zones_xml(std::slice::from_ref(&subnets[0].zone)),
        "the update moves the group to the zone of the subnet it named, got {body}"
    );

    // And an update naming a subnet the account does not hold is refused, leaving the
    // group's zones alone.
    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "both-members"),
            ("VPCZoneIdentifier", "subnet-00000000000000000"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "an update naming an unknown subnet: got {status}: {body}"
    );
    assert!(
        body.contains("<Code>ValidationError</Code>"),
        "an update naming an unknown subnet must answer ValidationError, got {body}"
    );
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "both-members")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "AvailabilityZones"),
        zones_xml(std::slice::from_ref(&subnets[0].zone)),
        "a refused update leaves the group's zones alone, got {body}"
    );
}

/// A capacity change on a group that named only subnets is served: the group has zones,
/// so the instances it asks for are placed in them.
#[tokio::test]
async fn capacity_changes_on_a_subnets_only_group_are_served() {
    let engine = new_engine();
    let router = router_with(engine.clone());
    let subnets = seed_subnets(&engine, 2);
    let identifier = format!("{},{}", subnets[0].id, subnets[1].id);

    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "subnets-only"),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "4"),
            ("VPCZoneIdentifier", identifier.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "a group that names only subnets: got {status}: {body}");

    // (case, action, params, the capacity the group holds afterwards)
    type CapacityCase = (&'static str, &'static str, Vec<(&'static str, &'static str)>, &'static str);
    let cases: Vec<CapacityCase> = vec![
        (
            "SetDesiredCapacity",
            "SetDesiredCapacity",
            vec![
                ("AutoScalingGroupName", "subnets-only"),
                ("DesiredCapacity", "2"),
            ],
            "2",
        ),
        (
            "an update that raises the minimum",
            "UpdateAutoScalingGroup",
            vec![("AutoScalingGroupName", "subnets-only"), ("MinSize", "3")],
            "3",
        ),
    ];

    for (case, action, params, expected_capacity) in cases {
        let (status, body) = query(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );

        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", "subnets-only")],
        )
        .await;
        assert_eq!(
            tag_value(&body, "DesiredCapacity"),
            expected_capacity,
            "case {case:?}: expected a desired capacity of {expected_capacity}, got {body}"
        );
        let instance_zones = tag_values(&body, "AvailabilityZone");
        assert_eq!(
            instance_zones.len(),
            expected_capacity
                .parse::<usize>()
                .expect("the expected capacity is a count"),
            "case {case:?}: every unit of capacity is a placed instance, got {body}"
        );
        for zone in &instance_zones {
            assert!(
                [&subnets[0].zone, &subnets[1].zone].contains(&zone),
                "case {case:?}: an instance in {zone}, not a zone of the group's subnets: {body}"
            );
        }
    }
}

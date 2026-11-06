// SPDX-License-Identifier: CC0-1.0

//! `CreateReplicationSubnetGroup` and `DeleteReplicationSubnetGroup`: which subnet lists
//! DMS will hold a replication subnet group for, and what the group then reports.
//!
//! The rules under test:
//!
//! 1. A subnet a request names is EC2's resource, so it is resolved against the account's
//!    EC2 network before anything is persisted: a value no subnet of the account carries
//!    is refused with `InvalidSubnet`, and the VPC and each subnet's Availability Zone a
//!    group reports come from that resolution rather than from the request.
//! 2. A group is accepted exactly when its subnets cover at least TWO distinct
//!    Availability Zones — "The VPC needs to have at least one subnet in at least two
//!    availability zones in the Amazon Web Services Region, otherwise the service will
//!    throw a ReplicationSubnetGroupDoesNotCoverEnoughAZs exception"
//!    (`CreateReplicationSubnetGroup`'s own doc) — and a refused create persists nothing,
//!    so the delete of that identifier answers `ResourceNotFoundFault`.
//! 3. The identifier is held under the constraints its member doc publishes, folded to
//!    lower case ("This value is stored as a lowercase string").
//!
//! The rows span what the rules generalise over: how many subnets a request names, how
//! those subnets are distributed over zones (one zone, two subnets of one zone, two
//! zones, three zones, three subnets over two zones), where a named value fails to
//! resolve (an unused id, a value that is no subnet id at all), and whether the subnets
//! share a VPC. Nothing here reaches into either store: a subnet exists only because EC2
//! answered for it, and a group only because a later DMS call reported or resolved it.

use serde_json::{json, Value};
use service_com_amazonaws_databasemigrationservice_amazondmsv20160101::state::{
    MIN_SUBNET_GROUP_AVAILABILITY_ZONES, SUBNET_GROUP_IDENTIFIER_MAX_LEN,
};

use crate::migration_task_slice::{error_code, text, Emulator, REGION};

/// The error code DMS answers a subnet group that does not span enough zones with, as the
/// model declares the shape.
const NOT_ENOUGH_AZS: &str = "ReplicationSubnetGroupDoesNotCoverEnoughAZs";

/// The error code DMS answers a subnet it cannot use with.
const INVALID_SUBNET: &str = "InvalidSubnet";

/// The error code DMS answers a resource it does not hold with.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundFault";

/// The error code the framework answers a request that breaks a rule the model does not
/// express with.
const VALIDATION: &str = "ValidationException";

/// The status a replication subnet group reports once every subnet in it is usable.
const COMPLETE: &str = "Complete";

/// The CIDR of the VPC the fixture's subnets live in, and of the second VPC one row
/// crosses into. Each subnet below takes a /24 out of its VPC's /16.
const VPC_CIDR: &str = "10.32.0.0/16";
const OTHER_VPC_CIDR: &str = "10.33.0.0/16";

/// One subnet of the account, as EC2 answered for it.
///
/// Shared with the sibling suites in this target, so a subnet is created through EC2 in
/// exactly one place.
#[derive(Clone, Debug)]
pub(crate) struct Subnet {
    /// The id EC2 minted.
    pub(crate) id: String,
    /// The Availability Zone EC2 placed it in.
    pub(crate) zone: String,
}

/// The account's network, created through EC2 itself.
struct Network {
    /// The VPC all but one of the subnets below belong to.
    vpc: String,
    /// A second VPC, for the row whose subnets do not share one.
    other_vpc: String,
    /// The Availability Zones of [`REGION`], as EC2 reports them.
    zones: Vec<String>,
    /// One subnet per zone in `vpc`, in zone order.
    per_zone: Vec<Subnet>,
    /// A SECOND subnet in the first zone, so a two-subnet list can still cover one zone.
    twin_in_first_zone: Subnet,
    /// A subnet of `other_vpc`, in the second zone.
    in_other_vpc: Subnet,
}

/// How many zones the fixture spreads subnets over: enough to drive a list that covers
/// one, two and three of them.
const FIXTURE_ZONES: usize = 3;

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
    let other_vpc = create_vpc(ec2, OTHER_VPC_CIDR).await;

    let mut per_zone: Vec<Subnet> = Vec::with_capacity(FIXTURE_ZONES);
    for (index, zone) in zones.iter().take(FIXTURE_ZONES).enumerate() {
        per_zone.push(create_subnet(ec2, &vpc, &format!("10.32.{index}.0/24"), zone).await);
    }
    let twin_in_first_zone =
        create_subnet(ec2, &vpc, "10.32.100.0/24", &zones[0]).await;
    let in_other_vpc = create_subnet(ec2, &other_vpc, "10.33.0.0/24", &zones[1]).await;

    Network {
        vpc,
        other_vpc,
        zones,
        per_zone,
        twin_in_first_zone,
        in_other_vpc,
    }
}

/// A VPC of `cidr`, and the id EC2 minted for it.
pub(crate) async fn create_vpc(ec2: &::aws_sdk_ec2::Client, cidr: &str) -> String {
    ec2.create_vpc()
        .cidr_block(cidr)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must create the VPC {cidr}: {error:?}"))
        .vpc()
        .and_then(|vpc| vpc.vpc_id().map(str::to_string))
        .unwrap_or_else(|| panic!("setup: EC2 must report a VpcId for {cidr}"))
}

/// A subnet of `cidr` in `zone` of `vpc`, as EC2 answered for it.
pub(crate) async fn create_subnet(
    ec2: &::aws_sdk_ec2::Client,
    vpc: &str,
    cidr: &str,
    zone: &str,
) -> Subnet {
    let created = ec2
        .create_subnet()
        .vpc_id(vpc)
        .cidr_block(cidr)
        .availability_zone(zone)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("setup: EC2 must create the subnet {cidr} in {zone}: {error:?}")
        });
    let subnet = created
        .subnet()
        .unwrap_or_else(|| panic!("setup: EC2 must report the subnet {cidr}"));
    Subnet {
        id: subnet
            .subnet_id()
            .unwrap_or_else(|| panic!("setup: EC2 must report a SubnetId for {cidr}"))
            .to_string(),
        zone: subnet
            .availability_zone()
            .unwrap_or_else(|| panic!("setup: EC2 must report a zone for {cidr}"))
            .to_string(),
    }
}

/// What a row expects the create to answer.
enum Expect {
    /// HTTP 200, with a group over these subnets.
    Accepted,
    /// HTTP 400 with this error code, whose message names each of these values — the
    /// value the request named that could not be placed.
    Refused(&'static str, Vec<String>),
}

/// One subnet list the rules are asserted over.
struct Row {
    /// What this row varies.
    case: &'static str,
    /// The subnets the create names, as `(id, zone)` — a zone of `None` for a value no
    /// subnet of the account carries.
    named: Vec<(String, Option<String>)>,
    /// What the create must answer.
    expect: Expect,
}

/// Every row, built from the network EC2 answered for.
fn rows(network: &Network) -> Vec<Row> {
    let zone_of = |subnet: &Subnet| (subnet.id.clone(), Some(subnet.zone.clone()));
    let first = &network.per_zone[0];
    let second = &network.per_zone[1];
    let third = &network.per_zone[2];
    vec![
        Row {
            case: "no subnets at all, so no zone",
            named: Vec::new(),
            expect: Expect::Refused(NOT_ENOUGH_AZS, Vec::new()),
        },
        Row {
            case: "one subnet, so one zone",
            named: vec![zone_of(first)],
            expect: Expect::Refused(NOT_ENOUGH_AZS, Vec::new()),
        },
        Row {
            case: "two subnets of the SAME zone, so still one zone",
            named: vec![zone_of(first), zone_of(&network.twin_in_first_zone)],
            expect: Expect::Refused(NOT_ENOUGH_AZS, Vec::new()),
        },
        Row {
            case: "two subnets in two zones",
            named: vec![zone_of(first), zone_of(second)],
            expect: Expect::Accepted,
        },
        Row {
            case: "the same two zones, named in the other order",
            named: vec![zone_of(second), zone_of(first)],
            expect: Expect::Accepted,
        },
        Row {
            case: "three subnets in three zones",
            named: vec![zone_of(first), zone_of(second), zone_of(third)],
            expect: Expect::Accepted,
        },
        Row {
            case: "three subnets over two zones",
            named: vec![
                zone_of(first),
                zone_of(&network.twin_in_first_zone),
                zone_of(second),
            ],
            expect: Expect::Accepted,
        },
        Row {
            case: "an id no subnet of the account carries",
            named: vec![
                zone_of(first),
                ("subnet-0ffffffffffffffff".to_string(), None),
            ],
            expect: Expect::Refused(
                INVALID_SUBNET,
                vec!["subnet-0ffffffffffffffff".to_string()],
            ),
        },
        Row {
            case: "a value that is no subnet id at all",
            named: vec![zone_of(first), ("not-a-subnet".to_string(), None)],
            expect: Expect::Refused(INVALID_SUBNET, vec!["not-a-subnet".to_string()]),
        },
        Row {
            case: "two zones, but the subnets belong to two VPCs",
            named: vec![zone_of(first), zone_of(&network.in_other_vpc)],
            // A group reports ONE VpcId, so the refusal names the VPCs the request
            // spread its subnets over.
            expect: Expect::Refused(
                INVALID_SUBNET,
                vec![network.vpc.clone(), network.other_vpc.clone()],
            ),
        },
    ]
}

/// The `(SubnetIdentifier, SubnetAvailabilityZone.Name)` pairs a group reports, sorted so
/// two readings of one group compare regardless of order.
fn reported_subnets(group: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = group
        .pointer("/Subnets")
        .and_then(Value::as_array)
        .map(|subnets| {
            subnets
                .iter()
                .map(|subnet| {
                    (
                        text(subnet, "/SubnetIdentifier"),
                        text(subnet, "/SubnetAvailabilityZone/Name"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// Delete `identifier`, and answer the `(status, error code)` of the attempt.
pub(crate) async fn delete(emulator: &Emulator, identifier: &str) -> (u16, String) {
    let (status, response) = emulator
        .call(
            "DeleteReplicationSubnetGroup",
            &json!({ "ReplicationSubnetGroupIdentifier": identifier }),
        )
        .await;
    (status, error_code(&response))
}

#[tokio::test]
async fn a_subnet_group_is_held_only_for_subnets_that_resolve_and_cover_two_zones() {
    let (emulator, ec2, _iam) = Emulator::with_network();
    let network = create_network(&ec2).await;

    for (index, row) in rows(&network).into_iter().enumerate() {
        let identifier = format!("sng-{index}");
        let description = format!("subnets for {}", row.case);
        let subnet_ids: Vec<String> = row.named.iter().map(|(id, _)| id.clone()).collect();
        let (status, response) = emulator
            .call(
                "CreateReplicationSubnetGroup",
                &json!({
                    "ReplicationSubnetGroupIdentifier": identifier,
                    "ReplicationSubnetGroupDescription": description,
                    "SubnetIds": subnet_ids,
                }),
            )
            .await;

        match row.expect {
            Expect::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the create to be accepted, got {status} {response}",
                    row.case
                );
                let group = response
                    .pointer("/ReplicationSubnetGroup")
                    .unwrap_or(&Value::Null)
                    .clone();
                assert_eq!(
                    text(&group, "/ReplicationSubnetGroupIdentifier"),
                    identifier,
                    "case {:?}: expected the group to report its identifier, got {group}",
                    row.case
                );
                assert_eq!(
                    text(&group, "/ReplicationSubnetGroupDescription"),
                    description,
                    "case {:?}: expected the group to report its description, got {group}",
                    row.case
                );
                // The VPC is EC2's answer for those subnets, not a request member.
                assert_eq!(
                    text(&group, "/VpcId"),
                    network.vpc,
                    "case {:?}: expected the VPC EC2 reports for the subnets, got {group}",
                    row.case
                );
                assert_eq!(
                    text(&group, "/SubnetGroupStatus"),
                    COMPLETE,
                    "case {:?}: expected a group DMS provisions nothing for to be \
                     complete, got {group}",
                    row.case
                );
                let mut expected: Vec<(String, String)> = row
                    .named
                    .iter()
                    .map(|(id, zone)| (id.clone(), zone.clone().unwrap_or_default()))
                    .collect();
                expected.sort();
                let reported = reported_subnets(&group);
                assert_eq!(
                    reported, expected,
                    "case {:?}: expected each subnet with the zone EC2 placed it in, got \
                     {reported:?}",
                    row.case
                );

                // The group was PERSISTED: the delete of it succeeds once, and the
                // identifier is unknown afterwards.
                let (deleted, code) = delete(&emulator, &identifier).await;
                assert_eq!(
                    deleted, 200,
                    "case {:?}: expected the stored group to be deletable, got \
                     {deleted} {code}",
                    row.case
                );
                let (again, code) = delete(&emulator, &identifier).await;
                assert_eq!(
                    (again, code.as_str()),
                    (400, RESOURCE_NOT_FOUND),
                    "case {:?}: expected a deleted group to be unknown, got {again} {code}",
                    row.case
                );
            }
            Expect::Refused(expected_code, named_values) => {
                let code = error_code(&response);
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected_code),
                    "case {:?}: expected the create to be refused with {expected_code}, \
                     got {status} {response}",
                    row.case
                );
                let message = text(&response, "/message");
                if expected_code == NOT_ENOUGH_AZS {
                    // The coverage the request achieved, and the zones the Region has:
                    // both read off the request and EC2's answer rather than fixed text.
                    let covered: Vec<String> = {
                        let mut zones: Vec<String> = row
                            .named
                            .iter()
                            .filter_map(|(_, zone)| zone.clone())
                            .collect();
                        zones.sort();
                        zones.dedup();
                        zones
                    };
                    assert!(
                        message.contains(&format!("Current AZ coverage: {}", covered.len())),
                        "case {:?}: expected the refusal to report the coverage the \
                         request achieved, got {message:?}",
                        row.case
                    );
                    assert!(
                        covered.len() < MIN_SUBNET_GROUP_AVAILABILITY_ZONES,
                        "case {:?}: this row must cover fewer than \
                         {MIN_SUBNET_GROUP_AVAILABILITY_ZONES} zones, got {covered:?}",
                        row.case
                    );
                    for zone in &network.zones {
                        assert!(
                            message.contains(zone.as_str()),
                            "case {:?}: expected the refusal to list the Region's zone \
                             {zone}, got {message:?}",
                            row.case
                        );
                    }
                } else {
                    // An unusable subnet is reported by the values the REQUEST named.
                    for named in &named_values {
                        assert!(
                            message.contains(named.as_str()),
                            "case {:?}: expected the refusal to name {named}, got \
                             {message:?}",
                            row.case
                        );
                    }
                }
                // A refused create persisted nothing.
                let (deleted, code) = delete(&emulator, &identifier).await;
                assert_eq!(
                    (deleted, code.as_str()),
                    (400, RESOURCE_NOT_FOUND),
                    "case {:?}: a refused create must persist nothing, got \
                     {deleted} {code}",
                    row.case
                );
            }
        }
    }
}

/// The identifier rules the member doc publishes: `(case, identifier, accepted)`.
///
/// "Must contain no more than 255 alphanumeric characters, periods, underscores, or
/// hyphens. Must not be \"default\". Example: mySubnetgroup"
/// (`CreateReplicationSubnetGroup$ReplicationSubnetGroupIdentifier`).
fn identifier_rows() -> Vec<(&'static str, String, bool)> {
    vec![
        ("the example the member doc gives", "mySubnetgroup".to_string(), true),
        ("periods and underscores", "sng.one_two-3".to_string(), true),
        (
            "the longest identifier the doc allows",
            "s".repeat(SUBNET_GROUP_IDENTIFIER_MAX_LEN),
            true,
        ),
        (
            "one character past that length",
            "s".repeat(SUBNET_GROUP_IDENTIFIER_MAX_LEN + 1),
            false,
        ),
        ("the reserved name", "default".to_string(), false),
        (
            "the reserved name in another case, which folds to it",
            "Default".to_string(),
            false,
        ),
        ("a character outside the alphabet", "sng one".to_string(), false),
        ("a path separator", "sng/one".to_string(), false),
        ("empty", String::new(), false),
    ]
}

#[tokio::test]
async fn a_subnet_group_identifier_follows_its_member_doc_and_is_held_lowercased() {
    let (emulator, ec2, _iam) = Emulator::with_network();
    let network = create_network(&ec2).await;
    let subnet_ids: Vec<String> = network
        .per_zone
        .iter()
        .take(MIN_SUBNET_GROUP_AVAILABILITY_ZONES)
        .map(|subnet| subnet.id.clone())
        .collect();

    for (case, identifier, accepted) in identifier_rows() {
        let (status, response) = emulator
            .call(
                "CreateReplicationSubnetGroup",
                &json!({
                    "ReplicationSubnetGroupIdentifier": identifier,
                    "ReplicationSubnetGroupDescription": "identifier rules",
                    "SubnetIds": subnet_ids,
                }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected {identifier:?} to be accepted, got \
                 {status} {response}"
            );
            // "This value is stored as a lowercase string": the group reports the folded
            // identifier, and resolves under it.
            let reported = text(&response, "/ReplicationSubnetGroup/ReplicationSubnetGroupIdentifier");
            assert_eq!(
                reported,
                identifier.to_lowercase(),
                "case {case:?}: expected the identifier folded to lower case, got \
                 {reported:?}"
            );
            let (deleted, code) = delete(&emulator, &identifier.to_uppercase()).await;
            assert_eq!(
                deleted, 200,
                "case {case:?}: expected the group to resolve whatever case the delete \
                 names it in, got {deleted} {code}"
            );
        } else {
            let code = error_code(&response);
            assert_eq!(
                (status, code.as_str()),
                (400, VALIDATION),
                "case {case:?}: expected {identifier:?} to be refused, got \
                 {status} {response}"
            );
        }
    }

    // A mixed-case create resolves under the folded identifier everywhere a group is
    // named: the ROUND TRIP, read back through an implemented describe.
    let created = emulator
        .ok(
            "mixed case",
            "CreateReplicationSubnetGroup",
            &json!({
                "ReplicationSubnetGroupIdentifier": "Round-Trip-Group",
                "ReplicationSubnetGroupDescription": "read back through an instance",
                "SubnetIds": subnet_ids,
            }),
        )
        .await;
    let group = created
        .pointer("/ReplicationSubnetGroup")
        .unwrap_or(&Value::Null)
        .clone();
    emulator
        .ok(
            "mixed case",
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": "round-trip-instance",
                "ReplicationInstanceClass": "dms.t3.medium",
                "ReplicationSubnetGroupIdentifier": "ROUND-TRIP-GROUP",
            }),
        )
        .await;
    let described = emulator
        .ok("mixed case", "DescribeReplicationInstances", &json!({}))
        .await;
    assert_eq!(
        described.pointer("/ReplicationInstances/0/ReplicationSubnetGroup"),
        Some(&group),
        "expected the instance to report the group the create persisted, got {described}"
    );
}

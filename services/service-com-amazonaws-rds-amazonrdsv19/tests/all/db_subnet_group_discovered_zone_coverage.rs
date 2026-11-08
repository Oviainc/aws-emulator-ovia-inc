// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Availability Zones a DB subnet group COVERS when its subnets are
//! ones the caller DISCOVERED rather than ones it created a moment earlier.
//!
//! RULE: for every identifier a `SubnetIds` list names, the group reports the Availability
//! Zone the service that OWNS subnets reports for that identifier, so the SET of zones the
//! group reads back covering is exactly the set Amazon EC2 reports for those identifiers —
//! and the coverage the create is gated on is counted over that SET of zones, not over the
//! number of identifiers.
//!
//! The suite in `db_subnet_group_ec2_zones` asserts the same rule over subnets the test
//! MINTS, naming the zone of each one. This one spans the other way a client gets its
//! identifiers: it mints nothing at all and asks `ec2:DescribeSubnets` for the default subnet
//! of each Availability Zone — the subnets of the VPC every account has in a Region, whose
//! ids this process did not see created — and asserts the group's zone SET against what EC2
//! answers for exactly those ids. That is the shape a client's own code has: read the
//! network, hand two of its subnets to `CreateDBSubnetGroup`, read the group back and branch
//! on how many zones it covers.
//!
//! Both services run on ONE engine, and every expected value is one EC2 answered with, so no
//! assertion below recomputes what the code under test computes.

use std::collections::BTreeSet;

use service_com_amazonaws_rds_amazonrdsv19::state::MIN_DB_SUBNET_GROUP_AVAILABILITY_ZONES;

use crate::db_instances::{expect_error, expect_ok, subtrees, text};
use crate::db_subnet_group_ec2_zones::{assert_reported_zones, coverage_message, Account};

/// The `DescribeSubnets` filter that selects the default subnet of each Availability Zone:
/// "default-for-az - Indicates whether this is the default subnet for the Availability Zone"
/// (`DescribeSubnets$Filters`, the `com.amazonaws.ec2#AmazonEC2` model).
const DEFAULT_FOR_AZ_FILTER: &str = "default-for-az";

/// One default subnet per Availability Zone, as EC2 reports them: `(zone, subnet id)`, in
/// zone order.
///
/// Nothing here is created by this suite — the identifiers are the ones the account's own
/// network already carries, which is what a client discovers before it asks RDS for a group.
async fn default_subnets(account: &Account) -> Vec<(String, String)> {
    let filter = ::aws_sdk_ec2::types::Filter::builder()
        .name(DEFAULT_FOR_AZ_FILTER)
        .values("true")
        .build();
    let reported = account
        .ec2
        .describe_subnets()
        .filters(filter)
        .send()
        .await
        .unwrap_or_else(|error| panic!("EC2 must report the account's subnets: {error:?}"));
    let mut by_zone: Vec<(String, String)> = Vec::new();
    for subnet in reported.subnets() {
        // The discovery leg itself: what the filter selected is a DEFAULT subnet of its zone,
        // so a filter the owner ignored would be seen here rather than hidden by the
        // deduplication below.
        assert_eq!(
            subnet.default_for_az(),
            Some(true),
            "the {DEFAULT_FOR_AZ_FILTER} filter must select only default subnets, got \
             {subnet:?}"
        );
        let (Some(zone), Some(subnet_id)) = (subnet.availability_zone(), subnet.subnet_id())
        else {
            continue;
        };
        by_zone.push((zone.to_string(), subnet_id.to_string()));
    }
    by_zone.sort();
    by_zone.dedup_by(|left, right| left.0 == right.0);
    by_zone
}

/// The zones one `DBSubnetGroup` envelope COVERS: the distinct `SubnetAvailabilityZone` names
/// over its `Subnets`, which is the set a client branches its Multi-AZ decisions on.
fn reported_zone_set(group: &str) -> BTreeSet<String> {
    subtrees(group, "Subnet")
        .iter()
        .map(|subnet| text(subnet, "Name"))
        .collect()
}

/// RULE: the zone set a DB subnet group reads back covering equals the set EC2 reports for the
/// subnet ids the request named — so a group built from subnets of two distinct zones covers
/// two zones — and a list whose subnets cover fewer than the two zones a group must cover is
/// refused, the coverage being counted over zones rather than over identifiers.
#[tokio::test]
async fn a_group_covers_the_zone_set_ec2_reports_for_the_subnets_it_was_given() {
    let account = Account::new();
    let discovered = default_subnets(&account).await;
    assert!(
        discovered.len() >= 3,
        "the account's network must carry a default subnet in at least three Availability \
         Zones for these cases, got {discovered:?}"
    );
    let ids: Vec<String> = discovered
        .iter()
        .map(|(_, subnet_id)| subnet_id.clone())
        .collect();

    // Rows differ along how many subnets the list names, WHICH of the Region's zones those
    // lie in (adjacent zones, zones with one skipped, every zone) and in which order.
    let served: [(&str, Vec<String>); 4] = [
        (
            "the default subnets of the first two zones",
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "the default subnets of the first and third zones",
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "the default subnets of three zones",
            vec![ids[0].clone(), ids[1].clone(), ids[2].clone()],
        ),
        (
            "the same three named in reverse zone order",
            vec![ids[2].clone(), ids[1].clone(), ids[0].clone()],
        ),
    ];

    for (index, (case, subnets)) in served.into_iter().enumerate() {
        // The zone of each identifier the request names, asked of the owner of subnets one
        // identifier at a time, so the expectation is EC2's answer and not this test's model
        // of the network.
        let mut zones: Vec<String> = Vec::with_capacity(subnets.len());
        for subnet_id in &subnets {
            zones.push(account.zone_of(subnet_id).await);
        }
        let distinct: BTreeSet<String> = zones.iter().cloned().collect();
        assert!(
            distinct.len() >= MIN_DB_SUBNET_GROUP_AVAILABILITY_ZONES,
            "case {case:?}: the row must name subnets of at least \
             {MIN_DB_SUBNET_GROUP_AVAILABILITY_ZONES} zones for the create to be served, got \
             {distinct:?}"
        );

        let name = format!("emu-discovered-zones-{index}");
        let created = expect_ok(case, account.create_group(&name, &subnets).await);
        let read_back = expect_ok(case, account.describe_group(&name).await);
        for (label, envelope) in [
            ("CreateDBSubnetGroup", &created),
            ("DescribeDBSubnetGroups", &read_back),
        ] {
            // Per subnet: the identifier the request named, and the zone its owner reports
            // for that identifier — the statement the sibling suite makes for minted ids.
            assert_reported_zones(case, label, envelope, &subnets, &zones);
            // And the coverage that adds up to: the very zones EC2 reports for these
            // identifiers, which is the set a client branches on.
            let covered = reported_zone_set(envelope);
            assert_eq!(
                covered, distinct,
                "case {case:?} ({label}): the subnets lie in {distinct:?}, so the group must \
                 cover exactly those zones, got {covered:?}"
            );
        }
    }

    // The other side of the one rule: coverage is the number of distinct ZONES the named
    // subnets lie in, so a list of two identifiers that lie in ONE zone covers one zone and is
    // refused — naming that zone — and nothing is stored for it.
    let one_zone: [(&str, Vec<String>); 2] = [
        ("one default subnet", vec![ids[0].clone()]),
        (
            "the same default subnet named twice",
            vec![ids[0].clone(), ids[0].clone()],
        ),
    ];
    for (index, (case, subnets)) in one_zone.into_iter().enumerate() {
        let name = format!("emu-discovered-zones-thin-{index}");
        let body = expect_error(
            case,
            account.create_group(&name, &subnets).await,
            400,
            "DBSubnetGroupDoesNotCoverEnoughAZs",
        );
        let message = text(&body, "Message");
        let expected = coverage_message(&account.zone_of(&ids[0]).await);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        expect_error(
            case,
            account.describe_group(&name).await,
            404,
            "DBSubnetGroupNotFoundFault",
        );
    }
}

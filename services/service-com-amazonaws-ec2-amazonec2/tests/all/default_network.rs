// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ambient network a fresh account has: the default VPC, the
//! default subnet of each Availability Zone of the Region, and that VPC's default security
//! group — read the way a provisioning client reads them, over `ec2Query`.
//!
//! The rule under test is EC2's own: a fresh account/region already HOLDS a default
//! network, so an unfiltered `DescribeVpcs`/`DescribeSubnets`/`DescribeSecurityGroups`
//! answers with it rather than with an empty inventory, and the subnets it reports are
//! spread over distinct standard Availability Zones of the Region, `available`, and owned
//! by the caller's account. That is the ambient prerequisite every service that provisions
//! into a VPC starts from: it has to be able to name two subnets in two different zones
//! and a security group of the same VPC before it can call a create at all.
//!
//! Each table row is a distinct way of reading that network — no filters at all, and each
//! documented filter a client narrows with — and each must find the same network.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, availability_zones_in_region};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, filters};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// One call of `action` carrying `params`, asserted successful, as its XML body.
async fn read(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The default VPC `DescribeVpcs` reports, as `(vpc_id, body)`.
async fn default_vpc(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let body = read(
        router,
        case,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    let vpc_id = element(&body, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a default VPC in {body}"));
    (vpc_id, body)
}

/// Is `zone_name` a standard Availability Zone of `region` — `<region><letter>`, as
/// opposed to a Local Zone (`us-west-2-lax-1a`) or a Wavelength Zone?
fn is_standard_zone(region: &str, zone_name: &str) -> bool {
    zone_name
        .strip_prefix(region)
        .is_some_and(|suffix| suffix.len() == 1 && suffix.starts_with(|c: char| c.is_ascii_lowercase()))
}

// ---------------------------------------------------------------------------
// The ambient network is discoverable
// ---------------------------------------------------------------------------

/// A client that has created nothing can still name a network: the default VPC, two or
/// more of its default subnets in distinct standard Availability Zones, and a security
/// group of that VPC — each read with no arguments at all.
#[tokio::test]
async fn a_fresh_account_can_name_a_vpc_two_subnets_in_distinct_zones_and_a_security_group() {
    let (router, _engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id().to_string();

    let (vpc_id, vpc_body) = default_vpc(&router, "unfiltered discovery").await;
    assert!(
        vpc_body.contains("<state>available</state>"),
        "unfiltered discovery: expected the default VPC to be available, got {vpc_body}"
    );
    assert!(
        vpc_body.contains(&format!("<ownerId>{account}</ownerId>")),
        "unfiltered discovery: expected the default VPC to be owned by {account}, got {vpc_body}"
    );

    // An UNFILTERED DescribeSubnets is what a quick-start client calls, so it is the read
    // asserted here; the filtered forms are the table below.
    let subnet_body = read(&router, "unfiltered discovery", "DescribeSubnets", &[]).await;
    let subnet_ids = elements(&subnet_body, "subnetId");
    let zones = elements(&subnet_body, "availabilityZone");
    assert_eq!(
        subnet_ids.len(),
        zones.len(),
        "unfiltered discovery: every subnet reports its zone, got {subnet_ids:?} and {zones:?}"
    );

    let mut distinct: Vec<&String> = Vec::new();
    for zone in &zones {
        assert!(
            is_standard_zone(REGION, zone),
            "unfiltered discovery: expected a standard Availability Zone of {REGION}, got \
             {zone:?} from {subnet_body}"
        );
        if !distinct.contains(&zone) {
            distinct.push(zone);
        }
    }
    assert!(
        distinct.len() >= 2,
        "unfiltered discovery: expected subnets in at least two distinct zones, got {zones:?}"
    );
    assert_eq!(
        distinct.len(),
        availability_zones_in_region(REGION).len(),
        "unfiltered discovery: expected one default subnet per zone of {REGION}, got {zones:?}"
    );
    for member in [
        "<state>available</state>".to_string(),
        format!("<ownerId>{account}</ownerId>"),
        format!("<vpcId>{vpc_id}</vpcId>"),
    ] {
        assert!(
            subnet_body.contains(&member),
            "unfiltered discovery: expected every subnet to carry {member}, got {subnet_body}"
        );
    }
    assert_eq!(
        subnet_body.matches(&format!("<vpcId>{vpc_id}</vpcId>")).count(),
        subnet_ids.len(),
        "unfiltered discovery: expected every default subnet to be in the default VPC, got \
         {subnet_body}"
    );

    let group_body = read(
        &router,
        "unfiltered discovery",
        "DescribeSecurityGroups",
        &[],
    )
    .await;
    let group_arns = elements(&group_body, "securityGroupArn");
    assert!(
        !group_arns.is_empty(),
        "unfiltered discovery: expected a security group for the default VPC, got {group_body}"
    );
    assert!(
        group_body.contains(&format!("<vpcId>{vpc_id}</vpcId>")),
        "unfiltered discovery: expected a security group in the default VPC {vpc_id}, got \
         {group_body}"
    );
}

/// One row of [`every_documented_way_of_discovering_the_default_network_finds_the_same_network`]:
/// the case's name, the `DescribeSubnets` parameters and the `DescribeSecurityGroups` ones.
type DiscoveryCase = (&'static str, Vec<(String, String)>, Vec<(String, String)>);

/// However a client narrows the reads — unfiltered, by the VPC it found, by the
/// `default-for-az` or `state` attribute of a default subnet, or by the default group's own
/// name — it finds the same network: the same default VPC, subnets in at least two
/// distinct zones, and a group of that VPC.
#[tokio::test]
async fn every_documented_way_of_discovering_the_default_network_finds_the_same_network() {
    let (router, _engine) = super::ec2_query::emulator();
    let (vpc_id, _) = default_vpc(&router, "baseline").await;
    let expected_subnets = elements(
        &read(&router, "baseline", "DescribeSubnets", &[]).await,
        "subnetId",
    );

    // (case, the DescribeSubnets params, the DescribeSecurityGroups params)
    let cases: Vec<DiscoveryCase> = vec![
        ("no filters", Vec::new(), Vec::new()),
        (
            "by the VPC the client found",
            filters(&[("vpc-id", &[vpc_id.as_str()])]),
            filters(&[("vpc-id", &[vpc_id.as_str()])]),
        ),
        (
            "by the default-subnet attribute",
            filters(&[("default-for-az", &["true"])]),
            filters(&[("group-name", &["default"])]),
        ),
        (
            "by lifecycle state",
            filters(&[("state", &["available"])]),
            filters(&[("owner-id", &[engine::emulator_account_id()])]),
        ),
        (
            "by zone name",
            filters(&[(
                "availability-zone",
                &availability_zones_in_region(REGION)
                    .iter()
                    .map(|zone| zone.zone_name.as_str())
                    .collect::<Vec<_>>(),
            )]),
            Vec::new(),
        ),
    ];

    for (case, subnet_params, group_params) in cases {
        let subnet_body = read(&router, case, "DescribeSubnets", &subnet_params).await;
        let subnet_ids = elements(&subnet_body, "subnetId");
        assert_eq!(
            subnet_ids, expected_subnets,
            "case {case:?}: expected the default VPC's subnets, got {subnet_ids:?}"
        );
        let mut distinct: Vec<String> = Vec::new();
        for zone in elements(&subnet_body, "availabilityZone") {
            if !distinct.contains(&zone) {
                distinct.push(zone);
            }
        }
        assert!(
            distinct.len() >= 2,
            "case {case:?}: expected at least two distinct zones, got {distinct:?}"
        );

        let group_body = read(&router, case, "DescribeSecurityGroups", &group_params).await;
        assert!(
            group_body.contains(&format!("<vpcId>{vpc_id}</vpcId>")),
            "case {case:?}: expected a security group in {vpc_id}, got {group_body}"
        );
    }
}

/// A client that needs N subnets in N distinct zones can satisfy it from the ambient
/// network for every N the Region's zone count admits — which is what a multi-AZ create
/// (an MSK cluster's `ClientSubnets`, say) asks of the account's network.
#[tokio::test]
async fn the_default_subnets_cover_every_zone_count_the_region_admits() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone_count = availability_zones_in_region(REGION).len();

    let body = read(&router, "zone counts", "DescribeSubnets", &[]).await;
    let subnet_ids = elements(&body, "subnetId");
    let zones = elements(&body, "availabilityZone");

    for wanted in 1..=zone_count {
        let case = format!("{wanted} subnets in distinct zones");
        // Pick one subnet per distinct zone, the way a discovery helper does.
        let mut picked: Vec<(&String, &String)> = Vec::new();
        for (subnet_id, zone) in subnet_ids.iter().zip(zones.iter()) {
            if !picked.iter().any(|(_, seen)| *seen == zone) {
                picked.push((subnet_id, zone));
            }
        }
        assert!(
            picked.len() >= wanted,
            "case {case:?}: expected {wanted} distinct zones among {zones:?}, got {picked:?}"
        );
    }
}

/// Reading the ambient network does not change it: the ids and the store are the same
/// after every read above as before them.
#[tokio::test]
async fn discovering_the_default_network_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let before = {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        (
            guard.vpcs.len(),
            guard.subnets.len(),
            guard.security_groups.len(),
        )
    };

    let first_vpcs = elements(
        &read(&router, "first read", "DescribeVpcs", &[]).await,
        "vpcId",
    );
    let first_subnets = elements(
        &read(&router, "first read", "DescribeSubnets", &[]).await,
        "subnetId",
    );
    let first_groups = elements(
        &read(&router, "first read", "DescribeSecurityGroups", &[]).await,
        "securityGroupArn",
    );

    let second_vpcs = elements(
        &read(&router, "second read", "DescribeVpcs", &[]).await,
        "vpcId",
    );
    let second_subnets = elements(
        &read(&router, "second read", "DescribeSubnets", &[]).await,
        "subnetId",
    );
    let second_groups = elements(
        &read(&router, "second read", "DescribeSecurityGroups", &[]).await,
        "securityGroupArn",
    );

    assert_eq!(
        (&first_vpcs, &first_subnets, &first_groups),
        (&second_vpcs, &second_subnets, &second_groups),
        "a read must not change what a later caller sees"
    );

    let after = {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        (
            guard.vpcs.len(),
            guard.subnets.len(),
            guard.security_groups.len(),
        )
    };
    assert_eq!(
        after, before,
        "discovering the default network must not write to the store"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! The ambient default network is the one of THE REGION THE REQUEST IS SIGNED FOR — end to
//! end over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol"). The Region lives in the SigV4
//! credential scope of each call, so each row here is one Region's whole discovery.
//!
//! The rule under test: an account holds a default VPC in EVERY Region it can call in, with
//! one default subnet in each Availability Zone OF THAT REGION and that VPC's default
//! security group ("when you create an AWS account, we create a default VPC for you in each
//! AWS Region" / "when we create a default VPC, we create a default subnet in each
//! Availability Zone in the Region" — Amazon VPC User Guide, "Default VPCs"). So a client
//! bootstrapping anything VPC-bound in Region R — an MSK cluster's `ClientSubnets` needs two
//! subnets in two distinct zones of the Region the cluster is created in — can name that
//! network from R alone, whatever Region any other call used.
//!
//! [`super::default_network`] is the same discovery for ONE Region; this suite is the table
//! over Regions, which is what the rule generalises over: each row is a distinct Region,
//! spanning partitions (commercial, GovCloud, China) and Regions of different zone-id
//! prefixes, and the Region the store is constructed for is only one row of it.

use ec2::state::{
    availability_zones_in_region, is_catalog_region, ComAmazonawsEc2Amazonec2State as Ec2State,
    SubnetSpec, AVAILABILITY_ZONES_PER_REGION, DEFAULT_VPC_CIDR_BLOCK, SUBNET_STATE_AVAILABLE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, filters, indexed};

/// The Regions each test is a table over: Regions of three partitions, of differing zone-id
/// prefixes and differing code shapes, including the one the store is constructed for
/// (`us-east-1`) and the one the MSK reports were recorded in (`us-west-2`).
///
/// Written out here rather than derived, so a row is a deliberate expectation; every row is
/// checked against its source — the Region catalog [`is_catalog_region`] reads — inside the
/// tests, so this list cannot drift into naming a Region that does not exist.
const REGIONS: [&str; 6] = [
    "us-east-1",
    "us-west-2",
    "eu-central-1",
    "ap-southeast-2",
    "us-gov-west-1",
    "cn-north-1",
];

/// A Region code that is well formed but names no Region of the catalog — the case an
/// ambient network must NOT be seeded for.
const REGION_OUTSIDE_THE_CATALOG: &str = "zz-nowhere-1";

/// One call of `action` signed for `region`, asserted successful, as its XML body.
async fn read(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = super::ec2_query::call(router, region, action, params).await;
    assert_eq!(
        status, 200,
        "case {region:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Is `zone_name` a standard Availability Zone of `region` — `<region><letter>`, as opposed
/// to a zone of another Region, a Local Zone (`us-west-2-lax-1a`) or a Wavelength Zone?
fn is_standard_zone(region: &str, zone_name: &str) -> bool {
    zone_name.strip_prefix(region).is_some_and(|suffix| {
        suffix.len() == 1 && suffix.starts_with(|letter: char| letter.is_ascii_lowercase())
    })
}

/// The `VpcId` the default subnets of `region` belong to, read the way a client reads it:
/// an unfiltered `DescribeSubnets` signed for that Region, narrowed to the subnets whose
/// zone belongs to it.
async fn default_vpc_of(router: &MultiServiceRouter, region: &str) -> String {
    let body = read(router, region, "DescribeSubnets", &[]).await;
    let zones = elements(&body, "availabilityZone");
    let vpcs = elements(&body, "vpcId");
    assert_eq!(
        zones.len(),
        vpcs.len(),
        "case {region:?}: every reported subnet names its zone and its VPC, got {zones:?} and \
         {vpcs:?}"
    );
    let mut found: Vec<&String> = zones
        .iter()
        .zip(vpcs.iter())
        .filter(|(zone, _)| is_standard_zone(region, zone))
        .map(|(_, vpc)| vpc)
        .collect();
    found.dedup();
    assert_eq!(
        found.len(),
        1,
        "case {region:?}: expected the Region's default subnets to share one VPC, got \
         {found:?} from {body}"
    );
    found[0].clone()
}

/// The subnet ids an unfiltered `DescribeSubnets` signed for `region` reports whose zone
/// belongs to that Region.
async fn subnets_of(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let body = read(router, region, "DescribeSubnets", &[]).await;
    let ids = elements(&body, "subnetId");
    let zones = elements(&body, "availabilityZone");
    assert_eq!(
        ids.len(),
        zones.len(),
        "case {region:?}: every reported subnet names its zone, got {ids:?} and {zones:?}"
    );
    ids.into_iter()
        .zip(zones)
        .filter(|(_, zone)| is_standard_zone(region, zone))
        .map(|(id, _)| id)
        .collect()
}

// ---------------------------------------------------------------------------
// Every Region has its own ambient network
// ---------------------------------------------------------------------------

/// Whichever Region a client is signed for, it finds one default subnet per Availability
/// Zone of THAT Region — `available`, `DefaultForAz`, owned by the caller's account, with
/// the ARN of that Region — all in one default VPC that Region's `DescribeVpcs` reports, and
/// that VPC comes with its `default` security group.
#[tokio::test]
async fn every_region_reports_one_default_subnet_per_zone_of_that_region() {
    let (router, _engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id().to_string();

    for region in REGIONS {
        assert!(
            is_catalog_region(region),
            "case {region:?}: this table row must name a Region of the catalog"
        );
        let zones = availability_zones_in_region(region);
        assert_eq!(
            zones.len(),
            AVAILABILITY_ZONES_PER_REGION,
            "case {region:?}: expected the Region's zone count, got {zones:?}"
        );

        let vpc_id = default_vpc_of(&router, region).await;
        let mut seen: Vec<String> = Vec::new();

        for zone in &zones {
            let case = format!("{region} / {}", zone.zone_name);
            // One zone at a time, through the documented `availability-zone` filter, so the
            // row asserts about the subnet of THAT zone and not about whichever subnet the
            // store happens to report first.
            let body = read(
                &router,
                region,
                "DescribeSubnets",
                &filters(&[("availability-zone", &[zone.zone_name.as_str()])]),
            )
            .await;
            let ids = elements(&body, "subnetId");
            assert_eq!(
                ids.len(),
                1,
                "case {case:?}: expected exactly one default subnet in the zone, got {ids:?} \
                 from {body}"
            );
            let subnet_id = ids[0].clone();
            assert!(
                !seen.contains(&subnet_id),
                "case {case:?}: the subnet {subnet_id:?} was already reported for another zone"
            );
            seen.push(subnet_id.clone());

            let expected_members = [
                ("availabilityZone", zone.zone_name.clone()),
                ("availabilityZoneId", zone.zone_id.clone()),
                ("state", "available".to_string()),
                ("defaultForAz", "true".to_string()),
                ("mapPublicIpOnLaunch", "true".to_string()),
                ("ownerId", account.clone()),
                ("vpcId", vpc_id.clone()),
                (
                    "subnetArn",
                    engine::emulator_arn("ec2", region, &format!("subnet/{subnet_id}")),
                ),
            ];
            for (member, value) in expected_members {
                assert!(
                    body.contains(&format!("<{member}>{value}</{member}>")),
                    "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
                );
            }
        }

        // What every VPC-bound create asks of the account's network: two subnets in two
        // distinct zones of the Region it is creating in.
        assert!(
            seen.len() >= 2,
            "case {region:?}: expected at least two subnets in distinct zones, got {seen:?}"
        );

        // The VPC those subnets sit in is the Region's DEFAULT VPC, reported as such.
        let vpc_body = read(
            &router,
            region,
            "DescribeVpcs",
            &indexed("VpcId", &[vpc_id.as_str()]),
        )
        .await;
        for (member, value) in [
            ("isDefault", "true".to_string()),
            ("state", "available".to_string()),
            ("cidrBlock", DEFAULT_VPC_CIDR_BLOCK.to_string()),
            ("ownerId", account.clone()),
        ] {
            assert!(
                vpc_body.contains(&format!("<{member}>{value}</{member}>")),
                "case {region:?}: expected <{member}>{value}</{member}>, got {vpc_body}"
            );
        }

        // "Your VPC automatically comes with a default security group", so the Region's
        // default VPC has one — the third thing a provisioning client has to name.
        let group_body = read(
            &router,
            region,
            "DescribeSecurityGroups",
            &filters(&[("vpc-id", &[vpc_id.as_str()])]),
        )
        .await;
        assert_eq!(
            elements(&group_body, "groupName"),
            vec!["default".to_string()],
            "case {region:?}: expected the default group of {vpc_id}, got {group_body}"
        );
    }
}

/// Each Region's ambient network is its OWN: no two Regions share a default VPC or a subnet,
/// and no Region's subnets carry another Region's zones.
#[tokio::test]
async fn no_two_regions_share_the_default_network() {
    let (router, _engine) = super::ec2_query::emulator();

    let mut networks: Vec<(&str, String, Vec<String>)> = Vec::new();
    for region in REGIONS {
        let vpc_id = default_vpc_of(&router, region).await;
        let subnet_ids = subnets_of(&router, region).await;
        assert_eq!(
            subnet_ids.len(),
            AVAILABILITY_ZONES_PER_REGION,
            "case {region:?}: expected one default subnet per zone, got {subnet_ids:?}"
        );
        networks.push((region, vpc_id, subnet_ids));
    }

    for (index, (region, vpc_id, subnet_ids)) in networks.iter().enumerate() {
        for (other_region, other_vpc, other_subnets) in networks.iter().skip(index + 1) {
            assert_ne!(
                vpc_id, other_vpc,
                "case {region:?} vs {other_region:?}: each Region has its own default VPC, \
                 got {vpc_id} twice"
            );
            let shared: Vec<&String> = subnet_ids
                .iter()
                .filter(|id| other_subnets.contains(id))
                .collect();
            assert!(
                shared.is_empty(),
                "case {region:?} vs {other_region:?}: a subnet cannot be in two Regions, got \
                 {shared:?}"
            );
        }
    }
}

/// Discovering a Region's ambient network is IDEMPOTENT: the second and third reads report
/// exactly the ids the first did, and the store stops growing — the network a Region already
/// held is materialised, never a second copy of it.
#[tokio::test]
async fn discovering_a_regions_network_twice_reports_the_same_network() {
    let (router, engine) = super::ec2_query::emulator();
    let held = |label: &str| {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let guard = store.lock();
        let counts = (
            guard.vpcs.len(),
            guard.subnets.len(),
            guard.security_groups.len(),
        );
        assert!(
            counts.0 > 0,
            "case {label:?}: the store must hold the networks read so far"
        );
        counts
    };

    let mut first: Vec<(&str, String, Vec<String>)> = Vec::new();
    for region in REGIONS {
        let vpc_id = default_vpc_of(&router, region).await;
        first.push((region, vpc_id, subnets_of(&router, region).await));
    }
    let after_first = held("first pass");

    for (region, vpc_id, subnet_ids) in &first {
        let again_vpc = default_vpc_of(&router, region).await;
        let again_subnets = subnets_of(&router, region).await;
        assert_eq!(
            (&again_vpc, &again_subnets),
            (vpc_id, subnet_ids),
            "case {region:?}: a later read must see the same network"
        );
    }
    assert_eq!(
        held("second pass"),
        after_first,
        "reading a network that is already there must not write to the store"
    );

    for (region, _, _) in &first {
        let _ = read(&router, region, "DescribeSubnets", &[]).await;
        let _ = read(&router, region, "DescribeVpcs", &[]).await;
        let _ = read(&router, region, "DescribeSecurityGroups", &[]).await;
    }
    assert_eq!(
        held("third pass"),
        after_first,
        "a read must not change what a later caller sees"
    );
}

/// A credential scope naming something merely SHAPED like a Region code gets no network: an
/// account has a default VPC in the Regions that exist, so caller-controlled text cannot
/// make the store grow a network per value.
#[tokio::test]
async fn a_region_outside_the_catalog_gets_no_network() {
    let (router, engine) = super::ec2_query::emulator();
    assert!(
        !is_catalog_region(REGION_OUTSIDE_THE_CATALOG),
        "this test needs a Region code the catalog does not hold"
    );

    let before = {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let guard = store.lock();
        (guard.vpcs.len(), guard.subnets.len())
    };

    let body = read(&router, REGION_OUTSIDE_THE_CATALOG, "DescribeSubnets", &[]).await;
    let reported: Vec<String> = elements(&body, "availabilityZone")
        .into_iter()
        .filter(|zone| is_standard_zone(REGION_OUTSIDE_THE_CATALOG, zone))
        .collect();
    assert!(
        reported.is_empty(),
        "expected no subnet of {REGION_OUTSIDE_THE_CATALOG:?}, got {reported:?}"
    );

    let after = {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let guard = store.lock();
        (guard.vpcs.len(), guard.subnets.len())
    };
    assert_eq!(
        after, before,
        "a Region outside the catalog must not make the store grow"
    );
}

/// A store with no room for a WHOLE Region's default subnets seeds nothing for that Region
/// — not a default VPC and a group without them — so a Region is never left holding a
/// network that is missing the subnets a client has to name.
#[tokio::test]
async fn a_store_with_no_room_for_the_subnets_seeds_no_partial_network() {
    let (_router, engine) = super::ec2_query::emulator();
    // A Region no other test in this file reads, so only this test's ceiling decides it.
    const CROWDED_REGION: &str = "eu-west-1";
    assert!(
        is_catalog_region(CROWDED_REGION),
        "this test needs a Region of the catalog"
    );

    let zone = availability_zones_in_region(CROWDED_REGION)
        .first()
        .cloned()
        .expect("a Region has zones");

    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let vpc_id = guard
        .vpcs
        .keys()
        .next()
        .cloned()
        .expect("a fresh store holds the default VPC of its own Region");
    // Fill the subnet store to its ceiling through the store's OWN writer, so the ceiling
    // is reached by records rather than by a flag.
    let mut filler = 0;
    while guard.has_subnet_capacity() {
        filler += 1;
        guard.insert_subnet(SubnetSpec {
            vpc_id: vpc_id.clone(),
            cidr_block: format!("10.{}.{}.0/24", filler / 256, filler % 256),
            availability_zone: zone.zone_name.clone(),
            availability_zone_id: zone.zone_id.clone(),
            region: CROWDED_REGION.to_string(),
            state: SUBNET_STATE_AVAILABLE.to_string(),
            default_for_az: false,
            map_public_ip_on_launch: false,
            tags: Vec::new(),
        });
    }
    let before = (
        guard.vpcs.len(),
        guard.subnets.len(),
        guard.security_groups.len(),
    );

    assert!(
        guard.ensure_default_network(CROWDED_REGION).is_none(),
        "a store with no room for the Region's subnets must seed no network"
    );
    assert_eq!(
        (
            guard.vpcs.len(),
            guard.subnets.len(),
            guard.security_groups.len()
        ),
        before,
        "a network that cannot be seeded whole must write nothing at all"
    );
}

// ---------------------------------------------------------------------------
// The writers that fall back on "the default VPC"
// ---------------------------------------------------------------------------

/// A `CreateSecurityGroup` that names no `VpcId` lands in the default VPC OF THE REGION it
/// is signed for — read back through `DescribeSecurityGroups` — so the same request in two
/// Regions creates two groups in two different VPCs rather than both in one.
#[tokio::test]
async fn a_group_created_without_a_vpc_id_lands_in_the_regions_default_vpc() {
    let (router, _engine) = super::ec2_query::emulator();

    for region in REGIONS {
        let expected_vpc = default_vpc_of(&router, region).await;
        let created = read(
            &router,
            region,
            "CreateSecurityGroup",
            &[
                ("GroupName".to_string(), "regional-group".to_string()),
                (
                    "GroupDescription".to_string(),
                    "a group in the default VPC of this Region".to_string(),
                ),
            ],
        )
        .await;
        let group_id = element(&created, "groupId")
            .unwrap_or_else(|| panic!("case {region:?}: expected a groupId in {created}"));

        let read_back = read(
            &router,
            region,
            "DescribeSecurityGroups",
            &indexed("GroupId", &[group_id.as_str()]),
        )
        .await;
        assert_eq!(
            element(&read_back, "vpcId"),
            Some(expected_vpc.clone()),
            "case {region:?}: expected the group in {expected_vpc}, got {read_back}"
        );
        assert_eq!(
            element(&read_back, "securityGroupArn"),
            Some(engine::emulator_arn(
                "ec2",
                region,
                &format!("security-group/{group_id}")
            )),
            "case {region:?}: expected an ARN of this Region, got {read_back}"
        );
    }
}

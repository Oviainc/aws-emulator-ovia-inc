// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeSubnets`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeSubnets`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with
//! is the account's subnets narrowed by the `SubnetIds` and `Filters` the request names
//! and cut into `MaxResults`-sized pages. Every table row is a distinct input along what
//! that generalises over — which filter (each documented name and each documented
//! alternative spelling), how many values, how many filters, which ids, which page size —
//! so the default VPC's own subnets are three rows among many rather than the
//! specification.

use ec2::ops::types::{SubnetCidrBlockState, SubnetIpv6CidrBlockAssociation, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, SubnetSpec, availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The subnet store holds the account's subnets
/// rather than one Region's, and the default VPC's default subnets are seeded for the
/// emulator's default Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// The IPv6 CIDR block associated with the one seeded subnet that has one.
const SEEDED_IPV6_CIDR_BLOCK: &str = "2600:1f14:abc:1200::/64";

/// The customer-owned IPv4 pool the one Outpost-style seeded subnet draws from.
const SEEDED_COIP_POOL: &str = "ipv4pool-coip-0abcdef1234567890";

/// The Outpost that subnet is on.
const SEEDED_OUTPOST_ARN: &str = "arn:aws:outposts:us-east-1:000000000000:outpost/op-0abcdef12";

/// One `DescribeSubnets` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeSubnets", params).await
}

/// The subnet ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "subnetId")
}

/// The XML of the one subnet `subnet_id` names.
async fn describe_one(router: &MultiServiceRouter, case: &str, subnet_id: &str) -> String {
    let (status, body) = describe(router, &indexed("SubnetId", &[subnet_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded subnets carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded subnet is created with, beyond the ids the service mints itself.
struct SubnetFixture {
    cidr_block: &'static str,
    /// The zone of [`availability_zones_in_region`] this subnet is in.
    zone_index: usize,
    state: &'static str,
    map_public_ip_on_launch: bool,
    tags: Vec<Tag>,
}

/// The subnets the filter and paging tables are written against, beyond the default
/// subnets a fresh account's default VPC already has. They differ along every attribute
/// the documented filters select on: zone, CIDR block size, lifecycle state, the
/// launch-time flags and tags.
fn seeded_fixtures() -> Vec<SubnetFixture> {
    vec![
        SubnetFixture {
            cidr_block: "10.0.0.0/20",
            zone_index: 0,
            state: "available",
            map_public_ip_on_launch: false,
            tags: vec![tag("Name", "web"), tag("Owner", "TeamA")],
        },
        SubnetFixture {
            cidr_block: "10.0.16.0/24",
            zone_index: 1,
            state: "available",
            map_public_ip_on_launch: true,
            tags: vec![tag("Name", "batch"), tag("Owner", "TeamB")],
        },
        SubnetFixture {
            cidr_block: "10.0.17.0/28",
            zone_index: 2,
            state: "pending",
            map_public_ip_on_launch: false,
            tags: vec![tag("Name", "web")],
        },
        SubnetFixture {
            cidr_block: "10.0.18.0/24",
            zone_index: 0,
            state: "available",
            map_public_ip_on_launch: false,
            tags: Vec::new(),
        },
        SubnetFixture {
            cidr_block: "10.0.19.0/24",
            zone_index: 1,
            state: "available",
            map_public_ip_on_launch: false,
            tags: vec![tag("Project", "atlas")],
        },
    ]
}

/// What a seeded store holds, for the tables to name rows by.
struct Seeded {
    /// Every subnet id, in the order `DescribeSubnets` reports them (the store's key
    /// order): the three default subnets first, then [`seeded_fixtures`] in order.
    ids: Vec<String>,
    /// The VPC the default subnets are in.
    default_vpc_id: String,
    /// The VPC the fixtures above are in.
    fixture_vpc_id: String,
    /// The association id of the one IPv6 CIDR block association seeded.
    ipv6_association_id: String,
}

/// Write the seeded subnets into the engine's store, the way a creating operation would,
/// and report the ids they were given. The subnets that carry an IPv6 association, an
/// Outpost, a customer-owned pool or non-default hostname options get them here, so every
/// documented filter has something to select on.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let default_vpc_id = guard
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has its default VPC");
    let fixture_vpc_id =
        guard.insert_vpc("10.0.0.0/16", false, "default", "available", REGION, Vec::new());

    let zones = availability_zones_in_region(REGION);
    let mut fixture_ids = Vec::new();
    for fixture in seeded_fixtures() {
        let zone = &zones[fixture.zone_index];
        fixture_ids.push(guard.insert_subnet(SubnetSpec {
            vpc_id: fixture_vpc_id.clone(),
            cidr_block: fixture.cidr_block.to_string(),
            availability_zone: zone.zone_name.clone(),
            availability_zone_id: zone.zone_id.clone(),
            region: REGION.to_string(),
            state: fixture.state.to_string(),
            default_for_az: false,
            map_public_ip_on_launch: fixture.map_public_ip_on_launch,
            tags: fixture.tags,
        }));
    }

    let ipv6_association_id = guard.next_id("subnet-cidr-assoc");
    if let Some(subnet) = guard.subnets.get_mut(&fixture_ids[1]) {
        subnet.ipv6_cidr_block_association_set = Some(vec![SubnetIpv6CidrBlockAssociation {
            association_id: Some(ipv6_association_id.clone()),
            ipv6_cidr_block: Some(SEEDED_IPV6_CIDR_BLOCK.to_string()),
            ipv6_cidr_block_state: Some(SubnetCidrBlockState {
                state: Some("associated".to_string()),
                status_message: None,
            }),
            ipv6_address_attribute: None,
            ip_source: None,
        }]);
    }
    if let Some(subnet) = guard.subnets.get_mut(&fixture_ids[3]) {
        subnet.outpost_arn = Some(SEEDED_OUTPOST_ARN.to_string());
        subnet.customer_owned_ipv4_pool = Some(SEEDED_COIP_POOL.to_string());
        subnet.map_customer_owned_ip_on_launch = Some(true);
        subnet.enable_lni_at_device_index = Some(1);
    }
    if let Some(subnet) = guard.subnets.get_mut(&fixture_ids[4]) {
        subnet.ipv6_native = Some(true);
        subnet.enable_dns64 = Some(true);
        subnet.private_dns_name_options_on_launch =
            Some(ec2::ops::types::PrivateDnsNameOptionsOnLaunch {
                hostname_type: Some("resource-name".to_string()),
                enable_resource_name_dns_a_record: Some(true),
                enable_resource_name_dns_aaaa_record: Some(true),
            });
    }

    Seeded {
        ids: guard.subnets.keys().cloned().collect(),
        default_vpc_id,
        fixture_vpc_id,
        ipv6_association_id,
    }
}

/// The ids at `indices` of a seeded store, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| seeded.ids[*index].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account's default VPC already has one default subnet per Availability Zone —
/// AWS creates them with the VPC — and describing them is a READ: two calls report the
/// same subnets and the store is unchanged.
#[tokio::test]
async fn a_fresh_account_reports_the_default_subnet_of_each_zone_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, first) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {first}"
    );
    let first_ids = elements(&first, "subnetId");
    let zones = availability_zones_in_region(REGION);
    assert_eq!(
        first_ids.len(),
        zones.len(),
        "fresh account: expected one default subnet per zone of {REGION}, got {first_ids:?} from \
         {first}"
    );

    // The VPC those subnets belong to is the one `DescribeVpcs` reports as the default, so
    // the two describes agree about the account's default network.
    let (vpc_status, vpc_body) =
        super::ec2_query::call(&router, REGION, "DescribeVpcs", &filters(&[("is-default", &["true"])])).await;
    assert_eq!(
        vpc_status, 200,
        "fresh account: expected HTTP 200 from DescribeVpcs, got {vpc_status} with body {vpc_body}"
    );
    let default_vpc_id = element(&vpc_body, "vpcId").unwrap_or_default();

    // Zone n gets the n-th /20 block of the default VPC's 172.31.0.0/16, and a default
    // subnet is `DefaultForAz` with the auto-assign public IPv4 attribute set.
    let expected_blocks = ["172.31.0.0/20", "172.31.16.0/20", "172.31.32.0/20"];
    for (index, zone) in zones.iter().enumerate() {
        let case = format!("default subnet in {}", zone.zone_name);
        let body = describe_one(&router, &case, &first_ids[index]).await;
        // A /20 holds 4096 addresses, five of which AWS reserves in every subnet.
        let expected_members = [
            ("availabilityZone", zone.zone_name.clone()),
            ("availabilityZoneId", zone.zone_id.clone()),
            ("cidrBlock", expected_blocks[index].to_string()),
            ("availableIpAddressCount", "4091".to_string()),
            ("defaultForAz", "true".to_string()),
            ("mapPublicIpOnLaunch", "true".to_string()),
            ("state", "available".to_string()),
            ("vpcId", default_vpc_id.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
            (
                "subnetArn",
                engine::emulator_arn("ec2", REGION, &format!("subnet/{}", first_ids[index])),
            ),
        ];
        for (member, value) in expected_members {
            assert!(
                body.contains(&format!("<{member}>{value}</{member}>")),
                "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
            );
        }
    }

    let second_ids = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second_ids, first_ids,
        "a read must not change what a later caller sees: {first_ids:?} then {second_ids:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().subnets.len(),
        zones.len(),
        "describing subnets must not write to the store"
    );
}

/// Every subnet the store holds is reported, in store order, and a subnet written after a
/// call shows up in the next one with the members it was stored with — the answer is
/// computed from the store, not fixed.
#[tokio::test]
async fn every_stored_subnet_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all subnets", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all subnets: expected every stored subnet in store order, got {reported:?}"
    );

    let zones = availability_zones_in_region(REGION);
    let late_id = engine.state::<Ec2State>().lock().insert_subnet(SubnetSpec {
        vpc_id: seeded.fixture_vpc_id.clone(),
        cidr_block: "10.0.32.0/24".to_string(),
        availability_zone: zones[2].zone_name.clone(),
        availability_zone_id: zones[2].zone_id.clone(),
        region: REGION.to_string(),
        state: "available".to_string(),
        default_for_az: false,
        map_public_ip_on_launch: true,
        tags: vec![tag("Name", "late")],
    });

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.ids.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );

    // Every member that write set round-trips through the describe.
    let body = describe_one(&router, "the late subnet", &late_id).await;
    let expected_members = [
        ("subnetId", late_id.clone()),
        ("vpcId", seeded.fixture_vpc_id.clone()),
        ("cidrBlock", "10.0.32.0/24".to_string()),
        ("availabilityZone", zones[2].zone_name.clone()),
        ("availabilityZoneId", zones[2].zone_id.clone()),
        ("availableIpAddressCount", "251".to_string()),
        ("state", "available".to_string()),
        ("defaultForAz", "false".to_string()),
        ("mapPublicIpOnLaunch", "true".to_string()),
        ("key", "Name".to_string()),
        ("value", "late".to_string()),
    ];
    for (member, value) in expected_members {
        assert!(
            body.contains(&format!("<{member}>{value}</{member}>")),
            "the late subnet: expected <{member}>{value}</{member}>, got {body}"
        );
    }
}

/// `AvailableIpAddressCount` is the subnet's CIDR block less the five addresses AWS
/// reserves in every subnet, whatever the block's size.
#[tokio::test]
async fn available_ip_address_count_is_the_block_less_the_five_reserved_addresses() {
    let (router, engine) = super::ec2_query::emulator();
    let zones = availability_zones_in_region(REGION);

    // "The allowed block size is between a /28 netmask and /16 netmask" (Amazon VPC User
    // Guide, "Subnet CIDR blocks"): the rows are both published bounds and sizes between
    // them, each in its own VPC-sized block so the CIDRs cannot overlap.
    let cases = [
        ("192.168.0.0/16", 65531),
        ("192.168.0.0/17", 32763),
        ("192.168.0.0/20", 4091),
        ("192.168.0.0/24", 251),
        ("192.168.0.0/25", 123),
        ("192.168.0.0/28", 11),
    ];
    let vpc_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_vpc(
            "192.168.0.0/16",
            false,
            "default",
            "available",
            REGION,
            Vec::new(),
        );
    for (cidr_block, expected_count) in cases {
        let subnet_id = engine.state::<Ec2State>().lock().insert_subnet(SubnetSpec {
            vpc_id: vpc_id.clone(),
            cidr_block: cidr_block.to_string(),
            availability_zone: zones[0].zone_name.clone(),
            availability_zone_id: zones[0].zone_id.clone(),
            region: REGION.to_string(),
            state: "available".to_string(),
            default_for_az: false,
            map_public_ip_on_launch: false,
            tags: Vec::new(),
        });
        let body = describe_one(&router, cidr_block, &subnet_id).await;
        let reported = element(&body, "availableIpAddressCount").unwrap_or_default();
        assert_eq!(
            reported,
            expected_count.to_string(),
            "case {cidr_block:?}: expected {expected_count} available addresses, got \
             {reported:?} from {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter — under its documented name and under every alternative
/// spelling the operation publishes — selects on its own attribute; values within one
/// filter are OR'd, and the comparison is exact and case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_subnets_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let zones = availability_zones_in_region(REGION);
    let all: Vec<usize> = (0..seeded.ids.len()).collect();
    let subnet_arn = engine::emulator_arn("ec2", REGION, &format!("subnet/{}", seeded.ids[4]));

    // One row per filter name `DescribeSubnetsRequest$Filters` documents, each asserted on
    // a subnet the issue never cited.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "availability-zone",
            vec![zones[0].zone_name.clone()],
            vec![0, 3, 6],
        ),
        // "You can also use availabilityZone as the filter name."
        (
            "availabilityZone",
            vec![zones[1].zone_name.clone()],
            vec![1, 4, 7],
        ),
        (
            "availability-zone-id",
            vec![zones[2].zone_id.clone()],
            vec![2, 5],
        ),
        // "You can also use availabilityZoneId as the filter name."
        (
            "availabilityZoneId",
            vec![zones[0].zone_id.clone()],
            vec![0, 3, 6],
        ),
        // Two zones in one filter: joined with OR.
        (
            "availability-zone",
            vec![zones[1].zone_name.clone(), zones[2].zone_name.clone()],
            vec![1, 2, 4, 5, 7],
        ),
        // A /20 holds 4091 available addresses, a /24 251 and a /28 11.
        ("available-ip-address-count", vec!["4091".into()], vec![
            0, 1, 2, 3,
        ]),
        ("available-ip-address-count", vec!["251".into()], vec![
            4, 6, 7,
        ]),
        ("available-ip-address-count", vec!["11".into()], vec![5]),
        ("available-ip-address-count", vec!["4092".into()], vec![]),
        ("cidr-block", vec!["10.0.0.0/20".into()], vec![3]),
        // "You can also use cidr or cidrBlock as the filter names."
        ("cidr", vec!["172.31.16.0/20".into()], vec![1]),
        ("cidrBlock", vec!["10.0.17.0/28".into()], vec![5]),
        // A CIDR block no subnet has: an empty set, not an error.
        ("cidr-block", vec!["10.9.0.0/24".into()], vec![]),
        (
            "customer-owned-ipv4-pool",
            vec![SEEDED_COIP_POOL.into()],
            vec![6],
        ),
        ("default-for-az", vec!["true".into()], vec![0, 1, 2]),
        ("default-for-az", vec!["false".into()], vec![3, 4, 5, 6, 7]),
        // "You can also use defaultForAz as the filter name."
        ("defaultForAz", vec!["true".into()], vec![0, 1, 2]),
        ("enable-dns64", vec!["true".into()], vec![7]),
        ("enable-dns64", vec!["false".into()], vec![
            0, 1, 2, 3, 4, 5, 6,
        ]),
        ("enable-lni-at-device-index", vec!["1".into()], vec![6]),
        ("enable-lni-at-device-index", vec!["0".into()], vec![]),
        (
            "ipv6-cidr-block-association.ipv6-cidr-block",
            vec![SEEDED_IPV6_CIDR_BLOCK.into()],
            vec![4],
        ),
        (
            "ipv6-cidr-block-association.association-id",
            vec![seeded.ipv6_association_id.clone()],
            vec![4],
        ),
        (
            "ipv6-cidr-block-association.state",
            vec!["associated".into()],
            vec![4],
        ),
        ("ipv6-native", vec!["true".into()], vec![7]),
        ("ipv6-native", vec!["false".into()], vec![
            0, 1, 2, 3, 4, 5, 6,
        ]),
        (
            "map-customer-owned-ip-on-launch",
            vec!["true".into()],
            vec![6],
        ),
        ("map-public-ip-on-launch", vec!["true".into()], vec![
            0, 1, 2, 4,
        ]),
        ("outpost-arn", vec![SEEDED_OUTPOST_ARN.into()], vec![6]),
        ("owner-id", vec![account], all.clone()),
        ("owner-id", vec!["111122223333".into()], vec![]),
        (
            "private-dns-name-options-on-launch.hostname-type",
            vec!["resource-name".into()],
            vec![7],
        ),
        (
            "private-dns-name-options-on-launch.hostname-type",
            vec!["ip-name".into()],
            vec![0, 1, 2, 3, 4, 5, 6],
        ),
        (
            "private-dns-name-options-on-launch.enable-resource-name-dns-a-record",
            vec!["true".into()],
            vec![7],
        ),
        (
            "private-dns-name-options-on-launch.enable-resource-name-dns-aaaa-record",
            vec!["true".into()],
            vec![7],
        ),
        ("state", vec!["pending".into()], vec![5]),
        (
            "state",
            vec!["available".into(), "pending".into()],
            all.clone(),
        ),
        ("subnet-arn", vec![subnet_arn], vec![4]),
        ("subnet-id", vec![seeded.ids[6].clone()], vec![6]),
        (
            "subnet-id",
            vec![seeded.ids[0].clone(), seeded.ids[3].clone()],
            vec![0, 3],
        ),
        ("tag-key", vec!["Owner".into()], vec![3, 4]),
        ("tag-key", vec!["Project".into()], vec![7]),
        ("tag:Name", vec!["web".into()], vec![3, 5]),
        ("tag:Owner", vec!["TeamA".into()], vec![3]),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("tag:Owner", vec!["teama".into()], vec![]),
        ("vpc-id", vec![seeded.default_vpc_id.clone()], vec![0, 1, 2]),
        ("vpc-id", vec![seeded.fixture_vpc_id.clone()], vec![
            3, 4, 5, 6, 7,
        ]),
        // Filter values carry EC2's wildcards: `*` for zero or more characters, `?` for
        // exactly one ("List and filter using the CLI and API", the guide the modelled
        // `Filter` shape links to).
        ("cidr-block", vec!["10.*".into()], vec![3, 4, 5, 6, 7]),
        ("cidr-block", vec!["172.31.?.0/20".into()], vec![0]),
        ("tag:Name", vec!["we*".into()], vec![3, 5]),
        ("tag:Name", vec!["*atch".into()], vec![4]),
        ("tag-key", vec!["*".into()], vec![3, 4, 5, 7]),
        // A documented filter carrying no value at all: there is no value for the
        // attribute to match, so it selects nothing rather than everything.
        ("state", vec![], vec![]),
        ("subnet-id", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the subnets that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the subnets matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("default-for-az", &["false"][..]),
                ("tag:Name", &["web"][..]),
            ],
            vec![3, 5],
        ),
        (
            vec![
                ("state", &["available"][..]),
                ("tag:Name", &["web"][..]),
                ("cidr-block", &["10.0.0.0/20"][..]),
            ],
            vec![3],
        ),
        // Each filter matches something, but no subnet matches both.
        (
            vec![
                ("default-for-az", &["true"][..]),
                ("state", &["pending"][..]),
            ],
            vec![],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name the operation does not define is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a near-miss of a defined name, a filter of a different describe, the
    // underscore spelling, a defined name with the wrong case, the tag filter without its
    // key separator, the tag filter with a separator but no key, and an empty name.
    let cases = [
        "cidr-blocks",
        "is-default",
        "default_for_az",
        "Default-For-Az",
        "availabilityzone",
        "tag",
        "tag:",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        // The EC2 error envelope is XML, so the message's apostrophes arrive escaped.
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// SubnetIds
// ---------------------------------------------------------------------------

/// `SubnetIds` narrows the report to the ids named, in store order whatever order they
/// were asked for.
#[tokio::test]
async fn subnet_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![7],
        vec![2, 4],
        vec![4, 2],
        vec![5, 1, 3],
        (0..seeded.ids.len()).collect(),
    ];
    for asked in cases {
        let ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("SubnetIds={borrowed:?}");
        let reported = described_ids(&router, &case, &indexed("SubnetId", &borrowed)).await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        let expected = ids_at(&seeded, &expected);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A `SubnetIds` entry that names no subnet of the account is refused with
/// `InvalidSubnetID.NotFound` — even alongside ids that do exist.
#[tokio::test]
async fn a_subnet_id_that_names_no_subnet_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.ids[0].clone();

    // Rows: a well-formed id nothing has, the 8-hex-digit id form, a malformed id, and an
    // unknown id carried beside a known one.
    let cases: Vec<Vec<String>> = vec![
        vec!["subnet-0ffffffffffffffff".to_string()],
        vec!["subnet-0abc1234".to_string()],
        vec!["not-a-subnet-id".to_string()],
        vec![known.clone(), "subnet-0ffffffffffffffff".to_string()],
        vec!["subnet-0ffffffffffffffff".to_string(), known],
    ];
    for ids in cases {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("SubnetIds={borrowed:?}");
        let (status, body) = describe(&router, &indexed("SubnetId", &borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSubnetID.NotFound",
            "case {case:?}: expected InvalidSubnetID.NotFound, got {body}"
        );
        assert!(
            elements(&body, "subnetId").is_empty(),
            "case {case:?}: a refused request reports no subnet, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every subnet exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_subnet_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeSubnetsMaxResults` is modelled `range 5..1000`, so the rows are its
    // published bounds, a size that divides the store exactly, one that does not, and the
    // size that holds everything.
    for max_results in [5, 6, 8, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "subnetId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: expected every subnet exactly once in store order, got {walked:?}"
        );
    }
}

/// A page size is a cut of the filtered set, not of the whole store: `MaxResults` and a
/// filter compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let mut params = filters(&[("default-for-az", &["false"])]);
    params.extend(max_results_param(5));
    let first = described_ids(&router, "filtered first page", &params).await;
    assert_eq!(
        first,
        ids_at(&seeded, &[3, 4, 5, 6, 7]),
        "filtered first page: expected the five non-default subnets, got {first:?}"
    );

    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "filtered first page: got {status} {body}");
    assert!(
        elements(&body, "nextToken").is_empty(),
        "filtered first page: the filtered set ends here, so no token is due, got {body}"
    );

    let mut narrower = filters(&[("vpc-id", &[seeded.fixture_vpc_id.as_str()])]);
    narrower.extend(max_results_param(5));
    let page = described_ids(&router, "one VPC's subnets", &narrower).await;
    assert_eq!(
        page,
        ids_at(&seeded, &[3, 4, 5, 6, 7]),
        "one VPC's subnets: expected the fixture VPC's subnets, got {page:?}"
    );
}

/// A token is issued exactly when a subnet follows the page, and resuming from it starts
/// at that subnet.
#[tokio::test]
async fn a_token_is_issued_only_when_a_subnet_follows_the_page() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &max_results_param(5)).await;
    assert_eq!(status, 200, "first page: got {status} {body}");
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("first page: expected a NextToken in {body}"));

    let mut params = max_results_param(5);
    params.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "second page", &params).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[5, 6, 7]),
        "second page: expected the subnets after the first five, got {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`range 5..1000`) and one past each.
    let cases = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and that id hex-encoded (the shape
    // the token takes, without the marker and checksum a minted token carries).
    let forged_marker = ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded.ids[0]));
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(&seeded.ids[0]),
        forged_marker,
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[("DryRun".to_string(), "true".to_string())]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "subnetId").is_empty(),
        "DryRun=true: a dry run reports no subnet, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded.ids,
        "DryRun=false: expected the ordinary report, got {reported:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `CreateDefaultVpc` end to end over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=CreateDefaultVpc`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). The Region lives in the
//! SigV4 credential scope of each call, so each row below is one Region's own default VPC.
//!
//! The rule under test: `CreateDefaultVpc` creates THE DEFAULT VPC OF THE REGION THE REQUEST
//! IS SIGNED FOR — "a default VPC with a size /16 IPv4 CIDR block and a default subnet in
//! each Availability Zone", with that VPC's default security group (`CreateDefaultVpc`;
//! Amazon VPC User Guide, "Default VPCs") — and refuses the request when the account already
//! holds one there: "you cannot have more than one default VPC per Region"
//! (`CreateDefaultVpc`). An account that "deleted your previous default VPC" is the one
//! caller this serves, and the network it gets back is the same network a fresh account
//! holds, so every operation that falls back on "the default VPC" of the Region then finds
//! the new one.
//!
//! The rows are Regions — what the rule generalises over, since a default VPC is per Region
//! and its subnets' zones and its group's ARN name that Region — spanning three partitions,
//! and each row is checked against the Region catalog so no row can name a Region that does
//! not exist. The ambient network each Region already holds is
//! [`super::default_network_per_region`]'s subject; this suite is what CREATING one does.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::state::{
    is_catalog_region, ComAmazonawsEc2Amazonec2State as Ec2State, AVAILABILITY_ZONES_PER_REGION,
    DEFAULT_VPC_ALREADY_EXISTS, DEFAULT_VPC_CIDR_BLOCK,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::wire::{is_long_id, member, param, Param};

/// The Regions the tables below are over: Regions of three partitions, including the one the
/// store is constructed for (`us-east-1`).
///
/// Written out here rather than derived, so a row is a deliberate expectation; every row is
/// checked against its source — the Region catalog [`is_catalog_region`] reads — inside the
/// tests, so this list cannot drift into naming a Region that does not exist.
const REGIONS: [&str; 4] = ["us-east-1", "eu-central-1", "us-gov-west-1", "cn-north-1"];

/// A Region code that is well formed but names no Region of the catalog: there is no
/// Availability Zone there to put a default subnet in.
const REGION_OUTSIDE_THE_CATALOG: &str = "zz-nowhere-1";

/// The message AWS words [`DEFAULT_VPC_ALREADY_EXISTS`] with — fixed text with no variable
/// part: it says only that the account already has one where it asked.
const ALREADY_EXISTS_MESSAGE: &str = "A Default VPC already exists for this account in this region.";

/// Long enough past [`ec2::state::TRANSITION_DELAY`] that the move the clock owes a created
/// VPC is due, without depending on the exact span.
fn past_the_transition() -> Duration {
    ec2::state::TRANSITION_DELAY + Duration::from_millis(400)
}

/// One `action` call signed for `region`, as its HTTP status and XML body.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> (u16, String) {
    super::ec2_query::call(router, region, action, params).await
}

/// One `action` call signed for `region`, required to succeed, as its XML body.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> String {
    let (status, body) = call(router, region, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The id of the default VPC `region` holds, as the store answers it — the index every
/// operation that falls back on "the default VPC" reads.
fn held_default_vpc(engine: &engine::InMemoryEngine, region: &str) -> Option<String> {
    engine.state::<Ec2State>().lock().default_vpc_id(region)
}

/// How many VPCs, subnets and security groups the store holds, so a refusal can be shown to
/// have written nothing.
fn held_counts(engine: &engine::InMemoryEngine) -> (usize, usize, usize) {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    (
        guard.vpcs.len(),
        guard.subnets.len(),
        guard.security_groups.len(),
    )
}

/// The subnet ids `DescribeSubnets` reports for `vpc_id`.
async fn subnets_of(case: &str, router: &MultiServiceRouter, region: &str, vpc_id: &str) -> Vec<String> {
    let body = ok(
        case,
        router,
        region,
        "DescribeSubnets",
        &filters(&[("vpc-id", &[vpc_id])]),
    )
    .await;
    elements(&body, "subnetId")
}

/// Delete the whole default network `region` holds — its default subnets first, because "you
/// must detach or delete all gateways and resources that are associated with the VPC before
/// you can delete it" (`DeleteVpc`) — and answer the `VpcId` that was deleted.
///
/// The state a caller that "deleted your previous default VPC" is in, reached the way that
/// caller reaches it: over the wire, through the operations that do it.
async fn delete_the_default_network(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    engine: &engine::InMemoryEngine,
) -> String {
    // The read is what materialises the ambient network of a Region nothing has touched yet,
    // so the id is read after it rather than before.
    let _ = ok(case, router, region, "DescribeVpcs", &[]).await;
    let vpc_id = held_default_vpc(engine, region)
        .unwrap_or_else(|| panic!("case {case:?}: an account holds a default VPC in {region}"));

    for subnet_id in subnets_of(case, router, region, &vpc_id).await {
        let _ = ok(
            case,
            router,
            region,
            "DeleteSubnet",
            &[param("SubnetId", &subnet_id)],
        )
        .await;
    }
    let _ = ok(
        case,
        router,
        region,
        "DeleteVpc",
        &[param("VpcId", &vpc_id)],
    )
    .await;
    assert_eq!(
        held_default_vpc(engine, region),
        None,
        "case {case:?}: after the delete the Region holds no default VPC"
    );
    vpc_id
}

/// The zone names `DescribeAvailabilityZones` reports for `region` — the zones the Region
/// HAS, read over the wire rather than written down here, so the expectation cannot drift
/// from what the service reports.
async fn zone_names(case: &str, router: &MultiServiceRouter, region: &str) -> BTreeSet<String> {
    let body = ok(case, router, region, "DescribeAvailabilityZones", &[]).await;
    elements(&body, "zoneName").into_iter().collect()
}

// ---------------------------------------------------------------------------
// One default VPC per Region
// ---------------------------------------------------------------------------

/// The rule: a Region that already has the account's default VPC refuses a `CreateDefaultVpc`
/// with [`DEFAULT_VPC_ALREADY_EXISTS`], and writes nothing — whether or not anything had
/// read that Region's network before, because the network has been owed since the account
/// existed rather than since the first read.
///
/// The rows differ in the Region and in whether the account's network there had already been
/// discovered when the create was called.
#[tokio::test]
async fn create_default_vpc_refuses_a_region_that_already_has_the_default_vpc() {
    let (router, engine) = super::ec2_query::emulator();

    // (case, region, whether the Region's network is read before the create)
    let cases: [(&str, &str, bool); 5] = [
        ("the Region the store is constructed for", REGIONS[0], false),
        ("a Region nothing has read yet", REGIONS[1], false),
        ("a Region already read through DescribeVpcs", REGIONS[1], true),
        ("a Region of another partition", REGIONS[2], false),
        ("a Region of a third partition", REGIONS[3], true),
    ];

    for (case, region, read_first) in cases {
        assert!(
            is_catalog_region(region),
            "case {case:?}: this table row must name a Region of the catalog"
        );
        if read_first {
            let _ = ok(case, &router, region, "DescribeVpcs", &[]).await;
        }

        let before_default = held_default_vpc(&engine, region);
        let before = held_counts(&engine);
        let (status, body) = call(&router, region, "CreateDefaultVpc", &[]).await;
        // A Region nothing had read is where this is NOT a free refusal: the account's
        // network there has not been materialised, and the answer is still that it already
        // has one, because the network has been owed since the account existed.
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a second default VPC, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            DEFAULT_VPC_ALREADY_EXISTS,
            "case {case:?}: expected {DEFAULT_VPC_ALREADY_EXISTS}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            ALREADY_EXISTS_MESSAGE,
            "case {case:?}: expected the fixed text of this error class, got {body}"
        );
        // The refusal is a READ: it writes nothing at all, materialised network or not.
        assert_eq!(
            held_counts(&engine),
            before,
            "case {case:?}: a refused create must write nothing, it held {before:?}"
        );
        assert_eq!(
            held_default_vpc(&engine, region),
            before_default,
            "case {case:?}: a refused create must not change which VPC is the default"
        );

        // And the default VPC the refusal was about is the one the Region reports: a read of
        // the Region materialises the network the account has been owed, and it is a default
        // VPC of the published shape.
        let _ = ok(case, &router, region, "DescribeVpcs", &[]).await;
        let held = held_default_vpc(&engine, region)
            .unwrap_or_else(|| panic!("case {case:?}: the Region holds its default VPC"));
        if let Some(before_default) = before_default {
            assert_eq!(
                held, before_default,
                "case {case:?}: the Region's default VPC must be the one it already had"
            );
        }
        let body = ok(
            case,
            &router,
            region,
            "DescribeVpcs",
            &indexed("VpcId", &[held.as_str()]),
        )
        .await;
        for (name, expected) in [
            ("isDefault", "true".to_string()),
            ("cidrBlock", DEFAULT_VPC_CIDR_BLOCK.to_string()),
        ] {
            assert_eq!(
                member(case, &body, name),
                expected,
                "case {case:?}: expected <{name}>{expected}</{name}>, got {body}"
            );
        }
    }}

/// The rule: once the previous default VPC is gone, `CreateDefaultVpc` creates the Region's
/// default VPC again — the `/16` at [`DEFAULT_VPC_CIDR_BLOCK`], `IsDefault`, one default
/// subnet in every Availability Zone of THAT Region and the VPC's default security group —
/// answers it in the non-terminal `pending` state, and every member of it reads back through
/// `DescribeVpcs`, `DescribeSubnets` and `DescribeSecurityGroups`. The VPC the Region now
/// falls back on is the new one, and a second create is refused again.
///
/// The rows differ in the Region, which is what the network's zones, its subnets' ARNs and
/// its group's ARN are derived from.
#[tokio::test]
async fn create_default_vpc_creates_the_regions_network_after_the_previous_one_was_deleted() {
    let (router, engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id().to_string();
    let mut created: Vec<(&str, String)> = Vec::new();

    for region in REGIONS {
        let case = region;
        assert!(
            is_catalog_region(region),
            "case {case:?}: this table row must name a Region of the catalog"
        );
        let zones = zone_names(case, &router, region).await;
        assert_eq!(
            zones.len(),
            AVAILABILITY_ZONES_PER_REGION,
            "case {case:?}: expected the Region's zones, got {zones:?}"
        );
        let deleted = delete_the_default_network(case, &router, region, &engine).await;

        let body = ok(case, &router, region, "CreateDefaultVpc", &[]).await;
        let vpc_id = element(&body, "vpcId")
            .unwrap_or_else(|| panic!("case {case:?}: the create must report the VPC: {body}"));
        assert!(
            is_long_id("vpc", &vpc_id),
            "case {case:?}: expected a vpc-<17 hex> id, got {vpc_id:?}"
        );
        assert_ne!(
            vpc_id, deleted,
            "case {case:?}: the new default VPC is a new resource, not the deleted one"
        );
        for (name, expected) in [
            ("cidrBlock", DEFAULT_VPC_CIDR_BLOCK.to_string()),
            ("isDefault", "true".to_string()),
            ("state", "pending".to_string()),
            ("instanceTenancy", "default".to_string()),
            ("ownerId", account.clone()),
        ] {
            assert_eq!(
                member(case, &body, name),
                expected,
                "case {case:?}: expected <{name}>{expected}</{name}>, got {body}"
            );
        }
        let dhcp_options = member(case, &body, "dhcpOptionsId");
        assert!(
            dhcp_options.starts_with("dopt-"),
            "case {case:?}: a default VPC inherits the Region's DHCP option set, got {body}"
        );

        // The VPC reads back as the Region's default VPC.
        let read_back = ok(
            case,
            &router,
            region,
            "DescribeVpcs",
            &indexed("VpcId", &[vpc_id.as_str()]),
        )
        .await;
        for (name, expected) in [
            ("cidrBlock", DEFAULT_VPC_CIDR_BLOCK.to_string()),
            ("isDefault", "true".to_string()),
            ("dhcpOptionsId", dhcp_options.clone()),
        ] {
            assert_eq!(
                member(case, &read_back, name),
                expected,
                "case {case:?}: expected <{name}>{expected}</{name}> from DescribeVpcs, \
                 got {read_back}"
            );
        }

        // "A default subnet in each Availability Zone": one per zone of THIS Region, each
        // `DefaultForAz` and public-IP-on-launch, and no two in the same zone.
        let subnets = ok(
            case,
            &router,
            region,
            "DescribeSubnets",
            &filters(&[("vpc-id", &[vpc_id.as_str()])]),
        )
        .await;
        let subnet_ids = elements(&subnets, "subnetId");
        assert_eq!(
            subnet_ids.len(),
            AVAILABILITY_ZONES_PER_REGION,
            "case {case:?}: expected one default subnet per zone, got {subnets}"
        );
        let subnet_zones: BTreeSet<String> = elements(&subnets, "availabilityZone")
            .into_iter()
            .collect();
        assert_eq!(
            subnet_zones, zones,
            "case {case:?}: expected a subnet in every zone DescribeAvailabilityZones \
             reports, got {subnet_zones:?}"
        );
        for (name, expected) in [
            ("defaultForAz", "true"),
            ("mapPublicIpOnLaunch", "true"),
            ("state", "available"),
        ] {
            let reported = elements(&subnets, name);
            assert_eq!(
                reported,
                vec![expected.to_string(); subnet_ids.len()],
                "case {case:?}: every default subnet reports <{name}>{expected}</{name}>, \
                 got {reported:?}"
            );
        }

        // "Your VPC automatically comes with a default security group", so the created
        // default VPC has exactly the one group a caller can launch into.
        let groups = ok(
            case,
            &router,
            region,
            "DescribeSecurityGroups",
            &filters(&[("vpc-id", &[vpc_id.as_str()])]),
        )
        .await;
        assert_eq!(
            elements(&groups, "groupName"),
            vec!["default".to_string()],
            "case {case:?}: expected the default group of {vpc_id}, got {groups}"
        );

        // The Region now falls back on the VPC that was just created: a group created
        // without a `VpcId` lands in it — "the ID of the VPC. Required for a nondefault VPC"
        // (`CreateSecurityGroup$VpcId`).
        let fallback = ok(
            case,
            &router,
            region,
            "CreateSecurityGroup",
            &[
                param("GroupName", "after-the-default-vpc-was-recreated"),
                param("GroupDescription", "a group in the new default VPC of the Region"),
            ],
        )
        .await;
        let group_id = element(&fallback, "groupId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a groupId in {fallback}"));
        let fallback_group = ok(
            case,
            &router,
            region,
            "DescribeSecurityGroups",
            &indexed("GroupId", &[group_id.as_str()]),
        )
        .await;
        assert_eq!(
            element(&fallback_group, "vpcId"),
            Some(vpc_id.clone()),
            "case {case:?}: expected the group in the new default VPC {vpc_id}, got \
             {fallback_group}"
        );

        // "You cannot have more than one default VPC per Region" holds for the VPC this
        // operation itself created, not only for the one the account was given.
        let (status, again) = call(&router, region, "CreateDefaultVpc", &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second create is refused, got {status} with body {again}"
        );
        assert_eq!(
            error_code(&again),
            DEFAULT_VPC_ALREADY_EXISTS,
            "case {case:?}: expected {DEFAULT_VPC_ALREADY_EXISTS}, got {again}"
        );

        created.push((region, vpc_id));
    }

    // The move the clock owes every created VPC: `available` one TRANSITION_DELAY after the
    // create, whether or not anybody called in between. Waited out once for the whole table.
    tokio::time::sleep(past_the_transition()).await;
    for (region, vpc_id) in &created {
        let body = ok(
            region,
            &router,
            region,
            "DescribeVpcs",
            &indexed("VpcId", &[vpc_id.as_str()]),
        )
        .await;
        assert_eq!(
            member(region, &body, "state"),
            "available",
            "case {region:?}: one TRANSITION_DELAY after the create the VPC must be \
             available, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the create refuses
// ---------------------------------------------------------------------------

/// The rule: `DryRun` reports the answer the request would have got and writes nothing —
/// `DryRunOperation` for a create that would have succeeded, and the refusal itself for one
/// that would not.
///
/// The rows differ in whether the Region still has its default VPC, which is what decides
/// which of the two a dry run reports.
#[tokio::test]
async fn create_default_vpc_dry_run_reports_the_outcome_without_writing() {
    let (router, engine) = super::ec2_query::emulator();

    // (case, region, whether the Region's default VPC is deleted first, status, error code)
    let cases: [(&str, &str, bool, u16, &str); 2] = [
        (
            "a Region that still has its default VPC",
            REGIONS[0],
            false,
            400,
            DEFAULT_VPC_ALREADY_EXISTS,
        ),
        (
            "a Region whose default VPC was deleted",
            REGIONS[1],
            true,
            412,
            "DryRunOperation",
        ),
    ];

    for (case, region, delete_first, expected_status, expected_code) in cases {
        if delete_first {
            let _ = delete_the_default_network(case, &router, region, &engine).await;
        }
        let before = held_counts(&engine);

        let (status, body) = call(&router, region, "CreateDefaultVpc", &[param("DryRun", "true")]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            held_counts(&engine),
            before,
            "case {case:?}: a dry run must write nothing, it held {before:?}"
        );
        if delete_first {
            assert_eq!(
                held_default_vpc(&engine, region),
                None,
                "case {case:?}: a dry run must not create the default VPC"
            );
            // And the real call after it still works: the dry run refused nothing but
            // itself.
            let body = ok(case, &router, region, "CreateDefaultVpc", &[]).await;
            assert_eq!(
                member(case, &body, "isDefault"),
                "true",
                "case {case:?}: the create after a dry run must make the default VPC, got \
                 {body}"
            );
        }
    }
}

/// The rule: a credential scope naming something merely SHAPED like a Region code has no
/// Availability Zone to put a default subnet in, so the create is refused rather than
/// serving a default VPC with nothing in it — caller-controlled text cannot make the store
/// grow a network per value.
#[tokio::test]
async fn create_default_vpc_refuses_a_region_outside_the_catalog() {
    let (router, engine) = super::ec2_query::emulator();
    assert!(
        !is_catalog_region(REGION_OUTSIDE_THE_CATALOG),
        "this test needs a Region code the catalog does not hold"
    );

    let before = held_counts(&engine);
    let (status, body) = call(&router, REGION_OUTSIDE_THE_CATALOG, "CreateDefaultVpc", &[]).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for a Region that does not exist, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "UnsupportedOperation",
        "expected UnsupportedOperation, got {body}"
    );
    assert_eq!(
        held_counts(&engine),
        before,
        "a Region outside the catalog must not make the store grow, it held {before:?}"
    );
    assert_eq!(
        held_default_vpc(&engine, REGION_OUTSIDE_THE_CATALOG),
        None,
        "a Region outside the catalog holds no default VPC"
    );
}

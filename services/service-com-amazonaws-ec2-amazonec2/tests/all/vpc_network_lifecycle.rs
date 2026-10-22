// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the VPC-scoped network resource lifecycle — `CreateSubnet`,
//! `DeleteSubnet`, `CreateSecurityGroup`, `DeleteSecurityGroup` and `DeleteVpc` — driven
//! over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rule under test is that a VPC-SCOPED NETWORK RESOURCE IS CREATED INTO THE VPC THE
//! REQUEST NAMES, READS BACK THROUGH ITS DESCRIBE, AND IS DELETED ONLY WHILE NOTHING
//! DEPENDS ON IT — and that the VPC itself is deleted only once it holds none of them
//! ("You must detach or delete all gateways and resources that are associated with the VPC
//! before you can delete it … When you delete the VPC, it deletes the default security
//! group, network ACL, and route table for the VPC" — `DeleteVpc`), a request naming a
//! resource that is gone being refused with that resource's `Invalid*ID.NotFound` rather
//! than answered as a no-op.
//!
//! Every table row is a distinct input along what that generalises over — the block value
//! and its size, the zone, the tag count, the name's case and characters, which resource
//! holds the VPC, which member names the resource — so any one recorded call is one row
//! among many rather than the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed};
use super::wire::{param, Param};

/// The Region every call here is signed for: the Region the default VPC and its default
/// subnets are seeded for.
const REGION: &str = "us-east-1";

/// How long a `pending` resource takes to reach `available`: the span the clock owes a
/// `CreateVpc`/`CreateSubnet` answer ([`ec2::state::TRANSITION_DELAY`]), plus a margin so
/// a slow machine cannot read the state before it is due.
const SETTLE_WAIT: Duration = Duration::from_millis(900);

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A VPC with `cidr_block`, as its id — the container every resource below is created in.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateVpc",
        &[param("CidrBlock", cidr_block)],
    )
    .await;
    member(case, &body, "vpcId")
}

/// A subnet of `vpc_id` with `cidr_block`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    member(case, &body, "subnetId")
}

/// A security group of `vpc_id` named `group_name`, as its id.
async fn create_security_group(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    group_name: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSecurityGroup",
        &[
            param("VpcId", vpc_id),
            param("GroupName", group_name),
            param("GroupDescription", "a group the lifecycle suite created"),
        ],
    )
    .await;
    member(case, &body, "groupId")
}

/// The subnet ids `DescribeSubnets` reports for `vpc_id`.
async fn subnets_of(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeSubnets",
        &filters(&[("vpc-id", &[vpc_id])]),
    )
    .await;
    elements(&body, "subnetId")
}

/// The security group ids `DescribeSecurityGroups` reports for `vpc_id`.
///
/// Read off each group's `securityGroupArn`, the way [`super::describe_security_groups`]
/// does: a `<groupId>` element also appears inside a rule that references another group,
/// while the ARN is one per reported group and ends in that group's id.
async fn groups_of(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeSecurityGroups",
        &filters(&[("vpc-id", &[vpc_id])]),
    )
    .await;
    elements(&body, "securityGroupArn")
        .iter()
        .filter_map(|arn| arn.rsplit('/').next().map(str::to_string))
        .collect()
}

/// Whether `value` is an EC2 long resource id of `prefix` — the shared check the VPC suite
/// uses.
fn is_long_id(prefix: &str, value: &str) -> bool {
    super::wire::is_long_id(prefix, value)
}

// ---------------------------------------------------------------------------
// CreateSubnet
// ---------------------------------------------------------------------------

/// The rule: `CreateSubnet` stores a subnet of the VPC the request names, carrying the
/// requested IPv4 block in canonical form, and every member of the subnet it answers reads
/// back through `DescribeSubnets`.
///
/// The rows differ in the block size, in how many host bits the request set, and in where
/// inside the VPC the block sits; both published bounds of the subnet block size ("between
/// a /28 netmask (16 IP addresses) and a /16 netmask (65,536 IP addresses)" —
/// `CreateSubnet`) are rows of their own.
#[tokio::test]
async fn create_subnet_stores_the_requested_block_and_round_trips_through_describe_subnets() {
    let router = emulator();
    // (case, the VPC's block, the requested block, the canonical form, the addresses left)
    let cases = [
        ("smallest allowed block", "10.1.0.0/16", "10.1.4.16/28", "10.1.4.16/28", 11),
        ("largest allowed block", "10.2.0.0/16", "10.2.0.0/16", "10.2.0.0/16", 65531),
        ("host bits set", "10.3.0.0/16", "10.3.9.37/24", "10.3.9.0/24", 251),
        ("a /20 mid-VPC", "10.4.0.0/16", "10.4.32.0/20", "10.4.32.0/20", 4091),
        ("a /26", "192.168.0.0/20", "192.168.3.64/26", "192.168.3.64/26", 59),
    ];

    for (case, vpc_cidr, requested, canonical, available) in cases {
        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        let body = ok(
            case,
            &router,
            "CreateSubnet",
            &[param("VpcId", &vpc_id), param("CidrBlock", requested)],
        )
        .await;

        let subnet_id = member(case, &body, "subnetId");
        assert!(
            is_long_id("subnet", &subnet_id),
            "case {case:?}: expected a long subnet id, got {subnet_id:?}"
        );
        let reported_cidr = member(case, &body, "cidrBlock");
        assert_eq!(
            reported_cidr, canonical,
            "case {case:?}: expected the canonical block {canonical:?}, got {reported_cidr:?}"
        );
        let reported_vpc = member(case, &body, "vpcId");
        assert_eq!(
            reported_vpc, vpc_id,
            "case {case:?}: expected the subnet in VPC {vpc_id:?}, got {reported_vpc:?}"
        );
        let state = member(case, &body, "state");
        assert_eq!(
            state,
            ec2::state::SUBNET_STATE_PENDING,
            "case {case:?}: expected a subnet the clock still owes `available`, got {state:?}"
        );
        let count = member(case, &body, "availableIpAddressCount");
        assert_eq!(
            count,
            available.to_string(),
            "case {case:?}: expected {available} available addresses, got {count:?}"
        );
        // Only a default VPC's own subnets are the default for their zone or auto-assign a
        // public address (Amazon VPC User Guide, "Default VPCs").
        for (name, expected) in [("defaultForAz", "false"), ("mapPublicIpOnLaunch", "false")] {
            let reported = member(case, &body, name);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {name} {expected:?}, got {reported:?}"
            );
        }

        // The round trip: every member above is what the describe reports too.
        let described = ok(
            case,
            &router,
            "DescribeSubnets",
            &indexed("SubnetId", &[&subnet_id]),
        )
        .await;
        for name in [
            "subnetId",
            "cidrBlock",
            "vpcId",
            "availabilityZone",
            "availabilityZoneId",
            "availableIpAddressCount",
            "ownerId",
            "subnetArn",
            "defaultForAz",
            "mapPublicIpOnLaunch",
        ] {
            let created = member(case, &body, name);
            let read_back = member(case, &described, name);
            assert_eq!(
                read_back, created,
                "case {case:?}: DescribeSubnets reported {name} {read_back:?}, \
                 CreateSubnet answered {created:?}"
            );
        }
    }
}

/// The rule: the zone a subnet is in is the zone the request named — by name or by id —
/// and the Region's first Availability Zone when it named none.
#[tokio::test]
async fn create_subnet_places_the_subnet_in_the_zone_the_request_names() {
    let router = emulator();
    let zones = ec2::state::availability_zones_in_region(REGION);
    let first = zones.first().expect("us-east-1 has Availability Zones").clone();
    let last = zones.last().expect("us-east-1 has Availability Zones").clone();
    // (case, the zone members the request carries, the zone name expected)
    let cases: [(&str, Vec<Param>, String); 4] = [
        ("no zone named", Vec::new(), first.zone_name.clone()),
        (
            "by zone name",
            vec![param("AvailabilityZone", &last.zone_name)],
            last.zone_name.clone(),
        ),
        (
            "by zone id",
            vec![param("AvailabilityZoneId", &last.zone_id)],
            last.zone_name.clone(),
        ),
        (
            "by both, agreeing",
            vec![
                param("AvailabilityZone", &first.zone_name),
                param("AvailabilityZoneId", &first.zone_id),
            ],
            first.zone_name.clone(),
        ),
    ];

    let vpc_id = create_vpc("zones", &router, "10.9.0.0/16").await;
    for (index, (case, zone_params, expected)) in cases.into_iter().enumerate() {
        let mut params = vec![
            param("VpcId", &vpc_id),
            param("CidrBlock", &format!("10.9.{index}.0/24")),
        ];
        params.extend(zone_params);
        let body = ok(case, &router, "CreateSubnet", &params).await;
        let reported = member(case, &body, "availabilityZone");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the subnet in zone {expected:?}, got {reported:?}"
        );
    }
}

/// The rule: a `CreateSubnet` the service cannot satisfy is refused with the code that
/// says why, and nothing is stored.
///
/// The rows are the ways a request can fail to name a usable block in a usable VPC: a
/// block outside the VPC, one overlapping a subnet already there (in each of the three
/// forms an overlap takes), one outside the published block size, one that is not a block
/// at all, a missing block, a VPC that does not exist, and a zone the Region does not
/// have.
#[tokio::test]
async fn create_subnet_refuses_a_block_it_cannot_place() {
    let router = emulator();
    let vpc_id = create_vpc("refusals", &router, "10.20.0.0/16").await;
    let held = create_subnet("refusals", &router, &vpc_id, "10.20.8.0/22").await;
    assert!(
        is_long_id("subnet", &held),
        "the suite's own fixture subnet must exist, got {held:?}"
    );

    // (case, the request's fields, the EC2 error code, the HTTP status)
    let cases: [(&str, Vec<Param>, &str, u16); 11] = [
        (
            "the identical block",
            vec![param("VpcId", &vpc_id), param("CidrBlock", "10.20.8.0/22")],
            "InvalidSubnet.Conflict",
            400,
        ),
        (
            "a block inside the held one",
            vec![param("VpcId", &vpc_id), param("CidrBlock", "10.20.9.0/24")],
            "InvalidSubnet.Conflict",
            400,
        ),
        (
            "a block containing the held one",
            vec![param("VpcId", &vpc_id), param("CidrBlock", "10.20.0.0/16")],
            "InvalidSubnet.Conflict",
            400,
        ),
        (
            "a block straddling the held one",
            vec![param("VpcId", &vpc_id), param("CidrBlock", "10.20.10.0/23")],
            "InvalidSubnet.Conflict",
            400,
        ),
        (
            "a block in another private range",
            vec![param("VpcId", &vpc_id), param("CidrBlock", "192.168.4.0/24")],
            "InvalidSubnet.Range",
            400,
        ),
        (
            "a block just past the VPC's",
            vec![param("VpcId", &vpc_id), param("CidrBlock", "10.21.0.0/24")],
            "InvalidSubnet.Range",
            400,
        ),
        (
            "a block smaller than the smallest allowed",
            vec![param("VpcId", &vpc_id), param("CidrBlock", "10.20.32.0/29")],
            "InvalidParameterValue",
            400,
        ),
        (
            "a block larger than the largest allowed",
            vec![param("VpcId", &vpc_id), param("CidrBlock", "10.0.0.0/15")],
            "InvalidParameterValue",
            400,
        ),
        (
            "not a CIDR block at all",
            vec![param("VpcId", &vpc_id), param("CidrBlock", "10.20.32.0")],
            "InvalidParameterValue",
            400,
        ),
        (
            "no block at all",
            vec![param("VpcId", &vpc_id)],
            "MissingParameter",
            400,
        ),
        (
            "a VPC that does not exist",
            vec![
                param("VpcId", "vpc-00000000000000bad"),
                param("CidrBlock", "10.20.32.0/24"),
            ],
            "InvalidVpcID.NotFound",
            400,
        ),
    ];

    for (case, params, expected_code, expected_status) in cases {
        let (status, body) = call(&router, "CreateSubnet", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} in {body}"
        );
    }

    // A zone the Region does not have, and an IPv6 subnet this emulator holds no state
    // for: the first is the caller's error, the second is honestly unwritten (501).
    let (status, body) = call(
        &router,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", "10.20.32.0/24"),
            param("AvailabilityZone", "us-east-1z"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a zone the Region does not have must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a zone the Region does not have must be an InvalidParameterValue: {body}"
    );
    let (status, body) = call(
        &router,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", "10.20.32.0/24"),
            param("Ipv6CidrBlock", "2600:1f14:abc:1200::/64"),
        ],
    )
    .await;
    assert_eq!(
        status, 501,
        "an IPv6 subnet is not written yet and must answer 501, got {status} with body {body}"
    );

    // Nothing above was stored: the VPC still holds the one subnet the suite created.
    let reported = subnets_of("refusals", &router, &vpc_id).await;
    assert_eq!(
        reported,
        vec![held.clone()],
        "a refused CreateSubnet must store nothing, got {reported:?}"
    );
}

/// The rule: a subnet is answered in the non-terminal `pending` state and the clock owes
/// it `available`, so a caller reading straight after the create still sees `pending`.
#[tokio::test]
async fn create_subnet_settles_to_available_when_the_clock_owes_it() {
    let case = "the clock-owed move";
    let router = emulator();
    let vpc_id = create_vpc(case, &router, "10.30.0.0/16").await;
    let subnet_id = create_subnet(case, &router, &vpc_id, "10.30.1.0/24").await;

    let before = ok(
        case,
        &router,
        "DescribeSubnets",
        &indexed("SubnetId", &[&subnet_id]),
    )
    .await;
    let state = member(case, &before, "state");
    assert_eq!(
        state,
        ec2::state::SUBNET_STATE_PENDING,
        "case {case:?}: a subnet read straight after the create must still be pending, \
         got {state:?}"
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let after = ok(
        case,
        &router,
        "DescribeSubnets",
        &indexed("SubnetId", &[&subnet_id]),
    )
    .await;
    let state = member(case, &after, "state");
    assert_eq!(
        state,
        ec2::state::SUBNET_STATE_AVAILABLE,
        "case {case:?}: a subnet past its due instant must report available, got {state:?}"
    );
}

// ---------------------------------------------------------------------------
// DeleteSubnet
// ---------------------------------------------------------------------------

/// The rule: `DeleteSubnet` removes the subnet the request names, the describe stops
/// reporting it, and a second delete of the same id is refused with
/// `InvalidSubnetID.NotFound` rather than answered as a no-op.
///
/// The rows differ in which subnet is deleted — one the suite created, one of the default
/// VPC's seeded default subnets — and in the block it carries.
#[tokio::test]
async fn delete_subnet_removes_the_subnet_and_refuses_an_unknown_id() {
    let router = emulator();
    let vpc_id = create_vpc("deletes", &router, "10.40.0.0/16").await;
    let created: Vec<String> = {
        let mut ids = Vec::new();
        for block in ["10.40.1.0/24", "10.40.2.0/28", "10.40.16.0/20"] {
            ids.push(create_subnet(block, &router, &vpc_id, block).await);
        }
        ids
    };
    // The default VPC's own subnets, which a fresh account already has: deleting one of
    // those is the same operation on a subnet nothing in this test created.
    let default_vpc_subnets = {
        let body = ok(
            "the default VPC",
            &router,
            "DescribeSubnets",
            &filters(&[("default-for-az", &["true"])]),
        )
        .await;
        elements(&body, "subnetId")
    };
    assert!(
        !default_vpc_subnets.is_empty(),
        "a fresh account's default VPC must come with default subnets"
    );

    let mut cases: Vec<String> = created.clone();
    cases.push(
        default_vpc_subnets
            .first()
            .expect("at least one default subnet")
            .clone(),
    );

    for subnet_id in cases {
        let case = subnet_id.clone();
        let body = ok(
            &case,
            &router,
            "DeleteSubnet",
            &[param("SubnetId", &subnet_id)],
        )
        .await;
        assert!(
            !body.contains("<Error>"),
            "case {case:?}: DeleteSubnet answered an error envelope: {body}"
        );

        // The describe stops reporting it.
        let (status, body) = call(
            &router,
            "DescribeSubnets",
            &indexed("SubnetId", &[&subnet_id]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted subnet must not be describable, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSubnetID.NotFound",
            "case {case:?}: expected InvalidSubnetID.NotFound from the describe: {body}"
        );

        // And the second delete is refused rather than answered as a no-op.
        let (status, body) = call(&router, "DeleteSubnet", &[param("SubnetId", &subnet_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second DeleteSubnet must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSubnetID.NotFound",
            "case {case:?}: expected InvalidSubnetID.NotFound from the second delete: {body}"
        );
    }

    let left = subnets_of("deletes", &router, &vpc_id).await;
    assert!(
        left.is_empty(),
        "every subnet of the VPC was deleted, but the describe still reports {left:?}"
    );
}

/// The rule: a `DryRun` delete reports that it would have succeeded and removes nothing.
///
/// The rows are the two deletes this suite covers, each on a resource whose delete WOULD
/// have succeeded — a dry run reports `DryRunOperation` only for a request that would
/// otherwise have gone through, so the VPC row gets a VPC nothing is associated with.
#[tokio::test]
async fn a_dry_run_delete_removes_nothing() {
    let router = emulator();
    let held_vpc = create_vpc("dry runs", &router, "10.50.0.0/16").await;
    let subnet_id = create_subnet("dry runs", &router, &held_vpc, "10.50.1.0/24").await;
    let empty_vpc = create_vpc("dry runs", &router, "10.51.0.0/16").await;
    // (case, the action, the fields that name the resource)
    let cases: [(&str, &str, Vec<Param>); 2] = [
        (
            "DeleteSubnet",
            "DeleteSubnet",
            vec![param("SubnetId", &subnet_id), param("DryRun", "true")],
        ),
        (
            "DeleteVpc",
            "DeleteVpc",
            vec![param("VpcId", &empty_vpc), param("DryRun", "true")],
        ),
    ];

    for (case, action, params) in cases {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: a dry run must answer HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation: {body}"
        );
    }

    let left = subnets_of("dry runs", &router, &held_vpc).await;
    assert_eq!(
        left,
        vec![subnet_id.clone()],
        "a dry run must remove nothing, but the VPC now holds {left:?}"
    );
    let (status, body) = call(&router, "DescribeVpcs", &indexed("VpcId", &[&empty_vpc])).await;
    assert_eq!(
        status, 200,
        "a dry run must leave the VPC in place, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// CreateSecurityGroup / DeleteSecurityGroup
// ---------------------------------------------------------------------------

/// The rule: `CreateSecurityGroup` stores a group in the VPC the request names, under the
/// name and description it carries, and the group reads back through
/// `DescribeSecurityGroups`.
///
/// The rows differ in the name's length, case and characters — "Valid characters: a-z,
/// A-Z, 0-9, spaces, and `._-:/()#,@[]+=&;{}!$*`"
/// (`CreateSecurityGroupRequest$GroupName`) — and in whether the request names the VPC or
/// leaves it to the account's default VPC.
#[tokio::test]
async fn create_security_group_stores_the_group_and_round_trips_through_describe() {
    let router = emulator();
    let vpc_id = create_vpc("groups", &router, "10.60.0.0/16").await;
    // (case, the name, the description)
    let cases = [
        ("a plain name", "alb-ingress", "the load balancer group"),
        ("mixed case", "Alb-Ingress-2", "a second group"),
        ("a name with spaces", "task group one", "spaces are valid"),
        (
            "documented punctuation",
            "task._-:/()#,@[]{}!$*group",
            "every documented punctuation character",
        ),
        ("a one-character name", "a", "the shortest name"),
    ];

    for (case, group_name, description) in cases {
        let body = ok(
            case,
            &router,
            "CreateSecurityGroup",
            &[
                param("VpcId", &vpc_id),
                param("GroupName", group_name),
                param("GroupDescription", description),
            ],
        )
        .await;
        let group_id = member(case, &body, "groupId");
        assert!(
            is_long_id("sg", &group_id),
            "case {case:?}: expected a long security group id, got {group_id:?}"
        );
        let arn = member(case, &body, "securityGroupArn");
        assert!(
            arn.ends_with(&format!("security-group/{group_id}")),
            "case {case:?}: expected an ARN naming {group_id:?}, got {arn:?}"
        );

        let described = ok(
            case,
            &router,
            "DescribeSecurityGroups",
            &indexed("GroupId", &[&group_id]),
        )
        .await;
        let reported_name = member(case, &described, "groupName");
        assert_eq!(
            reported_name, group_name,
            "case {case:?}: expected the stored name {group_name:?}, got {reported_name:?}"
        );
        let reported_description = member(case, &described, "groupDescription");
        assert_eq!(
            reported_description, description,
            "case {case:?}: expected the stored description {description:?}, \
             got {reported_description:?}"
        );
        let reported_vpc = member(case, &described, "vpcId");
        assert_eq!(
            reported_vpc, vpc_id,
            "case {case:?}: expected the group in VPC {vpc_id:?}, got {reported_vpc:?}"
        );
        // "By default, a security group includes an outbound rule that allows all outbound
        // traffic" (Amazon VPC User Guide, "Security group rules"), and no inbound rule.
        assert!(
            described.contains("<ipPermissionsEgress>"),
            "case {case:?}: a new group must carry its outbound rule: {described}"
        );
        assert!(
            described.contains("0.0.0.0/0"),
            "case {case:?}: the outbound rule must allow every IPv4 destination: \
             {described}"
        );
    }

    // A request that names no VPC puts the group in the account's default VPC.
    let case = "no VPC named";
    let body = ok(
        case,
        &router,
        "CreateSecurityGroup",
        &[
            param("GroupName", "in-the-default-vpc"),
            param("GroupDescription", "a group of the default VPC"),
        ],
    )
    .await;
    let group_id = member(case, &body, "groupId");
    let described = ok(
        case,
        &router,
        "DescribeSecurityGroups",
        &indexed("GroupId", &[&group_id]),
    )
    .await;
    let default_vpc = {
        let body = ok(
            case,
            &router,
            "DescribeVpcs",
            &filters(&[("is-default", &["true"])]),
        )
        .await;
        member(case, &body, "vpcId")
    };
    let reported_vpc = member(case, &described, "vpcId");
    assert_eq!(
        reported_vpc, default_vpc,
        "case {case:?}: a group created without a VpcId must be in the default VPC \
         {default_vpc:?}, got {reported_vpc:?}"
    );
}

/// The rule: a group name is unique within its VPC and case-insensitive, and a name that
/// is already taken there is refused with `InvalidGroup.Duplicate` — while the same name in
/// another VPC is a different group.
#[tokio::test]
async fn create_security_group_refuses_a_name_already_taken_in_the_vpc() {
    let router = emulator();
    let first_vpc = create_vpc("duplicates", &router, "10.70.0.0/16").await;
    let second_vpc = create_vpc("duplicates", &router, "10.71.0.0/16").await;
    let taken = create_security_group("duplicates", &router, &first_vpc, "service-sg").await;
    assert!(
        is_long_id("sg", &taken),
        "the suite's own fixture group must exist, got {taken:?}"
    );

    // (case, the name the second create asks for)
    for (case, name) in [
        ("the same name", "service-sg"),
        ("the same name upper-cased", "SERVICE-SG"),
        ("the same name mixed-cased", "Service-Sg"),
        ("the VPC's default group", ec2::state::DEFAULT_SECURITY_GROUP_NAME),
    ] {
        let (status, body) = call(
            &router,
            "CreateSecurityGroup",
            &[
                param("VpcId", &first_vpc),
                param("GroupName", name),
                param("GroupDescription", "a second group of this name"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name already taken in the VPC must be refused, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidGroup.Duplicate",
            "case {case:?}: expected InvalidGroup.Duplicate: {body}"
        );
    }

    // The same name in ANOTHER VPC is a different group, so it is accepted.
    let elsewhere = create_security_group("duplicates", &router, &second_vpc, "service-sg").await;
    assert_ne!(
        elsewhere, taken,
        "the same name in another VPC must be a different group, got {elsewhere:?}"
    );
}

/// The rule: a `CreateSecurityGroup` the service cannot satisfy is refused with the code
/// that says why, and nothing is stored.
#[tokio::test]
async fn create_security_group_refuses_a_name_or_description_it_cannot_accept() {
    let router = emulator();
    let vpc_id = create_vpc("group refusals", &router, "10.80.0.0/16").await;
    let too_long = "n".repeat(ec2::state::MAX_SECURITY_GROUP_TEXT_CHARS + 1);
    // (case, the name, the description, the EC2 error code)
    let cases: [(&str, &str, &str, &str); 7] = [
        ("an empty name", "", "a description", "MissingParameter"),
        ("an empty description", "a-group", "", "MissingParameter"),
        (
            "a name shaped like an id",
            "sg-0123456789abcdef0",
            "a description",
            "InvalidParameterValue",
        ),
        (
            "a name one character too long",
            &too_long,
            "a description",
            "InvalidParameterValue",
        ),
        (
            "a name with an undocumented character",
            "a|group",
            "a description",
            "InvalidParameterValue",
        ),
        (
            "a description with an undocumented character",
            "a-group",
            "a|description",
            "InvalidParameterValue",
        ),
        (
            "a description one character too long",
            "a-group",
            &too_long,
            "InvalidParameterValue",
        ),
    ];

    for (case, group_name, description, expected_code) in cases {
        let (status, body) = call(
            &router,
            "CreateSecurityGroup",
            &[
                param("VpcId", &vpc_id),
                param("GroupName", group_name),
                param("GroupDescription", description),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} in {body}"
        );
    }

    // A VPC that does not exist is refused by its own code.
    let (status, body) = call(
        &router,
        "CreateSecurityGroup",
        &[
            param("VpcId", "vpc-00000000000000bad"),
            param("GroupName", "a-group"),
            param("GroupDescription", "a description"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a VPC that does not exist must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVpcID.NotFound",
        "expected InvalidVpcID.NotFound: {body}"
    );

    // Nothing above was stored: the VPC still holds only the default group it came with.
    let reported = groups_of("group refusals", &router, &vpc_id).await;
    assert_eq!(
        reported.len(),
        1,
        "a refused CreateSecurityGroup must store nothing, but the VPC holds {reported:?}"
    );
}

/// The rule: `DeleteSecurityGroup` removes the group the request names — by id, or by name
/// within the default VPC — answers `true`, and refuses a second delete of the same group;
/// the default group of a VPC is refused with `CannotDelete`, because only `DeleteVpc`
/// removes it.
#[tokio::test]
async fn delete_security_group_removes_a_created_group_and_refuses_the_default_one() {
    let router = emulator();
    let vpc_id = create_vpc("group deletes", &router, "10.90.0.0/16").await;
    let by_id = create_security_group("by id", &router, &vpc_id, "delete-by-id").await;
    let default_vpc = {
        let body = ok(
            "the default VPC",
            &router,
            "DescribeVpcs",
            &filters(&[("is-default", &["true"])]),
        )
        .await;
        member("the default VPC", &body, "vpcId")
    };
    let by_name =
        create_security_group("by name", &router, &default_vpc, "delete-by-name").await;

    // (case, the fields that name the group, the id expected back)
    let cases: [(&str, Vec<Param>, String); 2] = [
        ("by id", vec![param("GroupId", &by_id)], by_id.clone()),
        (
            "by name in the default VPC",
            vec![param("GroupName", "delete-by-name")],
            by_name.clone(),
        ),
    ];

    for (case, params, expected_id) in cases {
        let body = ok(case, &router, "DeleteSecurityGroup", &params).await;
        let reported = member(case, &body, "return");
        assert_eq!(
            reported, "true",
            "case {case:?}: expected the delete to report true, got {reported:?}"
        );
        let reported_id = member(case, &body, "groupId");
        assert_eq!(
            reported_id, expected_id,
            "case {case:?}: expected the deleted group {expected_id:?}, got {reported_id:?}"
        );

        let (status, body) = call(
            &router,
            "DescribeSecurityGroups",
            &indexed("GroupId", &[&expected_id]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted group must not be describable, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidGroup.NotFound",
            "case {case:?}: expected InvalidGroup.NotFound from the describe: {body}"
        );

        let (status, body) = call(
            &router,
            "DeleteSecurityGroup",
            &[param("GroupId", &expected_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second delete must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidGroup.NotFound",
            "case {case:?}: expected InvalidGroup.NotFound from the second delete: {body}"
        );
    }

    // The default group of each VPC: refused, and still there afterwards.
    for vpc in [&vpc_id, &default_vpc] {
        let case = format!("the default group of {vpc}");
        let groups = groups_of(&case, &router, vpc).await;
        let default_group = groups
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the VPC must hold its default group"))
            .clone();
        let (status, body) = call(
            &router,
            "DeleteSecurityGroup",
            &[param("GroupId", &default_group)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the default group must not be deletable, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "CannotDelete",
            "case {case:?}: expected CannotDelete: {body}"
        );
        let still_there = groups_of(&case, &router, vpc).await;
        assert!(
            still_there.contains(&default_group),
            "case {case:?}: the refused delete must leave {default_group:?} in place, \
             got {still_there:?}"
        );
    }

    // A name and an id that name different groups cannot hold together.
    let other = create_security_group("mismatch", &router, &vpc_id, "a-third-group").await;
    let (status, body) = call(
        &router,
        "DeleteSecurityGroup",
        &[
            param("GroupId", &other),
            param("GroupName", "not-this-groups-name"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a GroupId and a GroupName naming different groups must be refused, got {status}: \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterCombination",
        "expected InvalidParameterCombination: {body}"
    );
    // And a request that names neither.
    let (status, body) = call(&router, "DeleteSecurityGroup", &[]).await;
    assert_eq!(
        status, 400,
        "a delete naming neither a GroupId nor a GroupName must be refused, got {status}: \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "MissingParameter",
        "expected MissingParameter: {body}"
    );
}

// ---------------------------------------------------------------------------
// DeleteVpc
// ---------------------------------------------------------------------------

/// The rule: `DeleteVpc` refuses with `DependencyViolation` while anything is still
/// associated with the VPC, and removes it — with the default security group it came with
/// — once nothing is.
///
/// The rows differ in WHAT holds the VPC: a subnet, a security group the caller created,
/// both at once, the default VPC's own seeded subnets, and nothing at all.
#[tokio::test]
async fn delete_vpc_refuses_while_anything_is_associated_with_it() {
    let router = emulator();

    // Row 1: a VPC held by a subnet.
    let case = "held by a subnet";
    let vpc_id = create_vpc(case, &router, "10.100.0.0/16").await;
    let subnet_id = create_subnet(case, &router, &vpc_id, "10.100.1.0/24").await;
    expect_dependency_violation(case, &router, &vpc_id, &subnet_id).await;

    // Row 2: and by a security group the caller created. Deleting the subnet is not
    // enough while the group is there.
    let case = "held by a security group";
    let group_id = create_security_group(case, &router, &vpc_id, "holds-the-vpc").await;
    ok(
        case,
        &router,
        "DeleteSubnet",
        &[param("SubnetId", &subnet_id)],
    )
    .await;
    expect_dependency_violation(case, &router, &vpc_id, &group_id).await;

    // Row 3: with both gone the VPC deletes, and the default group it came with goes with
    // it — "When you delete the VPC, it deletes the default security group, network ACL,
    // and route table for the VPC" (`DeleteVpc`).
    let case = "nothing left";
    let default_group = groups_of(case, &router, &vpc_id)
        .await
        .into_iter()
        .find(|id| *id != group_id)
        .expect("the VPC must hold the default group it came with");
    ok(
        case,
        &router,
        "DeleteSecurityGroup",
        &[param("GroupId", &group_id)],
    )
    .await;
    ok(case, &router, "DeleteVpc", &[param("VpcId", &vpc_id)]).await;
    let (status, body) = call(&router, "DescribeVpcs", &indexed("VpcId", &[&vpc_id])).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a deleted VPC must not be describable, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVpcID.NotFound",
        "case {case:?}: expected InvalidVpcID.NotFound: {body}"
    );
    let (status, body) = call(
        &router,
        "DescribeSecurityGroups",
        &indexed("GroupId", &[&default_group]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the deleted VPC's default group must be gone too, got {status}: \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidGroup.NotFound",
        "case {case:?}: expected InvalidGroup.NotFound for the default group: {body}"
    );

    // Row 4: a second delete of the same VPC is refused rather than answered as a no-op.
    let (status, body) = call(&router, "DeleteVpc", &[param("VpcId", &vpc_id)]).await;
    assert_eq!(
        status, 400,
        "a second DeleteVpc must be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVpcID.NotFound",
        "expected InvalidVpcID.NotFound from the second delete: {body}"
    );

    // Row 5: the default VPC a fresh account already has is held by its own seeded
    // subnets, and deletes once they are gone.
    let case = "the default VPC";
    let default_vpc = {
        let body = ok(
            case,
            &router,
            "DescribeVpcs",
            &filters(&[("is-default", &["true"])]),
        )
        .await;
        member(case, &body, "vpcId")
    };
    let seeded = subnets_of(case, &router, &default_vpc).await;
    assert!(
        !seeded.is_empty(),
        "case {case:?}: the default VPC must come with default subnets"
    );
    expect_dependency_violation(
        case,
        &router,
        &default_vpc,
        seeded.first().expect("one seeded subnet"),
    )
    .await;
    for subnet_id in &seeded {
        ok(
            case,
            &router,
            "DeleteSubnet",
            &[param("SubnetId", subnet_id)],
        )
        .await;
    }
    ok(case, &router, "DeleteVpc", &[param("VpcId", &default_vpc)]).await;

    // And a VPC id no VPC of the account has.
    let (status, body) = call(
        &router,
        "DeleteVpc",
        &[param("VpcId", "vpc-00000000000000bad")],
    )
    .await;
    assert_eq!(
        status, 400,
        "a VPC that does not exist must be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidVpcID.NotFound",
        "expected InvalidVpcID.NotFound: {body}"
    );
}

/// `DeleteVpc` must refuse with `DependencyViolation`, naming `dependency`, and leave the
/// VPC in place.
async fn expect_dependency_violation(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    dependency: &str,
) {
    let (status, body) = call(router, "DeleteVpc", &[param("VpcId", vpc_id)]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a refusal while {dependency} holds the VPC, got {status}: \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "DependencyViolation",
        "case {case:?}: expected DependencyViolation: {body}"
    );
    assert!(
        body.contains(dependency),
        "case {case:?}: the refusal must name the dependency {dependency:?}: {body}"
    );
    let (status, body) = call(router, "DescribeVpcs", &indexed("VpcId", &[vpc_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a refused delete must leave the VPC in place, got {status}: {body}"
    );
}

// ---------------------------------------------------------------------------
// The store's byte budget
// ---------------------------------------------------------------------------

/// The rule: a delete gives the store's byte budget back exactly what the matching create
/// charged it, so a create/delete loop cannot walk the budget away in either direction.
///
/// The rows are the three resources this suite deletes, each carrying caller-chosen bytes
/// (a tag, and for a group its name and description), plus the default group a VPC comes
/// with — whose name and description the SERVICE chose, so the budget never counted them.
#[tokio::test]
async fn a_delete_returns_exactly_the_bytes_its_create_charged() {
    let case = "the byte budget";
    let (router, engine) = super::ec2_query::emulator();
    let budget = || {
        engine
            .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
            .lock()
            .stored_bytes()
    };

    let before = budget();
    let body = ok(
        case,
        &router,
        "CreateVpc",
        &[
            param("CidrBlock", "10.110.0.0/16"),
            param("TagSpecification.1.ResourceType", "vpc"),
            param("TagSpecification.1.Tag.1.Key", "Name"),
            param("TagSpecification.1.Tag.1.Value", "budget-vpc"),
        ],
    )
    .await;
    let vpc_id = member(case, &body, "vpcId");
    let body = ok(
        case,
        &router,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", "10.110.1.0/24"),
            param("TagSpecification.1.ResourceType", "subnet"),
            param("TagSpecification.1.Tag.1.Key", "Name"),
            param("TagSpecification.1.Tag.1.Value", "budget-subnet"),
        ],
    )
    .await;
    let subnet_id = member(case, &body, "subnetId");
    let body = ok(
        case,
        &router,
        "CreateSecurityGroup",
        &[
            param("VpcId", &vpc_id),
            param("GroupName", "budget-group"),
            param("GroupDescription", "a group with caller-chosen bytes"),
            param("TagSpecification.1.ResourceType", "security-group"),
            param("TagSpecification.1.Tag.1.Key", "Name"),
            param("TagSpecification.1.Tag.1.Value", "budget-group"),
        ],
    )
    .await;
    let group_id = member(case, &body, "groupId");
    let charged = budget();
    assert!(
        charged > before,
        "case {case:?}: the creates must charge caller-chosen bytes, \
         the budget went from {before} to {charged}"
    );

    // Deleting all three — the VPC delete taking its default group with it — must put the
    // budget back exactly where it started.
    for (action, params) in [
        ("DeleteSubnet", vec![param("SubnetId", &subnet_id)]),
        ("DeleteSecurityGroup", vec![param("GroupId", &group_id)]),
        ("DeleteVpc", vec![param("VpcId", &vpc_id)]),
    ] {
        ok(case, &router, action, &params).await;
    }
    let after = budget();
    assert_eq!(
        after, before,
        "case {case:?}: the deletes must return exactly what the creates charged, \
         the budget went from {before} to {charged} and back to {after}"
    );
}

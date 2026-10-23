// SPDX-License-Identifier: CC0-1.0

//! Which EC2 ERROR CODE a refusal carries, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! EC2's model declares no error shapes, so the `<Code>` on the wire is the whole contract
//! a client can branch on: a refusal that names the wrong condition sends repair logic down
//! a branch that can never succeed. The rules under test are the ones that decide WHICH
//! code a refusal carries, each asserted over a table of inputs rather than over one
//! recorded call:
//!
//! 1. AN ID OUTSIDE THE FORM EC2 ISSUES IS A BAD ID, NOT A MISSING RESOURCE. A volume id is
//!    `vol-` plus 8 or 17 lowercase hexadecimal digits; anything outside that form is
//!    refused before any lookup, and only a well-formed id the account holds no volume for
//!    is `InvalidVolume.NotFound`.
//! 2. WHICH bad-id code is a fact of the OPERATION, not of the id: one that resolves the
//!    single volume it acts on (`CopyVolumes`) answers the resource-level
//!    `InvalidVolumeID.Malformed`, while one whose id LIST is validated as a request
//!    parameter (`DescribeVolumes`) answers `InvalidParameterValue` naming the parameter and
//!    the form it expected.
//! 3. A CIDR BLOCK OF A SIZE THE RESOURCE DOES NOT PERMIT IS A RANGE VIOLATION: that
//!    resource's own `Invalid<Resource>.Range` (`InvalidVpc.Range`, `InvalidSubnet.Range`),
//!    where a value that is not a CIDR block at all could not be read and is
//!    `InvalidParameterValue`.
//! 4. TWO MEMBERS THAT MAY NOT BOTH BE PRESENT make the REQUEST invalid — `InvalidRequest` —
//!    where two members that are alternative spellings of ONE value, and so hold together
//!    for some pair of values, are a parameter COMBINATION.
//! 5. The same malformed-before-unknown ordering on another surface: `DescribeCoipPools`
//!    answers `InvalidPoolID.Malformed` for a value that is not a customer-owned address
//!    pool id and `InvalidPoolID.NotFound` only for one that could be.
//!
//! The sixth rule of the same family — which of SEVERAL ids a request names is reported
//! when more than one cannot be resolved — is asserted beside the multicast domain fixtures
//! it needs, in [`super::transit_gateway_acceptance`].

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, indexed, unescaped};
use super::volume_lifecycle::{p, zone_id, zone_name, REGION};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: a bad volume id, and which code says so
// ---------------------------------------------------------------------------

/// One row of the volume-id table: the case, the id a request names, the code `CopyVolumes`
/// refuses it with, and the code `DescribeVolumes` refuses it with.
type VolumeIdCase = (&'static str, &'static str, &'static str, &'static str);
/// EC2's codes, named once so a row reads as a rule rather than as a literal.
const MALFORMED: &str = "InvalidVolumeID.Malformed";
const NOT_FOUND: &str = "InvalidVolume.NotFound";
const PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The volume ids the table below is written against.
///
/// The first two rows are ids of the form EC2 issues — the long and the short one — so they
/// are resolved and found missing. Every other row is outside that form, along a different
/// way of being outside it: a suffix of neither published length, a non-hexadecimal digit,
/// an upper-case digit, another resource's prefix and no prefix at all.
fn volume_id_cases() -> Vec<VolumeIdCase> {
    vec![
        (
            "a long id nothing was created for",
            "vol-04fd27e9e8a1b2c3d",
            NOT_FOUND,
            NOT_FOUND,
        ),
        (
            "a short id nothing was created for",
            "vol-04fd27e9",
            NOT_FOUND,
            NOT_FOUND,
        ),
        (
            "a suffix of neither published length",
            "vol-012345",
            MALFORMED,
            PARAMETER_VALUE,
        ),
        (
            "a suffix one digit too long",
            "vol-04fd27e9e8a1b2c3de",
            MALFORMED,
            PARAMETER_VALUE,
        ),
        (
            "a non-hexadecimal digit",
            "vol-04fd27e9e8a1b2c3z",
            MALFORMED,
            PARAMETER_VALUE,
        ),
        (
            "an upper-case digit",
            "vol-04FD27E9E8A1B2C3D",
            MALFORMED,
            PARAMETER_VALUE,
        ),
        (
            "another resource's prefix",
            "snap-04fd27e9e8a1b2c3d",
            MALFORMED,
            PARAMETER_VALUE,
        ),
        (
            "the prefix alone",
            "vol-",
            MALFORMED,
            PARAMETER_VALUE,
        ),
        ("no prefix at all", "not-a-volume", MALFORMED, PARAMETER_VALUE),
    ]
}

/// Create one volume, so that every refusal below is about the id the request named rather
/// than about an empty store, and answer its id.
async fn stored_volume(router: &MultiServiceRouter, case: &str) -> String {
    let params = vec![
        p("AvailabilityZone", zone_name(0)),
        p("VolumeType", "gp3"),
        p("Size", "8"),
    ];
    let (status, body) = call(router, REGION, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 seeding a volume, got {status} with body {body}"
    );
    element(&body, "volumeId").unwrap_or_default()
}

/// RULES 1 and 2: each row's id is refused by both operations, with the code its own layer
/// answers — and an id EC2 could have issued is resolved and reported missing instead.
#[tokio::test]
async fn a_volume_id_outside_the_form_ec2_issues_is_a_bad_id_not_a_missing_volume() {
    for (case, volume_id, copy_code, describe_code) in volume_id_cases() {
        let router = emulator();
        let stored = stored_volume(&router, case).await;

        let (status, body) = call(
            &router,
            REGION,
            "CopyVolumes",
            &[p("SourceVolumeId", volume_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from CopyVolumes, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            copy_code,
            "case {case:?}: expected {copy_code} from CopyVolumes, got {}",
            error_code(&body)
        );

        let (status, body) = call(
            &router,
            REGION,
            "DescribeVolumes",
            &indexed("VolumeId", &[volume_id]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DescribeVolumes, got {status} with body \
             {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, describe_code,
            "case {case:?}: expected {describe_code} from DescribeVolumes, got {code}"
        );
        // A parameter-level refusal says WHICH parameter and what form it expected; the
        // resource-level one names the id it could not read.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        if code == PARAMETER_VALUE {
            assert!(
                message.contains(&format!(
                    "for parameter {}",
                    ec2::state::VOLUME_IDS_PARAMETER
                )) && message.contains("Expected: 'vol-...'"),
                "case {case:?}: expected the refusal to name the parameter and the form it \
                 expected, got {message:?}"
            );
        }

        // The volume the account really holds is still described, so no row refused the
        // whole store.
        let (status, body) = call(
            &router,
            REGION,
            "DescribeVolumes",
            &indexed("VolumeId", &[stored.as_str()]),
        )
        .await;
        assert_eq!(
            (status, element(&body, "volumeId")),
            (200, Some(stored.clone())),
            "case {case:?}: the stored volume must still be described, got {status} with body \
             {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a block size the resource does not permit
// ---------------------------------------------------------------------------

/// One CIDR row: the case, the block, and the code the create is refused with — or `None`
/// for a block that is accepted.
type CidrCase = (&'static str, &'static str, Option<&'static str>);

/// The VPC blocks the table is written against: both published bounds (accepted), one past
/// each bound and far past the lower one (a RANGE violation), and the ways a value can fail
/// to be an IPv4 CIDR block at all (a parameter-value refusal).
///
/// The bounds themselves come from the Amazon VPC User Guide sentence `crate::state` keeps
/// them under: "The allowed block size is between a /16 netmask (65,536 IP addresses) and
/// /28 netmask (16 IP addresses)."
fn vpc_cidr_cases() -> Vec<CidrCase> {
    vec![
        ("the /16 bound", "10.0.0.0/16", None),
        ("the /28 bound", "10.1.0.0/28", None),
        ("one past the /16 bound", "10.0.0.0/15", Some("InvalidVpc.Range")),
        ("far past the /16 bound", "10.0.0.0/8", Some("InvalidVpc.Range")),
        ("one past the /28 bound", "10.0.0.0/29", Some("InvalidVpc.Range")),
        ("a single address", "10.0.0.0/32", Some("InvalidVpc.Range")),
        ("no prefix length", "10.0.0.0", Some(PARAMETER_VALUE)),
        ("a prefix length off the scale", "10.0.0.0/33", Some(PARAMETER_VALUE)),
        ("not an address at all", "ten.zero.zero.zero/16", Some(PARAMETER_VALUE)),
    ]
}

/// The subnet blocks the table is written against, inside a `10.20.0.0/16` VPC: both
/// published bounds, one past each, a block that parses but lies outside the VPC (the same
/// `.Range` code, since both are a block the subnet's range rules do not admit), and a
/// value that is no CIDR block.
fn subnet_cidr_cases() -> Vec<CidrCase> {
    vec![
        ("the /16 bound, the VPC's whole block", "10.20.0.0/16", None),
        ("the /28 bound", "10.20.32.0/28", None),
        (
            "one past the /28 bound",
            "10.20.33.0/29",
            Some("InvalidSubnet.Range"),
        ),
        (
            "one past the /16 bound",
            "10.0.0.0/15",
            Some("InvalidSubnet.Range"),
        ),
        (
            "a block outside the VPC's own",
            "192.168.4.0/24",
            Some("InvalidSubnet.Range"),
        ),
        ("not an address at all", "10.20.32.0", Some(PARAMETER_VALUE)),
    ]
}

/// RULE 3, for a VPC's own block: a size outside the published range is `InvalidVpc.Range`,
/// a value that is not a CIDR block is `InvalidParameterValue`, and a block at either bound
/// is accepted and read back.
#[tokio::test]
async fn a_vpc_cidr_block_of_a_size_ec2_does_not_permit_is_a_range_violation() {
    for (case, cidr, expected) in vpc_cidr_cases() {
        let router = emulator();
        let (status, body) = call(&router, REGION, "CreateVpc", &[p("CidrBlock", cidr)]).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a block at a published bound is accepted, got {status} with \
                     body {body}"
                );
                let vpc_id = element(&body, "vpcId").unwrap_or_default();
                let (status, read) = call(
                    &router,
                    REGION,
                    "DescribeVpcs",
                    &indexed("VpcId", &[vpc_id.as_str()]),
                )
                .await;
                assert_eq!(
                    (status, element(&read, "cidrBlock")),
                    (200, Some(cidr.to_string())),
                    "case {case:?}: the accepted block must be stored, got {status} with body \
                     {read}"
                );
            }
            Some(expected_code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    expected_code,
                    "case {case:?}: expected {expected_code}, got {}",
                    error_code(&body)
                );
            }
        }
    }
}

/// RULE 3, one resource down: the same reading of a block size for a SUBNET, whose own code
/// is `InvalidSubnet.Range` — and a block outside its VPC's range carries that code too.
#[tokio::test]
async fn a_subnet_cidr_block_of_a_size_ec2_does_not_permit_is_a_range_violation() {
    for (case, cidr, expected) in subnet_cidr_cases() {
        let router = emulator();
        let (status, body) = call(
            &router,
            REGION,
            "CreateVpc",
            &[p("CidrBlock", "10.20.0.0/16")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 seeding the VPC, got {status} with body {body}"
        );
        let vpc_id = element(&body, "vpcId").unwrap_or_default();

        let params = vec![p("VpcId", &vpc_id), p("CidrBlock", cidr)];
        let (status, body) = call(&router, REGION, "CreateSubnet", &params).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a block at a published bound is accepted, got {status} with \
                     body {body}"
                );
                assert_eq!(
                    element(&body, "cidrBlock"),
                    Some(cidr.to_string()),
                    "case {case:?}: the accepted block must be the one asked for, got {body}"
                );
            }
            Some(expected_code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    expected_code,
                    "case {case:?}: expected {expected_code}, got {}",
                    error_code(&body)
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: members that may not both be present
// ---------------------------------------------------------------------------

/// RULE 4: `AllocateHosts` refuses a request that sets both the instance type and the
/// instance family — "You cannot specify InstanceFamily and InstanceType in the same
/// request" (`AllocateHostsRequest$InstanceFamily`) — with `InvalidRequest`, whatever the
/// two values are, because no pair of them describes one host. The zone name and the zone
/// id, which are two spellings of ONE value, stay a parameter COMBINATION.
#[tokio::test]
async fn two_members_that_may_not_both_be_present_make_the_request_invalid() {
    // Rows: three different (type, family) pairs — one where the family is the type's own,
    // two where it is not — then the pair of alternative spellings, and the request that
    // names neither dimension at all.
    let rows: Vec<(&str, Params, &str)> = vec![
        (
            "a type with its own family",
            vec![
                p("InstanceType", "m5.large"),
                p("InstanceFamily", "m5"),
                p("AvailabilityZone", zone_name(0)),
                p("Quantity", "1"),
            ],
            "InvalidRequest",
        ),
        (
            "a type with another family",
            vec![
                p("InstanceType", "c5.xlarge"),
                p("InstanceFamily", "m5"),
                p("AvailabilityZone", zone_name(0)),
                p("Quantity", "1"),
            ],
            "InvalidRequest",
        ),
        (
            "a metal type with a family",
            vec![
                p("InstanceType", "mac2.metal"),
                p("InstanceFamily", "mac2"),
                p("AvailabilityZone", zone_name(1)),
                p("Quantity", "2"),
            ],
            "InvalidRequest",
        ),
        (
            "both spellings of one zone",
            vec![
                p("InstanceType", "m5.large"),
                p("AvailabilityZone", zone_name(0)),
                p("AvailabilityZoneId", zone_id(0)),
                p("Quantity", "1"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "neither a type nor a family",
            vec![
                p("AvailabilityZone", zone_name(0)),
                p("Quantity", "1"),
            ],
            "MissingParameter",
        ),
    ];

    for (case, params, expected_code) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let (status, body) = call(&router, REGION, "AllocateHosts", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            error_code(&body)
        );
        // A refused allocation stores nothing, so the account still holds no Dedicated
        // Host. `DescribeHosts` is still a scaffold, so the store itself is what is read.
        let held = engine
            .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
            .lock()
            .dedicated_hosts
            .len();
        assert_eq!(
            held, 0,
            "case {case:?}: a refused allocation must store no host, got {held}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the same ordering on the customer-owned address pool surface
// ---------------------------------------------------------------------------

/// RULE 5: a value that is not a customer-owned address pool id is refused as malformed
/// before any lookup, where an id that could be one is `InvalidPoolID.NotFound`. The rows
/// differ along the way the value fails: another address pool's prefix, a prefix EC2 mints
/// no id with, the wrong suffix length, a non-hexadecimal digit and no prefix at all.
#[tokio::test]
async fn a_value_that_is_not_a_coip_pool_id_is_malformed_before_it_is_looked_up() {
    let rows: [(&str, &str, &str); 6] = [
        (
            "an id nothing created",
            "ipv4pool-coip-00000000000000000",
            "InvalidPoolID.NotFound",
        ),
        (
            "an EC2 address pool id",
            "ipv4pool-ec2-00000000000000000",
            "InvalidPoolID.Malformed",
        ),
        (
            "a prefix EC2 mints no id with",
            "coip-pool-00000000000000000",
            "InvalidPoolID.Malformed",
        ),
        (
            "a suffix of neither published length",
            "ipv4pool-coip-00000",
            "InvalidPoolID.Malformed",
        ),
        (
            "a non-hexadecimal digit",
            "ipv4pool-coip-0000000000000000z",
            "InvalidPoolID.Malformed",
        ),
        ("no prefix at all", "not-a-pool", "InvalidPoolID.Malformed"),
    ];

    let router = emulator();
    for (case, pool_id, expected_code) in rows {
        let (status, body) = call(
            &router,
            REGION,
            "DescribeCoipPools",
            &indexed("PoolId", &[pool_id]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            error_code(&body)
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateDhcpOptions`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: `AssociateDhcpOptions` points the VPC the request names at the DHCP
//! option set it names — or at `default`, the word that stands for "no DHCP options" — so
//! that VPC and no other reports that value as its `DhcpOptionsId` from then on, while a
//! value that is not a dhcp-options id at all, an id the account holds no option set for,
//! and a VPC that does not exist are each refused with EC2's own error code and change
//! nothing.
//!
//! The tables below therefore span that class rather than one recorded request: every kind
//! of value the request's `DhcpOptionsId` can carry (the set the VPC already reports,
//! another option set the account holds, a third one, the word `default`, and the same value
//! twice), both kinds of VPC (the seeded default one and ones `CreateVpc` made), and every
//! form of id that cannot be resolved — each hex length and case the id form admits or
//! refuses. An account holds one default option set per Region
//! ([`super::dhcp_options`]), so a row needing a set the subject VPC does not already
//! report creates a VPC in another Region to mint one. Every assertion names the case and
//! the actual value.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, envelope_children, error_code, filters, indexed};
use super::wire::{member, param, Param};

/// The Region the subject VPCs are created in, and the one the account's seeded default
/// network belongs to.
const REGION: &str = ec2::state::DEFAULT_REGION;

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `AssociateDhcpOptions` call, as its HTTP status and XML body.
async fn associate(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "AssociateDhcpOptions", params).await
}

/// The form fields of an `AssociateDhcpOptions` naming `dhcp_options_id` and `vpc_id`.
fn associate_params(vpc_id: &str, dhcp_options_id: &str) -> Vec<Param> {
    vec![
        param("VpcId", vpc_id),
        param("DhcpOptionsId", dhcp_options_id),
    ]
}

/// A VPC `CreateVpc` makes in `region`, as its `VpcId`.
async fn create_vpc(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    cidr: &str,
) -> String {
    let (status, body) =
        super::ec2_query::call(router, region, "CreateVpc", &[param("CidrBlock", cidr)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    member(case, &body, "vpcId")
}

/// An option set the account holds that no VPC of [`REGION`] reports: the default set of
/// `region`, minted by creating a VPC there. Answers the VPC and the set id.
async fn option_set_of_another_region(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    cidr: &str,
) -> (String, String) {
    let vpc_id = create_vpc(router, case, region, cidr).await;
    let set = reported_dhcp_options_id(router, case, region, &vpc_id).await;
    (vpc_id, set)
}

/// The account's default VPC, as its `VpcId`.
async fn default_vpc(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcs, got {status} with body {body}"
    );
    element(&body, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a default VPC in {body}"))
}

/// The `DhcpOptionsId` `DescribeVpcs` reports for the one VPC `vpc_id` names, read in the
/// Region that VPC was created in — EC2 is a regional service, so a describe signed for
/// another Region reports no VPC at all.
async fn reported_dhcp_options_id(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    vpc_id: &str,
) -> String {
    let (status, body) =
        super::ec2_query::call(router, region, "DescribeVpcs", &indexed("VpcId", &[vpc_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcs for {vpc_id}, got {status} \
         with body {body}"
    );
    member(case, &body, "dhcpOptionsId")
}

/// Every `DhcpOptionsId` `DescribeDhcpOptions` reports for the account.
async fn held_option_sets(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeDhcpOptions", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeDhcpOptions, got {status} with \
         body {body}"
    );
    elements(&body, "dhcpOptionsId")
}

/// One association required to succeed, asserted on its status, its `<return>` and the
/// `DhcpOptionsId` the VPC reports afterwards.
async fn associate_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    vpc_id: &str,
    dhcp_options_id: &str,
) {
    let (status, body) = associate(router, &associate_params(vpc_id, dhcp_options_id)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 associating {dhcp_options_id} with {vpc_id}, \
         got {status} with body {body}"
    );
    // The envelope's success flag, exactly once: this operation's modelled output declares
    // no members, so `Ec2QueryEnvelope` is what writes it (`src/service.rs`) and the
    // handler must not write a second one.
    let flags = envelope_children(case, &body, "AssociateDhcpOptions", "return");
    assert_eq!(
        flags,
        vec!["true".to_string()],
        "case {case:?}: expected exactly one <return>true</return>, got {flags:?} with \
         body {body}"
    );
    let request_ids = envelope_children(case, &body, "AssociateDhcpOptions", "requestId");
    assert_eq!(
        request_ids.len(),
        1,
        "case {case:?}: expected one <requestId> beside the flag, got {request_ids:?} \
         with body {body}"
    );
    let stored = reported_dhcp_options_id(router, case, region, vpc_id).await;
    assert_eq!(
        stored, dhcp_options_id,
        "case {case:?}: expected {vpc_id} to report {dhcp_options_id:?} afterwards, got \
         {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// The rule: the association IS the VPC's `DhcpOptionsId`, so after an
/// `AssociateDhcpOptions` the VPC reports the value the request named, whatever kind of
/// value it was — and a repeat of the same association is not a different answer.
///
/// The rows are every kind of value `DhcpOptionsId` can carry, applied in order to one
/// VPC: the set it already reports, another option set the account holds, a third one, the
/// word `default`, `default` twice over, and a set again after `default`.
#[tokio::test]
async fn associate_dhcp_options_points_the_vpc_at_the_value_the_request_names() {
    let router = emulator();
    let subject = create_vpc(&router, "the subject VPC", REGION, "10.10.0.0/16").await;
    let (other_vpc, other_set) =
        option_set_of_another_region(&router, "another option set", "us-west-2", "10.11.0.0/16")
            .await;
    let (third_vpc, third_set) =
        option_set_of_another_region(&router, "a third option set", "eu-west-1", "10.12.0.0/16")
            .await;
    let own_set = reported_dhcp_options_id(&router, "the subject VPC", REGION, &subject).await;

    for (case, set) in [
        ("the subject VPC", &own_set),
        ("another option set", &other_set),
        ("a third option set", &third_set),
    ] {
        assert!(
            ec2::state::dhcp_options_id_is_well_formed(set),
            "case {case:?}: a VPC must report a dhcp-options id, got {set:?}"
        );
    }
    assert!(
        own_set != other_set && other_set != third_set && own_set != third_set,
        "the table needs three distinct option sets, got {own_set:?}, {other_set:?} and \
         {third_set:?}"
    );

    // (case, the value to associate)
    let steps = [
        ("the set it already reports", own_set.clone()),
        ("that same set a second time", own_set.clone()),
        ("another option set the account holds", other_set.clone()),
        ("a third option set", third_set.clone()),
        (
            "no DHCP options at all",
            ec2::state::DHCP_OPTIONS_DEFAULT.to_string(),
        ),
        (
            "no DHCP options twice",
            ec2::state::DHCP_OPTIONS_DEFAULT.to_string(),
        ),
        ("a set again after default", other_set.clone()),
    ];

    for (case, value) in steps {
        associate_ok(&router, case, REGION, &subject, &value).await;
        // The association is made on the VPC the request named and on no other: the two
        // VPCs that minted the other sets still report the sets they were created with,
        // read in the Region each was created in.
        for (other_case, other_region, vpc_id, expected) in [
            (
                "the VPC of the second set",
                "us-west-2",
                &other_vpc,
                &other_set,
            ),
            (
                "the VPC of the third set",
                "eu-west-1",
                &third_vpc,
                &third_set,
            ),
        ] {
            let reported =
                reported_dhcp_options_id(&router, other_case, other_region, vpc_id).await;
            assert_eq!(
                &reported, expected,
                "case {case:?}: {other_case} must still report {expected:?} after the \
                 association, got {reported:?}"
            );
        }
    }
}

/// The rule holds for either kind of VPC: the account's seeded default VPC is associated
/// the same way as one `CreateVpc` made.
///
/// The rows are the two kinds of VPC, each associated with a set it did not come with and
/// then with `default`.
#[tokio::test]
async fn either_kind_of_vpc_can_be_associated_and_disassociated() {
    let router = emulator();
    let created = create_vpc(&router, "a VPC CreateVpc made", REGION, "10.20.0.0/16").await;
    let seeded = default_vpc(&router, "the account's default VPC").await;
    let (_, elsewhere) = option_set_of_another_region(
        &router,
        "an option set neither VPC came with",
        "ap-southeast-1",
        "10.21.0.0/16",
    )
    .await;

    // (case, the VPC) — each row is moved onto a set it did not come with, then off every
    // set with `default`.
    let cases = [
        ("a VPC CreateVpc made", &created),
        ("the account's default VPC", &seeded),
    ];

    for (case, vpc_id) in cases {
        associate_ok(&router, case, REGION, vpc_id, &elsewhere).await;
        associate_ok(&router, case, REGION, vpc_id, ec2::state::DHCP_OPTIONS_DEFAULT).await;
    }
}

/// An option set is a resource of the account, not of one VPC: it stays associable after
/// the VPC whose creation first needed it is gone, and after no VPC reports it at all.
///
/// The rows are the two ways a set can lose the VPC it was minted for — that VPC is pointed
/// at another value, and it is deleted outright — each followed by an association of the
/// same set with a VPC that never reported it.
#[tokio::test]
async fn a_set_stays_associable_after_the_vpc_it_was_minted_for_is_gone() {
    let router = emulator();
    let (minted_for, set) = option_set_of_another_region(
        &router,
        "the VPC the set was minted for",
        "eu-central-1",
        "10.40.0.0/16",
    )
    .await;
    let late = create_vpc(&router, "a VPC created in the account's Region", REGION, "10.42.0.0/16")
        .await;

    // Its minting VPC is pointed away from it, so no VPC reports the set at all: it is
    // still the account's.
    associate_ok(
        &router,
        "the minting VPC points elsewhere",
        "eu-central-1",
        &minted_for,
        ec2::state::DHCP_OPTIONS_DEFAULT,
    )
    .await;
    let held = held_option_sets(&router, "after the minting VPC moved off").await;
    assert!(
        held.contains(&set),
        "case \"after the minting VPC moved off\": expected the account to still hold \
         {set:?}, got {held:?}"
    );
    associate_ok(&router, "after the minting VPC moved off", REGION, &late, &set).await;

    // And the minting VPC is deleted outright: the set outlives it.
    associate_ok(
        &router,
        "the late VPC moves off again",
        REGION,
        &late,
        ec2::state::DHCP_OPTIONS_DEFAULT,
    )
    .await;
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DeleteVpc",
        &[param("VpcId", &minted_for)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the minting VPC is deleted\": expected HTTP 200 from DeleteVpc, got \
         {status} with body {body}"
    );
    let held = held_option_sets(&router, "after the minting VPC was deleted").await;
    assert!(
        held.contains(&set),
        "case \"after the minting VPC was deleted\": expected the account to still hold \
         {set:?}, got {held:?}"
    );
    associate_ok(&router, "after the minting VPC was deleted", REGION, &late, &set).await;
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// The rule: a `DhcpOptionsId` that is not a dhcp-options id at all is refused as
/// malformed BEFORE it is looked up, so a caller can tell that from "no such option set";
/// a well-formed id the account holds no option set for is refused as not found; a VPC id
/// no VPC of the account has is refused as not found; and a dry run of a request that would
/// have succeeded is refused with `DryRunOperation`. None of them writes anything.
///
/// The rows differ along what makes the request unanswerable: each form an id can fail (no
/// suffix, a non-hex suffix, one hex digit short of and one past each of the two published
/// lengths, upper-case hex, another resource's prefix, the `default` word misspelt), both
/// id lengths for an unknown-but-well-formed set, the VPC, and the dry run.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    let vpc_id = create_vpc(&router, "the refusal table's VPC", REGION, "10.30.0.0/16").await;
    let own_set =
        reported_dhcp_options_id(&router, "the refusal table's VPC", REGION, &vpc_id).await;
    /// A VPC id of the right shape that no VPC of the account has.
    const ABSENT_VPC: &str = "vpc-0000000000000beef";
    /// A dhcp-options id of the right shape, in its 17-digit form, that no option set of
    /// the account has.
    const ABSENT_SET_LONG: &str = "dopt-0000000000000beef";
    /// The same, in the 8-digit short form — well-formed too, so it must be answered as
    /// unknown rather than as malformed.
    const ABSENT_SET_SHORT: &str = "dopt-0000beef";

    // (case, the DhcpOptionsId, the VpcId, the HTTP status, the EC2 error code)
    let cases: [(&str, &str, &str, u16, &str); 12] = [
        (
            "a prefix with no hex suffix",
            "dopt-",
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED,
        ),
        (
            "a suffix that is not hex",
            "dopt-zzzzzzzz",
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED,
        ),
        (
            "one digit short of the short form",
            "dopt-0000bee",
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED,
        ),
        (
            "one digit past the short form",
            "dopt-0000beef0",
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED,
        ),
        (
            "one digit short of the long form",
            "dopt-0000000000000bee",
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED,
        ),
        (
            "one digit past the long form",
            "dopt-0000000000000beef0",
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED,
        ),
        (
            "upper-case hex",
            "dopt-0000000000000BEEF",
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED,
        ),
        (
            "another resource's id",
            &own_set.replace("dopt-", "vpc-"),
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED,
        ),
        (
            "the default word misspelt",
            "defaults",
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED,
        ),
        (
            "a long id no option set has",
            ABSENT_SET_LONG,
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTION_ID_NOT_FOUND,
        ),
        (
            "a short id no option set has",
            ABSENT_SET_SHORT,
            &vpc_id,
            400,
            ec2::state::INVALID_DHCP_OPTION_ID_NOT_FOUND,
        ),
        (
            "a VPC that does not exist",
            &own_set,
            ABSENT_VPC,
            400,
            ec2::state::INVALID_VPC_ID_NOT_FOUND,
        ),
    ];

    for (case, dhcp_options_id, vpc, status_expected, code_expected) in cases {
        let (status, body) = associate(&router, &associate_params(vpc, dhcp_options_id)).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from AssociateDhcpOptions, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?}, got {code:?} with \
             body {body}"
        );
    }

    // A dry run of the very request the table's VPC would have accepted: refused with
    // `DryRunOperation`, which is what says the caller holds the permissions.
    let mut dry = associate_params(&vpc_id, ec2::state::DHCP_OPTIONS_DEFAULT);
    dry.push(param("DryRun", "true"));
    let (status, body) = associate(&router, &dry).await;
    assert_eq!(
        status, 412,
        "case \"a dry run that would have succeeded\": expected HTTP 412, got {status} \
         with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code,
        ec2::state::DRY_RUN_OPERATION,
        "case \"a dry run that would have succeeded\": expected error code \
         {expected:?}, got {code:?} with body {body}",
        expected = ec2::state::DRY_RUN_OPERATION
    );

    // None of those refusals wrote anything: the VPC still reports the set it was created
    // with, the dry run included.
    let reported =
        reported_dhcp_options_id(&router, "after every refusal", REGION, &vpc_id).await;
    assert_eq!(
        reported, own_set,
        "case \"after every refusal\": expected {vpc_id} to still report {own_set:?}, \
         got {reported:?}"
    );
}

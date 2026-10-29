// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the security group VPC ASSOCIATION slice:
//! `AssociateSecurityGroupVpc`, which makes one of the account's security groups usable in a
//! VPC it was not created in, and `DescribeSecurityGroupVpcAssociations`, which reports the
//! associations the account holds. Driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. `DescribeSecurityGroupVpcAssociations` REPORTS THE STORE: exactly the (group, VPC)
//!    pairs `AssociateSecurityGroupVpc` wrote, in (`GroupId`, `VpcId`) order, each with the
//!    account that owns the group and the account that owns the VPC, and the empty set for
//!    an account that holds none;
//! 2. an association is answered in the non-terminal `associating` state and the CLOCK, not
//!    a caller, moves it to `associated`;
//! 3. every documented filter selects on the association member it names, and a filter name
//!    the operation does not document is REFUSED rather than read as "no results";
//! 4. reading associations WRITES NOTHING: two reads report the same associations, and a
//!    read never creates one;
//! 5. a request the service cannot satisfy is REFUSED with the code and the wording EC2
//!    answers that refusal class with, and writes nothing — an id that resolves to no VPC or
//!    no security group, a resource the feature excludes (the default VPC, a default
//!    security group), the VPC the group was created in, and a pair the account already
//!    holds an association for;
//! 6. an association cannot outlive either side of its pair: deleting the security group or
//!    the VPC takes it with them;
//! 7. the report PAGES: `MaxResults` cuts the page and the `NextToken` it answers walks the
//!    rest, each association reported exactly once.
//!
//! Each table row is a distinct input along what those rules generalise over — which group,
//! which VPC, how many associations one group holds, which filter selects and on what value,
//! the form of an id that names nothing (absent, each malformed shape, a well-formed id at
//! both published id lengths), and which side of the pair is deleted — so no one row is the
//! specification.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The state an association the service has just written is in.
const ASSOCIATING: &str = "associating";

/// The state the clock then owes it.
const ASSOCIATED: &str = "associated";

/// One association as the wire reports it: the pair it joins, the two owning accounts and
/// its state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Association {
    group_id: String,
    vpc_id: String,
    group_owner_id: String,
    vpc_owner_id: String,
    state: String,
}

/// One row of a refusal table: the case, the group and VPC the request names, the error code
/// and the message it must be refused with.
type RefusalRow<'a> = (&'a str, String, String, &'a str, String);

/// One (group, VPC) pair, as a report row is compared by.
type Pair = (String, String);

/// One row of the filter table: the case, the filter name, its values, and the pairs it must
/// select.
type FilterRow<'a> = (&'a str, &'a str, Vec<String>, BTreeSet<Pair>);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn action(router: &MultiServiceRouter, name: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, name, params).await
}

/// One call that must succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, name: &str, params: &[Param]) -> String {
    let (status, body) = action(router, name, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {name}, got {status} with body {body}"
    );
    body
}

/// A VPC with `cidr_block`, and the id it was minted with.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no vpcId in {body}"))
}

/// A security group named `group_name` in `vpc_id`, and the id it was minted with.
async fn create_group(
    case: &str,
    router: &MultiServiceRouter,
    group_name: &str,
    vpc_id: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSecurityGroup",
        &[
            param("GroupName", group_name),
            param("GroupDescription", "a group this suite associates"),
            param("VpcId", vpc_id),
        ],
    )
    .await;
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no groupId in {body}"))
}

/// The id of the Region's DEFAULT VPC — the one the account was given rather than one this
/// suite created, read through the `is-default` filter `DescribeVpcs` documents.
async fn default_vpc(case: &str, router: &MultiServiceRouter) -> String {
    let body = ok(case, router, "DescribeVpcs", &filters(&[("is-default", &["true"])])).await;
    element(&body, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: no default VPC in {body}"))
}

/// The id of the default security group of `vpc_id` — the group every VPC comes with, read
/// through the filters `DescribeSecurityGroups` documents rather than assumed.
async fn default_group_of(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeSecurityGroups",
        &filters(&[("vpc-id", &[vpc_id]), ("group-name", &["default"])]),
    )
    .await;
    element(&body, "groupId")
        .unwrap_or_else(|| panic!("case {case:?}: no default group of {vpc_id} in {body}"))
}

/// The form fields one `AssociateSecurityGroupVpc` call carries: the pair it names, plus
/// whatever extra parameters the case needs.
fn associate_params(group_id: &str, vpc_id: &str, extra: &[Param]) -> Vec<Param> {
    let mut params = vec![param("GroupId", group_id), param("VpcId", vpc_id)];
    params.extend_from_slice(extra);
    params
}

/// `AssociateSecurityGroupVpc` for one pair, with whatever extra parameters the case needs.
async fn associate(
    router: &MultiServiceRouter,
    group_id: &str,
    vpc_id: &str,
    extra: &[Param],
) -> (u16, String) {
    action(
        router,
        "AssociateSecurityGroupVpc",
        &associate_params(group_id, vpc_id, extra),
    )
    .await
}

/// One associate that must succeed, with the `State` it answered.
async fn associate_ok(
    case: &str,
    router: &MultiServiceRouter,
    group_id: &str,
    vpc_id: &str,
) -> String {
    let (status, body) = associate(router, group_id, vpc_id, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AssociateSecurityGroupVpc, got {status} with \
         body {body}"
    );
    element(&body, "state").unwrap_or_else(|| panic!("case {case:?}: no state in {body}"))
}

/// Every association a `DescribeSecurityGroupVpcAssociations` body carries, in document
/// order.
///
/// An association is the element that carries BOTH ids, so the association's own `<state>` is
/// never confused with a member of something else.
fn associations(case: &str, body: &str) -> Vec<Association> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && child(*node, "groupId").is_some()
                && child(*node, "vpcId").is_some()
        })
        .map(|node| Association {
            group_id: child_text(node, "groupId").unwrap_or_default(),
            vpc_id: child_text(node, "vpcId").unwrap_or_default(),
            group_owner_id: child_text(node, "groupOwnerId").unwrap_or_default(),
            vpc_owner_id: child_text(node, "vpcOwnerId").unwrap_or_default(),
            state: child_text(node, "state").unwrap_or_default(),
        })
        .collect()
}

/// The associations the report answers for `params`.
async fn reported(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<Association> {
    let body = ok(
        case,
        router,
        "DescribeSecurityGroupVpcAssociations",
        params,
    )
    .await;
    associations(case, &body)
}

/// The (group, VPC) pairs the unfiltered report answers.
async fn reported_pairs(case: &str, router: &MultiServiceRouter) -> Vec<(String, String)> {
    reported(case, router, &[])
        .await
        .into_iter()
        .map(|association| (association.group_id, association.vpc_id))
        .collect()
}

// ---------------------------------------------------------------------------
// Rules 1, 2 and 4: the read reports the store, and only observes the clock
// ---------------------------------------------------------------------------

/// The rule: the report answers exactly the pairs the associate wrote, in (`GroupId`,
/// `VpcId`) order, each in the state the clock has reached — `associating` until the
/// service's own transition span has passed and `associated` after it — with the owning
/// account of each side, and the read itself writes nothing.
///
/// The rows differ in which group holds the association, which VPC it is with, and how many
/// associations one group holds (one, two) — and one group of the account holds none, so the
/// report is a property of the store rather than of the one pair a recording used.
#[tokio::test]
async fn the_report_answers_the_associations_the_associate_wrote() {
    let router = emulator();
    let account = engine::emulator_account_id().to_string();

    // Three VPCs of the account and three groups spread over two of them, so both sides of
    // a pair vary and one group is never associated at all.
    let first_vpc = create_vpc("the first VPC", &router, "10.11.0.0/16").await;
    let second_vpc = create_vpc("the second VPC", &router, "10.12.0.0/16").await;
    let third_vpc = create_vpc("the third VPC", &router, "10.13.0.0/16").await;
    let first_group = create_group("the first group", &router, "first", &first_vpc).await;
    let second_group = create_group("the second group", &router, "second", &first_vpc).await;
    let unassociated = create_group("a group nobody associates", &router, "third", &second_vpc).await;

    // An account that has associated nothing reports nothing — not an error, and not the
    // groups it holds.
    assert!(
        reported("before any associate", &router, &[]).await.is_empty(),
        "case {case:?}: an account with no association must report none, got {reported:?}",
        case = "before any associate",
        reported = reported("before any associate", &router, &[]).await
    );

    let pairs: [(&str, &str, &str); 3] = [
        ("the first group with a second VPC", &first_group, &second_vpc),
        ("the first group with a third VPC", &first_group, &third_vpc),
        ("the second group with a second VPC", &second_group, &second_vpc),
    ];
    for (index, (case, group_id, vpc_id)) in pairs.into_iter().enumerate() {
        let state = associate_ok(case, &router, group_id, vpc_id).await;
        // Rule 2: the association AWS hands back is not yet established.
        assert_eq!(
            state, ASSOCIATING,
            "case {case:?}: expected the new association in {ASSOCIATING}, got {state:?}"
        );
        // Rule 2, through the STORE rather than through the answer, asked of the FIRST pair
        // only — immediately after its own associate, so the transient state is read inside
        // the span that call started rather than after two more round trips: the read
        // OBSERVES the transient state rather than a state nobody waited for.
        if index == 0 {
            let transient = reported("read at once", &router, &[]).await;
            assert_eq!(
                transient
                    .iter()
                    .map(|association| association.state.as_str())
                    .collect::<Vec<&str>>(),
                vec![ASSOCIATING],
                "case {case:?}: the association just written must still be \
                 {ASSOCIATING:?}, got {transient:?}"
            );
        }
    }

    past_the_transition().await;

    // Rule 1: exactly the three pairs written, in (group, VPC) order, each `associated` and
    // each naming the account that owns the group and the account that owns the VPC.
    let mut expected = vec![
        Association {
            group_id: first_group.clone(),
            vpc_id: second_vpc.clone(),
            group_owner_id: account.clone(),
            vpc_owner_id: account.clone(),
            state: ASSOCIATED.to_string(),
        },
        Association {
            group_id: first_group.clone(),
            vpc_id: third_vpc.clone(),
            group_owner_id: account.clone(),
            vpc_owner_id: account.clone(),
            state: ASSOCIATED.to_string(),
        },
        Association {
            group_id: second_group.clone(),
            vpc_id: second_vpc.clone(),
            group_owner_id: account.clone(),
            vpc_owner_id: account.clone(),
            state: ASSOCIATED.to_string(),
        },
    ];
    expected.sort();
    let settled = reported("past the transition", &router, &[]).await;
    assert_eq!(
        settled, expected,
        "case {case:?}: expected {expected:?}, got {settled:?}",
        case = "past the transition"
    );
    // The group that was never associated appears nowhere, however many groups the account
    // holds.
    assert!(
        settled
            .iter()
            .all(|association| association.group_id != unassociated),
        "case {case:?}: a group with no association must not be reported, got {settled:?}",
        case = "past the transition"
    );

    // Rule 4: the read changed nothing a later caller sees.
    let again = reported("read twice", &router, &[]).await;
    assert_eq!(
        again, expected,
        "case {case:?}: a second read must report {expected:?}, got {again:?}",
        case = "read twice"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: every documented filter selects, and nothing else is a filter
// ---------------------------------------------------------------------------

/// The rule: each filter name the operation documents selects on the member it names, with
/// EC2's wildcard matching, and a name it does not document is refused.
///
/// The rows differ in which filter is applied and in whether its value selects every
/// association, some of them, or none.
#[tokio::test]
async fn every_documented_filter_selects_and_an_undocumented_one_is_refused() {
    let router = emulator();
    let account = engine::emulator_account_id().to_string();

    let home = create_vpc("the home VPC", &router, "10.21.0.0/16").await;
    let left = create_vpc("the left VPC", &router, "10.22.0.0/16").await;
    let right = create_vpc("the right VPC", &router, "10.23.0.0/16").await;
    let first_group = create_group("the first group", &router, "first", &home).await;
    let second_group = create_group("the second group", &router, "second", &home).await;

    associate_ok("the first group with the left VPC", &router, &first_group, &left).await;
    associate_ok("the first group with the right VPC", &router, &first_group, &right).await;
    associate_ok("the second group with the left VPC", &router, &second_group, &left).await;
    past_the_transition().await;

    let all: BTreeSet<(String, String)> = [
        (first_group.clone(), left.clone()),
        (first_group.clone(), right.clone()),
        (second_group.clone(), left.clone()),
    ]
    .into_iter()
    .collect();
    let of_first: BTreeSet<(String, String)> = all
        .iter()
        .filter(|(group_id, _)| group_id == &first_group)
        .cloned()
        .collect();
    let with_left: BTreeSet<(String, String)> = all
        .iter()
        .filter(|(_, vpc_id)| vpc_id == &left)
        .cloned()
        .collect();

    // (case, the filter, its values, the pairs it must select)
    let rows: [FilterRow<'_>; 9] = [
        (
            "group-id selects one group's associations",
            "group-id",
            vec![first_group.clone()],
            of_first.clone(),
        ),
        (
            "group-id naming both groups selects every association",
            "group-id",
            vec![first_group.clone(), second_group.clone()],
            all.clone(),
        ),
        (
            "group-id with EC2's wildcard selects every group",
            "group-id",
            vec!["sg-*".to_string()],
            all.clone(),
        ),
        (
            "vpc-id selects the associations with one VPC",
            "vpc-id",
            vec![left.clone()],
            with_left.clone(),
        ),
        (
            "vpc-id naming the VPC the groups were created in selects none",
            "vpc-id",
            vec![home.clone()],
            BTreeSet::new(),
        ),
        (
            "state selects the settled associations",
            "state",
            vec![ASSOCIATED.to_string()],
            all.clone(),
        ),
        (
            "state naming the transient state selects none",
            "state",
            vec![ASSOCIATING.to_string()],
            BTreeSet::new(),
        ),
        (
            "group-owner-id selects the owning account's associations",
            "group-owner-id",
            vec![account.clone()],
            all.clone(),
        ),
        (
            "vpc-owner-id naming another account selects none",
            "vpc-owner-id",
            vec!["210987654321".to_string()],
            BTreeSet::new(),
        ),
    ];

    for (case, name, values, expected) in rows {
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let selected: BTreeSet<(String, String)> =
            reported(case, &router, &filters(&[(name, &borrowed)]))
                .await
                .into_iter()
                .map(|association| (association.group_id, association.vpc_id))
                .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected the filter {name:?}={values:?} to select {expected:?}, \
             got {selected:?}"
        );
    }

    // Two filters hold together with AND: one group AND one VPC is the single pair.
    let group_and_vpc: BTreeSet<(String, String)> = reported(
        "group-id and vpc-id together",
        &router,
        &filters(&[("group-id", &[&first_group]), ("vpc-id", &[&right])]),
    )
    .await
    .into_iter()
    .map(|association| (association.group_id, association.vpc_id))
    .collect();
    assert_eq!(
        group_and_vpc,
        [(first_group.clone(), right.clone())].into_iter().collect(),
        "case {case:?}: two filters must be joined with AND, got {group_and_vpc:?}",
        case = "group-id and vpc-id together"
    );

    // A filter name this operation does not document is refused, whichever resource it
    // would have named on another operation.
    for (case, name) in [
        ("a filter of another operation", "group-name"),
        ("a tag filter this operation documents none of", "tag:Name"),
        ("a misspelling of a documented filter", "group_id"),
    ] {
        let (status, body) = action(
            &router,
            "DescribeSecurityGroupVpcAssociations",
            &filters(&[(name, &["anything"])]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for the filter {name:?}, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue for {name:?}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The filter '{name}' is invalid"),
            "case {case:?}: unexpected message, got body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: a request the service cannot satisfy is refused, and writes nothing
// ---------------------------------------------------------------------------

/// The rule: an associate the service cannot satisfy answers the code and wording EC2
/// answers that refusal class with, built from the request's own values, and leaves the
/// store exactly as it was — including under `DryRun`, which asks whether the request WOULD
/// have succeeded.
///
/// The rows differ in WHAT cannot be satisfied: a parameter left out, a `VpcId` that resolves
/// to no VPC (at each malformed shape and each published id length), a `GroupId` that
/// resolves to no group, the two resources the feature excludes, the VPC the group was
/// created in, and a pair already associated.
#[tokio::test]
async fn an_associate_the_service_cannot_satisfy_is_refused_and_writes_nothing() {
    let router = emulator();

    let home = create_vpc("the home VPC", &router, "10.31.0.0/16").await;
    let other = create_vpc("another VPC", &router, "10.32.0.0/16").await;
    let group = create_group("the group", &router, "first", &home).await;
    let home_default_group = default_group_of("the home VPC's default group", &router, &home).await;
    let region_default_vpc = default_vpc("the Region's default VPC", &router).await;

    // One association the account does hold, so the refusals below can be shown to leave
    // the store untouched rather than merely empty.
    associate_ok("the association that stands", &router, &group, &other).await;
    past_the_transition().await;
    let before = reported_pairs("before the refusals", &router).await;

    let rows: [RefusalRow<'_>; 12] = [
        (
            "no GroupId at all",
            String::new(),
            other.clone(),
            "MissingParameter",
            "The request must contain the parameter groupId".to_string(),
        ),
        (
            "no VpcId at all",
            group.clone(),
            String::new(),
            "MissingParameter",
            "The request must contain the parameter vpcId".to_string(),
        ),
        (
            "a well-formed long VPC id no VPC has",
            group.clone(),
            "vpc-0a1b2c3d4e5f60718".to_string(),
            "InvalidVpcID.NotFound",
            "The vpc ID 'vpc-0a1b2c3d4e5f60718' does not exist".to_string(),
        ),
        (
            "a well-formed short VPC id no VPC has",
            group.clone(),
            "vpc-9f8e7d6c".to_string(),
            "InvalidVpcID.NotFound",
            "The vpc ID 'vpc-9f8e7d6c' does not exist".to_string(),
        ),
        (
            "a value that is no VPC id at all",
            group.clone(),
            "not-a-vpc".to_string(),
            "InvalidVpcID.NotFound",
            "The vpc ID 'not-a-vpc' does not exist".to_string(),
        ),
        (
            "the prefix with no digits as the group id",
            "sg-".to_string(),
            other.clone(),
            "InvalidGroupId.Malformed",
            "Invalid id: \"sg-\"".to_string(),
        ),
        (
            "a group id suffix of neither published length",
            "sg-0a1b2c".to_string(),
            other.clone(),
            "InvalidGroupId.Malformed",
            "Invalid id: \"sg-0a1b2c\"".to_string(),
        ),
        (
            "another resource's id as the group id",
            "vpc-0a1b2c3d4e5f60718".to_string(),
            other.clone(),
            "InvalidGroupId.Malformed",
            "Invalid id: \"vpc-0a1b2c3d4e5f60718\"".to_string(),
        ),
        (
            "a well-formed group id no group has",
            "sg-0a1b2c3d4e5f60718".to_string(),
            other.clone(),
            "InvalidGroup.NotFound",
            "The security group 'sg-0a1b2c3d4e5f60718' does not exist".to_string(),
        ),
        (
            "the Region's default VPC as the target",
            group.clone(),
            region_default_vpc.clone(),
            "OperationNotPermitted",
            format!(
                "The vpc '{region_default_vpc}' is the default VPC, which a security group \
                 cannot be associated with"
            ),
        ),
        (
            "a default security group",
            home_default_group.clone(),
            other.clone(),
            "OperationNotPermitted",
            format!(
                "The security group '{home_default_group}' is a default security group, which \
                 cannot be associated with another VPC"
            ),
        ),
        (
            "the VPC the group was created in",
            group.clone(),
            home.clone(),
            "InvalidParameterValue",
            format!(
                "Value ({home}) for parameter vpcId is invalid. The security group '{group}' \
                 was created in that VPC"
            ),
        ),
    ];

    for (case, group_id, vpc_id, code, message) in rows {
        for (how, extra) in [
            ("as sent", Vec::new()),
            ("under DryRun", vec![param("DryRun", "true")]),
        ] {
            let (status, body) = associate(&router, &group_id, &vpc_id, &extra).await;
            assert_eq!(
                status, 400,
                "case {case:?} {how}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                code,
                "case {case:?} {how}: expected {code}, got body {body}"
            );
            assert_eq!(
                error_message(&body),
                message,
                "case {case:?} {how}: expected the message {message:?}, got body {body}"
            );
        }
        let after = reported_pairs(case, &router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused associate must write nothing, expected {before:?}, got \
             {after:?}"
        );
    }

    // A pair the account already holds an association for: refused rather than restarting
    // the association's clock, so the association a later caller reads is still settled.
    let (status, body) = associate(&router, &group, &other, &[]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}",
        case = "a pair already associated"
    );
    assert_eq!(
        error_code(&body),
        "Resource.AlreadyAssociated",
        "case {case:?}: expected Resource.AlreadyAssociated, got body {body}",
        case = "a pair already associated"
    );
    assert_eq!(
        error_message(&body),
        format!("The security group '{group}' is already associated with the vpc '{other}'"),
        "case {case:?}: unexpected message, got body {body}",
        case = "a pair already associated"
    );
    let repeated = reported("after the repeat", &router, &[]).await;
    assert_eq!(
        repeated
            .iter()
            .map(|association| association.state.clone())
            .collect::<Vec<String>>(),
        vec![ASSOCIATED.to_string()],
        "case {case:?}: a refused repeat must not move the association back to \
         {ASSOCIATING:?}, got {repeated:?}",
        case = "a pair already associated"
    );

    // A request that WOULD have succeeded answers `DryRunOperation` and writes nothing: the
    // pair is still unassociated afterwards.
    let third = create_vpc("a third VPC", &router, "10.33.0.0/16").await;
    let (dry_status, dry_body) = associate(&router, &group, &third, &[param("DryRun", "true")]).await;
    assert_eq!(
        dry_status, 412,
        "case {case:?}: expected HTTP 412, got {dry_status} with body {dry_body}",
        case = "a dry run that would have succeeded"
    );
    assert_eq!(
        error_code(&dry_body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got body {dry_body}",
        case = "a dry run that would have succeeded"
    );
    let after_dry_run = reported_pairs("after the dry run", &router).await;
    assert_eq!(
        after_dry_run, before,
        "case {case:?}: a dry run must write nothing, expected {before:?}, got \
         {after_dry_run:?}",
        case = "a dry run that would have succeeded"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: an association cannot outlive either side of its pair
// ---------------------------------------------------------------------------

/// The rule: an association is the (group, VPC) pair, so deleting either side takes it with
/// them rather than leaving a report of a resource that no longer exists.
///
/// The rows differ in WHICH side is deleted — the security group, and the VPC it was
/// associated with.
#[tokio::test]
async fn deleting_either_side_of_the_pair_takes_the_association_with_it() {
    let router = emulator();

    let home = create_vpc("the home VPC", &router, "10.41.0.0/16").await;
    let group_side = create_vpc("the VPC the deleted group reached", &router, "10.42.0.0/16").await;
    let vpc_side = create_vpc("the VPC that is deleted", &router, "10.43.0.0/16").await;
    let doomed_group = create_group("the group that is deleted", &router, "doomed", &home).await;
    let surviving_group = create_group("the surviving group", &router, "survivor", &home).await;

    associate_ok("the deleted group's association", &router, &doomed_group, &group_side).await;
    associate_ok("the deleted VPC's association", &router, &surviving_group, &vpc_side).await;
    associate_ok("the association that survives both", &router, &surviving_group, &group_side).await;
    past_the_transition().await;

    // Deleting the security group takes only the associations of THAT group.
    ok(
        "delete the group",
        &router,
        "DeleteSecurityGroup",
        &[param("GroupId", &doomed_group)],
    )
    .await;
    let after_group = reported_pairs("after deleting the group", &router).await;
    let mut expected_after_group = vec![
        (surviving_group.clone(), group_side.clone()),
        (surviving_group.clone(), vpc_side.clone()),
    ];
    expected_after_group.sort();
    assert_eq!(
        after_group, expected_after_group,
        "case {case:?}: expected {expected_after_group:?}, got {after_group:?}",
        case = "after deleting the group"
    );

    // Deleting the VPC takes only the associations naming THAT VPC.
    ok(
        "delete the VPC",
        &router,
        "DeleteVpc",
        &[param("VpcId", &vpc_side)],
    )
    .await;
    let after_vpc = reported_pairs("after deleting the VPC", &router).await;
    assert_eq!(
        after_vpc,
        vec![(surviving_group.clone(), group_side.clone())],
        "case {case:?}: expected only the untouched pair, got {after_vpc:?}",
        case = "after deleting the VPC"
    );
}

// ---------------------------------------------------------------------------
// Rule 7: the report pages
// ---------------------------------------------------------------------------

/// The rule: `MaxResults` cuts the page, the `NextToken` the response carries resumes from
/// the end of it, and walking the token chain reports every association exactly once — with
/// no token on the last page.
///
/// The rows differ in the page size, including one at the documented minimum, one that
/// divides the association count exactly and one that does not.
#[tokio::test]
async fn the_report_pages_over_every_association_exactly_once() {
    let router = emulator();

    let home = create_vpc("the home VPC", &router, "10.51.0.0/16").await;
    let group = create_group("the group", &router, "first", &home).await;
    let mut expected: Vec<(String, String)> = Vec::new();
    for index in 0..8u8 {
        let case = format!("the VPC {index}");
        let vpc_id = create_vpc(&case, &router, &format!("10.6{index}.0.0/16")).await;
        associate_ok(&case, &router, &group, &vpc_id).await;
        expected.push((group.clone(), vpc_id));
    }
    expected.sort();
    past_the_transition().await;

    // The documented range is 5..1000, so 5 is the smallest page a request may ask for.
    for page_size in [5usize, 8, 1000] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(next) = token.as_deref() {
                params.push(param("NextToken", next));
            }
            let body = ok(
                &case,
                &router,
                "DescribeSecurityGroupVpcAssociations",
                &params,
            )
            .await;
            let page = associations(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} associations, got {}",
                page.len()
            );
            walked.extend(
                page.into_iter()
                    .map(|association| (association.group_id, association.vpc_id)),
            );
            token = super::ec2_query::member(&body, "nextToken").filter(|next| !next.is_empty());
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= expected.len(),
                "case {case:?}: the token chain must terminate, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the whole report once, got {walked:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetSecurityGroupsForVpc`: the security groups a network interface
//! in ONE VPC can be given. Driven over the wire the service actually speaks — `ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. THE REPORT ANSWERS THE GROUPS USABLE IN THE VPC: every group created in the VPC the
//!    request names, each reported with the description, name, owning account, id and tags
//!    the group is stored with, and with `primaryVpcId` the VPC it was CREATED in;
//! 2. a group of ANOTHER VPC becomes usable in this one only once the association
//!    `AssociateSecurityGroupVpc` started has been established by the CLOCK, and it is then
//!    reported with its own `primaryVpcId` rather than the VPC asked about;
//! 3. every documented filter selects on the member it names, several filters are joined
//!    with AND and several values of one filter with OR, and a filter name the operation
//!    does not document is REFUSED rather than read as "no results";
//! 4. the read WRITES NOTHING: two reads answer the same report, and the account's groups
//!    and associations are as they were;
//! 5. a `VpcId` that names no VPC of the account is REFUSED with the code and the wording
//!    EC2 answers that refusal class with, whatever the value's shape, and a `DryRun`
//!    reports the permission outcome instead of a report;
//! 6. the report PAGES: `MaxResults` cuts the page and the `NextToken` it answers walks the
//!    rest, each group reported exactly once.
//!
//! Each table row is a distinct input along what those rules generalise over — which VPC,
//! how many groups it holds, whether a group carries tags, which filter selects and on what
//! value, how many filters and values are combined, and the form of a `VpcId` that names
//! nothing (each malformed shape, a well-formed id at both published id lengths) — so no one
//! row is the specification.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{MAX_ECHOED_VPC_ID_CHARS, TRANSITION_DELAY};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, member};
use super::wire::{Param, child_text, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The name AWS gives the security group every VPC comes with.
const DEFAULT_GROUP_NAME: &str = "default";

/// The description AWS gives that group.
const DEFAULT_GROUP_DESCRIPTION: &str = "default VPC security group";

/// One reported group, as `GetSecurityGroupsForVpc` writes it on the wire.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Reported {
    group_id: String,
    group_name: String,
    description: String,
    owner_id: String,
    primary_vpc_id: String,
    tags: Vec<(String, String)>,
}

/// One row of the filter table: the case, the filter entries to send, and the group ids the
/// report must then answer.
type FilterRow<'a> = (&'a str, Vec<(&'a str, Vec<String>)>, BTreeSet<String>);

/// One row of the refusal table: the case and the `VpcId` the request names.
type RefusalRow<'a> = (&'a str, String);

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

/// A security group in `vpc_id`, carrying `tags`, and the id it was minted with.
async fn create_group(
    case: &str,
    router: &MultiServiceRouter,
    group_name: &str,
    description: &str,
    vpc_id: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut params = vec![
        param("GroupName", group_name),
        param("GroupDescription", description),
        param("VpcId", vpc_id),
    ];
    if !tags.is_empty() {
        params.push(param("TagSpecification.1.ResourceType", "security-group"));
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(param(&format!("TagSpecification.1.Tag.{}.Key", index + 1), key));
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                value,
            ));
        }
    }
    let body = ok(case, router, "CreateSecurityGroup", &params).await;
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no groupId in {body}"))
}

/// The id of the Region's DEFAULT VPC — the one the account was given rather than one this
/// suite created, read through the `is-default` filter `DescribeVpcs` documents.
async fn default_vpc(case: &str, router: &MultiServiceRouter) -> String {
    let body = ok(
        case,
        router,
        "DescribeVpcs",
        &super::ec2_query::filters(&[("is-default", &["true"])]),
    )
    .await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no default VPC in {body}"))
}

/// The id of the default security group of `vpc_id`, read through the filters
/// `DescribeSecurityGroups` documents rather than assumed.
async fn default_group_of(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeSecurityGroups",
        &super::ec2_query::filters(&[("vpc-id", &[vpc_id]), ("group-name", &[DEFAULT_GROUP_NAME])]),
    )
    .await;
    element(&body, "groupId")
        .unwrap_or_else(|| panic!("case {case:?}: no default group of {vpc_id} in {body}"))
}

/// Every id `DescribeSecurityGroups` reports for the account, as the "nothing was written"
/// assertions compare.
async fn every_group_id(case: &str, router: &MultiServiceRouter) -> BTreeSet<String> {
    let body = ok(case, router, "DescribeSecurityGroups", &[]).await;
    super::ec2_query::elements(&body, "groupId")
        .into_iter()
        .collect()
}

/// Every (group, VPC) pair `DescribeSecurityGroupVpcAssociations` reports.
async fn every_association(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeSecurityGroupVpcAssociations",
        &[],
    )
    .await;
    super::ec2_query::elements(&body, "groupId")
}

/// The groups one `GetSecurityGroupsForVpc` body carries, in document order.
fn reported_groups(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "securityGroupForVpcSet")
    else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| Reported {
            group_id: child_text(item, "groupId").unwrap_or_default(),
            group_name: child_text(item, "groupName").unwrap_or_default(),
            description: child_text(item, "description").unwrap_or_default(),
            owner_id: child_text(item, "ownerId").unwrap_or_default(),
            primary_vpc_id: child_text(item, "primaryVpcId").unwrap_or_default(),
            tags: set_items(item, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The report `vpc_id` answers, with `extra` parameters beside it, and the `NextToken` it
/// carries.
async fn report(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    extra: &[Param],
) -> (Vec<Reported>, Option<String>) {
    let mut params = vec![param("VpcId", vpc_id)];
    params.extend_from_slice(extra);
    let body = ok(case, router, "GetSecurityGroupsForVpc", &params).await;
    (
        reported_groups(case, &body),
        member(&body, "nextToken").filter(|token| !token.is_empty()),
    )
}

/// The ids the report answers for `vpc_id`, as a set.
async fn reported_ids(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> BTreeSet<String> {
    report(case, router, vpc_id, &[])
        .await
        .0
        .into_iter()
        .map(|group| group.group_id)
        .collect()
}

/// The form fields for a `Filters` list whose values are owned strings.
fn filter_params(entries: &[(&str, Vec<String>)]) -> Vec<Param> {
    let mut params = Vec::new();
    for (index, (name, values)) in entries.iter().enumerate() {
        params.push(param(&format!("Filter.{}.Name", index + 1), name));
        for (position, value) in values.iter().enumerate() {
            params.push(param(
                &format!("Filter.{}.Value.{}", index + 1, position + 1),
                value,
            ));
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Rules 1 and 4: the report answers the VPC's own groups, and writes nothing
// ---------------------------------------------------------------------------

/// The rule: the report answers every group created in the VPC the request names, each with
/// the facts the group is stored with and with `primaryVpcId` naming the VPC it was created
/// in — and reading it twice answers the same thing.
///
/// The rows differ in which VPC is asked about (the Region's default VPC, two the suite
/// created), how many groups it holds (one, two, three) and whether a group carries tags, so
/// the report is a property of the store rather than of the one VPC a recording used.
#[tokio::test]
async fn the_report_answers_the_groups_created_in_the_vpc() {
    let router = emulator();
    let account = engine::emulator_account_id().to_string();

    let ambient = default_vpc("the Region's default VPC", &router).await;
    let ambient_default_group = default_group_of("the Region's default VPC", &router, &ambient).await;

    let first = create_vpc("the first VPC", &router, "10.21.0.0/16").await;
    let first_default_group = default_group_of("the first VPC", &router, &first).await;
    let first_extra = create_group(
        "the first VPC",
        &router,
        "first-extra",
        "a group of the first VPC",
        &first,
        &[],
    )
    .await;

    let second = create_vpc("the second VPC", &router, "10.22.0.0/16").await;
    let second_default_group = default_group_of("the second VPC", &router, &second).await;
    let second_plain = create_group(
        "the second VPC",
        &router,
        "second-plain",
        "an untagged group of the second VPC",
        &second,
        &[],
    )
    .await;
    let second_tagged = create_group(
        "the second VPC",
        &router,
        "second-tagged",
        "a tagged group of the second VPC",
        &second,
        &[("env", "dev")],
    )
    .await;

    let rows: [(&str, &str, Vec<Reported>); 3] = [
        (
            "the Region's default VPC",
            &ambient,
            vec![Reported {
                group_id: ambient_default_group.clone(),
                group_name: DEFAULT_GROUP_NAME.to_string(),
                description: DEFAULT_GROUP_DESCRIPTION.to_string(),
                owner_id: account.clone(),
                primary_vpc_id: ambient.clone(),
                tags: Vec::new(),
            }],
        ),
        (
            "a VPC with one group beside its default",
            &first,
            vec![
                Reported {
                    group_id: first_default_group.clone(),
                    group_name: DEFAULT_GROUP_NAME.to_string(),
                    description: DEFAULT_GROUP_DESCRIPTION.to_string(),
                    owner_id: account.clone(),
                    primary_vpc_id: first.clone(),
                    tags: Vec::new(),
                },
                Reported {
                    group_id: first_extra.clone(),
                    group_name: "first-extra".to_string(),
                    description: "a group of the first VPC".to_string(),
                    owner_id: account.clone(),
                    primary_vpc_id: first.clone(),
                    tags: Vec::new(),
                },
            ],
        ),
        (
            "a VPC holding a tagged group too",
            &second,
            vec![
                Reported {
                    group_id: second_default_group.clone(),
                    group_name: DEFAULT_GROUP_NAME.to_string(),
                    description: DEFAULT_GROUP_DESCRIPTION.to_string(),
                    owner_id: account.clone(),
                    primary_vpc_id: second.clone(),
                    tags: Vec::new(),
                },
                Reported {
                    group_id: second_plain.clone(),
                    group_name: "second-plain".to_string(),
                    description: "an untagged group of the second VPC".to_string(),
                    owner_id: account.clone(),
                    primary_vpc_id: second.clone(),
                    tags: Vec::new(),
                },
                Reported {
                    group_id: second_tagged.clone(),
                    group_name: "second-tagged".to_string(),
                    description: "a tagged group of the second VPC".to_string(),
                    owner_id: account.clone(),
                    primary_vpc_id: second.clone(),
                    tags: vec![("env".to_string(), "dev".to_string())],
                },
            ],
        ),
    ];

    let groups_before = every_group_id("before the reads", &router).await;
    for (case, vpc_id, expected) in rows {
        let mut wanted = expected;
        wanted.sort();
        let (reported, token) = report(case, &router, vpc_id, &[]).await;
        let mut sorted = reported.clone();
        sorted.sort();
        assert_eq!(
            sorted, wanted,
            "case {case:?}: expected the groups {wanted:?}, got {sorted:?}"
        );
        // Rule 6, on a report no `MaxResults` cut: nothing follows it, so no token.
        assert_eq!(
            token, None,
            "case {case:?}: an unpaged report must carry no NextToken, got {token:?}"
        );
        // Rule 4: a second read answers the same report, in the same order.
        let (again, _) = report(case, &router, vpc_id, &[]).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must report the same groups, got {again:?} after \
             {reported:?}"
        );
    }

    // Rule 4, through the store: the reads created no group and no association.
    let groups_after = every_group_id("after the reads", &router).await;
    assert_eq!(
        groups_after, groups_before,
        "the reads must leave the account's groups as they were, got {groups_after:?} after \
         {groups_before:?}"
    );
    let associations = every_association("after the reads", &router).await;
    assert!(
        associations.is_empty(),
        "the reads must create no association, got {associations:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a group of another VPC, once the clock has established its association
// ---------------------------------------------------------------------------

/// The rule: a group created in another VPC is usable in this one once the association
/// `AssociateSecurityGroupVpc` started has been established, and it is reported with the VPC
/// it was CREATED in as its `primaryVpcId`.
///
/// The rows are the two sides of the clock's span — the association the service has just
/// written, which is not established yet, and the same association after the span — and the
/// groups of the other VPC that were never associated, which stay out of the report either
/// way.
#[tokio::test]
async fn a_group_of_another_vpc_is_reported_once_its_association_is_established() {
    let router = emulator();
    let account = engine::emulator_account_id().to_string();

    let home = create_vpc("the home VPC", &router, "10.31.0.0/16").await;
    let target = create_vpc("the target VPC", &router, "10.32.0.0/16").await;
    let target_default_group = default_group_of("the target VPC", &router, &target).await;
    let guest = create_group(
        "the guest group",
        &router,
        "guest",
        "a group of the home VPC",
        &home,
        &[],
    )
    .await;
    let bystander = create_group(
        "a group nobody associates",
        &router,
        "bystander",
        "another group of the home VPC",
        &home,
        &[],
    )
    .await;

    // Before any associate, the target VPC reports only its own group.
    let before = reported_ids("before the associate", &router, &target).await;
    assert_eq!(
        before,
        BTreeSet::from([target_default_group.clone()]),
        "case {case:?}: expected only the VPC's own group, got {before:?}",
        case = "before the associate"
    );

    let (status, body) = action(
        &router,
        "AssociateSecurityGroupVpc",
        &[param("GroupId", &guest), param("VpcId", &target)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the associate must succeed, got {status} with body {body}"
    );

    // The association the service has just written is not established yet, so the group it
    // names cannot be used in the VPC yet either — read at once, inside the span the
    // associate started.
    let associating = reported_ids("while the association is still associating", &router, &target).await;
    assert_eq!(
        associating,
        BTreeSet::from([target_default_group.clone()]),
        "case {case:?}: expected only the VPC's own group, got {associating:?}",
        case = "while the association is still associating"
    );

    past_the_transition().await;

    let (settled, _) = report("past the transition", &router, &target, &[]).await;
    let mut expected = vec![
        Reported {
            group_id: target_default_group.clone(),
            group_name: DEFAULT_GROUP_NAME.to_string(),
            description: DEFAULT_GROUP_DESCRIPTION.to_string(),
            owner_id: account.clone(),
            primary_vpc_id: target.clone(),
            tags: Vec::new(),
        },
        Reported {
            group_id: guest.clone(),
            group_name: "guest".to_string(),
            description: "a group of the home VPC".to_string(),
            owner_id: account.clone(),
            // The VPC the group was CREATED in, not the one the request asked about.
            primary_vpc_id: home.clone(),
            tags: Vec::new(),
        },
    ];
    expected.sort();
    let mut sorted = settled.clone();
    sorted.sort();
    assert_eq!(
        sorted, expected,
        "case {case:?}: expected {expected:?}, got {sorted:?}",
        case = "past the transition"
    );
    assert!(
        !sorted.iter().any(|group| group.group_id == bystander),
        "case {case:?}: a group nobody associated must stay out of the report, got {sorted:?}",
        case = "past the transition"
    );

    // The home VPC still reports its own groups and not the target's: an association makes
    // the group usable in the OTHER VPC, it does not move it.
    let home_side = reported_ids("the home VPC past the transition", &router, &home).await;
    assert!(
        home_side.contains(&guest) && home_side.contains(&bystander),
        "case {case:?}: the home VPC must still report its own groups, got {home_side:?}",
        case = "the home VPC past the transition"
    );
    assert!(
        !home_side.contains(&target_default_group),
        "case {case:?}: the home VPC must not report the target's group, got {home_side:?}",
        case = "the home VPC past the transition"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the documented filters, and the names that are not
// ---------------------------------------------------------------------------

/// The rule: each filter the operation documents selects on the reported member it names,
/// several filters are joined with AND and several values of one filter with OR, and a name
/// the operation does not document is refused.
///
/// The catalog under test is the one the operation publishes
/// (`smithy_query(query="doc",shape="GetSecurityGroupsForVpcRequest",member="Filters")`:
/// `group-id`, `description`, `group-name`, `owner-id`, `primary-vpc-id`), and the refused
/// rows include the names the NEIGHBOURING security group reads do document — `vpc-id` and
/// `tag:<key>` — which this one does not.
#[tokio::test]
async fn every_documented_filter_selects_and_an_undocumented_one_is_refused() {
    let router = emulator();
    let account = engine::emulator_account_id().to_string();

    let vpc = create_vpc("the filtered VPC", &router, "10.41.0.0/16").await;
    let default_group = default_group_of("the filtered VPC", &router, &vpc).await;
    let web = create_group(
        "the web group",
        &router,
        "web",
        "the web tier",
        &vpc,
        &[("env", "dev")],
    )
    .await;
    let data = create_group(
        "the data group",
        &router,
        "data",
        "the data tier",
        &vpc,
        &[],
    )
    .await;
    let elsewhere = create_vpc("another VPC", &router, "10.42.0.0/16").await;

    let everything = BTreeSet::from([default_group.clone(), web.clone(), data.clone()]);
    let rows: Vec<FilterRow<'_>> = vec![
        (
            "group-id selects one group",
            vec![("group-id", vec![data.clone()])],
            BTreeSet::from([data.clone()]),
        ),
        (
            "group-name selects the group AWS gives every VPC",
            vec![("group-name", vec![DEFAULT_GROUP_NAME.to_string()])],
            BTreeSet::from([default_group.clone()]),
        ),
        (
            "description selects on the group's own sentence",
            vec![("description", vec!["the web tier".to_string()])],
            BTreeSet::from([web.clone()]),
        ),
        (
            "owner-id selects every group of the account",
            vec![("owner-id", vec![account.clone()])],
            everything.clone(),
        ),
        (
            "owner-id of another account selects none",
            vec![("owner-id", vec!["000000000001".to_string()])],
            BTreeSet::new(),
        ),
        (
            "primary-vpc-id selects the groups created in this VPC",
            vec![("primary-vpc-id", vec![vpc.clone()])],
            everything.clone(),
        ),
        (
            "primary-vpc-id of another VPC selects none",
            vec![("primary-vpc-id", vec![elsewhere.clone()])],
            BTreeSet::new(),
        ),
        (
            "two values of one filter are joined with OR",
            vec![(
                "group-name",
                vec!["web".to_string(), "data".to_string()],
            )],
            BTreeSet::from([web.clone(), data.clone()]),
        ),
        (
            "two filters are joined with AND",
            vec![
                ("owner-id", vec![account.clone()]),
                ("group-name", vec!["web".to_string()]),
            ],
            BTreeSet::from([web.clone()]),
        ),
        (
            "two filters with nothing in common select none",
            vec![
                ("group-name", vec!["web".to_string()]),
                ("description", vec!["the data tier".to_string()]),
            ],
            BTreeSet::new(),
        ),
    ];

    for (case, entries, expected) in rows {
        let (reported, _) = report(case, &router, &vpc, &filter_params(&entries)).await;
        let selected: BTreeSet<String> = reported
            .into_iter()
            .map(|group| group.group_id)
            .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected the groups {expected:?}, got {selected:?}"
        );
    }

    // A name this operation does not document is refused, whether or not a group would have
    // matched it — including the two the neighbouring security group reads do document.
    let refused: [(&str, &str); 5] = [
        ("a filter of the neighbouring describe", "vpc-id"),
        ("a tag filter", "tag:env"),
        ("the tag-key filter", "tag-key"),
        ("a documented name in the wrong case", "Group-Id"),
        ("a misspelling", "group_id"),
    ];
    for (case, name) in refused {
        let (status, body) = action(
            &router,
            "GetSecurityGroupsForVpc",
            &[
                param("VpcId", &vpc),
                param("Filter.1.Name", name),
                param("Filter.1.Value.1", "anything"),
            ],
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
            "case {case:?}: expected InvalidParameterValue for the filter {name:?}, got body \
             {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The filter '{name}' is invalid"),
            "case {case:?}: unexpected message in {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the refusals
// ---------------------------------------------------------------------------

/// The rule: a `VpcId` that names no VPC of the account gets the SAME refusal whatever the
/// value's shape, naming the value the request sent; and a `DryRun` reports the permission
/// outcome rather than a report.
///
/// The code and the sentence are the ones RECORDED for this operation, which answers them
/// for a well-formed VPC id the account holds no VPC for as well as for a value that is no
/// VPC id at all — the same one-answer refusal `CreateCarrierGateway` records for its own
/// `VpcId` (`tests/all/carrier_gateways.rs`), and NOT the `InvalidVpcID.NotFound` the
/// operations recorded answering that one keep. So the rows differ in the shape of the
/// unresolvable value — a well-formed id at each published id length, the prefix with no
/// digits, the all-zero placeholder, another resource's id, a value that is no id at all,
/// and a near miss of a held id — to pin that one answer across the whole class.
#[tokio::test]
async fn a_vpc_id_the_account_holds_no_vpc_for_is_refused() {
    let router = emulator();
    let held = create_vpc("the held VPC", &router, "10.51.0.0/16").await;

    let rows: Vec<RefusalRow<'_>> = vec![
        ("the long form", "vpc-0ffffffffffffffff".to_string()),
        ("the short form", "vpc-0fffffff".to_string()),
        ("the all-zero placeholder", "vpc-00000000000000000".to_string()),
        ("the prefix with no digits", "vpc-".to_string()),
        ("another resource's id", "subnet-0ffffffffffffffff".to_string()),
        ("no id at all", "not-a-vpc".to_string()),
        ("a near miss of a held id", format!("{held}0")),
    ];

    for (case, vpc_id) in &rows {
        let (status, body) = action(
            &router,
            "GetSecurityGroupsForVpc",
            &[param("VpcId", vpc_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcId.Malformed",
            "case {case:?}: expected InvalidVpcId.Malformed, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The vpc ID '{vpc_id}' is malformed"),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // The same request for the VPC the account DOES hold is served, so the refusals above
    // are the id's doing and nothing else.
    let (reported, _) = report("the held VPC", &router, &held, &[]).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the VPC's one default group, got {reported:?}",
        case = "the held VPC"
    );

    // A value far longer than any id EC2 issues is named back only as far as the service
    // echoes a refused one, so the message cannot carry an arbitrary request's bytes in
    // full. The bound is the service's own, not a literal, so the two cannot drift.
    let oversize = "vpc-".to_string() + &"f".repeat(MAX_ECHOED_VPC_ID_CHARS);
    let (status, body) = action(
        &router,
        "GetSecurityGroupsForVpc",
        &[param("VpcId", &oversize)],
    )
    .await;
    let case = "an id longer than the echo bound";
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let echoed: String = oversize.chars().take(MAX_ECHOED_VPC_ID_CHARS).collect();
    assert_eq!(
        error_message(&body),
        format!("The vpc ID '{echoed}' is malformed"),
        "case {case:?}: the message must name only the first {MAX_ECHOED_VPC_ID_CHARS} \
         characters, got body {body}"
    );

    // A dry run reports the permission outcome and no groups.
    let (status, body) = action(
        &router,
        "GetSecurityGroupsForVpc",
        &[param("VpcId", &held), param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}",
        case = "a dry run"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got body {body}",
        case = "a dry run"
    );

    // A `NextToken` this service did not issue is refused rather than read as a position.
    let (status, body) = action(
        &router,
        "GetSecurityGroupsForVpc",
        &[param("VpcId", &held), param("NextToken", "not-a-token")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}",
        case = "a token this service did not issue"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "case {case:?}: expected InvalidPaginationToken, got body {body}",
        case = "a token this service did not issue"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: paging
// ---------------------------------------------------------------------------

/// The rule: `MaxResults` cuts the page, the `NextToken` the report answers walks the rest,
/// and every group of the VPC is reported exactly once over the walk.
///
/// The rows differ in the page size — the smallest the model's range admits, one that
/// divides the set evenly, and one larger than the whole set, which is not paged at all.
#[tokio::test]
async fn the_report_pages_through_the_groups_of_one_vpc() {
    let router = emulator();
    let vpc = create_vpc("the paged VPC", &router, "10.61.0.0/16").await;
    // Seven groups in all: the one the VPC comes with and six of the suite's own, so a page
    // of five leaves a short second page.
    for index in 0..6 {
        create_group(
            "the paged VPC",
            &router,
            &format!("paged-{index}"),
            "a group of the paged VPC",
            &vpc,
            &[],
        )
        .await;
    }
    let everything = reported_ids("the whole set", &router, &vpc).await;
    assert_eq!(
        everything.len(),
        7,
        "the paged VPC must hold seven groups, got {everything:?}"
    );

    for page_size in [5_usize, 7, 1000] {
        let case = format!("a page size of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut extra = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = &token {
                extra.push(param("NextToken", token));
            }
            let (reported, next) = report(&case, &router, &vpc, &extra).await;
            assert!(
                reported.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} groups, got \
                 {count} of them",
                count = reported.len()
            );
            walked.extend(reported.into_iter().map(|group| group.group_id));
            pages += 1;
            assert!(
                pages <= 8,
                "case {case:?}: the walk must end, got {pages} pages and {walked:?}"
            );
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        let unique: BTreeSet<String> = walked.iter().cloned().collect();
        assert_eq!(
            unique, everything,
            "case {case:?}: the walk must report every group of the VPC, got {unique:?}"
        );
        assert_eq!(
            walked.len(),
            unique.len(),
            "case {case:?}: no group may be reported twice, got {walked:?}"
        );
        let expected_pages = everything.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s), got {pages}"
        );
    }
}

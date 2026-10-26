// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service WORDS the one DB-subnet-group not-found fault,
//! and for the `ModifyDBSubnetGroup` path that answers it.
//!
//! The rule under test: `DBSubnetGroupNotFoundFault` is one modelled error — HTTP 404 with
//! the `awsQueryError` code `DBSubnetGroupNotFoundFault`
//! (`smithy_query(query="operation", operation="DeleteDBSubnetGroup,DescribeDBSubnetGroups,
//! ModifyDBSubnetGroup")`) — but RDS words its message by the ROLE the group plays in the
//! call: a call whose SUBJECT is the group and whose effect changes it
//! (`DeleteDBSubnetGroup`, `ModifyDBSubnetGroup`) answers "DB subnet group '<name>' does not
//! exist.", while the call that REPORTS groups (`DescribeDBSubnetGroups`) answers "DB Subnet
//! group '<name>' not found.". Both templates are parameterized by the requested
//! `DBSubnetGroupName`, in the folded spelling the group would be stored under ("This value
//! is stored as a lowercase string"), so a caller can tell "there is no such group to
//! report" from "there is no such group to change" whatever name it asked about.
//!
//! The table spans the classes the rule generalises over: the shape of a name nothing holds
//! (a fresh name, a near miss of a stored one, a single letter, mixed case, the punctuation
//! and spaces the member admits, a name at the published 255-character bound, a group only
//! another region holds) and how the name came to hold nothing (never created, or created
//! and then deleted).
//!
//! The second half pins the `ModifyDBSubnetGroup` behaviour the wording sits on: a
//! modification of a group that DOES exist is answered from the stored record and read back
//! by the report, and the subnet list it carries is refused by the same rule
//! `CreateDBSubnetGroup` applies to its own.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the assertions
//! read the XML the emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DEFAULT_DB_SUBNET_GROUP, MAX_SUBNETS_PER_DB_SUBNET_GROUP,
};

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, texts, REGION};

/// A second region, so a group that exists but is out of the signed region's scope is a row
/// of the table.
const OTHER_REGION: &str = "ap-south-1";

/// Two subnets in the 17-character id form: the smallest list a DB subnet group can be made
/// of, carried by every call below that is not about the list itself.
const TWO_SUBNETS: [&str; 2] = ["subnet-0123456789abcdef0", "subnet-0fedcba987654321f"];

/// The message `DescribeDBSubnetGroups` — the REPORT — answers for a name it cannot resolve.
fn report_wording(folded_name: &str) -> String {
    format!("DB Subnet group '{folded_name}' not found.")
}

/// The message `DeleteDBSubnetGroup` and `ModifyDBSubnetGroup` — the calls whose subject IS
/// the group — answer for a name they cannot resolve.
fn mutating_wording(folded_name: &str) -> String {
    format!("DB subnet group '{folded_name}' does not exist.")
}

/// The `SubnetIds.SubnetIdentifier.<n>` members for `subnets`, in the order given.
fn subnet_params(subnets: &[String]) -> Vec<(String, String)> {
    subnets
        .iter()
        .enumerate()
        .map(|(index, subnet)| {
            (
                format!("SubnetIds.SubnetIdentifier.{}", index + 1),
                subnet.clone(),
            )
        })
        .collect()
}

/// `CreateDBSubnetGroup` in `region`, over `subnets`.
async fn create_group(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    description: &str,
    subnets: &[String],
) -> (u16, String) {
    let mut params = vec![
        ("DBSubnetGroupName".to_string(), name.to_string()),
        (
            "DBSubnetGroupDescription".to_string(),
            description.to_string(),
        ),
    ];
    params.extend(subnet_params(subnets));
    call_in(router, region, "CreateDBSubnetGroup", &params).await
}

/// `ModifyDBSubnetGroup` in `region`: `SubnetIds` is required, the description optional.
async fn modify_group(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    description: Option<&str>,
    subnets: &[String],
) -> (u16, String) {
    let mut params = vec![("DBSubnetGroupName".to_string(), name.to_string())];
    if let Some(description) = description {
        params.push((
            "DBSubnetGroupDescription".to_string(),
            description.to_string(),
        ));
    }
    params.extend(subnet_params(subnets));
    call_in(router, region, "ModifyDBSubnetGroup", &params).await
}

/// The two subnets every "names nothing" row carries, as owned strings.
fn two_subnets() -> Vec<String> {
    TWO_SUBNETS.iter().map(|id| (*id).to_string()).collect()
}

/// A `DBSubnetGroupName` of exactly the 255 characters its member admits ("Must contain no
/// more than 255 letters, numbers, periods, underscores, spaces, or hyphens. First character
/// must be a letter"), so the published bound is one of the rows.
fn name_at_the_published_bound() -> String {
    let mut name = String::from("e");
    name.extend(std::iter::repeat_n('a', 254));
    assert_eq!(
        name.chars().count(),
        255,
        "the row at the published bound must be 255 characters, got {}",
        name.chars().count()
    );
    name
}

/// Assert both wordings for one name nothing holds in [`REGION`]: the report's and the
/// mutating calls', and that a refused mutation created nothing.
async fn assert_both_wordings(router: &MultiServiceRouter, case: &str, name: &str) {
    let folded = name.to_ascii_lowercase();

    let reported = expect_error(
        case,
        call(
            router,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", name)],
        )
        .await,
        404,
        "DBSubnetGroupNotFoundFault",
    );
    let reported = text(&reported, "Message");
    assert_eq!(
        reported,
        report_wording(&folded),
        "case {case:?}: expected the report's wording, got {reported:?}"
    );

    let deleted = expect_error(
        case,
        call(
            router,
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", name)],
        )
        .await,
        404,
        "DBSubnetGroupNotFoundFault",
    );
    let deleted = text(&deleted, "Message");
    assert_eq!(
        deleted,
        mutating_wording(&folded),
        "case {case:?}: expected the mutating wording, got {deleted:?}"
    );

    let modified = expect_error(
        case,
        modify_group(router, REGION, name, Some("a description"), &two_subnets()).await,
        404,
        "DBSubnetGroupNotFoundFault",
    );
    let modified = text(&modified, "Message");
    assert_eq!(
        modified,
        mutating_wording(&folded),
        "case {case:?}: expected the mutating wording, got {modified:?}"
    );

    assert_ne!(
        reported, deleted,
        "case {case:?}: the report and the mutating calls must not answer one \
         undifferentiated string, both answered {reported:?}"
    );

    // The refused mutation created nothing: the name still resolves to no group.
    let after = expect_error(
        case,
        call(
            router,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", name)],
        )
        .await,
        404,
        "DBSubnetGroupNotFoundFault",
    );
    let after = text(&after, "Message");
    assert_eq!(
        after,
        report_wording(&folded),
        "case {case:?}: expected the refused calls to have created nothing, got {after:?}"
    );
}

#[tokio::test]
async fn each_path_words_the_one_not_found_fault_the_way_that_path_words_it() {
    let router = emulator();

    // One group this region holds, so a near miss of its name is a row, and one that only
    // another region holds.
    expect_ok(
        "the stored group",
        create_group(
            &router,
            REGION,
            "emu-wording-subnets",
            "the stored group",
            &two_subnets(),
        )
        .await,
    );
    expect_ok(
        "the other region's group",
        create_group(
            &router,
            OTHER_REGION,
            "emu-wording-elsewhere",
            "another region's group",
            &two_subnets(),
        )
        .await,
    );

    let at_the_bound = name_at_the_published_bound();
    let rows: Vec<(&str, &str)> = vec![
        ("a name nothing was created under", "emu-wording-ghost"),
        ("a near miss of a stored name", "emu-wording-subnets-2"),
        ("a single letter", "e"),
        ("a name in mixed case, folded on the way in", "EMU-Wording-Ghost-Mixed"),
        (
            "the periods, underscores and spaces the member admits",
            "emu.wording_ghost 3",
        ),
        ("a name at the published 255-character bound", at_the_bound.as_str()),
        ("a group only another region holds", "emu-wording-elsewhere"),
    ];
    for (case, name) in rows {
        assert_both_wordings(&router, case, name).await;
    }

    // And a name whose group DID exist until a delete removed it: the two wordings are the
    // ones the path answers, not the ones the group's history suggests.
    expect_ok(
        "the group that is about to be deleted",
        create_group(
            &router,
            REGION,
            "emu-wording-deleted",
            "deleted below",
            &two_subnets(),
        )
        .await,
    );
    expect_ok(
        "the delete of a group that exists",
        call(
            &router,
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", "emu-wording-deleted")],
        )
        .await,
    );
    assert_both_wordings(
        &router,
        "a name whose group a delete removed",
        "emu-wording-deleted",
    )
    .await;
}

/// One modification of a group that exists.
struct ModificationCase {
    case: &'static str,
    /// The description the request carries, if any.
    description: Option<&'static str>,
    /// The subnets the modified group covers, in the order the request names them.
    subnets: &'static [&'static str],
    /// The description the group carries afterwards.
    expected_description: &'static str,
}

/// The modifications the round trip applies, in order, to one group created with
/// [`TWO_SUBNETS`] and the description `"the group as created"`.
const MODIFICATIONS: &[ModificationCase] = &[
    ModificationCase {
        case: "three subnets of the 17-character form and a new description",
        description: Some("the group as modified"),
        subnets: &[
            "subnet-0123456789abcdef0",
            "subnet-0fedcba987654321f",
            "subnet-00112233445566778",
        ],
        expected_description: "the group as modified",
    },
    ModificationCase {
        case: "two subnets of the legacy 8-character form, no description",
        description: None,
        subnets: &["subnet-0a1b2c3d", "subnet-4e5f6071"],
        expected_description: "the group as modified",
    },
    ModificationCase {
        case: "the subnets reordered, with a description carrying punctuation",
        description: Some("re-ordered. twice."),
        subnets: &["subnet-4e5f6071", "subnet-0a1b2c3d"],
        expected_description: "re-ordered. twice.",
    },
];

#[tokio::test]
async fn a_modification_of_a_group_that_exists_is_stored_and_read_back_by_the_report() {
    let router = emulator();

    let name = "emu-modify-subnets";
    expect_ok(
        "the group as created",
        create_group(
            &router,
            REGION,
            name,
            "the group as created",
            &two_subnets(),
        )
        .await,
    );

    for row in MODIFICATIONS {
        let subnets: Vec<String> = row.subnets.iter().map(|id| (*id).to_string()).collect();
        let answered = expect_ok(
            row.case,
            modify_group(&router, REGION, name, row.description, &subnets).await,
        );
        assert_eq!(
            text(&answered, "DBSubnetGroupName"),
            name,
            "case {:?}: expected the answer to name the group, got {answered}",
            row.case
        );
        assert_eq!(
            text(&answered, "DBSubnetGroupDescription"),
            row.expected_description,
            "case {:?}: expected the description a request carrying none leaves alone, \
             got {answered}",
            row.case
        );
        assert_eq!(
            texts(&answered, "SubnetIdentifier"),
            subnets,
            "case {:?}: expected the subnets the request named, in its order, got {answered}",
            row.case
        );

        // The report reads the SAME record back: the modification reached the store every
        // other operation reads, rather than only the answer.
        let reported = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBSubnetGroups",
                &[("DBSubnetGroupName", name)],
            )
            .await,
        );
        assert_eq!(
            texts(&reported, "SubnetIdentifier"),
            subnets,
            "case {:?}: expected the report to read the modified subnets back, got {reported}",
            row.case
        );
        assert_eq!(
            text(&reported, "DBSubnetGroupDescription"),
            row.expected_description,
            "case {:?}: expected the report to read the modified description back, \
             got {reported}",
            row.case
        );
    }

    // A modification reaches only the region it was signed for: the same name in another
    // region is the other region's group.
    expect_ok(
        "the other region's group of the same name",
        create_group(
            &router,
            OTHER_REGION,
            name,
            "untouched elsewhere",
            &two_subnets(),
        )
        .await,
    );
    expect_ok(
        "the modification in the signed region",
        modify_group(
            &router,
            REGION,
            name,
            Some("changed here only"),
            &two_subnets(),
        )
        .await,
    );
    let elsewhere = expect_ok(
        "the other region's group after the modification",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", name)],
        )
        .await,
    );
    assert_eq!(
        text(&elsewhere, "DBSubnetGroupDescription"),
        "untouched elsewhere",
        "expected the other region's group to be untouched, got {elsewhere}"
    );
}

#[tokio::test]
async fn a_modification_whose_subnet_list_no_group_can_be_made_of_is_refused() {
    let router = emulator();

    let name = "emu-modify-refused";
    expect_ok(
        "the group to modify",
        create_group(&router, REGION, name, "as created", &two_subnets()).await,
    );

    let one_subnet = vec![TWO_SUBNETS[0].to_string()];
    let repeated = vec![TWO_SUBNETS[0].to_string(), TWO_SUBNETS[0].to_string()];
    let not_a_subnet = vec![TWO_SUBNETS[0].to_string(), "subnet-nothexadecimal".to_string()];
    let over_quota: Vec<String> = (0..=MAX_SUBNETS_PER_DB_SUBNET_GROUP)
        .map(|index| format!("subnet-{index:08x}"))
        .collect();
    let rows: Vec<(&str, &Vec<String>, &str)> = vec![
        (
            "one subnet, which cannot cover two Availability Zones",
            &one_subnet,
            "DBSubnetGroupDoesNotCoverEnoughAZs",
        ),
        (
            "the same subnet twice, which is one distinct subnet",
            &repeated,
            "DBSubnetGroupDoesNotCoverEnoughAZs",
        ),
        (
            "a value that is not an EC2 subnet id",
            &not_a_subnet,
            "InvalidSubnet",
        ),
        (
            "one subnet more than a group holds",
            &over_quota,
            "DBSubnetQuotaExceededFault",
        ),
    ];
    for (case, subnets, code) in rows {
        expect_error(
            case,
            modify_group(&router, REGION, name, None, subnets).await,
            400,
            code,
        );
    }

    // Every refusal left the group exactly as the create stored it.
    let reported = expect_ok(
        "the group after the refusals",
        call(
            &router,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", name)],
        )
        .await,
    );
    assert_eq!(
        texts(&reported, "SubnetIdentifier"),
        two_subnets(),
        "expected the refused modifications to change nothing, got {reported}"
    );

    // "You can't modify the default subnet group. Must not be default." — the group the
    // account owns is refused for the VALUE the request carries, in every spelling, and not
    // reported absent: it does resolve for the report.
    for spelling in [DEFAULT_DB_SUBNET_GROUP, "Default", "DEFAULT"] {
        let refused = expect_error(
            spelling,
            modify_group(&router, REGION, spelling, None, &two_subnets()).await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&refused, "Message");
        assert!(
            message.contains("DBSubnetGroupName") && message.contains(DEFAULT_DB_SUBNET_GROUP),
            "case {spelling:?}: expected the refusal to name the member and the value it \
             excludes, got {message:?}"
        );
    }
    let default_group = expect_ok(
        "the account-owned group after the refusals",
        call(
            &router,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", DEFAULT_DB_SUBNET_GROUP)],
        )
        .await,
    );
    assert_eq!(
        text(&default_group, "DBSubnetGroupName"),
        DEFAULT_DB_SUBNET_GROUP,
        "expected the account-owned group to be untouched, got {default_group}"
    );
}

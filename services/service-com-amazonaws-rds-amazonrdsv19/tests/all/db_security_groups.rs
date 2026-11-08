// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's DB-security-group slice:
//! `CreateDBSecurityGroup` and the `DescribeDBSecurityGroups` that reads it back.
//!
//! Every call goes over the wire, through the same mounting helper and awsQuery form POST
//! the sibling suites use, so the assertions read the XML the emulator itself answered
//! with and a handler that computed nothing could not pass them.
//!
//! The rules are asserted over TABLES spanning the class each rule generalises over, not
//! over one request:
//!
//! * the create-then-read rule over rows differing in the name's case and length, the
//!   description, and how many tags the create carries;
//! * the name rule over the class it generalises over — every clause of
//!   `DBSecurityGroupName`'s prose the model does not constrain, at the stored bound and
//!   one character past it, plus the one name it reserves;
//! * the store bounds over the caller-sized values a create carries;
//! * the regional scope over two regions and one name;
//! * pagination at the documented page size and across the marker;
//! * the region's group quota, filled and then exceeded.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    self, DB_SECURITY_GROUP_QUOTA_PER_REGION, MAX_FREE_FORM_LEN, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, subtrees, text, texts, REGION,
};

/// The canonical account id every generated emulator presents, as the sibling suites
/// assert it in an ARN.
const ACCOUNT: &str = "000000000000";

/// A second region, so the regional scope of a create is observable.
const OTHER_REGION: &str = "eu-west-1";

/// A call that carries no parameters beyond `Action` and `Version`.
const NO_PARAMS: [(&str, &str); 0] = [];

/// The ARN a group stored in `region` under `name` reports: the `secgrp` resource type
/// AWS uses for a DB security group under the `rds` ARN namespace.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:secgrp:{name}")
}

/// How many `tag` elements a response carries.
fn count(xml: &str, tag: &str) -> usize {
    subtrees(xml, tag).len()
}

/// `CreateDBSecurityGroup` in `region`, with its two required members plus `tags`.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    description: &str,
    tags: &[(&str, &str)],
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![
        ("DBSecurityGroupName".to_string(), name.to_string()),
        (
            "DBSecurityGroupDescription".to_string(),
            description.to_string(),
        ),
    ];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push((format!("Tags.Tag.{position}.Key"), (*key).to_string()));
        params.push((format!("Tags.Tag.{position}.Value"), (*value).to_string()));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call_in(router, region, "CreateDBSecurityGroup", &borrowed).await
}

/// `CreateDBSecurityGroup` in [`REGION`].
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    description: &str,
    tags: &[(&str, &str)],
) -> (u16, String) {
    create_in(router, REGION, name, description, tags).await
}

/// `DescribeDBSecurityGroups` naming one group, in [`REGION`].
async fn describe_one(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "DescribeDBSecurityGroups",
        &[("DBSecurityGroupName", name)],
    )
    .await
}

/// One row of the create-then-read table: what the request asks for, and the name the
/// store is expected to key the group under.
struct GroupCase {
    case: &'static str,
    requested_name: &'static str,
    stored_name: &'static str,
    description: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The class the create rule generalises over: the name's case and length, the
/// description's shape, and how many tags the create assigns.
const GROUP_CASES: &[GroupCase] = &[
    GroupCase {
        case: "a mixed-case name is stored lowercased",
        requested_name: "Probe-SecGrp",
        stored_name: "probe-secgrp",
        description: "a plain description",
        tags: &[],
    },
    GroupCase {
        case: "an upper-case name is stored lowercased, with one tag",
        requested_name: "UPPER-SECGRP",
        stored_name: "upper-secgrp",
        description: "a description with spaces and punctuation.",
        tags: &[("env", "probe")],
    },
    GroupCase {
        case: "an already-lowercase name is stored as sent, with two tags",
        requested_name: "emu-secgrp-plain",
        stored_name: "emu-secgrp-plain",
        description: "the-second-group",
        tags: &[("env", "probe"), ("team", "db")],
    },
    GroupCase {
        case: "a name carrying digits and hyphens",
        requested_name: "secgrp-9-alpha0",
        stored_name: "secgrp-9-alpha0",
        description: "digits and hyphens",
        tags: &[],
    },
    GroupCase {
        case: "a single letter, the shortest name the rule admits",
        requested_name: "s",
        stored_name: "s",
        description: "one character",
        tags: &[],
    },
];

/// A create answers with the group it stored, and every later read reports that same
/// record: the name folded to lowercase, the description as sent, the owning account, an
/// ARN derived from the request's region and the stored name, and no ingress rule until
/// one is authorized. The tags the create assigned are the ones the resource carries.
#[tokio::test]
async fn a_created_group_is_computed_from_persisted_state() {
    let router = emulator();

    for GroupCase {
        case,
        requested_name,
        stored_name,
        description,
        tags,
    } in GROUP_CASES
    {
        let created = expect_ok(case, create(&router, requested_name, description, tags).await);
        assert!(
            created.contains("<CreateDBSecurityGroupResponse"),
            "case {case:?}: expected the awsQuery CreateDBSecurityGroupResponse envelope, \
             got {created}"
        );
        let arn = expected_arn(REGION, stored_name);
        assert_eq!(
            (
                text(&created, "DBSecurityGroupName"),
                text(&created, "DBSecurityGroupDescription"),
                text(&created, "OwnerId"),
                text(&created, "DBSecurityGroupArn"),
            ),
            (
                stored_name.to_string(),
                description.to_string(),
                ACCOUNT.to_string(),
                arn.clone(),
            ),
            "case {case:?}: the group the service answers with must be the one the request \
             asked for, got {created}"
        );
        // A group nobody has authorized ingress on holds no rule of either form.
        assert_eq!(
            (
                count(&created, "IPRange"),
                count(&created, "EC2SecurityGroup"),
            ),
            (0, 0),
            "case {case:?}: a freshly created group must report no ingress rule, got {created}"
        );

        // ...and it reads back by the lowercased name and by the name as sent.
        for (read_case, named) in [
            ("describe by the stored name", *stored_name),
            ("describe by the name as sent", *requested_name),
        ] {
            let case = format!("{case}: {read_case}");
            let listed = expect_ok(&case, describe_one(&router, named).await);
            assert_eq!(
                (
                    count(&listed, "DBSecurityGroup"),
                    text(&listed, "DBSecurityGroupName"),
                    text(&listed, "DBSecurityGroupDescription"),
                    text(&listed, "DBSecurityGroupArn"),
                ),
                (
                    1,
                    stored_name.to_string(),
                    description.to_string(),
                    arn.clone(),
                ),
                "case {case:?}: expected exactly the group just created, got {listed}"
            );
        }

        // The tags the create assigned are on the resource the ARN names, read back
        // through the tag operation every family of this service shares.
        let tagged = expect_ok(
            case,
            call(
                &router,
                "ListTagsForResource",
                &[("ResourceName", arn.as_str())],
            )
            .await,
        );
        assert_eq!(
            (texts(&tagged, "Key"), texts(&tagged, "Value")),
            (
                tags.iter().map(|(key, _)| (*key).to_string()).collect(),
                tags.iter().map(|(_, value)| (*value).to_string()).collect(),
            ),
            "case {case:?}: expected the tags the create assigned, got {tagged}"
        );
    }

    // The unfiltered describe reports every group the region holds, in name order.
    let listed = expect_ok(
        "describe every group",
        call(&router, "DescribeDBSecurityGroups", &NO_PARAMS).await,
    );
    let mut expected: Vec<String> = GROUP_CASES
        .iter()
        .map(|row| row.stored_name.to_string())
        .collect();
    expected.sort();
    assert_eq!(
        texts(&listed, "DBSecurityGroupName"),
        expected,
        "case \"describe every group\": expected every created group in name order, got {listed}"
    );

    // The account report counts what the store really holds rather than a constant.
    let attributes = expect_ok(
        "the account quota report",
        call(&router, "DescribeAccountAttributes", &NO_PARAMS).await,
    );
    let security_groups = subtrees(&attributes, "AccountQuota")
        .into_iter()
        .find(|entry| text(entry, "AccountQuotaName") == "DBSecurityGroups")
        .unwrap_or_else(|| panic!("expected a DBSecurityGroups quota entry, got {attributes}"));
    assert_eq!(
        (
            text(&security_groups, "Used"),
            text(&security_groups, "Max"),
        ),
        (
            GROUP_CASES.len().to_string(),
            DB_SECURITY_GROUP_QUOTA_PER_REGION.to_string(),
        ),
        "expected the report to count the groups the region holds, got {security_groups}"
    );
}

/// A second create of a name the region already holds is the modelled already-exists
/// refusal, whatever case it is sent in, and the stored group is left as it was.
#[tokio::test]
async fn a_name_the_region_already_holds_is_refused_whatever_its_case() {
    let router = emulator();
    expect_ok(
        "the first create",
        create(&router, "Dup-SecGrp", "the-first-group", &[("env", "first")]).await,
    );

    for named in ["Dup-SecGrp", "dup-secgrp", "DUP-SECGRP", "dUp-SeCgRp"] {
        let case = format!("a second create named {named:?}");
        expect_error(
            &case,
            create(&router, named, "the-second-group", &[]).await,
            400,
            "DBSecurityGroupAlreadyExists",
        );
    }

    let listed = expect_ok(
        "the group after the refusals",
        describe_one(&router, "dup-secgrp").await,
    );
    assert_eq!(
        (
            count(&listed, "DBSecurityGroup"),
            text(&listed, "DBSecurityGroupDescription"),
        ),
        (1, "the-first-group".to_string()),
        "a refused create must leave the stored group as the first one wrote it, got {listed}"
    );
}

/// One row of the name table: the value under test and whether the rule admits it.
struct RuleCase {
    case: &'static str,
    value: String,
    admitted: bool,
}

/// The longest `DBSecurityGroupName` this service stores ("Must be 1 to 255 letters,
/// numbers, or hyphens" — the length the member's documentation states, which is the one
/// clause of that prose the store enforces; see the test below).
const MAX_NAME_LEN: usize = 255;

/// The name rule this member actually carries, over the class it generalises over.
///
/// `CreateDBSecurityGroup$DBSecurityGroupName` is a plain string in the model — no
/// `@pattern`, no `@length`
/// (`smithy_query(query="operation",operation="CreateDBSecurityGroup")`) — and the real
/// service CREATES a group whose name breaks the alphabet-and-hyphen clauses its prose
/// states, so the identifier rule the DB instance, cluster, snapshot and parameter-group
/// names enforce is not this member's. What refuses a name here is only what the store
/// needs: a value at all, no longer than the documented length. The one name the member
/// reserves ("Must not be `Default`") is still refused, in the case the member documents
/// and in the case it is stored under.
#[tokio::test]
async fn the_name_rule_admits_every_name_the_member_does_not_constrain() {
    let router = emulator();
    let longest = format!("n{}", "a".repeat(MAX_NAME_LEN - 1));
    let too_long = format!("n{}", "a".repeat(MAX_NAME_LEN));
    let rows = [
        RuleCase {
            case: "a plain name",
            value: "names-plain".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "the longest name the store keeps",
            value: longest,
            admitted: true,
        },
        RuleCase {
            case: "one character past the bound",
            value: too_long,
            admitted: false,
        },
        RuleCase {
            case: "an empty name",
            value: String::new(),
            admitted: false,
        },
        // Every clause of the member's prose that the real service does not enforce: a
        // name is admitted whatever character it starts with, ends with or repeats.
        RuleCase {
            case: "a name that does not start with a letter",
            value: "9names-digit".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "a name that starts with a hyphen",
            value: "-names-hyphen".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "a name that ends with a hyphen",
            value: "names-trailing-".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "a name carrying two consecutive hyphens",
            value: "names--double".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "a name that both leads with two hyphens and trails with one",
            value: "--names-both-".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "a name carrying a character outside the identifier alphabet",
            value: "names.dotted".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "the name the member reserves",
            value: "Default".to_string(),
            admitted: false,
        },
        RuleCase {
            case: "the reserved name in the case it is stored under",
            value: "default".to_string(),
            admitted: false,
        },
    ];

    for RuleCase {
        case,
        value,
        admitted,
    } in rows
    {
        let response = create(&router, &value, "a-group-under-test", &[]).await;
        if admitted {
            let created = expect_ok(case, response);
            // The stored group carries the name as sent (folded, as the member documents),
            // and reads back through the describe — so an admitted name is a group that
            // exists rather than a 200 over nothing.
            let stored = value.to_ascii_lowercase();
            assert_eq!(
                text(&created, "DBSecurityGroupName"),
                stored,
                "case {case:?}: expected the create to echo {stored:?}, got {created}"
            );
            let read_back = expect_ok(case, describe_one(&router, &value).await);
            assert_eq!(
                text(&read_back, "DBSecurityGroupName"),
                stored,
                "case {case:?}: expected the describe to report {stored:?}, got {read_back}"
            );
            assert_eq!(
                text(&read_back, "DBSecurityGroupArn"),
                expected_arn(REGION, &stored),
                "case {case:?}: expected the ARN of the stored name, got {read_back}"
            );
        } else {
            expect_error(case, response, 400, "InvalidParameterValue");
            // The refusal stored nothing.
            expect_error(
                case,
                describe_one(&router, &value).await,
                404,
                "DBSecurityGroupNotFound",
            );
        }
    }
}

/// Every caller-sized value a create carries is bounded before any of it is stored: the
/// description's length and the tag count, each at the bound and one past it.
#[tokio::test]
async fn the_store_bounds_refuse_a_create_that_would_grow_past_them() {
    let router = emulator();

    let longest = "d".repeat(MAX_FREE_FORM_LEN);
    let too_long = "d".repeat(MAX_FREE_FORM_LEN + 1);
    expect_ok(
        "the longest description the store keeps",
        create(&router, "bounds-description", &longest, &[]).await,
    );
    expect_error(
        "one character past the description bound",
        create(&router, "bounds-description-over", &too_long, &[]).await,
        400,
        "ValidationException",
    );

    let keys: Vec<String> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| format!("key-{index:03}"))
        .collect();
    let at_bound: Vec<(&str, &str)> = keys[..MAX_TAGS_PER_RESOURCE]
        .iter()
        .map(|key| (key.as_str(), "probe"))
        .collect();
    let past_bound: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "probe")).collect();
    expect_ok(
        "the most tags one resource may carry",
        create(&router, "bounds-tags", "a-tagged-group", &at_bound).await,
    );
    expect_error(
        "one tag past the bound",
        create(&router, "bounds-tags-over", "a-tagged-group", &past_bound).await,
        400,
        "ValidationException",
    );
    for name in ["bounds-description-over", "bounds-tags-over"] {
        expect_error(
            name,
            describe_one(&router, name).await,
            404,
            "DBSecurityGroupNotFound",
        );
    }
}

/// One name is a different group in each region, and a create reaches only the region it
/// was signed for.
#[tokio::test]
async fn a_group_is_visible_only_in_the_region_it_was_created_in() {
    let router = emulator();

    for (region, description) in [
        (REGION, "the group in the signed region"),
        (OTHER_REGION, "the group in the other region"),
    ] {
        expect_ok(
            region,
            create_in(&router, region, "emu-regional-secgrp", description, &[]).await,
        );
        let listed = expect_ok(
            region,
            call_in(
                &router,
                region,
                "DescribeDBSecurityGroups",
                &[("DBSecurityGroupName", "emu-regional-secgrp")],
            )
            .await,
        );
        assert_eq!(
            (
                text(&listed, "DBSecurityGroupDescription"),
                text(&listed, "DBSecurityGroupArn"),
            ),
            (
                description.to_string(),
                expected_arn(region, "emu-regional-secgrp"),
            ),
            "case {region:?}: a describe must report the group its own region holds, got {listed}"
        );
    }

    // A name only one region holds is not found in the other.
    expect_ok(
        "a group only the signed region holds",
        create(&router, "only-here-secgrp", "a-group-in-one-region", &[]).await,
    );
    expect_error(
        "the same name in the other region",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBSecurityGroups",
            &[("DBSecurityGroupName", "only-here-secgrp")],
        )
        .await,
        404,
        "DBSecurityGroupNotFound",
    );
}

/// The describe refuses what it cannot serve and pages what it can: an unknown name is
/// the modelled 404, a `Filters` it does not support is refused, a `MaxRecords` outside
/// the documented range is refused, and a page resumes from the marker it handed out.
#[tokio::test]
async fn the_describe_refuses_what_it_cannot_serve_and_pages_what_it_can() {
    let router = emulator();

    expect_error(
        "a name no create stored",
        describe_one(&router, "absent-secgrp").await,
        404,
        "DBSecurityGroupNotFound",
    );
    expect_error(
        "a filter the operation does not support",
        call(
            &router,
            "DescribeDBSecurityGroups",
            &[
                ("Filters.Filter.1.Name", "db-security-group-name"),
                ("Filters.Filter.1.Values.Value.1", "any-secgrp"),
            ],
        )
        .await,
        400,
        "ValidationException",
    );
    // `MaxRecords`: "Constraints: Minimum 20, maximum 100."
    for (case, max_records) in [
        ("one below the documented minimum", "19"),
        ("one above the documented maximum", "101"),
    ] {
        expect_error(
            case,
            call(
                &router,
                "DescribeDBSecurityGroups",
                &[("MaxRecords", max_records)],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
    }

    // The region's groups, paged at the documented minimum page size: 20, a marker, then
    // the rest.
    let page_size = 20usize;
    let total = DB_SECURITY_GROUP_QUOTA_PER_REGION;
    assert!(
        total > page_size,
        "this test needs more groups than one page holds: {total} groups, {page_size} per page"
    );
    let mut names: Vec<String> = (0..total).map(|index| format!("page-{index:03}")).collect();
    for name in &names {
        expect_ok(name, create(&router, name, "a-paged-group", &[]).await);
    }
    names.sort();

    let first = expect_ok(
        "the first page",
        call(
            &router,
            "DescribeDBSecurityGroups",
            &[("MaxRecords", page_size.to_string())],
        )
        .await,
    );
    assert_eq!(
        texts(&first, "DBSecurityGroupName"),
        names[..page_size].to_vec(),
        "case \"the first page\": expected the first {page_size} groups in name order, got {first}"
    );
    let marker = text(&first, "Marker");
    assert_eq!(
        marker,
        state::encode_marker(&names[page_size - 1]),
        "case \"the first page\": the marker must be the opaque token for the last group the \
         page served, got {first}"
    );

    let second = expect_ok(
        "the second page",
        call(
            &router,
            "DescribeDBSecurityGroups",
            &[
                ("MaxRecords", page_size.to_string()),
                ("Marker", marker.clone()),
            ],
        )
        .await,
    );
    assert_eq!(
        texts(&second, "DBSecurityGroupName"),
        names[page_size..].to_vec(),
        "case \"the second page\": expected the groups after the marker, got {second}"
    );
    assert!(
        !second.contains("<Marker>"),
        "case \"the second page\": the last page must carry no marker, got {second}"
    );

    // A marker no page handed out is refused rather than resynchronised.
    expect_error(
        "a marker the service never issued",
        call(&router, "DescribeDBSecurityGroups", &[("Marker", "page-999")]).await,
        400,
        "InvalidParameterValue",
    );
}

/// The region's group count is capped: filling it is allowed, one more is the modelled
/// quota fault, and another region still has its own room.
#[tokio::test]
async fn the_region_quota_refuses_a_store_that_would_grow_past_it() {
    let router = emulator();
    for index in 0..DB_SECURITY_GROUP_QUOTA_PER_REGION {
        let name = format!("quota-{index:03}");
        expect_ok(&name, create(&router, &name, "a-quota-group", &[]).await);
    }
    expect_error(
        "one group past the region's quota",
        create(&router, "quota-over", "a-quota-group", &[]).await,
        400,
        "QuotaExceeded.DBSecurityGroup",
    );
    expect_error(
        "the refused group",
        describe_one(&router, "quota-over").await,
        404,
        "DBSecurityGroupNotFound",
    );
    expect_ok(
        "the same name in another region",
        create_in(
            &router,
            OTHER_REGION,
            "quota-over",
            "a-quota-group",
            &[],
        )
        .await,
    );
}

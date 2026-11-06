// SPDX-License-Identifier: CC0-1.0

//! Cluster parameter groups, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: the documented `ParameterGroupName` syntax and its lower-case
//! storage, the `ParameterGroupFamily` catalogue read off the default parameter
//! groups, uniqueness, the account quota, the documented `MaxRecords` window and
//! `Marker` paging, the tag filters, and the state a create leaves for a later
//! read.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::ClusterParameterGroup;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Emulator};

/// A name of exactly `len` characters that satisfies every other part of the
/// documented syntax: a leading letter and then alphanumerics.
fn name_of_length(len: usize) -> String {
    std::iter::once('g')
        .chain(std::iter::repeat_n('x', len - 1))
        .collect()
}

/// `call` takes borrowed pairs, so a case that builds owned parameters borrows
/// them through this.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The `ParameterGroupName` of every group a `DescribeClusterParameterGroups`
/// body carries, in the order the page lists them.
fn listed_groups(xml: &str) -> Vec<String> {
    all_text(xml, "ParameterGroupName")
}

/// The parameter group family of the engine version this service publishes,
/// taken from the same place the store seeds its default groups from rather than
/// spelled out here.
fn default_family() -> String {
    state::parameter_group_family(state::DEFAULT_CLUSTER_VERSION)
}

/// The name of that engine version's DEFAULT parameter group, which a fresh
/// account already has.
fn default_group_name() -> String {
    state::default_parameter_group(state::DEFAULT_CLUSTER_VERSION)
}

/// The name of EVERY default parameter group a fresh account already has, in the
/// order the store seeds them: one per published parameter group family (the
/// engine versions' families, plus the family a new cluster's default group
/// belongs to).
fn default_group_names() -> Vec<String> {
    state::published_parameter_group_families()
        .iter()
        .map(|family| state::family_default_parameter_group(family))
        .collect()
}

/// One row of the tag-filter table: the case name, the request's filter
/// parameters, and the group names that must come back.
type FilterCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<String>,
);

/// One row of the create/round-trip table: the case name, the request's
/// `ParameterGroupName`, `ParameterGroupFamily` and `Description`, the tag pairs
/// it carries, and the name the group must be STORED and reported under.
struct CreateCase {
    case: &'static str,
    requested_name: String,
    family: String,
    description: String,
    tags: Vec<(String, String)>,
    stored_name: String,
}

/// Create one parameter group and read it straight back, for every row of a
/// table that spans what the create rule generalises over: the name's case and
/// length, the description's content, and how many tags the request carries.
#[tokio::test]
async fn a_created_parameter_group_is_reported_exactly_as_stored() {
    let emulator = Emulator::new();
    // The one family this engine version offers, read off the default parameter
    // group rather than spelled at the call site (see the `families` assertion
    // in `the_parameter_group_family_catalogue_is_the_default_groups_families`).
    let family = default_family();

    let cases = vec![
        CreateCase {
            case: "minimal",
            requested_name: "wlm-group".to_string(),
            family: family.clone(),
            description: "queue assignment".to_string(),
            tags: Vec::new(),
            stored_name: "wlm-group".to_string(),
        },
        CreateCase {
            // "This value is stored as a lower-case string."
            case: "mixed case is folded",
            requested_name: "WLM-Group-Two".to_string(),
            family: family.clone(),
            description: "Mixed Case Description".to_string(),
            tags: Vec::new(),
            stored_name: "wlm-group-two".to_string(),
        },
        CreateCase {
            case: "shortest documented name",
            requested_name: name_of_length(1),
            family: family.clone(),
            description: "one character".to_string(),
            tags: Vec::new(),
            stored_name: name_of_length(1),
        },
        CreateCase {
            case: "longest documented name",
            requested_name: name_of_length(255),
            family: family.clone(),
            description: "at the documented bound".to_string(),
            tags: Vec::new(),
            stored_name: name_of_length(255),
        },
        CreateCase {
            // `Description` is modelled `length 0..`, so the empty string is a
            // value the member admits.
            case: "empty description",
            requested_name: "empty-description".to_string(),
            family: family.clone(),
            description: String::new(),
            tags: Vec::new(),
            stored_name: "empty-description".to_string(),
        },
        CreateCase {
            case: "one tag",
            requested_name: "one-tag".to_string(),
            family: family.clone(),
            description: "tagged once".to_string(),
            tags: vec![("owner".to_string(), "admin".to_string())],
            stored_name: "one-tag".to_string(),
        },
        CreateCase {
            case: "three tags",
            requested_name: "three-tags".to_string(),
            family: family.clone(),
            description: "tagged three times".to_string(),
            tags: vec![
                ("owner".to_string(), "admin".to_string()),
                ("environment".to_string(), "test".to_string()),
                ("cost-centre".to_string(), "42".to_string()),
            ],
            stored_name: "three-tags".to_string(),
        },
    ];

    for case in &cases {
        let mut params = vec![
            (
                "ParameterGroupName".to_string(),
                case.requested_name.clone(),
            ),
            ("ParameterGroupFamily".to_string(), case.family.clone()),
            ("Description".to_string(), case.description.clone()),
        ];
        for (index, (key, value)) in case.tags.iter().enumerate() {
            let position = index + 1;
            params.push((format!("Tags.Tag.{position}.Key"), key.clone()));
            params.push((format!("Tags.Tag.{position}.Value"), value.clone()));
        }

        let created = emulator
            .call("CreateClusterParameterGroup", &borrowed(&params))
            .await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200, got {} with body {}",
            case.case, created.status, created.body
        );
        assert_eq!(
            created.text("ParameterGroupName").as_deref(),
            Some(case.stored_name.as_str()),
            "case {}: expected the stored name {:?} back, got {:?}",
            case.case,
            case.stored_name,
            created.text("ParameterGroupName")
        );
        assert_eq!(
            created.text("ParameterGroupFamily").as_deref(),
            Some(case.family.as_str()),
            "case {}: expected the family {:?} back, got {:?}",
            case.case,
            case.family,
            created.text("ParameterGroupFamily")
        );

        // ROUND TRIP: the read reports what the write persisted.
        let described = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("ParameterGroupName", case.stored_name.as_str())],
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {}: expected the group to be readable back with 200, got {} with body {}",
            case.case, described.status, described.body
        );
        assert_eq!(
            listed_groups(&described.body),
            vec![case.stored_name.clone()],
            "case {}: expected exactly the created group back, got {:?}",
            case.case,
            listed_groups(&described.body)
        );
        assert_eq!(
            described.text("Description").as_deref(),
            Some(case.description.as_str()),
            "case {}: expected the description {:?} back, got {:?}",
            case.case,
            case.description,
            described.text("Description")
        );
        assert_eq!(
            all_text(&described.body, "Key"),
            case.tags
                .iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>(),
            "case {}: expected the tag keys the create carried, got {:?}",
            case.case,
            all_text(&described.body, "Key")
        );
        assert_eq!(
            all_text(&described.body, "Value"),
            case.tags
                .iter()
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>(),
            "case {}: expected the tag values the create carried, got {:?}",
            case.case,
            all_text(&described.body, "Value")
        );
    }

    // Every created group, and the account's default group, is on the full list
    // exactly once — the create persisted a record rather than only answering.
    let all = emulator
        .call("DescribeClusterParameterGroups", &[])
        .await;
    let mut expected: Vec<String> = default_group_names();
    expected.extend(cases.iter().map(|case| case.stored_name.clone()));
    assert_eq!(
        listed_groups(&all.body),
        expected,
        "case list every group: expected the default groups then the created ones in creation order, got {:?}",
        listed_groups(&all.body)
    );
}

/// One row of a syntax table: the case name, the requested name, and whether the
/// documented syntax admits it.
struct NameCase {
    case: &'static str,
    requested_name: String,
    accepted: bool,
}

/// The documented `ParameterGroupName` syntax, on each published bound and one
/// past it, and on each forbidden form.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
/// member="ParameterGroupName")` — "Must be 1 to 255 alphanumeric characters or
/// hyphens First character must be a letter. Cannot end with a hyphen or contain
/// two consecutive hyphens."
#[tokio::test]
async fn the_parameter_group_name_syntax_is_enforced() {
    let emulator = Emulator::new();
    let cases = vec![
        NameCase {
            case: "at the lower bound",
            requested_name: name_of_length(1),
            accepted: true,
        },
        NameCase {
            case: "at the upper bound",
            requested_name: name_of_length(255),
            accepted: true,
        },
        NameCase {
            case: "one past the upper bound",
            requested_name: name_of_length(256),
            accepted: false,
        },
        NameCase {
            case: "one below the lower bound",
            requested_name: String::new(),
            accepted: false,
        },
        NameCase {
            case: "mixed case is admitted, not refused",
            requested_name: "MixedCase".to_string(),
            accepted: true,
        },
        NameCase {
            case: "hyphens inside the name",
            requested_name: "a-b-c".to_string(),
            accepted: true,
        },
        NameCase {
            case: "first character is a digit",
            requested_name: "1group".to_string(),
            accepted: false,
        },
        NameCase {
            case: "first character is a hyphen",
            requested_name: "-group".to_string(),
            accepted: false,
        },
        NameCase {
            case: "ends with a hyphen",
            requested_name: "group-".to_string(),
            accepted: false,
        },
        NameCase {
            case: "two consecutive hyphens",
            requested_name: "gr--oup".to_string(),
            accepted: false,
        },
        NameCase {
            case: "neither alphanumeric nor hyphen: underscore",
            requested_name: "gr_oup".to_string(),
            accepted: false,
        },
        NameCase {
            case: "neither alphanumeric nor hyphen: dot",
            requested_name: "gr.oup".to_string(),
            accepted: false,
        },
    ];

    for case in &cases {
        let params = vec![
            (
                "ParameterGroupName".to_string(),
                case.requested_name.clone(),
            ),
            (
                "ParameterGroupFamily".to_string(),
                default_family(),
            ),
            ("Description".to_string(), "a syntax case".to_string()),
        ];
        let answer = emulator
            .call("CreateClusterParameterGroup", &borrowed(&params))
            .await;
        if case.accepted {
            assert_eq!(
                answer.status, 200,
                "case {}: expected the name to be accepted with 200, got {} / {:?}",
                case.case, answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("ValidationException".to_string())),
                "case {}: expected 400 ValidationException, got {} / {:?}",
                case.case,
                answer.status,
                answer.error_code()
            );
        }
    }

    // A refused create persisted nothing: the list holds the default group and
    // only the names the syntax admitted, folded to lower case.
    let all = emulator
        .call("DescribeClusterParameterGroups", &[])
        .await;
    let mut expected: Vec<String> = default_group_names();
    expected.extend(
        cases
            .iter()
            .filter(|case| case.accepted)
            .map(|case| case.requested_name.to_ascii_lowercase()),
    );
    assert_eq!(
        listed_groups(&all.body),
        expected,
        "case nothing else persisted: expected only the admitted names, got {:?}",
        listed_groups(&all.body)
    );
}

/// The valid `ParameterGroupFamily` values are the families of the DEFAULT
/// parameter groups the account has, and nothing else.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterParameterGroupMessage",
/// member="ParameterGroupFamily")` — "The parameter group family names associated
/// with the default parameter groups provide you the valid values. For example, a
/// valid family name is \"redshift-1.0\"."
#[tokio::test]
async fn the_parameter_group_family_catalogue_is_the_default_groups_families() {
    let emulator = Emulator::new();

    // The catalogue as the service itself reports it: the family of every group
    // a fresh account already has.
    let defaults = emulator
        .call("DescribeClusterParameterGroups", &[])
        .await;
    let families = all_text(&defaults.body, "ParameterGroupFamily");
    assert_eq!(
        families,
        state::published_parameter_group_families(),
        "case the default groups' families: expected every published family, got {families:?}"
    );

    // Each row names a family and whether the catalogue above holds it.
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "the family of the default group",
            families[0].clone(),
            true,
        ),
        (
            "the family a new cluster's default group belongs to",
            state::DEFAULT_PARAMETER_GROUP_FAMILY.to_string(),
            true,
        ),
        ("an engine version that does not exist", "redshift-9.9".to_string(), false),
        ("another service's family", "mysql8.0".to_string(), false),
        ("the family in another case", families[0].to_uppercase(), false),
        ("no family at all", String::new(), false),
        ("the default GROUP's name, not its family", default_group_name(), false),
    ];

    for (index, (case, family, accepted)) in cases.iter().enumerate() {
        let params = vec![
            ("ParameterGroupName".to_string(), format!("family-case-{index}")),
            ("ParameterGroupFamily".to_string(), family.clone()),
            ("Description".to_string(), "a family case".to_string()),
        ];
        let answer = emulator
            .call("CreateClusterParameterGroup", &borrowed(&params))
            .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the family {family:?} to be accepted with 200, got {} / {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.text("ParameterGroupFamily").as_deref(),
                Some(family.as_str()),
                "case {case}: expected the family back, got {:?}",
                answer.text("ParameterGroupFamily")
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("ValidationException".to_string())),
                "case {case}: expected the family {family:?} to be refused with 400 ValidationException, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }
}

/// A parameter group name is unique within the account, decided on the STORED
/// (lower-case) name, and the second create is refused without touching the
/// first group.
#[tokio::test]
async fn a_parameter_group_name_is_unique_within_the_account() {
    let emulator = Emulator::new();
    let family = default_family();

    let first = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", "wlm-group"),
                ("ParameterGroupFamily", family.as_str()),
                ("Description", "the first description"),
            ],
        )
        .await;
    assert_eq!(
        first.status, 200,
        "case first create: expected 200, got {} with body {}",
        first.status, first.body
    );

    // Each row spells the SAME stored name differently; every one collides.
    for (case, spelling) in [
        ("the same spelling", "wlm-group"),
        ("a different case", "WLM-Group"),
        ("one capital", "Wlm-group"),
    ] {
        let duplicate = emulator
            .call(
                "CreateClusterParameterGroup",
                &[
                    ("ParameterGroupName", spelling),
                    ("ParameterGroupFamily", family.as_str()),
                    ("Description", "the second description"),
                ],
            )
            .await;
        assert_eq!(
            (duplicate.status, duplicate.error_code()),
            (400, Some("ClusterParameterGroupAlreadyExists".to_string())),
            "case {case}: expected 400 ClusterParameterGroupAlreadyExists, got {} / {:?}",
            duplicate.status,
            duplicate.error_code()
        );
    }

    // The refused creates left the first group exactly as it was.
    let described = emulator
        .call(
            "DescribeClusterParameterGroups",
            &[("ParameterGroupName", "wlm-group")],
        )
        .await;
    assert_eq!(
        described.text("Description").as_deref(),
        Some("the first description"),
        "case the first group is untouched: expected the original description, got {:?}",
        described.text("Description")
    );
}

/// Reading parameter groups changes nothing a later caller sees, an unknown name
/// is a 404, and the default group is there without anybody creating it.
#[tokio::test]
async fn describing_parameter_groups_is_a_pure_report() {
    let emulator = Emulator::new();

    let first_read = emulator
        .call("DescribeClusterParameterGroups", &[])
        .await;
    assert_eq!(
        listed_groups(&first_read.body),
        default_group_names(),
        "case a fresh account: expected only the default parameter groups, got {:?}",
        listed_groups(&first_read.body)
    );
    let second_read = emulator
        .call("DescribeClusterParameterGroups", &[])
        .await;
    // Everything the report says about the groups, which a read must not move.
    // The envelope's `RequestId` is per-request and deliberately left out.
    for member in ["ParameterGroupName", "ParameterGroupFamily", "Description"] {
        assert_eq!(
            all_text(&second_read.body, member),
            all_text(&first_read.body, member),
            "case repeated read: expected the same {member} values, got {:?}",
            all_text(&second_read.body, member)
        );
    }

    // A name no group carries is a 404, and the stored name is the lower-case
    // one, so another spelling of it names no group either.
    let created = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", "Reporting-Group"),
                ("ParameterGroupFamily", default_family().as_str()),
                ("Description", "a reported group"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case create reporting-group: expected 200, got {} with body {}",
        created.status, created.body
    );
    for (case, name) in [
        ("a name nothing carries", "no-such-group"),
        ("the group's name in another case", "Reporting-Group"),
    ] {
        let missing = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("ParameterGroupName", name)],
            )
            .await;
        assert_eq!(
            (missing.status, missing.error_code()),
            (404, Some("ClusterParameterGroupNotFound".to_string())),
            "case {case}: expected 404 ClusterParameterGroupNotFound, got {} / {:?}",
            missing.status,
            missing.error_code()
        );
    }
}

/// The documented `MaxRecords` window and the `Marker` cursor, on each published
/// bound and one past it.
///
/// Source: `smithy_query(query="doc", shape="DescribeClusterParameterGroupsMessage",
/// member="MaxRecords")` — "Default: 100 Constraints: minimum 20, maximum 100."
#[tokio::test]
async fn parameter_groups_are_paginated_over_the_documented_window() {
    let emulator = Emulator::new();

    // 25 groups in all: the default groups a fresh account has, plus enough
    // created ones to need more than one page of 20.
    let total = 25usize;
    let seeded = total - default_group_names().len();
    for index in 1..=seeded {
        let params = vec![
            ("ParameterGroupName".to_string(), format!("page-group-{index:03}")),
            (
                "ParameterGroupFamily".to_string(),
                default_family(),
            ),
            ("Description".to_string(), format!("group {index}")),
        ];
        let created = emulator
            .call("CreateClusterParameterGroup", &borrowed(&params))
            .await;
        assert_eq!(
            created.status, 200,
            "case seed page-group-{index:03}: expected 200, got {} with body {}",
            created.status, created.body
        );
    }

    for (case, max_records, accepted) in [
        ("one below the documented minimum", "19", false),
        ("at the documented minimum", "20", true),
        ("inside the window", "50", true),
        ("at the documented maximum", "100", true),
        ("one past the documented maximum", "101", true),
        ("far past the documented maximum", "1000", true),
        ("zero", "0", false),
    ] {
        let answer = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("MaxRecords", max_records)],
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} to be served with 200, got {} / {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case {case}: expected MaxRecords={max_records} to be refused with 400 InvalidParameterValue, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // The default window takes every group in one page, with no cursor left.
    let unpaged = emulator
        .call("DescribeClusterParameterGroups", &[])
        .await;
    assert_eq!(
        listed_groups(&unpaged.body).len(),
        total,
        "case the default window: expected all {total} groups in one page, got {:?}",
        listed_groups(&unpaged.body)
    );
    assert_eq!(
        unpaged.text("Marker"),
        None,
        "case the default window: expected no cursor, got {:?}",
        unpaged.text("Marker")
    );

    // A page of 20 leaves a cursor the next call resumes from, and the two pages
    // are the one sequence the unpaged read gives, split in order.
    let first_page = emulator
        .call(
            "DescribeClusterParameterGroups",
            &[("MaxRecords", "20")],
        )
        .await;
    let first_names = listed_groups(&first_page.body);
    assert_eq!(
        first_names.len(),
        20,
        "case first page of 20: expected 20 groups, got {first_names:?}"
    );
    let marker = first_page
        .text("Marker")
        .expect("case first page of 20: records remain, so the page carries a cursor");
    let second_page = emulator
        .call(
            "DescribeClusterParameterGroups",
            &[("MaxRecords", "20"), ("Marker", marker.as_str())],
        )
        .await;
    let second_names = listed_groups(&second_page.body);
    assert_eq!(
        second_names.len(),
        total - 20,
        "case second page: expected the remaining {} groups, got {second_names:?}",
        total - 20
    );
    assert_eq!(
        second_page.text("Marker"),
        None,
        "case second page: nothing remains, so expected no cursor, got {:?}",
        second_page.text("Marker")
    );
    let walked: Vec<String> = first_names.into_iter().chain(second_names).collect();
    assert_eq!(
        walked,
        listed_groups(&unpaged.body),
        "case the walked pages: expected the unpaged sequence, got {walked:?}"
    );

    // A cursor this service did not hand out is refused rather than served.
    for (case, cursor) in [
        ("not a cursor at all", "nonsense"),
        ("a bare offset", "20"),
        ("an offset with the wrong tag", "20.deadbeef"),
    ] {
        let bogus = emulator
            .call("DescribeClusterParameterGroups", &[("Marker", cursor)])
            .await;
        assert_eq!(
            (bogus.status, bogus.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case {case}: expected the cursor {cursor:?} to be refused with 400 InvalidParameterValue, got {} / {:?}",
            bogus.status,
            bogus.error_code()
        );
    }
}

/// The `TagKeys` / `TagValues` filters select parameter groups by the tags the
/// create persisted; each filter is independent and inclusive.
#[tokio::test]
async fn parameter_groups_are_selected_by_their_tags() {
    let emulator = Emulator::new();

    for (name, key, value) in [
        ("tagged-owner-admin", "owner", "admin"),
        ("tagged-owner-tester", "owner", "tester"),
        ("tagged-env-test", "environment", "test"),
    ] {
        let created = emulator
            .call(
                "CreateClusterParameterGroup",
                &[
                    ("ParameterGroupName", name),
                    ("ParameterGroupFamily", default_family().as_str()),
                    ("Description", "a tagged group"),
                    ("Tags.Tag.1.Key", key),
                    ("Tags.Tag.1.Value", value),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case seed {name}: expected 200, got {} with body {}",
            created.status, created.body
        );
    }

    // Each row's expected list is the group NAMES the filter must select.
    let names = |expected: &[&str]| -> Vec<String> {
        expected.iter().map(|name| (*name).to_string()).collect()
    };
    let cases: Vec<FilterCase> = vec![
        (
            "no filter lists every group, tagged or not",
            vec![],
            default_group_names()
                .into_iter()
                .chain(names(&[
                    "tagged-owner-admin",
                    "tagged-owner-tester",
                    "tagged-env-test",
                ]))
                .collect(),
        ),
        (
            "one tag key",
            vec![("TagKeys.TagKey.1", "owner")],
            names(&["tagged-owner-admin", "tagged-owner-tester"]),
        ),
        (
            "two tag keys are inclusive",
            vec![
                ("TagKeys.TagKey.1", "owner"),
                ("TagKeys.TagKey.2", "environment"),
            ],
            names(&[
                "tagged-owner-admin",
                "tagged-owner-tester",
                "tagged-env-test",
            ]),
        ),
        (
            "one tag value",
            vec![("TagValues.TagValue.1", "admin")],
            names(&["tagged-owner-admin"]),
        ),
        (
            "a key and a value together",
            vec![
                ("TagKeys.TagKey.1", "owner"),
                ("TagValues.TagValue.1", "test"),
            ],
            Vec::new(),
        ),
        (
            "a key nothing carries",
            vec![("TagKeys.TagKey.1", "absent")],
            Vec::new(),
        ),
    ];

    for (case, filter, expected) in cases {
        let answer = emulator
            .call("DescribeClusterParameterGroups", &filter)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_groups(&answer.body),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            listed_groups(&answer.body)
        );
    }
}

/// The account's own parameter groups are bounded by
/// [`state::PARAMETER_GROUP_QUOTA`]; the default groups Redshift owns are outside
/// it.
#[tokio::test]
async fn creating_past_the_parameter_group_quota_is_refused() {
    let emulator = Emulator::new();

    // Fill the store to one below the quota, straight into the same shared store
    // the handlers read: a thousand round trips through the wire would say
    // nothing more about the bound.
    {
        let store = state::service_state(&*emulator.engine).expect("the Redshift store");
        let mut store = store.lock();
        for index in 0..state::PARAMETER_GROUP_QUOTA - 1 {
            store
                .parameter_groups
                .push(state::StoredParameterGroup {
                    group: ClusterParameterGroup {
                        parameter_group_name: Some(format!("seeded-{index:04}")),
                        parameter_group_family: Some(
                            default_family(),
                        ),
                        description: Some("a seeded group".to_string()),
                        tags: Some(Vec::new()),
                    },
                    default_group: false,
                    parameter_values: std::collections::BTreeMap::new(),
                });
        }
    }

    // The default group is in the store too, so this create proves the quota
    // counts only what the account created.
    let last = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", "the-last-one"),
                ("ParameterGroupFamily", default_family().as_str()),
                ("Description", "at the quota"),
            ],
        )
        .await;
    assert_eq!(
        last.status, 200,
        "case at the quota: expected the {}th own group to be accepted, got {} with body {}",
        state::PARAMETER_GROUP_QUOTA,
        last.status,
        last.body
    );

    let over = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", "one-too-many"),
                ("ParameterGroupFamily", default_family().as_str()),
                ("Description", "past the quota"),
            ],
        )
        .await;
    assert_eq!(
        (over.status, over.error_code()),
        (400, Some("ClusterParameterGroupQuotaExceeded".to_string())),
        "case past the quota: expected 400 ClusterParameterGroupQuotaExceeded, got {} / {:?}",
        over.status,
        over.error_code()
    );

    // And a name that already exists keeps the more specific fault even with the
    // store full.
    let duplicate = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", "the-last-one"),
                ("ParameterGroupFamily", default_family().as_str()),
                ("Description", "past the quota"),
            ],
        )
        .await;
    assert_eq!(
        (duplicate.status, duplicate.error_code()),
        (400, Some("ClusterParameterGroupAlreadyExists".to_string())),
        "case duplicate at the quota: expected 400 ClusterParameterGroupAlreadyExists, got {} / {:?}",
        duplicate.status,
        duplicate.error_code()
    );
}

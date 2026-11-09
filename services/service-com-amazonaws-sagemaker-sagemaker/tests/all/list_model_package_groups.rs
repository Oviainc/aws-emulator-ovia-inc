// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `ListModelPackageGroups`.
//!
//! Every case goes over the wire (`POST /` with
//! `X-Amz-Target: SageMaker.ListModelPackageGroups`, `awsJson1_1`) through the same
//! `MultiServiceRouter` the production router uses, so what these tests exercise is what
//! a real SDK client gets. The harness is reused from `model_package_registry`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: both
//! members of the modelled `ModelPackageGroupSortBy` catalog crossed with both of
//! `SortOrder` and with the documented defaults, every member of
//! `CrossAccountFilterOption`, the name and creation-time filters, page sizes from one
//! entry to each published `MaxResults` bound, the summary replaying what
//! `CreateModelPackageGroup` stored, and the refusals for an input outside a documented
//! value set.

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    CROSS_ACCOUNT_FILTER_OPTION_VALUES, MODEL_PACKAGE_GROUP_SORT_BY_VALUES, SORT_ORDER_VALUES,
};
use service_com_amazonaws_sagemaker_sagemaker::state;
use service_com_amazonaws_sagemaker_sagemaker::state::MAX_PAGE_SIZE;

use crate::inference_deployment_lifecycle::CREATE_SPACING;
use crate::model_package_registry::{error_code, str_field, Emulator};

/// The model groups the seeded store holds, created in THIS order. Their names are
/// deliberately not in creation order, so a `Name` sort and a `CreationTime` sort cannot
/// accidentally agree.
const GROUPS: [&str; 5] = [
    "zeta-group",
    "alpha-group",
    "mid-group",
    "beta-group",
    "omega-group",
];

/// The service's own server-side page ceiling, read from the service rather than
/// restated, so changing it does not fail this test for an unrelated reason.
const PAGE_CEILING: usize = MAX_PAGE_SIZE as usize;

/// The summaries a `ListModelPackageGroups` response carries, in response order.
fn summaries(body: &Value) -> Vec<Value> {
    body.get("ModelPackageGroupSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a ModelPackageGroupSummaryList in {body}"))
        .clone()
}

/// The listed groups' names, in response order.
fn listed_names(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| str_field(summary, "ModelPackageGroupName"))
        .collect()
}

/// Seed [`GROUPS`], spaced so `CreationTime` ordering is unambiguous, and answer their
/// names in CREATION order.
async fn seed(emulator: &Emulator) -> Vec<String> {
    let mut created = Vec::new();
    for name in GROUPS {
        emulator.create_group(state::DEFAULT_REGION, name).await;
        created.push(name.to_string());
        tokio::time::sleep(CREATE_SPACING).await;
    }
    created
}

/// The seeded names in `Name`-ascending order.
fn by_name_ascending() -> Vec<String> {
    let mut names: Vec<String> = GROUPS.iter().map(|name| name.to_string()).collect();
    names.sort();
    names
}

/// The order the listing must answer for one `(SortBy, SortOrder)` pair.
fn expected_order(sort_by: &str, sort_order: &str, creation_order: &[String]) -> Vec<String> {
    let mut ordered = match sort_by {
        "Name" => by_name_ascending(),
        _ => creation_order.to_vec(),
    };
    if sort_order == "Descending" {
        ordered.reverse();
    }
    ordered
}

/// RULE: the listing reports every model group the account holds and no other — a store
/// with no group answers an empty list, a created group appears, and a deleted one stops
/// appearing — and reading the listing changes nothing a later caller sees.
#[tokio::test]
async fn list_model_package_groups_reports_exactly_the_stored_groups() {
    let emulator = Emulator::new();

    let listed = emulator
        .ok("empty-store", "ListModelPackageGroups", &json!({}))
        .await;
    assert!(
        listed_names(&listed).is_empty(),
        "case empty-store: an account holding no model group lists none, got {listed:?}"
    );
    assert!(
        listed.get("NextToken").is_none(),
        "case empty-store: nothing is left to page, got {listed:?}"
    );

    let creation_order = seed(&emulator).await;
    let listed = emulator
        .ok("seeded", "ListModelPackageGroups", &json!({}))
        .await;
    assert_eq!(
        listed_names(&listed),
        creation_order,
        "case seeded: every created group must be listed, got {listed:?}"
    );

    // A read must not change what a later caller sees.
    let again = emulator
        .ok("stability", "ListModelPackageGroups", &json!({}))
        .await;
    assert_eq!(
        listed_names(&again),
        creation_order,
        "case stability: a second listing must answer the same set, got {again:?}"
    );

    // A deleted group stops being reported, and only that one does.
    let removed = creation_order[2].clone();
    emulator
        .ok(
            "delete",
            "DeleteModelPackageGroup",
            &json!({"ModelPackageGroupName": removed}),
        )
        .await;
    let expected: Vec<String> = creation_order
        .iter()
        .filter(|name| **name != removed)
        .cloned()
        .collect();
    let listed = emulator
        .ok("after-delete", "ListModelPackageGroups", &json!({}))
        .await;
    assert_eq!(
        listed_names(&listed),
        expected,
        "case after-delete: the deleted group must be gone and the others kept, \
         got {listed:?}"
    );
}

/// RULE: `SortBy` picks the member the result is ordered by and `SortOrder` reverses that
/// order, with the documented defaults (`CreationTime`, `Ascending`) applying to a request
/// that names neither.
#[tokio::test]
async fn list_model_package_groups_sorts_on_every_documented_key_and_order() {
    let emulator = Emulator::new();
    let creation_order = seed(&emulator).await;

    // The catalogs come from the model's own enums (`ModelPackageGroupSortBy`,
    // `SortOrder`), so a member added there is covered here without editing this test.
    for sort_by in MODEL_PACKAGE_GROUP_SORT_BY_VALUES {
        for sort_order in SORT_ORDER_VALUES {
            let case = format!("sort-by={sort_by} order={sort_order}");
            let listed = emulator
                .ok(
                    &case,
                    "ListModelPackageGroups",
                    &json!({"SortBy": sort_by, "SortOrder": sort_order}),
                )
                .await;
            assert_eq!(
                listed_names(&listed),
                expected_order(sort_by, sort_order, &creation_order),
                "case {case}: the listed order, got {listed:?}"
            );
        }
    }

    // The documented defaults: `CreationTime` ascending. Rows differ in which member is
    // omitted, so each default is exercised on its own as well as together.
    let default_cases: [(&str, Value, Vec<String>); 3] = [
        (
            "both-defaulted",
            json!({}),
            expected_order("CreationTime", "Ascending", &creation_order),
        ),
        (
            "sort-by-defaulted",
            json!({"SortOrder": "Descending"}),
            expected_order("CreationTime", "Descending", &creation_order),
        ),
        (
            "sort-order-defaulted",
            json!({"SortBy": "Name"}),
            expected_order("Name", "Ascending", &creation_order),
        ),
    ];
    for (case, body, expected) in default_cases {
        let listed = emulator.ok(case, "ListModelPackageGroups", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: the defaulted order, got {listed:?}"
        );
    }
}

/// RULE: `NameContains` keeps only the groups whose name contains the string, as a
/// substring anywhere in it, and composes with the sort.
#[tokio::test]
async fn list_model_package_groups_filters_on_name_contains() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Rows differ along where the needle sits in the name, how many groups it selects,
    // whether it is the whole name, and whether it matches nothing.
    let cases: [(&str, &str, Vec<&str>); 6] = [
        ("shared-suffix", "group", by_name_ascending_refs()),
        ("prefix", "alpha", vec!["alpha-group"]),
        ("infix", "ega-gr", vec!["omega-group"]),
        ("whole-name", "beta-group", vec!["beta-group"]),
        // The needle may carry the hyphen the name pattern admits.
        ("hyphenated-fragment", "-group", by_name_ascending_refs()),
        ("no-match", "nothing", vec![]),
    ];

    for (case, needle, expected) in cases {
        let listed = emulator
            .ok(
                case,
                "ListModelPackageGroups",
                &json!({"NameContains": needle, "SortBy": "Name", "SortOrder": "Ascending"}),
            )
            .await;
        assert_eq!(
            listed_names(&listed),
            expected
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<String>>(),
            "case {case}: NameContains={needle:?} selection, got {listed:?}"
        );
    }
}

/// The seeded names, `Name`-ascending, as `&str` — the expectation for a needle every
/// seeded group carries.
fn by_name_ascending_refs() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = GROUPS.to_vec();
    names.sort_unstable();
    names
}

/// RULE: `CreationTimeAfter`/`CreationTimeBefore` select exactly the groups on their side
/// of the instant, inclusive of a bound that lands on a stored instant, and compose with
/// each other and with the name filter.
#[tokio::test]
async fn list_model_package_groups_filters_on_the_creation_time_bounds() {
    let emulator = Emulator::new();
    let creation_order = seed(&emulator).await;

    // The stored instants, read back through the listing itself in creation order.
    let listed = emulator
        .ok(
            "instants",
            "ListModelPackageGroups",
            &json!({"SortBy": "CreationTime", "SortOrder": "Ascending"}),
        )
        .await;
    let instants: Vec<f64> = summaries(&listed)
        .iter()
        .map(|summary| {
            summary
                .get("CreationTime")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("expected a numeric CreationTime in {summary}"))
        })
        .collect();
    assert_eq!(
        instants.len(),
        creation_order.len(),
        "every seeded group must be listed, got {listed:?}"
    );

    // Bounds strictly between two neighbours, so each split is unambiguous.
    let split = (instants[1] + instants[2]) / 2.0;
    let upper = (instants[3] + instants[4]) / 2.0;

    // Rows differ along which bound is used, which side it selects, how many bounds
    // compose, whether a bound sits a hair off a stored instant, and what it composes
    // with.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "before-the-split",
            json!({"CreationTimeBefore": split}),
            creation_order[..2].to_vec(),
        ),
        (
            "after-the-split",
            json!({"CreationTimeAfter": split}),
            creation_order[2..].to_vec(),
        ),
        (
            "both-bounds",
            json!({"CreationTimeAfter": split, "CreationTimeBefore": upper}),
            creation_order[2..4].to_vec(),
        ),
        // A millisecond either side of an end instant: the finest split that is safe
        // through the wire's epoch-seconds float, and still well inside the 5 ms the
        // seeded groups are spaced by.
        (
            "bound-just-past-the-first-instant",
            json!({"CreationTimeBefore": instants[0] + 0.001}),
            creation_order[..1].to_vec(),
        ),
        (
            "bound-just-before-the-last-instant",
            json!({"CreationTimeAfter": instants[instants.len() - 1] - 0.001}),
            creation_order[creation_order.len() - 1..].to_vec(),
        ),
        (
            "bound-that-excludes-everything",
            json!({"CreationTimeAfter": instants[instants.len() - 1] + 3600.0}),
            vec![],
        ),
        (
            "bound-combined-with-the-name-filter",
            json!({"CreationTimeAfter": split, "NameContains": "omega"}),
            vec!["omega-group".to_string()],
        ),
    ];

    for (case, body, expected) in cases {
        let mut body = body;
        body["SortBy"] = Value::String("CreationTime".to_string());
        body["SortOrder"] = Value::String("Ascending".to_string());
        let listed = emulator.ok(case, "ListModelPackageGroups", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: the creation-time selection, got {listed:?}"
        );
    }
}

/// RULE: `MaxResults` caps one page and `NextToken` resumes after the entry the previous
/// page stopped at, so paging walks the whole ordered result set exactly once whatever the
/// page size, and the last page carries no token.
#[tokio::test]
async fn list_model_package_groups_pages_the_whole_result_set() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let names = by_name_ascending();

    // Rows differ along the page size: one entry, a size that does not divide the result,
    // one that matches it exactly, each published `MaxResults` bound (`range 1..100`), and
    // an absent `MaxResults`.
    let sizes: [Option<i64>; 6] = [Some(1), Some(2), Some(5), Some(100), None, Some(3)];
    for size in sizes {
        let case = match size {
            Some(size) => format!("max-results={size}"),
            None => "max-results-absent".to_string(),
        };
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"SortBy": "Name", "SortOrder": "Ascending"});
            if let Some(size) = size {
                body["MaxResults"] = Value::from(size);
            }
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok(&case, "ListModelPackageGroups", &body).await;
            let page = listed_names(&listed);
            let cap = size.map_or(PAGE_CEILING, |size| (size as usize).min(PAGE_CEILING));
            assert!(
                page.len() <= cap,
                "case {case}: a page must hold at most {cap} entries, got {page:?}"
            );
            seen.extend(page);
            token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case}: paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case {case}: paging must walk the whole ordered result exactly once"
        );
    }
}

/// RULE: `CrossAccountFilterOption` selects whose groups are reported — `SameAccount` and
/// an absent value the caller's own, `CrossAccount` only those another account made
/// discoverable, of which this emulator's single account holds none.
#[tokio::test]
async fn list_model_package_groups_selects_on_the_cross_account_filter_option() {
    let emulator = Emulator::new();
    let creation_order = seed(&emulator).await;

    // The catalog is the model's own `CrossAccountFilterOption` enum, so every member is
    // covered, plus the absent value its doc gives the `SameAccount` meaning.
    for option in CROSS_ACCOUNT_FILTER_OPTION_VALUES {
        let listed = emulator
            .ok(
                option,
                "ListModelPackageGroups",
                &json!({"CrossAccountFilterOption": option}),
            )
            .await;
        let expected = if *option == "CrossAccount" {
            Vec::new()
        } else {
            creation_order.clone()
        };
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {option}: the selected groups, got {listed:?}"
        );
    }

    let listed = emulator
        .ok("absent", "ListModelPackageGroups", &json!({}))
        .await;
    assert_eq!(
        listed_names(&listed),
        creation_order,
        "case absent: a null CrossAccountFilterOption reports the account's own groups, \
         got {listed:?}"
    );
}

/// RULE: every `ModelPackageGroupSummary` replays the stored group — the name it was
/// created under, its minted ARN, its description, the creation instant the service
/// recorded, its status and its managed configuration — and agrees member for member with
/// `DescribeModelPackageGroup`.
#[tokio::test]
async fn list_model_package_groups_summaries_replay_the_stored_group() {
    // `ModelPackageGroupDescription` is `EntityDescription, len 0..1024`;
    // `ManagedConfiguration.ManagedStorageType` is an enum whose only value is `Restricted`
    // (smithy_query shape=ManagedStorageType). Rows therefore span: no optional member at
    // all, an EMPTY description (distinguishable from an absent one), a unicode
    // description, the 1024-character upper bound, tags, and the managed configuration.
    let unicode_description = "Grupo de modelos — ñ 日本語 ✨ 42";
    let longest_description = "a".repeat(1024);
    let cases: [(&str, Value, Option<&str>, Option<&str>); 6] = [
        ("bare", json!({}), None, None),
        (
            "empty-description",
            json!({"ModelPackageGroupDescription": ""}),
            Some(""),
            None,
        ),
        (
            "unicode-description",
            json!({"ModelPackageGroupDescription": unicode_description}),
            Some(unicode_description),
            None,
        ),
        (
            "max-length-description",
            json!({"ModelPackageGroupDescription": longest_description.clone()}),
            Some(&longest_description),
            None,
        ),
        (
            "tags",
            json!({"Tags": [{"Key": "team", "Value": "ml"}]}),
            None,
            None,
        ),
        (
            "managed-configuration",
            json!({
                "ModelPackageGroupDescription": "managed",
                "ManagedConfiguration": {"ManagedStorageType": "Restricted"},
            }),
            Some("managed"),
            Some("Restricted"),
        ),
    ];

    for (case, extra, expected_description, expected_storage_type) in cases {
        let emulator = Emulator::new();
        let name = "replayed-group";
        let mut request = json!({"ModelPackageGroupName": name});
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            request
                .as_object_mut()
                .expect("the request is an object")
                .insert(member.clone(), value.clone());
        }
        let created = emulator.ok(case, "CreateModelPackageGroup", &request).await;
        let arn = str_field(&created, "ModelPackageGroupArn");

        let listed = emulator.ok(case, "ListModelPackageGroups", &json!({})).await;
        let found = summaries(&listed);
        assert_eq!(
            found.len(),
            1,
            "case {case}: one created group is listed once, got {listed:?}"
        );
        let summary = &found[0];
        assert_eq!(
            str_field(summary, "ModelPackageGroupName"),
            name,
            "case {case}: ModelPackageGroupName, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "ModelPackageGroupArn"),
            arn,
            "case {case}: the summary must carry the ARN create minted, got {summary:?}"
        );
        assert_eq!(
            summary
                .get("ModelPackageGroupDescription")
                .and_then(Value::as_str),
            expected_description,
            "case {case}: ModelPackageGroupDescription, got {summary:?}"
        );
        assert_eq!(
            summary
                .get("ManagedConfiguration")
                .and_then(|managed| managed.get("ManagedStorageType"))
                .and_then(Value::as_str),
            expected_storage_type,
            "case {case}: ManagedConfiguration.ManagedStorageType, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "ModelPackageGroupStatus"),
            "Completed",
            "case {case}: a synchronously created model group is already Completed, \
             got {summary:?}"
        );

        let described = emulator
            .ok(
                case,
                "DescribeModelPackageGroup",
                &json!({"ModelPackageGroupName": name}),
            )
            .await;
        for member in [
            "ModelPackageGroupName",
            "ModelPackageGroupArn",
            "ModelPackageGroupDescription",
            "CreationTime",
            "ModelPackageGroupStatus",
            "ManagedConfiguration",
        ] {
            assert_eq!(
                summary.get(member),
                described.get(member),
                "case {case}: the summary and DescribeModelPackageGroup must agree on \
                 {member}, got {summary:?} and {described:?}"
            );
        }
    }
}

/// RULE: an input outside a documented value set is refused with HTTP 400
/// `ValidationException` rather than silently ignored, and the refusal writes nothing.
#[tokio::test]
async fn list_model_package_groups_refuses_inputs_outside_the_documented_value_sets() {
    let emulator = Emulator::new();
    let creation_order = seed(&emulator).await;

    // Rows span: each enum-valued member given a value outside its catalog, a `NextToken`
    // naming no boundary, one past each published `MaxResults` bound (`range 1..100`), and
    // `NameContains` outside `^[a-zA-Z0-9\-]+$` / len 0..63.
    let too_long = "a".repeat(64);
    let cases: Vec<(&str, Value)> = vec![
        ("sort-by-outside-the-enum", json!({"SortBy": "Nope"})),
        ("sort-order-outside-the-enum", json!({"SortOrder": "Sideways"})),
        (
            "cross-account-filter-outside-the-enum",
            json!({"CrossAccountFilterOption": "EveryAccount"}),
        ),
        (
            "next-token-that-names-no-boundary",
            json!({"NextToken": "not-a-token"}),
        ),
        ("max-results-below-1", json!({"MaxResults": 0})),
        ("max-results-above-100", json!({"MaxResults": 101})),
        (
            "name-contains-with-an-illegal-character",
            json!({"NameContains": "bad name"}),
        ),
        ("name-contains-past-63", json!({"NameContains": too_long})),
    ];

    for (case, body) in cases {
        let (status, parsed) = emulator.call("ListModelPackageGroups", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: ListModelPackageGroups must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: refusal code, got {parsed}"
        );
    }

    // Nothing above reached state: every seeded group is still listed.
    let listed = emulator
        .ok("after-refusals", "ListModelPackageGroups", &json!({}))
        .await;
    assert_eq!(
        listed_names(&listed),
        creation_order,
        "a refused request must write nothing, got {listed:?}"
    );
}

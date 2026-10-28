// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the identity of a resource tag pair as `ListRules`' tag
//! filters select on it.
//!
//! The rule asserted here: this service identifies a resource tag pair by its key
//! TOGETHER with its value, where a tag that carries no value is the empty value — and
//! that one identity decides every comparison of two pairs, so a `ListRules` filter
//! reports a rule exactly when the rule holds the pair the filter names, whichever of
//! the two forms of a valueless pair each of them was written in. `ResourceTagValue` is
//! optional and `^[\S\s]{0,256}$`, so "no value" and "the empty value" are one value
//! (smithy_query(query="shape", shape="ResourceTag")), and the five-rules-per-pair quota
//! already counts them as one pair: a filter must therefore report the very holders the
//! quota refuses a further rule for.
//!
//! The consequence the tables also pin: a filter naming no value is NOT a wildcard over
//! every value of the key — it names the empty value, so it reports the rules selecting
//! on the empty value and no others.
//!
//! Both filter members are covered, `ResourceTags` and `ExcludeResourceTags`: they are
//! two doors onto the same comparison. Each table row is a different FORM of pair on
//! each side — value absent, value empty, a plain value, a value at the published
//! maximum length, a prefix of it, a value differing only in case, a non-ASCII value, a
//! key at the published maximum length. The first table runs every row against every
//! `ResourceType` the model declares; the later tables rotate the types across their
//! rows, the filter comparison being type-agnostic.
//! Every row drives the service through the real router
//! (`MultiServiceRouter::dispatch`), exactly as a signed SDK request would.

use serde_json::{json, Value};

use crate::retention_rule_lifecycle::{create_rule, list_of, list_rules, retention, router};
use service_com_amazonaws_rbin_amazonrecyclebin::state::MAX_RULES_PER_RESOURCE_TAG;
use smithy_server_runtime::MultiServiceRouter;

/// `ResourceType` accepts exactly these values.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// The two members that carry a tag filter on `ListRules`, and the members a rule's
/// matching selection is created with.
/// Source: smithy_query(query="operation", operation="ListRules") — input members
/// `ResourceTags` and `ExcludeResourceTags`.
const FILTER_MEMBERS: [&str; 2] = ["ResourceTags", "ExcludeResourceTags"];

/// `ResourceTagKey` is `^[\S\s]{1,128}$`.
/// Source: smithy_query(query="shape", shape="ResourceTag") — member `ResourceTagKey`.
const MAX_RESOURCE_TAG_KEY_LEN: usize = 128;

/// `ResourceTagValue` is `^[\S\s]{0,256}$`.
/// Source: smithy_query(query="shape", shape="ResourceTag") — member `ResourceTagValue`.
const MAX_RESOURCE_TAG_VALUE_LEN: usize = 256;

/// One resource tag pair as a request writes it: the value member is OMITTED when
/// `value` is `None`, which the model allows, and present otherwise.
fn pair(key: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "ResourceTagKey": key, "ResourceTagValue": value }),
        None => json!({ "ResourceTagKey": key }),
    }
}

/// One row of the filter table: the form of the pair a rule is created selecting on, the
/// form of the pair a later `ListRules` filters by, and whether the two name the SAME
/// pair — which is whether the filter must report the rule.
#[derive(Debug, Clone)]
struct FilterCase {
    case: &'static str,
    key: String,
    stored: Option<String>,
    filter: Option<String>,
    selects: bool,
}

/// The forms the stored pair and the filter's pair can take against each other.
fn filter_cases() -> Vec<FilterCase> {
    let longest = "v".repeat(MAX_RESOURCE_TAG_VALUE_LEN);
    let prefix: String = longest.chars().take(MAX_RESOURCE_TAG_VALUE_LEN - 1).collect();
    vec![
        FilterCase {
            case: "no value stored, no value in the filter",
            key: "ledger".to_string(),
            stored: None,
            filter: None,
            selects: true,
        },
        FilterCase {
            case: "no value stored, the empty value in the filter",
            key: "ledger".to_string(),
            stored: None,
            filter: Some(String::new()),
            selects: true,
        },
        FilterCase {
            case: "the empty value stored, no value in the filter",
            key: "cost-centre".to_string(),
            stored: Some(String::new()),
            filter: None,
            selects: true,
        },
        FilterCase {
            case: "the empty value stored, the empty value in the filter",
            key: "cost-centre".to_string(),
            stored: Some(String::new()),
            filter: Some(String::new()),
            selects: true,
        },
        FilterCase {
            case: "a key at the published maximum length, no value against the empty value",
            key: "k".repeat(MAX_RESOURCE_TAG_KEY_LEN),
            stored: None,
            filter: Some(String::new()),
            selects: true,
        },
        FilterCase {
            case: "a plain value stored, the same value in the filter",
            key: "retain".to_string(),
            stored: Some("q3".to_string()),
            filter: Some("q3".to_string()),
            selects: true,
        },
        FilterCase {
            case: "a value at the published maximum length, matched by itself",
            key: "archive".to_string(),
            stored: Some(longest.clone()),
            filter: Some(longest.clone()),
            selects: true,
        },
        FilterCase {
            case: "a value carrying whitespace and non-ASCII characters, matched by itself",
            key: "owner".to_string(),
            stored: Some("équipe un".to_string()),
            filter: Some("équipe un".to_string()),
            selects: true,
        },
        FilterCase {
            case: "a plain value stored, no value in the filter",
            key: "retain".to_string(),
            stored: Some("q3".to_string()),
            filter: None,
            selects: false,
        },
        FilterCase {
            case: "a plain value stored, the empty value in the filter",
            key: "retain".to_string(),
            stored: Some("q3".to_string()),
            filter: Some(String::new()),
            selects: false,
        },
        FilterCase {
            case: "no value stored, a plain value in the filter",
            key: "ledger".to_string(),
            stored: None,
            filter: Some("q3".to_string()),
            selects: false,
        },
        FilterCase {
            case: "the empty value stored, a plain value in the filter",
            key: "cost-centre".to_string(),
            stored: Some(String::new()),
            filter: Some("q3".to_string()),
            selects: false,
        },
        FilterCase {
            case: "a plain value stored, another value in the filter",
            key: "retain".to_string(),
            stored: Some("q3".to_string()),
            filter: Some("q4".to_string()),
            selects: false,
        },
        FilterCase {
            case: "a value stored, the same value in another case in the filter",
            key: "retain".to_string(),
            stored: Some("Q3".to_string()),
            filter: Some("q3".to_string()),
            selects: false,
        },
        FilterCase {
            case: "a value at the published maximum length, against a prefix of it",
            key: "archive".to_string(),
            stored: Some(longest),
            filter: Some(prefix),
            selects: false,
        },
    ]
}

/// Create a rule of `resource_type` whose `member` selection is `tags`, assert it was
/// accepted, and answer its identifier.
async fn create_selecting(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    member: &str,
    tags: Vec<Value>,
) -> String {
    let mut body = json!({ "ResourceType": resource_type, "RetentionPeriod": retention(7) });
    body[member] = Value::Array(tags);
    let (status, created) = create_rule(router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateRule, got {status} / {created:?}"
    );
    created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no Identifier: {created:?}"))
        .to_string()
}

/// The identifiers `ListRules` reports for `body`, asserting the call was served.
async fn selected_identifiers(
    router: &MultiServiceRouter,
    case: &str,
    body: Value,
) -> Vec<String> {
    let (status, reported) = list_rules(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListRules, got {status} / {reported:?}"
    );
    list_of(&reported, "Rules")
        .iter()
        .map(|rule| {
            rule["Identifier"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a reported rule carries no Identifier: {rule:?}")
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_tag_filter_reports_a_rule_exactly_when_it_names_the_same_pair() {
    for member in FILTER_MEMBERS {
        for (resource_type, row_case) in RESOURCE_TYPES
            .into_iter()
            .flat_map(|resource_type| filter_cases().into_iter().map(move |row| (resource_type, row)))
        {
            // A fresh store per row, so one row's rule cannot be reported for another's.
            let router = router();
            let case = format!("{member} / {resource_type} / {}", row_case.case);
            let identifier = create_selecting(
                &router,
                &case,
                resource_type,
                member,
                vec![pair(&row_case.key, row_case.stored.as_deref())],
            )
            .await;

            // Unfiltered, the rule is one of this type's rules: whatever the filtered
            // page reports is the filter's doing and nothing else's.
            let unfiltered =
                selected_identifiers(&router, &case, json!({ "ResourceType": resource_type }))
                    .await;
            assert!(
                unfiltered.contains(&identifier),
                "case {case:?}: an unfiltered ListRules must report the rule, got {unfiltered:?}"
            );

            let selected = selected_identifiers(
                &router,
                &case,
                json!({
                    "ResourceType": resource_type,
                    member: [pair(&row_case.key, row_case.filter.as_deref())],
                }),
            )
            .await;
            assert_eq!(
                selected.contains(&identifier),
                row_case.selects,
                "case {case:?}: expected the filter to {} the rule, got {selected:?}",
                if row_case.selects {
                    "report"
                } else {
                    "pass over"
                }
            );
        }
    }
}

#[tokio::test]
async fn a_tag_filter_reports_the_holders_the_pair_quota_counts() {
    // The two forms of a valueless pair, crossed: the form the holders are created in
    // against the form the quota refusal and the filter are written in.
    const VALUELESS: [Option<&str>; 2] = [None, Some("")];
    let key = "ledger";
    for (row, (stored, filter)) in VALUELESS
        .iter()
        .flat_map(|stored| VALUELESS.iter().map(move |filter| (*stored, *filter)))
        .enumerate()
    {
        let router = router();
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let case = format!("holders written as {stored:?}, filtered as {filter:?}");

        let mut holders = Vec::new();
        for nth in 0..MAX_RULES_PER_RESOURCE_TAG {
            holders.push(
                create_selecting(
                    &router,
                    &format!("{case}: holder {nth}"),
                    resource_type,
                    "ResourceTags",
                    vec![pair(key, stored)],
                )
                .await,
            );
        }

        // The quota says the two forms are ONE pair: a further rule written in the
        // filter's form is refused for the holders created in the other form.
        let (status, refused) = create_rule(
            &router,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(7),
                "ResourceTags": [pair(key, filter)],
            }),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {case:?}: a rule past the pair's quota must be refused, \
             got {status} / {refused:?}"
        );

        // So a filter in that same form reports exactly those holders: the caller can
        // see the rules the refusal is about.
        let mut selected = selected_identifiers(
            &router,
            &case,
            json!({
                "ResourceType": resource_type,
                "ResourceTags": [pair(key, filter)],
            }),
        )
        .await;
        selected.sort();
        let mut expected = holders.clone();
        expected.sort();
        assert_eq!(
            selected, expected,
            "case {case:?}: the filter must report the quota's holders, got {selected:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_naming_no_value_is_not_a_wildcard_over_the_key() {
    // Every form of the key's value a rule can select on, each on its own rule: the two
    // spellings of the valueless pair and two distinct values.
    let key = "ledger";
    let forms: [Option<&str>; 4] = [None, Some(""), Some("q3"), Some("q4")];
    for (row, member) in FILTER_MEMBERS.into_iter().enumerate() {
        let router = router();
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let mut holders = Vec::new();
        for form in forms {
            let case = format!("{member}: a rule selecting on {form:?}");
            holders.push((
                form,
                create_selecting(&router, &case, resource_type, member, vec![pair(key, form)])
                    .await,
            ));
        }

        // A filter reports the rules selecting on the pair it names, and the valueless
        // spellings name one pair, so each filter form selects the rules of its own
        // value — never every value of the key.
        for filter in forms {
            let case = format!("{member}: filtered on {filter:?}");
            let mut selected = selected_identifiers(
                &router,
                &case,
                json!({ "ResourceType": resource_type, member: [pair(key, filter)] }),
            )
            .await;
            selected.sort();
            let mut expected: Vec<String> = holders
                .iter()
                .filter(|(form, _)| form.unwrap_or_default() == filter.unwrap_or_default())
                .map(|(_, identifier)| identifier.clone())
                .collect();
            expected.sort();
            assert_eq!(
                selected, expected,
                "case {case:?}: expected the rules selecting on that one pair, got {selected:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_filter_value_past_its_published_bound_is_refused() {
    // The bound is `ResourceTagValue`'s own `^[\S\s]{0,256}$`: the empty value and a
    // value of the maximum length are filters the service serves (the table above
    // asserts what they select), and one character past the maximum is refused rather
    // than silently selecting anything.
    for (row, member) in FILTER_MEMBERS.into_iter().enumerate() {
        let router = router();
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        for (case, value, expected_status) in [
            ("the empty value", String::new(), 200),
            (
                "a value at the published maximum length",
                "v".repeat(MAX_RESOURCE_TAG_VALUE_LEN),
                200,
            ),
            (
                "a value one character past the published maximum length",
                "v".repeat(MAX_RESOURCE_TAG_VALUE_LEN + 1),
                400,
            ),
        ] {
            let case = format!("{member}: {case}");
            let (status, body) = list_rules(
                &router,
                json!({
                    "ResourceType": resource_type,
                    member: [pair("ledger", Some(&value))],
                }),
            )
            .await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} / {body:?}"
            );
        }
    }
}

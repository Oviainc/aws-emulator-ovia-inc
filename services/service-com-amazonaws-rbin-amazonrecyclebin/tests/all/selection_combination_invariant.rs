// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the one rule BOTH doors onto a retention rule's resource
//! selection are held to: a rule selects what it retains either by resource tags
//! (tag-level) or by Region with exclusion tags, never by both at once.
//!
//! `ResourceTags` makes a rule tag-level, and `ExcludeResourceTags` is "[Region-level
//! retention rules only] ... You can't specify exclusion tags for tag-level retention
//! rules" (smithy_query(query="doc", shape="CreateRuleRequest",
//! member="ExcludeResourceTags")), so a selection in which both lists carry a value is one
//! no retention rule can hold. Two operations can produce such a selection — `CreateRule`,
//! from the lists the request names, and `UpdateRule`, from the merge of the lists the
//! request names with the ones the addressed rule already stores — and both are held to the
//! invariant by the single `crate::state` helper, so this suite drives it through each of
//! them: refused with HTTP 400 `ValidationException`, `Reason` `INVALID_PARAMETER_VALUE`
//! and the clause naming `excludeResourceTags` and the requirement it failed, with nothing
//! minted and nothing changed.
//!
//! `UpdateRule` is driven on all three ways a merged selection can name both lists — both
//! sent in one request, exclusion tags sent onto a stored tag-level rule, and resource tags
//! sent onto a stored rule with exclusion tags — because only the merge, not the request
//! alone, decides what the rule would end up selecting on.
//!
//! The complement is asserted too: a selection naming ONE of the lists (with the other
//! absent or sent empty) is accepted through both doors and read back, so the shared
//! predicate refuses the combination and nothing wider.
//!
//! Every case drives the service through the real router, exactly as a signed SDK request
//! would, and every row states its own case so a failure names the input.

use serde_json::{json, Value};

use crate::error_response_bodies::{error_code, message, one_validation_error};
use crate::retention_rule_lifecycle::{
    create_rule, get_rule, list_of, list_rules, retention, router, supported_retention,
    update_rule, RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

/// The member path a refused `ExcludeResourceTags` is named by, as a validation clause
/// spells a member: the modelled member name, lower camel case.
/// Source: smithy_query(query="operation", operation="CreateRule") — input member
/// `ExcludeResourceTags`.
const EXCLUDE_RESOURCE_TAGS_MEMBER: &str = "excludeResourceTags";

/// The requirement an exclusion tag fails by riding along with resource tags: the rule the
/// resource tags make is a tag-level one, and exclusion tags cannot be specified for such a
/// rule (smithy_query(query="doc", shape="CreateRuleRequest",
/// member="ExcludeResourceTags")).
const TAG_LEVEL_REQUIREMENT: &str = "Member must not be specified for a tag-level retention rule";

/// The published maximum number of resource tags one rule may select on.
/// Source: smithy_query(query="shape", shape="ResourceTags") — length {min: 0, max: 50}.
const MAX_RESOURCE_TAGS: usize = 50;

/// The published maximum number of exclusion tags one rule may carry.
/// Source: smithy_query(query="shape", shape="ExcludeResourceTags") — length {min: 0,
/// max: 5}.
const MAX_EXCLUDE_RESOURCE_TAGS: usize = 5;

/// `count` resource tags whose keys are unique to `prefix`, so no two rows of the table
/// select on the same key and value pair. `value` is the tag value each carries, or `None`
/// for a key-only tag (`ResourceTagValue` is optional).
fn selection(prefix: &str, count: usize, value: Option<&str>) -> Value {
    let tags: Vec<Value> = (0..count)
        .map(|nth| match value {
            Some(value) => json!({
                "ResourceTagKey": format!("{prefix}-{nth}"),
                "ResourceTagValue": value,
            }),
            None => json!({ "ResourceTagKey": format!("{prefix}-{nth}") }),
        })
        .collect();
    Value::Array(tags)
}

/// One row of the table: a resource-tag selection and an exclusion-tag selection that
/// cannot hold together, for a rule of `resource_type`.
struct Row {
    case: &'static str,
    resource_type: &'static str,
    resource_tags: Value,
    exclude_resource_tags: Value,
}

/// Every selection naming both lists, varied along what the invariant generalises over:
/// how many tags each list holds (one, several, each published maximum), whether a tag
/// carries a value and whether that value is empty, what the key looks like (plain, spaced,
/// non-ASCII — `ResourceTagKey` is `^[\S\s]{1,128}$`), and which resource type the rule is
/// for.
fn both_lists_rows() -> Vec<Row> {
    vec![
        Row {
            case: "one valued resource tag and one key-only exclusion tag",
            resource_type: RESOURCE_TYPES[1],
            resource_tags: selection("retain", 1, Some("yes")),
            exclude_resource_tags: selection("scratch", 1, None),
        },
        Row {
            case: "one key-only resource tag and one valued exclusion tag",
            resource_type: RESOURCE_TYPES[2],
            resource_tags: selection("tier", 1, None),
            exclude_resource_tags: selection("skip", 1, Some("true")),
        },
        Row {
            case: "resource tags at the published maximum with one exclusion tag",
            resource_type: RESOURCE_TYPES[0],
            resource_tags: selection("bulk", MAX_RESOURCE_TAGS, Some("retained")),
            exclude_resource_tags: selection("ignore", 1, Some("ignored")),
        },
        Row {
            case: "one resource tag with exclusion tags at the published maximum",
            resource_type: RESOURCE_TYPES[1],
            resource_tags: selection("keep", 1, Some("always")),
            exclude_resource_tags: selection("drop", MAX_EXCLUDE_RESOURCE_TAGS, None),
        },
        Row {
            case: "both lists at their published maxima",
            resource_type: RESOURCE_TYPES[2],
            resource_tags: selection("every", MAX_RESOURCE_TAGS, None),
            exclude_resource_tags: selection("none", MAX_EXCLUDE_RESOURCE_TAGS, Some("excluded")),
        },
        Row {
            case: "spaced and non-ASCII keys with an empty tag value",
            resource_type: RESOURCE_TYPES[0],
            resource_tags: selection("two words", 3, Some("")),
            exclude_resource_tags: selection("клавиша", 2, Some("значение")),
        },
    ]
}

/// Every resource tag key and value a selection names, which a refusal must not quote back.
fn submitted_tag_text(selections: &[&Value]) -> Vec<String> {
    let mut text = Vec::new();
    for selection in selections {
        for tag in selection.as_array().into_iter().flatten() {
            for member in ["ResourceTagKey", "ResourceTagValue"] {
                if let Some(value) = tag.get(member).and_then(Value::as_str) {
                    if !value.is_empty() {
                        text.push(value.to_string());
                    }
                }
            }
        }
    }
    text
}

/// Assert one refusal of the combination: the status, the modelled error, the fault class,
/// the clause the shared helper composes, and that nothing the request named is echoed.
fn assert_refused(case: &str, status: u16, body: &Value, submitted: &[&Value]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {body:?}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("INVALID_PARAMETER_VALUE"),
        "case {case:?}: expected Reason INVALID_PARAMETER_VALUE, got {:?}",
        body["Reason"]
    );
    let expected = one_validation_error(EXCLUDE_RESOURCE_TAGS_MEMBER, TAG_LEVEL_REQUIREMENT);
    assert_eq!(
        message(body),
        expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
        message(body)
    );
    for text in submitted_tag_text(submitted) {
        assert!(
            !message(body).contains(&text),
            "case {case:?}: the refusal echoed the submitted {text:?}: {:?}",
            message(body)
        );
    }
}

/// Create a rule of `resource_type` carrying `selection_members` and hand back its
/// identifier.
async fn created_rule(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    nth: usize,
    selection_members: Value,
) -> String {
    let mut body = json!({
        "ResourceType": resource_type,
        "RetentionPeriod": retention(supported_retention(resource_type, nth)),
    });
    for (member, value) in selection_members.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    let (status, created) = create_rule(router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier: {created:?}"))
        .to_string()
}

#[tokio::test]
async fn create_rule_refuses_a_selection_naming_both_tag_sets() {
    let router = router();
    for row in both_lists_rows() {
        let case = row.case;
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": row.resource_type,
                "RetentionPeriod": retention(supported_retention(row.resource_type, 3)),
                "Description": "a selection no rule can hold",
                "ResourceTags": row.resource_tags.clone(),
                "ExcludeResourceTags": row.exclude_resource_tags.clone(),
            }),
        )
        .await;
        assert_refused(
            case,
            status,
            &body,
            &[&row.resource_tags, &row.exclude_resource_tags],
        );

        // Nothing was minted: the refusal comes before the rule exists, so the Region holds
        // no rule of the type the request named.
        let (status, listed) =
            list_rules(&router, json!({ "ResourceType": row.resource_type })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListRules expected HTTP 200, got {status} / {listed:?}"
        );
        assert!(
            list_of(&listed, "Rules").is_empty(),
            "case {case:?}: expected the refusal to mint no rule, got {:?}",
            listed["Rules"]
        );
    }
}

#[tokio::test]
async fn update_rule_refuses_every_merge_that_names_both_tag_sets() {
    // The merge, not the request alone, decides what the rule would select on: each row is
    // driven through all three ways a merged selection can name both lists.
    for (door, label) in [
        (0_usize, "both lists in one request onto a Region-level rule"),
        (1, "exclusion tags onto a stored tag-level rule"),
        (2, "resource tags onto a stored rule with exclusion tags"),
    ] {
        let router = router();
        for (nth, row) in both_lists_rows().into_iter().enumerate() {
            let case = format!("{} — {label}", row.case);
            let (stored, update) = match door {
                0 => (
                    json!({}),
                    json!({
                        "ResourceTags": row.resource_tags.clone(),
                        "ExcludeResourceTags": row.exclude_resource_tags.clone(),
                    }),
                ),
                1 => (
                    json!({ "ResourceTags": row.resource_tags.clone() }),
                    json!({ "ExcludeResourceTags": row.exclude_resource_tags.clone() }),
                ),
                _ => (
                    json!({ "ExcludeResourceTags": row.exclude_resource_tags.clone() }),
                    json!({ "ResourceTags": row.resource_tags.clone() }),
                ),
            };
            let identifier =
                created_rule(&router, &case, row.resource_type, nth, stored.clone()).await;

            let (status, body) = update_rule(&router, &identifier, update.clone()).await;
            assert_refused(
                &case,
                status,
                &body,
                &[&row.resource_tags, &row.exclude_resource_tags],
            );

            // The refused update changed nothing: the rule keeps exactly the selection it
            // was created with, on both members.
            let (status, fetched) = get_rule(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetRule after the refusal expected HTTP 200, got {status} / \
                 {fetched:?}"
            );
            for member in ["ResourceTags", "ExcludeResourceTags"] {
                let expected = stored.get(member).cloned().unwrap_or_else(|| json!([]));
                assert_eq!(
                    fetched[member], expected,
                    "case {case:?}: expected the stored {member} to stay {expected:?}, got {:?}",
                    fetched[member]
                );
            }
        }
    }
}

#[tokio::test]
async fn one_tag_set_alone_is_accepted_through_both_doors() {
    // The complement of the refusal table: a selection naming ONE list — with the other
    // absent, or sent as the empty list, which names nothing — is a selection a rule can
    // hold, so both doors accept it and a later reader sees it.
    let router = router();
    let mut rows: Vec<(String, Value)> = Vec::new();
    for row in both_lists_rows() {
        rows.push((
            format!("{} — resource tags alone", row.case),
            json!({ "ResourceTags": row.resource_tags.clone() }),
        ));
        rows.push((
            format!("{} — resource tags with an empty exclusion list", row.case),
            json!({ "ResourceTags": row.resource_tags.clone(), "ExcludeResourceTags": [] }),
        ));
        rows.push((
            format!("{} — exclusion tags alone", row.case),
            json!({ "ExcludeResourceTags": row.exclude_resource_tags.clone() }),
        ));
        rows.push((
            format!("{} — exclusion tags with an empty resource list", row.case),
            json!({
                "ResourceTags": [],
                "ExcludeResourceTags": row.exclude_resource_tags.clone(),
            }),
        ));
    }
    for (nth, (case, selection_members)) in rows.into_iter().enumerate() {
        let resource_type = RESOURCE_TYPES[nth % RESOURCE_TYPES.len()];

        // Door one: CreateRule mints the rule with the selection, and a later read sees it.
        let identifier = created_rule(
            &router,
            &case,
            resource_type,
            nth,
            selection_members.clone(),
        )
        .await;
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule after CreateRule expected HTTP 200, got {status} / {fetched:?}"
        );
        for member in ["ResourceTags", "ExcludeResourceTags"] {
            let expected = selection_members
                .get(member)
                .cloned()
                .unwrap_or_else(|| json!([]));
            assert_eq!(
                fetched[member], expected,
                "case {case:?}: expected CreateRule to store {member} as {expected:?}, got {:?}",
                fetched[member]
            );
        }

        // Door two: UpdateRule puts the same selection on a rule that stores none, and a
        // later read sees it.
        let plain = created_rule(&router, &case, resource_type, nth, json!({})).await;
        let (status, updated) = update_rule(&router, &plain, selection_members.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateRule expected HTTP 200, got {status} / {updated:?}"
        );
        let (status, fetched) = get_rule(&router, &plain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule after UpdateRule expected HTTP 200, got {status} / {fetched:?}"
        );
        for member in ["ResourceTags", "ExcludeResourceTags"] {
            let expected = selection_members
                .get(member)
                .cloned()
                .unwrap_or_else(|| json!([]));
            assert_eq!(
                fetched[member], expected,
                "case {case:?}: expected UpdateRule to store {member} as {expected:?}, got {:?}",
                fetched[member]
            );
        }
    }
}

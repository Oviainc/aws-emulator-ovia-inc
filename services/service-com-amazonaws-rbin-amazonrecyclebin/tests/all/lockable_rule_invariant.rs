// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the one kind of retention rule Recycle Bin can hold a lock
//! on.
//!
//! The rule under test: a lock belongs only to a Region-level retention rule that
//! carries no exclusion tags, so a rule that selects resources by resource tags (which
//! makes it tag-level) or that carries exclusion tags can never be locked — through
//! EITHER door. `CreateRule` refuses a `LockConfiguration` on such a rule with HTTP 400
//! `ValidationException` before anything is minted, `LockRule` refuses the same rule
//! once it exists with HTTP 409 `ConflictException`, and a rule that was refused a lock
//! is still an ordinary unlocked rule afterwards: readable, modifiable and deletable.
//! A Region-level rule with neither kind of tag is locked through both doors.
//!
//! Every case drives the service through the real router, exactly as a signed SDK
//! request would, and every row states its own case so a failure names the input.

use serde_json::{json, Value};

use crate::error_response_bodies::{message, one_validation_error};
use crate::retention_rule_lifecycle::{
    create_rule, delete_rule, get_rule, list_of, list_rules, lock_configuration, retention, router,
    update_rule,
};
use crate::rule_lock_transitions::lock_rule;
use smithy_server_runtime::MultiServiceRouter;

/// `ResourceType` accepts exactly these values.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// Published bounds of `UnlockDelay$UnlockDelayValue`, in `UnlockDelayUnit` `DAYS`.
/// Source: smithy_query(query="shape", shape="UnlockDelay") — range {min: 7, max: 30}.
const UNLOCK_DELAY_MIN: i64 = 7;
const UNLOCK_DELAY_MAX: i64 = 30;

/// The published maximum number of exclusion tags one rule may carry.
/// Source: smithy_query(query="operation", operation="CreateRule") —
/// `ExcludeResourceTags`, length {min: 0, max: 5}.
const MAX_EXCLUDE_RESOURCE_TAGS: usize = 5;

/// What a refused `LockConfiguration` must state as the requirement it failed, for each
/// kind of rule a lock cannot hold on: a tag-level rule, and a Region-level rule that
/// carries exclusion tags (smithy_query(query="doc", shape="LockRule") — "You can't lock
/// tag-level retention rules, or Region-level retention rules that have exclusion tags").
const TAG_LEVEL_REQUIREMENT: &str =
    "Member must not be specified for a tag-level retention rule";
const EXCLUSION_TAGS_REQUIREMENT: &str =
    "Member must not be specified for a retention rule with exclusion tags";

/// How a refused lock on an EXISTING rule of each kind names the blocking condition,
/// after "because it is".
const TAG_LEVEL_CONDITION: &str = "a tag-level rule";
const EXCLUSION_TAGS_CONDITION: &str = "a Region-level rule with exclusion tags";

/// How many rules of `resource_type` the store holds.
async fn rule_count(router: &MultiServiceRouter, resource_type: &str) -> usize {
    let (status, listed) = list_rules(router, json!({ "ResourceType": resource_type })).await;
    assert_eq!(
        status, 200,
        "ListRules for {resource_type:?} expected HTTP 200, got {status} / {listed:?}"
    );
    list_of(&listed, "Rules").len()
}

/// One row of the unlockable-rule table: how the rule selects what it retains, and what
/// a refusal must state for that kind of rule.
struct Row {
    case: &'static str,
    resource_type: &'static str,
    /// The `ResourceTags`/`ExcludeResourceTags` the rule is created with; a member left
    /// `Value::Null` is not sent at all.
    resource_tags: Value,
    exclude_resource_tags: Value,
    /// The requirement a refused `CreateRule` must state, and the condition a refused
    /// `LockRule` must name.
    requirement: &'static str,
    condition: &'static str,
}

/// Every kind of rule a lock cannot hold on, varied along what the invariant
/// generalises over: which member carries the tags, how many tags there are (one, three,
/// the published maximum), whether a tag carries a value, which resource type the rule
/// is for, and whether the OTHER selection member is absent or sent empty.
fn unlockable_rows() -> Vec<Row> {
    let mut exclusion_tags = Vec::new();
    for nth in 0..MAX_EXCLUDE_RESOURCE_TAGS {
        exclusion_tags.push(json!({ "ResourceTagKey": format!("scratch-{nth}") }));
    }
    vec![
        Row {
            case: "tag-level rule, one resource tag with a value",
            resource_type: RESOURCE_TYPES[0],
            resource_tags: json!([{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }]),
            exclude_resource_tags: Value::Null,
            requirement: TAG_LEVEL_REQUIREMENT,
            condition: TAG_LEVEL_CONDITION,
        },
        Row {
            case: "tag-level rule, one resource tag with no value",
            resource_type: RESOURCE_TYPES[1],
            resource_tags: json!([{ "ResourceTagKey": "tier" }]),
            exclude_resource_tags: Value::Null,
            requirement: TAG_LEVEL_REQUIREMENT,
            condition: TAG_LEVEL_CONDITION,
        },
        Row {
            case: "tag-level rule, three resource tags",
            resource_type: RESOURCE_TYPES[2],
            resource_tags: json!([
                { "ResourceTagKey": "team", "ResourceTagValue": "platform" },
                { "ResourceTagKey": "env", "ResourceTagValue": "prod" },
                { "ResourceTagKey": "keep" },
            ]),
            exclude_resource_tags: Value::Null,
            requirement: TAG_LEVEL_REQUIREMENT,
            condition: TAG_LEVEL_CONDITION,
        },
        Row {
            case: "tag-level rule whose exclusion tags are sent empty",
            resource_type: RESOURCE_TYPES[0],
            resource_tags: json!([{ "ResourceTagKey": "archive", "ResourceTagValue": "quarterly" }]),
            exclude_resource_tags: json!([]),
            requirement: TAG_LEVEL_REQUIREMENT,
            condition: TAG_LEVEL_CONDITION,
        },
        Row {
            case: "region-level rule, one exclusion tag with a value",
            resource_type: RESOURCE_TYPES[1],
            resource_tags: Value::Null,
            exclude_resource_tags: json!([
                { "ResourceTagKey": "temporary", "ResourceTagValue": "true" },
            ]),
            requirement: EXCLUSION_TAGS_REQUIREMENT,
            condition: EXCLUSION_TAGS_CONDITION,
        },
        Row {
            case: "region-level rule, one exclusion tag with no value",
            resource_type: RESOURCE_TYPES[2],
            resource_tags: Value::Null,
            exclude_resource_tags: json!([{ "ResourceTagKey": "ephemeral" }]),
            requirement: EXCLUSION_TAGS_REQUIREMENT,
            condition: EXCLUSION_TAGS_CONDITION,
        },
        Row {
            case: "region-level rule at the published maximum of exclusion tags",
            resource_type: RESOURCE_TYPES[0],
            resource_tags: Value::Null,
            exclude_resource_tags: Value::Array(exclusion_tags),
            requirement: EXCLUSION_TAGS_REQUIREMENT,
            condition: EXCLUSION_TAGS_CONDITION,
        },
        Row {
            case: "region-level rule with exclusion tags whose resource tags are sent empty",
            resource_type: RESOURCE_TYPES[1],
            resource_tags: json!([]),
            exclude_resource_tags: json!([{ "ResourceTagKey": "cache" }]),
            requirement: EXCLUSION_TAGS_REQUIREMENT,
            condition: EXCLUSION_TAGS_CONDITION,
        },
    ]
}

/// A `CreateRule` body for one row, with the lock configuration included or left out.
fn create_body(row: &Row, unlock_delay: Option<i64>) -> Value {
    let mut body = json!({
        "ResourceType": row.resource_type,
        "RetentionPeriod": retention(30),
    });
    if !row.resource_tags.is_null() {
        body["ResourceTags"] = row.resource_tags.clone();
    }
    if !row.exclude_resource_tags.is_null() {
        body["ExcludeResourceTags"] = row.exclude_resource_tags.clone();
    }
    if let Some(delay) = unlock_delay {
        body["LockConfiguration"] = lock_configuration(delay);
    }
    body
}

/// Every resource tag key and value the row's request names, which a refusal must not
/// quote back.
fn submitted_tag_text(row: &Row) -> Vec<String> {
    let mut text = Vec::new();
    for member in [&row.resource_tags, &row.exclude_resource_tags] {
        for tag in member.as_array().unwrap_or(&Vec::new()) {
            for key in ["ResourceTagKey", "ResourceTagValue"] {
                if let Some(value) = tag.get(key).and_then(Value::as_str) {
                    if !value.is_empty() {
                        text.push(value.to_string());
                    }
                }
            }
        }
    }
    text
}

#[tokio::test]
async fn create_rule_refuses_a_lock_configuration_on_every_rule_that_cannot_be_locked() {
    let router = router();
    for row in unlockable_rows() {
        let case = row.case;
        // Both published unlock delay bounds, and one value between them: the refusal is
        // a property of the RULE, not of the delay asked for.
        for unlock_delay in [UNLOCK_DELAY_MIN, 19, UNLOCK_DELAY_MAX] {
            let before = rule_count(&router, row.resource_type).await;
            let (status, body) = create_rule(&router, create_body(&row, Some(unlock_delay))).await;
            assert_eq!(
                status, 400,
                "case {case:?} (unlock delay {unlock_delay}): expected HTTP 400, \
                 got {status} / {body:?}"
            );
            assert_eq!(
                body["Reason"].as_str(),
                Some("INVALID_PARAMETER_VALUE"),
                "case {case:?} (unlock delay {unlock_delay}): expected Reason \
                 INVALID_PARAMETER_VALUE, got {:?}",
                body["Reason"]
            );
            let expected = one_validation_error("lockConfiguration", row.requirement);
            assert_eq!(
                message(&body),
                expected,
                "case {case:?} (unlock delay {unlock_delay}): expected the message \
                 {expected:?}, got {:?}",
                message(&body)
            );
            // The refusal states the member and the requirement; nothing the request
            // named is quoted back.
            for submitted in submitted_tag_text(&row) {
                assert!(
                    !message(&body).contains(&submitted),
                    "case {case:?} (unlock delay {unlock_delay}): the refusal echoed the \
                     submitted {submitted:?}: {:?}",
                    message(&body)
                );
            }
            // Nothing was minted: the store holds exactly the rules it held before.
            let after = rule_count(&router, row.resource_type).await;
            assert_eq!(
                after, before,
                "case {case:?} (unlock delay {unlock_delay}): expected the refused create to \
                 mint nothing, {before} rules became {after}"
            );
        }

        // The SAME rule without a lock configuration is created happily — the lock is
        // what cannot hold, not the way the rule selects resources.
        let (status, created) = create_rule(&router, create_body(&row, None)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRule without a lock configuration expected HTTP 201, \
             got {status} / {created:?}"
        );
        let identifier = created["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier"))
            .to_string();
        assert_eq!(
            created["LockState"].as_str(),
            None,
            "case {case:?}: expected a never-locked rule to report no LockState, got {:?}",
            created["LockState"]
        );

        // And the other door is shut too: locking it is refused with the modelled
        // conflict, naming the condition that blocks it.
        let (status, refused) = lock_rule(
            &router,
            &identifier,
            json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: LockRule expected HTTP 409, got {status} / {refused:?}"
        );
        let expected = format!(
            "Cannot lock Recycle Bin rule {identifier} because it is {}.",
            row.condition
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected the conflict message {expected:?}, got {:?}",
            message(&refused)
        );

        // So the rule is an ordinary unlocked rule: it reports no lock state, it can be
        // modified, and it can be deleted.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            None,
            "case {case:?}: expected the stored rule to report no LockState, got {:?}",
            fetched["LockState"]
        );
        assert_eq!(
            fetched.get("LockConfiguration").cloned().unwrap_or(Value::Null),
            Value::Null,
            "case {case:?}: expected the stored rule to hold no LockConfiguration, got {:?}",
            fetched["LockConfiguration"]
        );
        let (status, updated) =
            update_rule(&router, &identifier, json!({ "Description": "still editable" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateRule expected HTTP 200, got {status} / {updated:?}"
        );
        let (status, deleted) = delete_rule(&router, &identifier).await;
        assert_eq!(
            status, 204,
            "case {case:?}: DeleteRule expected HTTP 204, got {status} / {deleted:?}"
        );
    }
}

#[tokio::test]
async fn a_region_level_rule_without_exclusion_tags_is_locked_through_either_door() {
    let router = router();
    // The complement of the table above: the one kind of rule a lock CAN hold on, over
    // every modelled resource type, both published unlock delay bounds, and a request
    // that sends both selection members as empty lists (which still selects the whole
    // Region).
    let mut rows: Vec<(String, Value, i64)> = Vec::new();
    for resource_type in RESOURCE_TYPES {
        for unlock_delay in [UNLOCK_DELAY_MIN, UNLOCK_DELAY_MAX] {
            rows.push((
                format!("{resource_type} rule, unlock delay {unlock_delay}"),
                json!({ "ResourceType": resource_type, "RetentionPeriod": retention(30) }),
                unlock_delay,
            ));
        }
    }
    rows.push((
        "rule whose selection members are both sent empty".to_string(),
        json!({
            "ResourceType": RESOURCE_TYPES[0],
            "RetentionPeriod": retention(30),
            "ResourceTags": [],
            "ExcludeResourceTags": [],
        }),
        UNLOCK_DELAY_MIN,
    ));

    for (case, body, unlock_delay) in rows {
        let configuration = lock_configuration(unlock_delay);
        // Door one: the lock arrives with the create.
        let mut at_create = body.clone();
        at_create["LockConfiguration"] = configuration.clone();
        let (status, created) = create_rule(&router, at_create).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRule with a lock configuration expected HTTP 201, \
             got {status} / {created:?}"
        );
        assert_eq!(
            created["LockState"].as_str(),
            Some("locked"),
            "case {case:?}: expected LockState=locked, got {:?}",
            created["LockState"]
        );
        let identifier = created["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier"))
            .to_string();
        // ROUND TRIP: the lock is persisted, and it is a real lock.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            Some("locked"),
            "case {case:?}: expected a persisted LockState=locked, got {:?}",
            fetched["LockState"]
        );
        assert_eq!(
            fetched["LockConfiguration"], configuration,
            "case {case:?}: expected the persisted LockConfiguration {configuration:?}, got {:?}",
            fetched["LockConfiguration"]
        );
        let (status, refused) = delete_rule(&router, &identifier).await;
        assert_eq!(
            status, 409,
            "case {case:?}: DeleteRule on the locked rule expected HTTP 409, \
             got {status} / {refused:?}"
        );

        // Door two: the same rule created without a lock is locked by `LockRule`.
        let (status, plain) = create_rule(&router, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRule without a lock expected HTTP 201, got {status} / {plain:?}"
        );
        let identifier = plain["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier"))
            .to_string();
        let (status, locked) =
            lock_rule(&router, &identifier, json!({ "LockConfiguration": configuration })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: LockRule expected HTTP 200, got {status} / {locked:?}"
        );
        assert_eq!(
            locked["LockState"].as_str(),
            Some("locked"),
            "case {case:?}: expected LockState=locked from LockRule, got {:?}",
            locked["LockState"]
        );
    }
}

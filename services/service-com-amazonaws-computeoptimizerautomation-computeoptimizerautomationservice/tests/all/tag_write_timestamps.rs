// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `lastUpdatedTimestamp` a tag write leaves on an
//! automation rule.
//!
//! The rule under test: a call that changes a stored rule stamps that rule's
//! `lastUpdatedTimestamp` with the instant the store applied the change — the
//! member means "The timestamp when the automation rule was last updated"
//! (`smithy_query(query="doc",shape="AutomationRule",member="lastUpdatedTimestamp")`)
//! — while a call that leaves the rule exactly as it stood moves it nowhere.
//! Adding and removing tags are both changes of the rule the `resourceArn`
//! names, so each stamps it, and neither touches `ruleRevision` or
//! `createdTimestamp`.
//!
//! Every case drives the service over the wire through the same router the
//! emulator binary mounts (the harness lives in
//! [`super::automation_rule_lifecycle`]) and reads the rule back through two
//! independent readers, so a timestamp that is not the one the store holds
//! fails here.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use super::automation_rule_lifecycle::{call_ok, emulator, member, minimal_rule, string_member};

/// A pause between the create and the tag write, so "the timestamp moved" is a
/// statement about the write rather than about the resolution of the clock the
/// two calls read.
const CLOCK_MARGIN: Duration = Duration::from_millis(25);

/// One row of the table: a rule created carrying `created_with`, one tag write,
/// and what the stored rule looks like afterwards.
struct Case {
    name: &'static str,
    /// The `tags` the rule is created with; an empty list creates it untagged.
    created_with: Value,
    /// `TagResource` or `UntagResource`.
    operation: &'static str,
    /// The `tags` (`TagResource`) or `tagKeys` (`UntagResource`) the write names.
    payload: Value,
    /// The tag list the readers must report after the write.
    expected_tags: Value,
    /// Whether the write changes the stored tag list, which is what decides
    /// whether the rule was updated at all.
    changes_the_rule: bool,
}

/// The rows differ along what the rule generalises over: which operation writes,
/// how many tags the rule starts with, how many the write names, whether the
/// write actually changes the stored list (a key already carried with the same
/// value, a key the rule does not carry, an untagged rule), and the lengths and
/// character classes of the keys and values written.
fn cases() -> Vec<Case> {
    // Lengths and character classes are the model's:
    // `smithy_query(query="shape",shape="Tag,TagKeyList")` — key: length 1..128,
    // pattern ^[\w\s\.\-\:\/\=\+\@]+$; value: length ..256, pattern
    // ^[\w\s\.\-\:\/\=\+\@]*$.
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);

    vec![
        Case {
            name: "untag-the-only-key",
            created_with: json!([{"key": "owner", "value": "payments"}]),
            operation: "UntagResource",
            payload: json!(["owner"]),
            expected_tags: json!([]),
            changes_the_rule: true,
        },
        Case {
            name: "untag-one-key-of-three",
            created_with: json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"},
                {"key": "owner", "value": "payments"}
            ]),
            operation: "UntagResource",
            payload: json!(["env"]),
            expected_tags: json!([
                {"key": "team", "value": "platform"},
                {"key": "owner", "value": "payments"}
            ]),
            changes_the_rule: true,
        },
        Case {
            name: "untag-every-key-at-once",
            created_with: json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"}
            ]),
            operation: "UntagResource",
            payload: json!(["team", "env"]),
            expected_tags: json!([]),
            changes_the_rule: true,
        },
        Case {
            name: "untag-mixing-a-carried-key-and-an-absent-one",
            created_with: json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"}
            ]),
            operation: "UntagResource",
            payload: json!(["env", "never-set"]),
            expected_tags: json!([{"key": "team", "value": "platform"}]),
            changes_the_rule: true,
        },
        Case {
            name: "untag-the-longest-modelled-key",
            created_with: json!([{"key": long_key.clone(), "value": long_value.clone()}]),
            operation: "UntagResource",
            payload: json!([long_key.clone()]),
            expected_tags: json!([]),
            changes_the_rule: true,
        },
        Case {
            name: "untag-a-key-the-rule-does-not-carry",
            created_with: json!([{"key": "team", "value": "platform"}]),
            operation: "UntagResource",
            payload: json!(["never-set"]),
            expected_tags: json!([{"key": "team", "value": "platform"}]),
            changes_the_rule: false,
        },
        Case {
            name: "untag-an-untagged-rule",
            created_with: json!([]),
            operation: "UntagResource",
            payload: json!(["never-set"]),
            expected_tags: json!([]),
            changes_the_rule: false,
        },
        Case {
            name: "tag-an-untagged-rule",
            created_with: json!([]),
            operation: "TagResource",
            payload: json!([{"key": "team", "value": "platform"}]),
            expected_tags: json!([{"key": "team", "value": "platform"}]),
            changes_the_rule: true,
        },
        Case {
            name: "tag-appends-a-new-key",
            created_with: json!([{"key": "team", "value": "platform"}]),
            operation: "TagResource",
            payload: json!([{"key": "env", "value": "prod"}]),
            expected_tags: json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"}
            ]),
            changes_the_rule: true,
        },
        Case {
            name: "tag-several-keys-at-once",
            created_with: json!([{"key": "team", "value": "platform"}]),
            operation: "TagResource",
            payload: json!([
                {"key": "env", "value": "prod"},
                {"key": "owner", "value": "payments"}
            ]),
            expected_tags: json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"},
                {"key": "owner", "value": "payments"}
            ]),
            changes_the_rule: true,
        },
        Case {
            name: "tag-a-carried-key-with-another-value",
            created_with: json!([{"key": "team", "value": "platform"}]),
            operation: "TagResource",
            payload: json!([{"key": "team", "value": "payments"}]),
            expected_tags: json!([{"key": "team", "value": "payments"}]),
            changes_the_rule: true,
        },
        Case {
            name: "tag-the-longest-modelled-key-and-value",
            created_with: json!([]),
            operation: "TagResource",
            payload: json!([{"key": long_key.clone(), "value": long_value.clone()}]),
            expected_tags: json!([{"key": long_key.clone(), "value": long_value.clone()}]),
            changes_the_rule: true,
        },
        Case {
            name: "tag-a-carried-key-with-the-value-it-already-has",
            created_with: json!([{"key": "team", "value": "platform"}]),
            operation: "TagResource",
            payload: json!([{"key": "team", "value": "platform"}]),
            expected_tags: json!([{"key": "team", "value": "platform"}]),
            changes_the_rule: false,
        },
        Case {
            name: "tag-the-whole-carried-list-again",
            created_with: json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"}
            ]),
            operation: "TagResource",
            payload: json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"}
            ]),
            expected_tags: json!([
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "prod"}
            ]),
            changes_the_rule: false,
        },
    ]
}

/// Create a rule carrying `tags` and answer `(ruleArn, ruleRevision)`.
async fn create_rule(
    router: &MultiServiceRouter,
    name: &str,
    tags: &Value,
    case: &str,
) -> (String, i64) {
    let mut request = minimal_rule(name);
    // A row whose rule starts untagged sends no `tags` member at all, which is
    // the state a plain create leaves.
    if !tags.as_array().is_some_and(Vec::is_empty) {
        request["tags"] = tags.clone();
    }
    let created = call_ok(router, "CreateAutomationRule", &request, case).await;
    (
        string_member(&created, "ruleArn"),
        member(&created, "ruleRevision")
            .as_i64()
            .expect("the created rule carries a revision"),
    )
}

/// The rule `GetAutomationRule` reports for `rule_arn`.
async fn read_rule(router: &MultiServiceRouter, rule_arn: &str, case: &str) -> Value {
    call_ok(
        router,
        "GetAutomationRule",
        &json!({"ruleArn": rule_arn}),
        case,
    )
    .await
}

/// The `AutomationRule` summary `ListAutomationRules` reports for `rule_arn`.
async fn listed_rule(router: &MultiServiceRouter, rule_arn: &str, case: &str) -> Value {
    let listed = call_ok(
        router,
        "ListAutomationRules",
        &json!({"maxResults": 100}),
        case,
    )
    .await;
    let found = member(&listed, "automationRules")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|rule| string_member(rule, "ruleArn") == rule_arn);
    assert!(
        found.is_some(),
        "case {case:?}: the rule {rule_arn} must be listed, got {listed}"
    );
    found.unwrap_or(Value::Null)
}

/// The member, required to be present and an epoch-seconds Timestamp.
fn timestamp(body: &Value, name: &str, case: &str) -> f64 {
    let value = member(body, name).as_f64();
    assert!(
        value.is_some_and(|seconds| seconds > 0.0),
        "case {case:?}: {name} must be present and an epoch-seconds Timestamp, got {body}"
    );
    value.unwrap_or_default()
}

/// A tag write that changes the stored rule stamps its `lastUpdatedTimestamp`;
/// one that leaves the rule as it stood leaves the timestamp alone. Neither
/// moves `ruleRevision` or `createdTimestamp`, and both readers of a stored rule
/// report the same instant.
#[tokio::test]
async fn a_tag_write_stamps_last_updated_timestamp_when_it_changes_the_rule() {
    let router = emulator();

    for (index, case) in cases().iter().enumerate() {
        let name = case.name;
        let (rule_arn, revision) = create_rule(
            &router,
            &format!("tag-stamp-rule-{index}"),
            &case.created_with,
            name,
        )
        .await;

        let before = read_rule(&router, &rule_arn, name).await;
        let created_at = timestamp(&before, "createdTimestamp", name);
        let before_write = timestamp(&before, "lastUpdatedTimestamp", name);
        assert!(
            (before_write - created_at).abs() < f64::EPSILON,
            "case {name:?}: a rule never updated was last changed when it was created, got \
             {before_write} against {created_at}"
        );

        sleep(CLOCK_MARGIN).await;

        let member_name = if case.operation == "TagResource" {
            "tags"
        } else {
            "tagKeys"
        };
        call_ok(
            &router,
            case.operation,
            &json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision,
                (member_name): case.payload.clone(),
            }),
            name,
        )
        .await;

        let after = read_rule(&router, &rule_arn, name).await;
        assert_eq!(
            member(&after, "tags"),
            &case.expected_tags,
            "case {name:?}: expected the stored tags {}, got {after}",
            case.expected_tags
        );
        assert_eq!(
            member(&after, "ruleRevision").as_i64(),
            Some(revision),
            "case {name:?}: a tag write is not a rule revision, got {after}"
        );
        let stamped = timestamp(&after, "lastUpdatedTimestamp", name);
        if case.changes_the_rule {
            assert!(
                stamped > before_write,
                "case {name:?}: a write that changes the rule stamps the instant it was \
                 applied, expected a lastUpdatedTimestamp later than {before_write}, got \
                 {stamped}"
            );
        } else {
            assert!(
                (stamped - before_write).abs() < f64::EPSILON,
                "case {name:?}: a write that changes nothing updated nothing, expected \
                 lastUpdatedTimestamp to stay {before_write}, got {stamped}"
            );
        }
        let created_after = timestamp(&after, "createdTimestamp", name);
        assert!(
            (created_after - created_at).abs() < f64::EPSILON,
            "case {name:?}: a tag write never moves createdTimestamp, expected {created_at}, \
             got {created_after}"
        );

        // The second reader of the same stored rule agrees, so the instant is
        // the store's and not one reader's invention.
        let summary = listed_rule(&router, &rule_arn, name).await;
        let listed_stamp = timestamp(&summary, "lastUpdatedTimestamp", name);
        assert!(
            (listed_stamp - stamped).abs() < f64::EPSILON,
            "case {name:?}: the listed rule must carry the lastUpdatedTimestamp Get reports, \
             expected {stamped}, got {listed_stamp}"
        );
    }
}

/// A repeat of the same tag write under the same `clientToken` re-applies
/// nothing — the idempotent replay answers as the first call did — so it leaves
/// the timestamp that first call stamped.
#[tokio::test]
async fn a_replayed_tag_write_keeps_the_timestamp_the_first_call_stamped() {
    let router = emulator();

    // (case, operation, the member the write carries)
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "tag-replay",
            "TagResource",
            json!({"tags": [{"key": "env", "value": "prod"}]}),
        ),
        ("untag-replay", "UntagResource", json!({"tagKeys": ["owner"]})),
    ];

    for (index, (case, operation, payload)) in rows.iter().enumerate() {
        let (rule_arn, revision) = create_rule(
            &router,
            &format!("tag-replay-rule-{index}"),
            &json!([{"key": "owner", "value": "payments"}]),
            case,
        )
        .await;

        let mut request = json!({
            "resourceArn": &rule_arn,
            "ruleRevision": revision,
            "clientToken": format!("tag-replay-token-{index}"),
        });
        for (name, value) in payload.as_object().expect("the row is an object") {
            request[name.as_str()] = value.clone();
        }

        call_ok(&router, operation, &request, case).await;
        let first = timestamp(
            &read_rule(&router, &rule_arn, case).await,
            "lastUpdatedTimestamp",
            case,
        );

        sleep(CLOCK_MARGIN).await;

        call_ok(&router, operation, &request, case).await;
        let replayed = timestamp(
            &read_rule(&router, &rule_arn, case).await,
            "lastUpdatedTimestamp",
            case,
        );
        assert!(
            (replayed - first).abs() < f64::EPSILON,
            "case {case:?}: a replayed write applies nothing, so it moves no timestamp, \
             expected {first}, got {replayed}"
        );
    }
}

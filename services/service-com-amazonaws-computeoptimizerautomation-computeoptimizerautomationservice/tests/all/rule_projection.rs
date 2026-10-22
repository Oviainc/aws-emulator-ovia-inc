// SPDX-License-Identifier: CC0-1.0

//! Every read path projects a stored automation rule IN FULL.
//!
//! A rule's `description`, `priority` and `lastUpdatedTimestamp` are members of
//! the rule itself, not of the request that created it: a create that named none
//! of them still yields a rule that reports all three, because the store holds a
//! value for each one from the moment the rule exists. These cases drive
//! `CreateAutomationRule`, `GetAutomationRule`, `ListAutomationRules` and
//! `UpdateAutomationRule` over a table of creates that differ in WHICH optional
//! members they carry and in the values they carry, and assert each response
//! carries the member the model declares on it.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    DEFAULT_RULE_DESCRIPTION, DEFAULT_RULE_PRIORITY,
};

use crate::automation_rule_lifecycle::{call_ok, emulator, member, minimal_rule, string_member};

/// The member, required to be present and a string.
fn present_string(body: &Value, name: &str, case: &str) -> String {
    let value = body.get(name);
    assert!(
        value.is_some_and(Value::is_string),
        "case {case:?}: {name} must be present and a string, got {body}"
    );
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The member, required to be present and an epoch-seconds Timestamp.
fn present_timestamp(body: &Value, name: &str, case: &str) -> f64 {
    let value = member(body, name).as_f64();
    assert!(
        value.is_some_and(|seconds| seconds > 0.0),
        "case {case:?}: {name} must be present and an epoch-seconds Timestamp, got {body}"
    );
    value.unwrap_or_default()
}

/// The rule with `rule_id` in a `ListAutomationRules` page.
fn listed_rule(listed: &Value, rule_id: &str, case: &str) -> Value {
    let rules = member(listed, "automationRules")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let found = rules
        .into_iter()
        .find(|rule| string_member(rule, "ruleId") == rule_id);
    assert!(
        found.is_some(),
        "case {case:?}: the created rule {rule_id} must be listed, got {listed}"
    );
    found.unwrap_or(Value::Null)
}

/// `priority` is "a string representation of a decimal number between 0 and 1
/// (having up to 30 digits after the decimal point)"
/// (`smithy_query(query="doc",shape="AutomationRule",member="priority")`).
fn is_documented_priority(priority: &str) -> bool {
    priority
        .parse::<f64>()
        .is_ok_and(|value| (0.0..=1.0).contains(&value))
}

/// A create that omits an optional rule member still produces a rule whose
/// `description`, `priority` and `lastUpdatedTimestamp` every read path reports:
/// the omitted description is the empty string, the omitted priority is the
/// service's default precedence, and a rule never updated was last changed when
/// it was created.
#[tokio::test]
async fn every_read_path_projects_a_rules_description_priority_and_last_update() {
    let router = emulator();

    // (case, the optional members the create carries, expected description,
    // expected priority). The rows differ along WHICH optional members are sent
    // and along the values they carry: `description` at nothing, a sentence, the
    // empty string and the model's 1024-character maximum
    // (`RuleDescription`, len 0..1024), and `priority` at both documented bounds,
    // in between, and with the documented maximum of 30 fraction digits.
    let max_length_description = "d".repeat(1024);
    let smallest_priority = format!("0.{}1", "0".repeat(29));
    let cases: Vec<(&str, Value, String, String)> = vec![
        (
            "no-optional-members",
            json!({}),
            DEFAULT_RULE_DESCRIPTION.to_string(),
            DEFAULT_RULE_PRIORITY.to_string(),
        ),
        (
            "description-only",
            json!({"description": "Upgrades gp2 volumes overnight."}),
            "Upgrades gp2 volumes overnight.".to_string(),
            DEFAULT_RULE_PRIORITY.to_string(),
        ),
        (
            "priority-only",
            json!({"priority": "0.25"}),
            DEFAULT_RULE_DESCRIPTION.to_string(),
            "0.25".to_string(),
        ),
        (
            "description-and-priority",
            json!({"description": "both members", "priority": "0.5"}),
            "both members".to_string(),
            "0.5".to_string(),
        ),
        (
            "explicitly-empty-description",
            json!({"description": ""}),
            String::new(),
            DEFAULT_RULE_PRIORITY.to_string(),
        ),
        (
            "description-at-its-maximum-length",
            json!({"description": max_length_description.clone()}),
            max_length_description.clone(),
            DEFAULT_RULE_PRIORITY.to_string(),
        ),
        (
            "priority-at-the-bottom-of-its-range",
            json!({"priority": "0"}),
            DEFAULT_RULE_DESCRIPTION.to_string(),
            "0".to_string(),
        ),
        (
            "priority-at-the-top-of-its-range",
            json!({"priority": "1"}),
            DEFAULT_RULE_DESCRIPTION.to_string(),
            "1".to_string(),
        ),
        (
            "priority-with-thirty-fraction-digits",
            json!({"priority": smallest_priority.clone()}),
            DEFAULT_RULE_DESCRIPTION.to_string(),
            smallest_priority.clone(),
        ),
    ];

    for (index, (case, optional, expected_description, expected_priority)) in
        cases.iter().enumerate()
    {
        let mut request = minimal_rule(&format!("projection-rule-{index}"));
        for (name, value) in optional.as_object().expect("the row is an object") {
            request[name.as_str()] = value.clone();
        }

        // Create reports both members the shape declares on its response.
        let created = call_ok(&router, "CreateAutomationRule", &request, case).await;
        let rule_id = string_member(&created, "ruleId");
        let rule_arn = string_member(&created, "ruleArn");
        assert_eq!(
            &present_string(&created, "description", case),
            expected_description,
            "case {case:?}: create must report the stored description, got {created}"
        );
        assert_eq!(
            &present_string(&created, "priority", case),
            expected_priority,
            "case {case:?}: create must report the stored priority, got {created}"
        );

        // Get reports all three, and a rule never updated was last changed when
        // it was created.
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_eq!(
            &present_string(&fetched, "description", case),
            expected_description,
            "case {case:?}: Get must report the stored description, got {fetched}"
        );
        assert_eq!(
            &present_string(&fetched, "priority", case),
            expected_priority,
            "case {case:?}: Get must report the stored priority, got {fetched}"
        );
        let created_at = present_timestamp(&fetched, "createdTimestamp", case);
        let last_updated = present_timestamp(&fetched, "lastUpdatedTimestamp", case);
        assert!(
            (last_updated - created_at).abs() < f64::EPSILON,
            "case {case:?}: a rule never updated was last changed when it was created, got \
             {last_updated} against {created_at}"
        );

        // List projects exactly what Get reports — the AutomationRule summary
        // carries description, priority and lastUpdatedTimestamp too.
        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": 100}),
            case,
        )
        .await;
        let summary = listed_rule(&listed, &rule_id, case);
        assert_eq!(
            &present_string(&summary, "description", case),
            expected_description,
            "case {case:?}: the listed rule must carry the stored description, got {summary}"
        );
        assert_eq!(
            &present_string(&summary, "priority", case),
            expected_priority,
            "case {case:?}: the listed rule must carry the stored priority, got {summary}"
        );
        assert!(
            (present_timestamp(&summary, "lastUpdatedTimestamp", case) - last_updated).abs()
                < f64::EPSILON,
            "case {case:?}: the listed rule must carry the same lastUpdatedTimestamp Get \
             reports ({last_updated}), got {summary}"
        );
        assert!(
            is_documented_priority(expected_priority),
            "case {case:?}: a projected priority is a decimal between 0 and 1, got \
             {expected_priority:?}"
        );
    }
}

/// An update changes only what it carries: the members it leaves alone keep the
/// values the create stored, and `lastUpdatedTimestamp` moves to the update's own
/// instant on every read path — so a client can detect the change.
#[tokio::test]
async fn an_update_moves_the_last_update_time_and_leaves_the_other_members_projected() {
    let router = emulator();

    // (case, the member the update carries) — one row per member whose absence
    // must leave the stored value standing.
    let cases: Vec<(&str, Value)> = vec![
        ("status-only", json!({"status": "Inactive"})),
        ("name-only", json!({"name": "renamed-rule"})),
        (
            "recommended-action-types-only",
            json!({"recommendedActionTypes": ["SnapshotAndDeleteUnattachedEbsVolume"]}),
        ),
    ];

    for (index, (case, change)) in cases.iter().enumerate() {
        let mut request = minimal_rule(&format!("untouched-rule-{index}"));
        request["description"] = json!("kept across the update");
        request["priority"] = json!("0.75");
        let created = call_ok(&router, "CreateAutomationRule", &request, case).await;
        let rule_id = string_member(&created, "ruleId");
        let rule_arn = string_member(&created, "ruleArn");
        let created_at = present_timestamp(&created, "createdTimestamp", case);

        let mut update = change.clone();
        update["ruleArn"] = json!(&rule_arn);
        update["ruleRevision"] = member(&created, "ruleRevision").clone();
        let updated = call_ok(&router, "UpdateAutomationRule", &update, case).await;
        let stamped = present_timestamp(&updated, "lastUpdatedTimestamp", case);
        assert!(
            stamped >= created_at,
            "case {case:?}: an update's lastUpdatedTimestamp is no earlier than the creation \
             time {created_at}, got {updated}"
        );
        assert_eq!(
            present_string(&updated, "description", case),
            "kept across the update",
            "case {case:?}: an update that carries no description keeps the stored one, got \
             {updated}"
        );
        assert_eq!(
            present_string(&updated, "priority", case),
            "0.75",
            "case {case:?}: an update that carries no priority keeps the stored one, got \
             {updated}"
        );

        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": 100}),
            case,
        )
        .await;
        let summary = listed_rule(&listed, &rule_id, case);
        assert_eq!(
            present_string(&summary, "description", case),
            "kept across the update",
            "case {case:?}: the listed rule keeps the stored description, got {summary}"
        );
        assert_eq!(
            present_string(&summary, "priority", case),
            "0.75",
            "case {case:?}: the listed rule keeps the stored priority, got {summary}"
        );
        assert!(
            (present_timestamp(&summary, "lastUpdatedTimestamp", case) - stamped).abs()
                < f64::EPSILON,
            "case {case:?}: the listed rule reports the update's own timestamp ({stamped}), got \
             {summary}"
        );
    }
}

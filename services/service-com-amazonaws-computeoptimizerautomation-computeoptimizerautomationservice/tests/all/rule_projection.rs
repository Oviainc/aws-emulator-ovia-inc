// SPDX-License-Identifier: CC0-1.0

//! Every read path projects a stored automation rule as it is STORED.
//!
//! A rule holds only the values its create (or a later update) supplied. The
//! optional members of a rule — `description` and `priority` — have no default
//! in the model or in the operation docs, so a rule whose create named neither
//! holds neither, and `CreateAutomationRule`, `GetAutomationRule`,
//! `ListAutomationRules` and `UpdateAutomationRule` all leave them out of their
//! responses instead of reporting a value of the emulator's own. What the create
//! DID supply comes back verbatim, and `lastUpdatedTimestamp` — which is known
//! for every rule that exists, because a rule never updated was last changed
//! when it was created — is reported always.
//!
//! These cases drive the four operations over a table of creates that differ in
//! WHICH optional members they carry and in the values they carry.

use serde_json::{json, Value};

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

/// Assert `name` is projected exactly as the store holds it: the string the
/// service holds a value for, and NOTHING AT ALL when it holds none.
fn assert_projected(body: &Value, name: &str, expected: Option<&String>, case: &str) {
    match expected {
        Some(expected) => assert_eq!(
            &present_string(body, name, case),
            expected,
            "case {case:?}: {name} must be reported as stored, got {body}"
        ),
        None => assert!(
            body.get(name).is_none(),
            "case {case:?}: the service holds no {name} for this rule, so the response must \
             carry no {name} member, got {:?} in {body}",
            body.get(name)
        ),
    }
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

/// The member, required to be present and an epoch-seconds Timestamp.
fn present_timestamp(body: &Value, name: &str, case: &str) -> f64 {
    let value = member(body, name).as_f64();
    assert!(
        value.is_some_and(|seconds| seconds > 0.0),
        "case {case:?}: {name} must be present and an epoch-seconds Timestamp, got {body}"
    );
    value.unwrap_or_default()
}

/// (case, the optional members the create carries, expected `description`,
/// expected `priority`) — `None` meaning the member must be ABSENT from every
/// response.
///
/// The rows differ along WHICH optional members are sent and along the values
/// they carry: `description` at nothing, a sentence, the explicitly empty string
/// and the model's 1024-character maximum (`RuleDescription`, len 0..1024), and
/// `priority` at nothing, at both documented bounds, in between, and with the
/// documented maximum of 30 fraction digits
/// (`smithy_query(query="doc",shape="AutomationRule",member="priority")`).
fn projection_cases() -> Vec<(&'static str, Value, Option<String>, Option<String>)> {
    let max_length_description = "d".repeat(1024);
    let smallest_priority = format!("0.{}1", "0".repeat(29));
    vec![
        ("no-optional-members", json!({}), None, None),
        (
            "description-only",
            json!({"description": "Upgrades gp2 volumes overnight."}),
            Some("Upgrades gp2 volumes overnight.".to_string()),
            None,
        ),
        (
            "priority-only",
            json!({"priority": "0.25"}),
            None,
            Some("0.25".to_string()),
        ),
        (
            "description-and-priority",
            json!({"description": "both members", "priority": "0.5"}),
            Some("both members".to_string()),
            Some("0.5".to_string()),
        ),
        (
            "explicitly-empty-description",
            json!({"description": ""}),
            Some(String::new()),
            None,
        ),
        (
            "description-at-its-maximum-length",
            json!({"description": max_length_description.clone()}),
            Some(max_length_description),
            None,
        ),
        (
            "priority-at-the-bottom-of-its-range",
            json!({"priority": "0"}),
            None,
            Some("0".to_string()),
        ),
        (
            "priority-at-the-top-of-its-range",
            json!({"priority": "1"}),
            None,
            Some("1".to_string()),
        ),
        (
            "priority-with-thirty-fraction-digits",
            json!({"priority": smallest_priority.clone()}),
            None,
            Some(smallest_priority),
        ),
    ]
}

/// A create request carrying the row's optional members.
fn request_for(index: usize, optional: &Value) -> Value {
    let mut request = minimal_rule(&format!("projection-rule-{index}"));
    for (name, value) in optional.as_object().expect("the row is an object") {
        request[name.as_str()] = value.clone();
    }
    request
}

/// Every read path reports the optional rule members the create supplied, and
/// omits the ones it did not: no emulator-chosen description or precedence is
/// invented for a rule whose create named none.
#[tokio::test]
async fn every_read_path_projects_only_the_optional_rule_members_the_create_supplied() {
    let router = emulator();

    for (index, (case, optional, expected_description, expected_priority)) in
        projection_cases().iter().enumerate()
    {
        let request = request_for(index, optional);

        // Create reports what it stored.
        let created = call_ok(&router, "CreateAutomationRule", &request, case).await;
        let rule_id = string_member(&created, "ruleId");
        let rule_arn = string_member(&created, "ruleArn");
        assert_projected(&created, "description", expected_description.as_ref(), case);
        assert_projected(&created, "priority", expected_priority.as_ref(), case);

        // Get reports exactly the same, and a rule never updated was last
        // changed when it was created.
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_projected(&fetched, "description", expected_description.as_ref(), case);
        assert_projected(&fetched, "priority", expected_priority.as_ref(), case);
        let created_at = present_timestamp(&fetched, "createdTimestamp", case);
        let last_updated = present_timestamp(&fetched, "lastUpdatedTimestamp", case);
        assert!(
            (last_updated - created_at).abs() < f64::EPSILON,
            "case {case:?}: a rule never updated was last changed when it was created, got \
             {last_updated} against {created_at}"
        );

        // The AutomationRule summary carries the same two optional members, so
        // List projects them the same way.
        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": 100}),
            case,
        )
        .await;
        let summary = listed_rule(&listed, &rule_id, case);
        assert_projected(&summary, "description", expected_description.as_ref(), case);
        assert_projected(&summary, "priority", expected_priority.as_ref(), case);
        assert!(
            (present_timestamp(&summary, "lastUpdatedTimestamp", case) - last_updated).abs()
                < f64::EPSILON,
            "case {case:?}: the listed rule must carry the same lastUpdatedTimestamp Get \
             reports ({last_updated}), got {summary}"
        );
    }
}

/// An update that carries neither optional member leaves the rule exactly as the
/// create left it — so a rule created without a description or a precedence
/// still reports none after it has been updated, on every read path.
#[tokio::test]
async fn an_update_that_names_neither_optional_member_keeps_the_rules_projection() {
    let router = emulator();

    for (index, (case, optional, expected_description, expected_priority)) in
        projection_cases().iter().enumerate()
    {
        let request = request_for(1_000 + index, optional);
        let created = call_ok(&router, "CreateAutomationRule", &request, case).await;
        let rule_arn = string_member(&created, "ruleArn");
        let rule_id = string_member(&created, "ruleId");

        let updated = call_ok(
            &router,
            "UpdateAutomationRule",
            &json!({
                "ruleArn": &rule_arn,
                "ruleRevision": member(&created, "ruleRevision").clone(),
                "status": "Inactive",
            }),
            case,
        )
        .await;
        assert_projected(&updated, "description", expected_description.as_ref(), case);
        assert_projected(&updated, "priority", expected_priority.as_ref(), case);

        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_projected(&fetched, "description", expected_description.as_ref(), case);
        assert_projected(&fetched, "priority", expected_priority.as_ref(), case);

        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": 100}),
            case,
        )
        .await;
        let summary = listed_rule(&listed, &rule_id, case);
        assert_projected(&summary, "description", expected_description.as_ref(), case);
        assert_projected(&summary, "priority", expected_priority.as_ref(), case);
    }
}

/// An update is what gives a rule created without one a description or a
/// precedence: the member it supplies becomes the stored value and every read
/// path reports it, while the one it leaves alone stays as it was.
#[tokio::test]
async fn an_update_sets_an_optional_member_a_rule_was_created_without() {
    let router = emulator();

    // (case, the member the update carries, expected description after it,
    // expected priority after it) — each row starts from a rule created with no
    // optional members at all.
    let long_description = "u".repeat(1024);
    let cases: Vec<(&str, Value, Option<String>, Option<String>)> = vec![
        (
            "update-adds-a-description",
            json!({"description": "described by the update"}),
            Some("described by the update".to_string()),
            None,
        ),
        (
            "update-adds-an-empty-description",
            json!({"description": ""}),
            Some(String::new()),
            None,
        ),
        (
            "update-adds-a-maximum-length-description",
            json!({"description": long_description.clone()}),
            Some(long_description),
            None,
        ),
        (
            "update-adds-a-priority",
            json!({"priority": "0.125"}),
            None,
            Some("0.125".to_string()),
        ),
        (
            "update-adds-the-top-priority-value",
            json!({"priority": "1"}),
            None,
            Some("1".to_string()),
        ),
        (
            "update-adds-both",
            json!({"description": "both at once", "priority": "0"}),
            Some("both at once".to_string()),
            Some("0".to_string()),
        ),
    ];

    for (index, (case, change, expected_description, expected_priority)) in
        cases.iter().enumerate()
    {
        let created = call_ok(
            &router,
            "CreateAutomationRule",
            &minimal_rule(&format!("late-set-rule-{index}")),
            case,
        )
        .await;
        let rule_arn = string_member(&created, "ruleArn");
        let rule_id = string_member(&created, "ruleId");
        assert_projected(&created, "description", None, case);
        assert_projected(&created, "priority", None, case);

        let mut update = change.clone();
        update["ruleArn"] = json!(&rule_arn);
        update["ruleRevision"] = member(&created, "ruleRevision").clone();
        let updated = call_ok(&router, "UpdateAutomationRule", &update, case).await;
        assert_projected(&updated, "description", expected_description.as_ref(), case);
        assert_projected(&updated, "priority", expected_priority.as_ref(), case);

        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_projected(&fetched, "description", expected_description.as_ref(), case);
        assert_projected(&fetched, "priority", expected_priority.as_ref(), case);

        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": 100}),
            case,
        )
        .await;
        let summary = listed_rule(&listed, &rule_id, case);
        assert_projected(&summary, "description", expected_description.as_ref(), case);
        assert_projected(&summary, "priority", expected_priority.as_ref(), case);
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

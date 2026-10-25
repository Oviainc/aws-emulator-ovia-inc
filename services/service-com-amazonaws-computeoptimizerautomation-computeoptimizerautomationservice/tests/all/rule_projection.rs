// SPDX-License-Identifier: CC0-1.0

//! Every read path projects a stored automation rule, and the rule members the
//! service itself materialises are always reported.
//!
//! `description` and `priority` are optional in a rule REQUEST but the service
//! reports both for every rule it HOLDS, so a create that named neither still
//! leaves the rule with a value for each: the empty description for a rule
//! nobody described, and the bottom of the precedence order for a rule nobody
//! prioritised ("the rule with the lowest priority value (highest priority)",
//! `smithy_query(query="doc",shape="AutomationRule",member="priority")` — so a
//! rule nobody prioritised takes the TOP of the documented 0..1 range and
//! outranks none that named one). `CreateAutomationRule`, `GetAutomationRule`,
//! `ListAutomationRules` and `UpdateAutomationRule` all report both members from
//! the stored rule, so a client that walks a listing never sees them missing.
//! What the create DID supply comes back verbatim, and `lastUpdatedTimestamp` —
//! known for every rule that exists, because a rule never updated was last
//! changed when it was created — is reported always.
//!
//! These cases drive the four operations over a table of creates that differ in
//! WHICH optional members they carry and in the values they carry.

use serde_json::{json, Value};

use crate::automation_rule_lifecycle::{call_ok, emulator, member, minimal_rule, string_member};

/// What a response must report for a rule member a create may or may not have
/// named.
#[derive(Debug, Clone)]
enum Expected {
    /// Exactly the string the request supplied.
    AsSent(String),
    /// The description the service holds for a rule nobody described: present,
    /// and empty.
    EmptyDescription,
    /// The precedence the service holds for a rule nobody prioritised: present,
    /// a decimal inside the documented 0..1 range, and at the bottom of the
    /// precedence order, so it outranks no rule that named a priority.
    BottomOfPrecedence,
}

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

/// The member as the decimal number `priority` is documented to be.
fn priority_number(body: &Value, case: &str) -> f64 {
    let reported = present_string(body, "priority", case);
    let parsed = reported.parse::<f64>();
    assert!(
        parsed.as_ref().is_ok_and(|value| (0.0..=1.0).contains(value)),
        "case {case:?}: priority must be a decimal number between 0 and 1, got {reported:?} in \
         {body}"
    );
    parsed.unwrap_or_default()
}

/// Assert `name` is projected as the service holds it: never missing, and
/// carrying the value the request supplied or the one the service materialised
/// for a request that supplied none.
fn assert_projected(body: &Value, name: &str, expected: &Expected, case: &str) {
    match expected {
        Expected::AsSent(expected) => assert_eq!(
            &present_string(body, name, case),
            expected,
            "case {case:?}: {name} must be reported as stored, got {body}"
        ),
        Expected::EmptyDescription => assert_eq!(
            present_string(body, name, case),
            "",
            "case {case:?}: a rule nobody described reports the empty {name}, got {body}"
        ),
        Expected::BottomOfPrecedence => {
            let reported = priority_number(body, case);
            assert!(
                (reported - 1.0).abs() < f64::EPSILON,
                "case {case:?}: a rule nobody prioritised sits at the bottom of the precedence \
                 order (the top of the 0..1 range), got {reported} in {body}"
            );
        }
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
/// expected `priority`).
///
/// The rows differ along WHICH optional members are sent and along the values
/// they carry: `description` at nothing, a sentence, the explicitly empty string
/// and the model's 1024-character maximum (`RuleDescription`, len 0..1024), and
/// `priority` at nothing, at both documented bounds, in between, and with the
/// documented maximum of 30 fraction digits
/// (`smithy_query(query="doc",shape="AutomationRule",member="priority")`).
fn projection_cases() -> Vec<(&'static str, Value, Expected, Expected)> {
    let max_length_description = "d".repeat(1024);
    let smallest_priority = format!("0.{}1", "0".repeat(29));
    vec![
        (
            "no-optional-members",
            json!({}),
            Expected::EmptyDescription,
            Expected::BottomOfPrecedence,
        ),
        (
            "description-only",
            json!({"description": "Upgrades gp2 volumes overnight."}),
            Expected::AsSent("Upgrades gp2 volumes overnight.".to_string()),
            Expected::BottomOfPrecedence,
        ),
        (
            "priority-only",
            json!({"priority": "0.25"}),
            Expected::EmptyDescription,
            Expected::AsSent("0.25".to_string()),
        ),
        (
            "description-and-priority",
            json!({"description": "both members", "priority": "0.5"}),
            Expected::AsSent("both members".to_string()),
            Expected::AsSent("0.5".to_string()),
        ),
        (
            "explicitly-empty-description",
            json!({"description": ""}),
            Expected::AsSent(String::new()),
            Expected::BottomOfPrecedence,
        ),
        (
            "description-at-its-maximum-length",
            json!({"description": max_length_description.clone()}),
            Expected::AsSent(max_length_description),
            Expected::BottomOfPrecedence,
        ),
        (
            "priority-at-the-bottom-of-its-range",
            json!({"priority": "0"}),
            Expected::EmptyDescription,
            Expected::AsSent("0".to_string()),
        ),
        (
            "priority-at-the-top-of-its-range",
            json!({"priority": "1"}),
            Expected::EmptyDescription,
            Expected::AsSent("1".to_string()),
        ),
        (
            "priority-with-thirty-fraction-digits",
            json!({"priority": smallest_priority.clone()}),
            Expected::EmptyDescription,
            Expected::AsSent(smallest_priority),
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

/// Every read path reports the rule members the create supplied, and the values
/// the service materialises for the ones it did not: no read path ever leaves
/// `description` or `priority` out.
#[tokio::test]
async fn every_read_path_reports_the_description_and_priority_of_a_stored_rule() {
    let router = emulator();

    for (index, (case, optional, expected_description, expected_priority)) in
        projection_cases().iter().enumerate()
    {
        let request = request_for(index, optional);

        // Create reports what it stored.
        let created = call_ok(&router, "CreateAutomationRule", &request, case).await;
        let rule_id = string_member(&created, "ruleId");
        let rule_arn = string_member(&created, "ruleArn");
        assert_projected(&created, "description", expected_description, case);
        assert_projected(&created, "priority", expected_priority, case);

        // Get reports exactly the same, and a rule never updated was last
        // changed when it was created.
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_projected(&fetched, "description", expected_description, case);
        assert_projected(&fetched, "priority", expected_priority, case);
        let created_at = present_timestamp(&fetched, "createdTimestamp", case);
        let last_updated = present_timestamp(&fetched, "lastUpdatedTimestamp", case);
        assert!(
            (last_updated - created_at).abs() < f64::EPSILON,
            "case {case:?}: a rule never updated was last changed when it was created, got \
             {last_updated} against {created_at}"
        );

        // The AutomationRule summary carries the same two members, so List
        // reports them the same way.
        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": 100}),
            case,
        )
        .await;
        let summary = listed_rule(&listed, &rule_id, case);
        assert_projected(&summary, "description", expected_description, case);
        assert_projected(&summary, "priority", expected_priority, case);
        assert!(
            (present_timestamp(&summary, "lastUpdatedTimestamp", case) - last_updated).abs()
                < f64::EPSILON,
            "case {case:?}: the listed rule must carry the same lastUpdatedTimestamp Get \
             reports ({last_updated}), got {summary}"
        );
    }
}

/// The precedence the service gives a rule nobody prioritised outranks no rule
/// that named one: its value is no lower than every priority a create may send,
/// so it claims a recommended action only when no prioritised rule matches.
#[tokio::test]
async fn a_rule_created_without_a_priority_outranks_no_prioritised_rule() {
    let router = emulator();

    let unprioritised = call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("precedence-default"),
        "unprioritised",
    )
    .await;
    let default_priority = priority_number(&unprioritised, "unprioritised");

    // Every priority the documented range admits, from its bottom to its top.
    let sent = [
        "0",
        "0.000000000000000000000000000001",
        "0.25",
        "0.5",
        "0.999",
        "1",
    ];
    for (index, priority) in sent.iter().enumerate() {
        let case = format!("prioritised-{priority}");
        let mut request = minimal_rule(&format!("precedence-rule-{index}"));
        request["priority"] = json!(priority);
        let created = call_ok(&router, "CreateAutomationRule", &request, &case).await;
        let explicit = priority_number(&created, &case);
        assert!(
            default_priority >= explicit,
            "case {case:?}: the priority of a rule nobody prioritised ({default_priority}) must \
             be no lower than one a create named ({explicit})"
        );
    }
}

/// An update that carries neither member leaves the rule exactly as the create
/// left it — so the values the service materialised at create time are still
/// what every read path reports after an unrelated update.
#[tokio::test]
async fn an_update_that_names_neither_member_keeps_the_rules_projection() {
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
        assert_projected(&updated, "description", expected_description, case);
        assert_projected(&updated, "priority", expected_priority, case);

        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_projected(&fetched, "description", expected_description, case);
        assert_projected(&fetched, "priority", expected_priority, case);

        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": 100}),
            case,
        )
        .await;
        let summary = listed_rule(&listed, &rule_id, case);
        assert_projected(&summary, "description", expected_description, case);
        assert_projected(&summary, "priority", expected_priority, case);
    }
}

/// An update replaces the value the service materialised: the member it supplies
/// becomes the stored value and every read path reports it, while the one it
/// leaves alone keeps what the create left.
#[tokio::test]
async fn an_update_replaces_a_member_a_rule_was_created_without() {
    let router = emulator();

    // (case, the member the update carries, expected description after it,
    // expected priority after it) — each row starts from a rule created with no
    // optional members at all.
    let long_description = "u".repeat(1024);
    let cases: Vec<(&str, Value, Expected, Expected)> = vec![
        (
            "update-adds-a-description",
            json!({"description": "described by the update"}),
            Expected::AsSent("described by the update".to_string()),
            Expected::BottomOfPrecedence,
        ),
        (
            "update-adds-an-empty-description",
            json!({"description": ""}),
            Expected::AsSent(String::new()),
            Expected::BottomOfPrecedence,
        ),
        (
            "update-adds-a-maximum-length-description",
            json!({"description": long_description.clone()}),
            Expected::AsSent(long_description),
            Expected::BottomOfPrecedence,
        ),
        (
            "update-adds-a-priority",
            json!({"priority": "0.125"}),
            Expected::EmptyDescription,
            Expected::AsSent("0.125".to_string()),
        ),
        (
            "update-adds-the-top-priority-value",
            json!({"priority": "1"}),
            Expected::EmptyDescription,
            Expected::AsSent("1".to_string()),
        ),
        (
            "update-adds-both",
            json!({"description": "both at once", "priority": "0"}),
            Expected::AsSent("both at once".to_string()),
            Expected::AsSent("0".to_string()),
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
        assert_projected(&created, "description", &Expected::EmptyDescription, case);
        assert_projected(&created, "priority", &Expected::BottomOfPrecedence, case);

        let mut update = change.clone();
        update["ruleArn"] = json!(&rule_arn);
        update["ruleRevision"] = member(&created, "ruleRevision").clone();
        let updated = call_ok(&router, "UpdateAutomationRule", &update, case).await;
        assert_projected(&updated, "description", expected_description, case);
        assert_projected(&updated, "priority", expected_priority, case);

        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_projected(&fetched, "description", expected_description, case);
        assert_projected(&fetched, "priority", expected_priority, case);

        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": 100}),
            case,
        )
        .await;
        let summary = listed_rule(&listed, &rule_id, case);
        assert_projected(&summary, "description", expected_description, case);
        assert_projected(&summary, "priority", expected_priority, case);
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

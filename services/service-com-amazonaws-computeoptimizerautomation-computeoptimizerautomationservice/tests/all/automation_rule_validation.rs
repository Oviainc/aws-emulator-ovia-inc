// SPDX-License-Identifier: CC0-1.0

//! The rule a `tags` list is held to on the path that CREATES a rule.
//!
//! The rule under test: a tag list states one value per key — a resource holds
//! at most one entry per key, which `TagResource`'s merge and every reader of a
//! stored list already assume — so one request may not name a key twice, and
//! `CreateAutomationRule` refuses such a list with
//! `InvalidParameterValueException` (HTTP 400) and stores nothing. A list whose
//! keys are distinct is accepted and reads back as sent.
//!
//! The same rule on the other operation that carries a `TagList` is asserted in
//! [`crate::resource_tagging`] (`TagResource`); `UntagResource.tagKeys` is not a
//! tag list, and a repeated key there states no conflicting value, removal being
//! idempotent. The other members a rule is made of — `criteria`, `schedule`,
//! `recommendedActionTypes` — are asserted in
//! [`crate::rule_value_validation`], which this suite deliberately does not
//! restate.
//!
//! Every case drives the service over the wire through the shared awsJson1_0
//! harness in [`crate::automation_rule_lifecycle`].

use serde_json::{json, Value};

use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, member, minimal_rule, string_member,
};

/// A create carrying `tags` and nothing else unusual.
fn create_with_tags(name: &str, tags: &Value) -> Value {
    let mut request = minimal_rule(name);
    request["tags"] = tags.clone();
    request
}

/// A create whose `tags` name one key twice is refused and stores no rule.
///
/// The rows differ along what the rule generalises over: the two values for the
/// repeated key, where in the list the repeat sits, how long the list is, and
/// how many distinct keys surround it. Keys and values stay inside the modelled
/// `Tag` constraints (`smithy_query(query="shape",shape="Tag")` — key 1..128,
/// value ..256), so each refusal is about the repeat and not about a bound.
#[tokio::test]
async fn a_create_naming_one_tag_key_twice_is_refused_and_stores_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        (
            "two-values-for-one-key",
            json!([{"key": "env", "value": "staging"}, {"key": "env", "value": "prod"}]),
        ),
        (
            "the-same-value-twice",
            json!([{"key": "env", "value": "prod"}, {"key": "env", "value": "prod"}]),
        ),
        (
            "a-repeat-around-a-distinct-key",
            json!([
                {"key": "env", "value": "prod"},
                {"key": "team", "value": "platform"},
                {"key": "env", "value": "staging"}
            ]),
        ),
        (
            "a-repeat-at-the-end-of-a-longer-list",
            json!([
                {"key": "team", "value": "platform"},
                {"key": "cost-center", "value": "4711"},
                {"key": "tier", "value": "2"},
                {"key": "team", "value": "payments"}
            ]),
        ),
        (
            "every-entry-naming-the-same-key",
            json!([
                {"key": "k", "value": "1"},
                {"key": "k", "value": "2"},
                {"key": "k", "value": "3"}
            ]),
        ),
        (
            "a-repeat-of-a-key-at-its-maximum-length",
            json!([
                {"key": "k".repeat(128), "value": "1"},
                {"key": "k".repeat(128), "value": "2"}
            ]),
        ),
    ];

    for (case, tags) in &cases {
        let name = format!("refused-{case}");
        let (status, body) = call(
            &router,
            "CreateAutomationRule",
            &create_with_tags(&name, tags),
        )
        .await;
        assert_refused(
            (status, body.clone()),
            400,
            "InvalidParameterValueException",
            case,
        );
        // The refusal reports the member it refused, and echoes no request value.
        let message = string_member(&body, "message");
        assert!(
            message.contains("'tags'") && !message.contains("env"),
            "case {case:?}: expected a validation report naming tags and no request value, got \
             {message:?}"
        );

        let listed = call_ok(&router, "ListAutomationRules", &json!({}), case).await;
        let stored: Vec<String> = member(&listed, "automationRules")
            .as_array()
            .into_iter()
            .flatten()
            .map(|rule| string_member(rule, "name"))
            .collect();
        assert!(
            !stored.contains(&name),
            "case {case:?}: a refused create must store nothing, got {listed}"
        );
    }
}

/// The accepted side of the same rule: a list whose keys are distinct is stored
/// as sent and reads back through `GetAutomationRule`, across the span the
/// modelled `Tag`/`TagList` shapes allow — including keys that differ only in
/// case, which are two keys because a tag key is case-sensitive.
#[tokio::test]
async fn a_create_whose_tag_keys_are_distinct_is_stored_and_reads_back() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        ("no-tags-at-all", json!([])),
        ("one-tag", json!([{"key": "env", "value": "prod"}])),
        (
            "several-distinct-keys",
            json!([
                {"key": "env", "value": "prod"},
                {"key": "team", "value": "platform"},
                {"key": "tier", "value": "2"}
            ]),
        ),
        (
            "keys-differing-only-in-case",
            json!([{"key": "env", "value": "prod"}, {"key": "ENV", "value": "staging"}]),
        ),
        (
            "both-members-at-their-maximum-length",
            json!([{"key": "k".repeat(128), "value": "v".repeat(256)}]),
        ),
        (
            "an-empty-value-is-a-value",
            json!([{"key": "unset", "value": ""}]),
        ),
        (
            "two-keys-sharing-a-value",
            json!([{"key": "env", "value": "prod"}, {"key": "stage", "value": "prod"}]),
        ),
    ];

    for (case, tags) in &cases {
        let created = call_ok(
            &router,
            "CreateAutomationRule",
            &create_with_tags(&format!("created-{case}"), tags),
            case,
        )
        .await;
        assert_eq!(
            member(&created, "tags"),
            tags,
            "case {case:?}: the create must echo the stored tags, got {created}"
        );

        let rule_arn = string_member(&created, "ruleArn");
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_eq!(
            member(&fetched, "tags"),
            tags,
            "case {case:?}: the stored tags must read back as sent, got {fetched}"
        );

        // The same stored list through the tag reader, so the create is pinned
        // against two independent readers.
        let listed = call_ok(
            &router,
            "ListTagsForResource",
            &json!({"resourceArn": &rule_arn}),
            case,
        )
        .await;
        assert_eq!(
            member(&listed, "tags"),
            tags,
            "case {case:?}: ListTagsForResource must report the stored tags, got {listed}"
        );
    }
}

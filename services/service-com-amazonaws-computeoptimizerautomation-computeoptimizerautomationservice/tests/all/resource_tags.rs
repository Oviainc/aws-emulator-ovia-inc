// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListTagsForResource`.
//!
//! The rule under test: `ListTagsForResource` reports, for the resource its
//! `resourceArn` identifies, exactly the tag list the store holds for that
//! resource — unchanged by being read — and answers the modelled
//! `ResourceNotFoundException` when no resource has that ARN.
//!
//! Every case drives the service over the wire through the same router the
//! emulator binary mounts (the harness lives in
//! [`super::automation_rule_lifecycle`]), so a response that is not computed from
//! persisted state fails here.

use serde_json::{json, Value};

use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, member, minimal_rule, string_member,
};

/// The tag list of `rule_arn` as `ListTagsForResource` reports it.
async fn list_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    rule_arn: &str,
    case: &str,
) -> Value {
    let body = call_ok(
        router,
        "ListTagsForResource",
        &json!({"resourceArn": rule_arn}),
        case,
    )
    .await;
    member(&body, "tags").clone()
}

/// A tag key of `len` characters, all from the modelled `TagKey` pattern class
/// `^[\w\s\.\-\:\/\=\+\@]+$`.
fn tag_key(len: usize) -> String {
    "k".repeat(len)
}

/// A tag value of `len` characters, all from the modelled `TagValue` pattern
/// class `^[\w\s\.\-\:\/\=\+\@]*$`.
fn tag_value(len: usize) -> String {
    "v".repeat(len)
}

/// The tag list a rule is created with is exactly what `ListTagsForResource`
/// reports back, across the span the modelled `TagList`/`Tag` shapes allow: no
/// tags, one tag, several tags, every character class the key and value patterns
/// admit, both members at their documented maximum length, an empty value, and a
/// list at the `TagList` maximum of 200 entries.
#[tokio::test]
async fn created_tags_round_trip_through_list_tags_for_resource() {
    let router = emulator();

    // Lengths and character classes come from the model:
    // `smithy_query(query="shape",shape="Tag,TagList")` — key: length 1..128,
    // pattern ^[\w\s\.\-\:\/\=\+\@]+$; value: length ..256, pattern
    // ^[\w\s\.\-\:\/\=\+\@]*$; TagList: length ..200.
    let mut max_sized_list = Vec::new();
    for index in 0..200 {
        max_sized_list.push(json!({"key": format!("key-{index}"), "value": format!("{index}")}));
    }

    let cases: Vec<(&str, Value)> = vec![
        ("no-tags", json!([])),
        ("one-tag", json!([{"key": "team", "value": "platform"}])),
        (
            "several-tags",
            json!([
                {"key": "team", "value": "platform"},
                {"key": "cost-center", "value": "4711"},
                {"key": "env", "value": "prod"}
            ]),
        ),
        (
            "every-character-class-the-patterns-admit",
            json!([{"key": "a_b c.d-e:f/g=h+i@9", "value": "A_B C.D-E:F/G=H+I@0"}]),
        ),
        (
            "members-at-their-maximum-length",
            json!([{"key": tag_key(128), "value": tag_value(256)}]),
        ),
        ("empty-value", json!([{"key": "unset", "value": ""}])),
        ("list-at-its-maximum-length", Value::Array(max_sized_list)),
    ];

    for (case, tags) in &cases {
        let mut request = minimal_rule(&format!("tagged-{}", case.replace('-', "_")));
        // The "no-tags" row sends no `tags` member at all, so the rule is stored
        // with the empty list rather than with one the request named.
        if !tags.as_array().is_some_and(Vec::is_empty) {
            request["tags"] = tags.clone();
        }
        let created = call_ok(&router, "CreateAutomationRule", &request, case).await;
        let rule_arn = string_member(&created, "ruleArn");

        let listed = list_tags(&router, &rule_arn, case).await;
        assert_eq!(
            &listed, tags,
            "case {case:?}: ListTagsForResource must report the stored tag list, got {listed}"
        );

        // The same tags the rule itself reports: one stored list, two readers.
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": rule_arn}),
            case,
        )
        .await;
        assert_eq!(
            member(&fetched, "tags"),
            &listed,
            "case {case:?}: ListTagsForResource and GetAutomationRule must agree, got {fetched}"
        );

        // A read changes nothing a later caller sees.
        let again = list_tags(&router, &rule_arn, case).await;
        assert_eq!(
            again, listed,
            "case {case:?}: a second ListTagsForResource must report the same list, got {again}"
        );
    }
}

/// Tags belong to the resource the ARN names: each rule reports its own list,
/// whatever the other rules in the store hold.
#[tokio::test]
async fn tags_are_reported_per_resource() {
    let router = emulator();

    let cases: Vec<(&str, Value)> = vec![
        ("first-rule", json!([{"key": "owner", "value": "alice"}])),
        (
            "second-rule",
            json!([
                {"key": "owner", "value": "bob"},
                {"key": "tier", "value": "2"}
            ]),
        ),
        ("third-rule-untagged", json!([])),
    ];

    let mut arns: Vec<(&str, String, Value)> = Vec::new();
    for (case, tags) in &cases {
        let mut request = minimal_rule(&format!("per-resource-{}", arns.len()));
        if !tags.as_array().is_some_and(Vec::is_empty) {
            request["tags"] = tags.clone();
        }
        let created = call_ok(&router, "CreateAutomationRule", &request, case).await;
        arns.push((case, string_member(&created, "ruleArn"), tags.clone()));
    }

    for (case, rule_arn, tags) in &arns {
        let listed = list_tags(&router, rule_arn, case).await;
        assert_eq!(
            &listed, tags,
            "case {case:?}: each resource reports its own tags, got {listed}"
        );
    }
}

/// An update that carries no tags leaves the resource's tag list alone: the list
/// is the stored one, not one recomputed from the latest request.
#[tokio::test]
async fn an_update_that_names_no_tags_leaves_the_tag_list_alone() {
    let router = emulator();

    let tags = json!([
        {"key": "team", "value": "platform"},
        {"key": "env", "value": "staging"}
    ]);
    let mut request = minimal_rule("survives-updates");
    request["tags"] = tags.clone();
    let created = call_ok(&router, "CreateAutomationRule", &request, "create").await;
    let rule_arn = string_member(&created, "ruleArn");

    // Each row is an update of a member that is not `tags`; after each one the
    // tag list must still be the created one.
    let updates: Vec<(&str, Value)> = vec![
        ("update-status", json!({"status": "Inactive"})),
        ("update-description", json!({"description": "now described"})),
        ("update-priority", json!({"priority": "0.5"})),
    ];

    let mut revision = member(&created, "ruleRevision")
        .as_i64()
        .expect("the created rule carries a revision");
    for (case, patch) in &updates {
        let mut body = json!({"ruleArn": rule_arn, "ruleRevision": revision});
        for (name, value) in patch.as_object().expect("the patch is an object") {
            body[name] = value.clone();
        }
        let updated = call_ok(&router, "UpdateAutomationRule", &body, case).await;
        revision = member(&updated, "ruleRevision")
            .as_i64()
            .expect("the updated rule carries a revision");

        let listed = list_tags(&router, &rule_arn, case).await;
        assert_eq!(
            listed, tags,
            "case {case:?}: an update naming no tags keeps the stored list, got {listed}"
        );
    }
}

/// A `resourceArn` no resource has is the modelled `ResourceNotFoundException`
/// (404), and one that does not satisfy the modelled `RuleArn` pattern is refused
/// at the boundary with `InvalidParameterValueException` (400). Neither answers
/// 501: the operation is implemented.
#[tokio::test]
async fn an_unknown_or_malformed_resource_arn_is_refused() {
    let router = emulator();

    // A rule that exists, then is deleted, so a well-formed ARN of this account
    // stops resolving.
    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("doomed"),
        "create",
    )
    .await;
    let deleted_arn = string_member(&created, "ruleArn");
    call_ok(
        &router,
        "DeleteAutomationRule",
        &json!({
            "ruleArn": deleted_arn,
            "ruleRevision": member(&created, "ruleRevision").as_i64().unwrap_or_default()
        }),
        "delete",
    )
    .await;

    // (case, resourceArn, expected status, expected error code)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "deleted-rule",
            deleted_arn,
            404,
            "ResourceNotFoundException",
        ),
        (
            "never-created-rule",
            format!(
                "arn:aws:compute-optimizer::{}:automation-rule/0000000000000000",
                engine::EMULATOR_ACCOUNT_ID
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "another-accounts-rule",
            "arn:aws:compute-optimizer::210987654321:automation-rule/aaaaaaaaaaaaaaaa".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "not-an-arn",
            "automation-rule/aaaaaaaaaaaaaaaa".to_string(),
            400,
            "InvalidParameterValueException",
        ),
        (
            "wrong-arn-service",
            format!(
                "arn:aws:compute-optimiser::{}:automation-rule/aaaaaaaaaaaaaaaa",
                engine::EMULATOR_ACCOUNT_ID
            ),
            400,
            "InvalidParameterValueException",
        ),
        (
            "arn-with-an-account-that-is-not-12-digits",
            "arn:aws:compute-optimizer::12345:automation-rule/aaaaaaaaaaaaaaaa".to_string(),
            400,
            "InvalidParameterValueException",
        ),
    ];

    for (case, resource_arn, status, code) in &cases {
        let answer = call(
            &router,
            "ListTagsForResource",
            &json!({"resourceArn": resource_arn}),
        )
        .await;
        assert_refused(answer, *status, code, case);
    }

    // The absent required member is refused the same way, not with a 501.
    let answer = call(&router, "ListTagsForResource", &json!({})).await;
    assert_refused(
        answer,
        400,
        "InvalidParameterValueException",
        "missing-resource-arn",
    );
}

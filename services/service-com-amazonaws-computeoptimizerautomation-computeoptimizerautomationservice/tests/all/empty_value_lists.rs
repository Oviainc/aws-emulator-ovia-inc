// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the floor every caller-supplied value list of this
//! service is held to.
//!
//! The rule under test: a list a request sends to state WHAT the call applies to
//! — the accounts an association names, the action types a rule or a preview
//! executes, the values a criteria condition compares against, the tags an
//! addition carries, the keys a removal names — states the call's whole subject,
//! so a request sending it empty states a call with nothing to do and is refused
//! with `InvalidParameterValueException` (400), under the member's own name,
//! leaving the store and the caller's client token exactly as they were. A list
//! carrying at least one value is acted on, whether or not acting on it changes
//! anything.
//!
//! The rows span every required value-list member of this API
//! (`smithy_query(query="operation",operation="AssociateAccounts,DisassociateAccounts,CreateAutomationRule,ListAutomationRulePreview,ListAutomationRulePreviewSummaries,TagResource,UntagResource")`),
//! and drive the service over the wire through the harness
//! [`super::automation_rule_lifecycle`] owns, so a refusal that is not computed
//! on the request, or a write a refused request leaves behind, fails here.

use serde_json::{json, Value};

use super::automation_rule_lifecycle::{
    call, call_ok, emulator, error_code, member, minimal_rule, string_member,
};
use super::error_payloads::error_message;

/// One of the action types a rule of this service may execute
/// (`smithy_query(query="shape",shape="RecommendedActionType")`), for the rows
/// whose list has to be non-empty.
const UPGRADE: &str = "UpgradeEbsVolumeType";

/// Create a rule carrying `tags` and answer `(ruleArn, ruleRevision)`.
async fn tagged_rule(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    tags: &Value,
) -> (String, i64) {
    let mut request = minimal_rule(name);
    request["tags"] = tags.clone();
    let created = call_ok(router, "CreateAutomationRule", &request, "setup").await;
    (
        string_member(&created, "ruleArn"),
        member(&created, "ruleRevision")
            .as_i64()
            .expect("the created rule carries a revision"),
    )
}

/// The `(key, value)` pairs `ListTagsForResource` reports for `rule_arn`, in the
/// order it reports them.
async fn listed_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    rule_arn: &str,
    case: &str,
) -> Vec<(String, String)> {
    let body = call_ok(
        router,
        "ListTagsForResource",
        &json!({"resourceArn": rule_arn}),
        case,
    )
    .await;
    member(&body, "tags")
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| (string_member(tag, "key"), string_member(tag, "value")))
                .collect()
        })
        .unwrap_or_default()
}

/// The wording a refusal of an empty value list carries: the member's path and
/// the requirement it failed, with no value echoed.
fn empty_list_message(member_path: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
         Member must have length greater than or equal to 1"
    )
}

/// Every required value-list member of this API, sent empty, is refused the same
/// way — same status, same code, same report under the member's own name — and
/// the store the rows share is untouched by all of them.
#[tokio::test]
async fn an_empty_value_list_is_refused_wherever_a_request_carries_one() {
    let router = emulator();
    let (rule_arn, revision) = tagged_rule(
        &router,
        "empty-list-floor",
        &json!([{"key": "owner", "value": "team"}]),
    )
    .await;

    // (case, operation, request, the member the report must name). Every row is
    // a refusal, so they share one store: what none of them may change is
    // asserted once, after the table.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "untag-names-no-key",
            "UntagResource",
            json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tagKeys": []}),
            "tagKeys",
        ),
        (
            "tag-names-no-tag",
            "TagResource",
            json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tags": []}),
            "tags",
        ),
        (
            "associate-names-no-account",
            "AssociateAccounts",
            json!({"accountIds": []}),
            "accountIds",
        ),
        (
            "disassociate-names-no-account",
            "DisassociateAccounts",
            json!({"accountIds": []}),
            "accountIds",
        ),
        (
            "create-names-no-action-type",
            "CreateAutomationRule",
            {
                let mut request = minimal_rule("no-action-types");
                request["recommendedActionTypes"] = json!([]);
                request
            },
            "recommendedActionTypes",
        ),
        (
            "update-sends-no-action-type",
            "UpdateAutomationRule",
            json!({
                "ruleArn": &rule_arn,
                "ruleRevision": revision,
                "recommendedActionTypes": []
            }),
            "recommendedActionTypes",
        ),
        (
            "preview-names-no-action-type",
            "ListAutomationRulePreview",
            json!({"ruleType": "AccountRule", "recommendedActionTypes": []}),
            "recommendedActionTypes",
        ),
        (
            "preview-summaries-names-no-action-type",
            "ListAutomationRulePreviewSummaries",
            json!({"ruleType": "AccountRule", "recommendedActionTypes": []}),
            "recommendedActionTypes",
        ),
        (
            "preview-condition-names-no-value",
            "ListAutomationRulePreview",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": {"region": [{"comparison": "StringEquals", "values": []}]}
            }),
            "criteria.region.member.values",
        ),
    ];

    for (case, operation, request, member_path) in &rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}/{operation}: a list that names nothing is a client error, got {status} \
             and {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}/{operation}: expected the parameter-value error, got {body}"
        );
        let expected = empty_list_message(member_path);
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}/{operation}: expected {expected:?}, got {body}"
        );
    }

    // None of the refusals wrote anything: the rule still carries the tag it was
    // created with, at the revision it was created at.
    let tags = listed_tags(&router, &rule_arn, "after-the-refusals").await;
    assert_eq!(
        tags,
        vec![("owner".to_string(), "team".to_string())],
        "a refused request must change no tag, got {tags:?}"
    );
    let rule = call_ok(
        &router,
        "GetAutomationRule",
        &json!({"ruleArn": &rule_arn}),
        "after-the-refusals",
    )
    .await;
    let stored_revision = member(&rule, "ruleRevision").as_i64().unwrap_or_default();
    assert_eq!(
        stored_revision, revision,
        "a refused request must leave the revision alone, expected {revision}, got \
         {stored_revision}"
    );
    let stored_action_types = member(&rule, "recommendedActionTypes").clone();
    assert_eq!(
        stored_action_types,
        json!([UPGRADE]),
        "a refused update must leave the action types alone, got {stored_action_types}"
    );
    // The create that named no action type stored no rule either.
    let listed = call_ok(
        &router,
        "ListAutomationRules",
        &json!({}),
        "after-the-refusals",
    )
    .await;
    let rule_count = member(&listed, "automationRules")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        rule_count, 1,
        "a refused create must store no rule, expected the one rule of the setup, got \
         {rule_count}"
    );
}

/// One row of the removal table: the case name, the tags the rule starts with,
/// the keys the removal names, and the tags left on the rule afterwards.
type RemovalRow = (
    &'static str,
    Value,
    Value,
    Vec<(&'static str, &'static str)>,
);

/// A tag list naming at least one value IS acted on, whether or not acting on it
/// changes the stored list: the rule is about what the request names, not about
/// what the store happens to hold.
///
/// The rows differ along what the rule generalises over: how many keys the
/// request names, whether a named key is stored, whether it names a key twice,
/// and whether every stored key is named.
#[tokio::test]
async fn a_tag_list_naming_at_least_one_value_is_acted_on() {
    // (case, tags the rule starts with, keys the removal names, the tags left)
    let rows: Vec<RemovalRow> = vec![
        (
            "one-of-two-keys",
            json!([{"key": "owner", "value": "team"}, {"key": "env", "value": "prod"}]),
            json!(["owner"]),
            vec![("env", "prod")],
        ),
        (
            "every-stored-key",
            json!([{"key": "owner", "value": "team"}, {"key": "env", "value": "prod"}]),
            json!(["owner", "env"]),
            Vec::new(),
        ),
        (
            "a-key-the-rule-does-not-carry",
            json!([{"key": "owner", "value": "team"}]),
            json!(["absent"]),
            vec![("owner", "team")],
        ),
        (
            "the-same-key-twice",
            json!([{"key": "owner", "value": "team"}, {"key": "env", "value": "prod"}]),
            json!(["owner", "owner"]),
            vec![("env", "prod")],
        ),
        (
            "three-keys-of-which-one-is-stored",
            json!([{"key": "owner", "value": "team"}, {"key": "env", "value": "prod"}]),
            json!(["absent", "owner", "other"]),
            vec![("env", "prod")],
        ),
        (
            "a-key-with-every-punctuation-the-modelled-form-admits",
            // `TagKey`'s pattern is ^[\w\s\.\-\:\/\=\+\@]+$
            // (`smithy_query(query="shape",shape="TagKey")`).
            json!([{"key": "a b.c-d:e/f=g+h@i", "value": "x"}, {"key": "env", "value": "prod"}]),
            json!(["a b.c-d:e/f=g+h@i"]),
            vec![("env", "prod")],
        ),
    ];

    for (case, stored, removed, expected) in &rows {
        let router = emulator();
        let (rule_arn, revision) = tagged_rule(&router, "untag-acted-on", stored).await;

        call_ok(
            &router,
            "UntagResource",
            &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tagKeys": removed}),
            case,
        )
        .await;

        let tags = listed_tags(&router, &rule_arn, case).await;
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tags, expected,
            "case {case:?}: expected {expected:?} left on the rule, got {tags:?}"
        );
    }

    // The addition side of the same floor: a `tags` list carrying at least one
    // tag is applied, whatever its size.
    for (case, added, expected) in [
        (
            "add-one-tag",
            json!([{"key": "owner", "value": "team"}]),
            vec![("owner", "team")],
        ),
        (
            "add-three-tags",
            json!([
                {"key": "owner", "value": "team"},
                {"key": "env", "value": "prod"},
                {"key": "cost", "value": "42"}
            ]),
            vec![("owner", "team"), ("env", "prod"), ("cost", "42")],
        ),
    ] {
        let router = emulator();
        let (rule_arn, revision) = tagged_rule(&router, "tag-acted-on", &json!([])).await;
        call_ok(
            &router,
            "TagResource",
            &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tags": added}),
            case,
        )
        .await;
        let tags = listed_tags(&router, &rule_arn, case).await;
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tags, expected,
            "case {case:?}: expected {expected:?} on the rule, got {tags:?}"
        );
    }
}

/// A request refused for naming nothing consumes no client token: the token the
/// caller sent with it still works for the call the caller meant.
#[tokio::test]
async fn an_empty_value_list_consumes_no_client_token() {
    // (case, operation, the empty list member and its value, the request that
    // follows under the SAME token)
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        (
            "untag",
            "UntagResource",
            json!({"tagKeys": []}),
            json!({"tagKeys": ["owner"]}),
        ),
        (
            "tag",
            "TagResource",
            json!({"tags": []}),
            json!({"tags": [{"key": "env", "value": "prod"}]}),
        ),
    ];

    for (case, operation, empty, intended) in &rows {
        let router = emulator();
        let (rule_arn, revision) = tagged_rule(
            &router,
            "token-not-spent",
            &json!([{"key": "owner", "value": "team"}]),
        )
        .await;

        let token = "idempotency-token-1";
        let mut refused = empty.clone();
        refused["resourceArn"] = json!(&rule_arn);
        refused["ruleRevision"] = json!(revision);
        refused["clientToken"] = json!(token);
        let (status, body) = call(&router, operation, &refused).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a list that names nothing is a client error, got {status} and {body}"
        );

        let mut meant = intended.clone();
        meant["resourceArn"] = json!(&rule_arn);
        meant["ruleRevision"] = json!(revision);
        meant["clientToken"] = json!(token);
        let (status, body) = call(&router, operation, &meant).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the token a refused call carried must still be free for the call the \
             caller meant, got {status} and {body}"
        );

        let tags = listed_tags(&router, &rule_arn, case).await;
        let expected: Vec<(String, String)> = match *case {
            "untag" => Vec::new(),
            _ => vec![
                ("owner".to_string(), "team".to_string()),
                ("env".to_string(), "prod".to_string()),
            ],
        };
        assert_eq!(
            tags, expected,
            "case {case:?}: expected {expected:?} on the rule, got {tags:?}"
        );
    }
}

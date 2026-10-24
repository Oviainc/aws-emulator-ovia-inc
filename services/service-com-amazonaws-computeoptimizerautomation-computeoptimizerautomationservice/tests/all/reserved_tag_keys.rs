// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tag-key namespace a caller may not use.
//!
//! The rule under test: a tag key in the provider-reserved namespace (a key
//! opening with the prefix `state::RESERVED_TAG_KEY_PREFIX`, whatever the case of
//! that prefix) is a key a caller may neither set nor remove, so every operation
//! that takes caller-supplied tag keys refuses such a request with the modelled
//! `InvalidParameterValueException` (HTTP 400) and leaves the resource's stored
//! tag list exactly as it was; a key that merely CONTAINS the prefix, or opens
//! with those characters without the delimiter, is an ordinary key and is stored,
//! read back and removed like any other.
//!
//! The namespace is not expressible in the model — `TagKey` is
//! `@length(1..128)` with pattern `^[\w\s\.\-\:\/\=\+\@]+$`
//! (`smithy_query(query="shape",shape="TagKey")`), which admits the colon — so
//! the rule is the service's own and every case here drives it over the wire
//! through the same router the emulator binary mounts (the harness lives in
//! [`super::automation_rule_lifecycle`]).

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, member, minimal_rule, string_member,
};

/// Keys in the reserved namespace, spanning what the rule generalises over: the
/// case of the prefix, what follows it (nothing, a word, further colons,
/// whitespace) and the length of the whole key. The prefix itself comes from the
/// service's own constant, so no spelling of it is hand-written here.
fn reserved_keys() -> Vec<(String, String)> {
    let prefix = state::RESERVED_TAG_KEY_PREFIX;
    let upper = prefix.to_uppercase();
    let mixed: String = prefix
        .chars()
        .enumerate()
        .map(|(index, c)| {
            if index % 2 == 0 {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect();
    vec![
        ("the-prefix-and-a-word".to_string(), format!("{prefix}createdBy")),
        ("the-prefix-alone".to_string(), prefix.to_string()),
        ("the-prefix-uppercased".to_string(), format!("{upper}createdBy")),
        ("the-prefix-in-mixed-case".to_string(), format!("{mixed}owner")),
        (
            "a-further-colon-after-the-prefix".to_string(),
            format!("{prefix}cloudformation:stack-name"),
        ),
        (
            "whitespace-after-the-prefix".to_string(),
            format!("{prefix}managed by"),
        ),
        (
            "at-the-maximum-key-length".to_string(),
            format!("{prefix}{}", "k".repeat(128 - prefix.chars().count())),
        ),
    ]
}

/// Keys OUTSIDE the reserved namespace that the prefix's characters appear in:
/// each must be accepted, because the namespace is the prefix at the START of the
/// key and not those characters anywhere in it.
fn ordinary_keys() -> Vec<(String, String)> {
    let prefix = state::RESERVED_TAG_KEY_PREFIX;
    let without_delimiter: String = prefix.chars().filter(|c| c.is_alphanumeric()).collect();
    vec![
        ("the-prefix-word-without-its-delimiter".to_string(), without_delimiter.clone()),
        (
            "the-prefix-letters-then-more-letters".to_string(),
            format!("{without_delimiter}x{prefix}team"),
        ),
        (
            "the-prefix-letters-then-a-hyphen".to_string(),
            format!("{without_delimiter}-managed"),
        ),
        ("the-prefix-inside-the-key".to_string(), format!("team:{prefix}owner")),
        ("the-prefix-at-the-end".to_string(), format!("owner-{prefix}")),
        ("another-namespace".to_string(), "ops:createdBy".to_string()),
    ]
}

/// Create a rule carrying `tags` and answer `(ruleArn, ruleRevision)`.
async fn rule_with_tags(
    router: &MultiServiceRouter,
    name: &str,
    tags: &Value,
    case: &str,
) -> (String, i64) {
    let mut request = minimal_rule(name);
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

/// The tag list `ListTagsForResource` reports for `rule_arn`, cross-checked
/// against the list `GetAutomationRule` reports: one stored list, two readers.
async fn stored_tags(router: &MultiServiceRouter, rule_arn: &str, case: &str) -> Value {
    let listed = call_ok(
        router,
        "ListTagsForResource",
        &json!({"resourceArn": rule_arn}),
        case,
    )
    .await;
    let fetched = call_ok(
        router,
        "GetAutomationRule",
        &json!({"ruleArn": rule_arn}),
        case,
    )
    .await;
    assert_eq!(
        member(&listed, "tags"),
        member(&fetched, "tags"),
        "case {case:?}: ListTagsForResource and GetAutomationRule must agree, got {fetched}"
    );
    member(&listed, "tags").clone()
}

/// A create naming a reserved tag key is refused with the modelled
/// `InvalidParameterValueException` (400) and stores no rule at all, whether the
/// reserved key stands alone or beside an ordinary one.
#[tokio::test]
async fn creating_a_rule_with_a_reserved_tag_key_is_refused() {
    for (case, key) in reserved_keys() {
        for (variant, tags) in [
            ("alone", json!([{"key": key, "value": "me"}])),
            (
                "beside-an-ordinary-key",
                json!([{"key": "team", "value": "platform"}, {"key": key, "value": "me"}]),
            ),
        ] {
            let case = format!("{case}-{variant}");
            let router = emulator();
            let name = "reserved-create";
            let mut request = minimal_rule(name);
            request["tags"] = tags;

            assert_refused(
                call(&router, "CreateAutomationRule", &request).await,
                400,
                "InvalidParameterValueException",
                &case,
            );

            // A refused create stores nothing: the store is still empty.
            let listed = call_ok(&router, "ListAutomationRules", &json!({}), &case).await;
            let stored = member(&listed, "automationRules")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                stored, 0,
                "case {case:?}: a refused create must store no rule, got {listed}"
            );
        }
    }
}

/// `TagResource` refuses a reserved tag key with the modelled
/// `InvalidParameterValueException` (400) and leaves the stored tag list
/// untouched — the reserved key is not added, and neither is an ordinary key sent
/// in the same request.
#[tokio::test]
async fn tagging_with_a_reserved_tag_key_is_refused_and_adds_nothing() {
    let initial = json!([{"key": "keep", "value": "me"}]);

    for (case, key) in reserved_keys() {
        for (variant, tags) in [
            ("alone", json!([{"key": key, "value": "me"}])),
            (
                "beside-an-ordinary-key",
                json!([{"key": "env", "value": "prod"}, {"key": key, "value": "me"}]),
            ),
        ] {
            let case = format!("{case}-{variant}");
            let router = emulator();
            let (rule_arn, revision) =
                rule_with_tags(&router, "reserved-tag", &initial, &case).await;

            let (status, body) = call(
                &router,
                "TagResource",
                &json!({"resourceArn": &rule_arn, "ruleRevision": revision, "tags": tags}),
            )
            .await;
            assert_refused(
                (status, body.clone()),
                400,
                "InvalidParameterValueException",
                &case,
            );
            // The refusal states the namespace a caller may not use, so the
            // caller is told what it may not send.
            let message = string_member(&body, "message");
            assert!(
                message
                    .to_lowercase()
                    .contains(&state::RESERVED_TAG_KEY_PREFIX.to_lowercase()),
                "case {case:?}: the rejection must name the reserved prefix, got {message:?}"
            );

            let tags_now = stored_tags(&router, &rule_arn, &case).await;
            assert_eq!(
                tags_now, initial,
                "case {case:?}: a refused tagging must add nothing, got {tags_now}"
            );
        }
    }
}

/// `UntagResource` refuses a reserved tag key with the modelled
/// `InvalidParameterValueException` (400) and removes nothing: a reserved key is
/// not a caller's to remove either, so an ordinary key named in the same request
/// survives too.
#[tokio::test]
async fn untagging_a_reserved_tag_key_is_refused_and_removes_nothing() {
    let initial = json!([{"key": "keep", "value": "me"}, {"key": "env", "value": "prod"}]);

    for (case, key) in reserved_keys() {
        for (variant, tag_keys) in [
            ("alone", json!([key])),
            ("beside-an-ordinary-key", json!(["env", key])),
        ] {
            let case = format!("{case}-{variant}");
            let router = emulator();
            let (rule_arn, revision) =
                rule_with_tags(&router, "reserved-untag", &initial, &case).await;

            assert_refused(
                call(
                    &router,
                    "UntagResource",
                    &json!({
                        "resourceArn": &rule_arn,
                        "ruleRevision": revision,
                        "tagKeys": tag_keys
                    }),
                )
                .await,
                400,
                "InvalidParameterValueException",
                &case,
            );

            let tags_now = stored_tags(&router, &rule_arn, &case).await;
            assert_eq!(
                tags_now, initial,
                "case {case:?}: a refused untagging must remove nothing, got {tags_now}"
            );
        }
    }
}

/// A key the prefix's characters appear in without opening the key as the
/// delimited namespace is an ORDINARY key: it is accepted by a create, by
/// `TagResource` and by `UntagResource`, and round-trips through both readers.
#[tokio::test]
async fn a_key_outside_the_reserved_namespace_round_trips() {
    for (case, key) in ordinary_keys() {
        let router = emulator();

        // Created with the key: it is stored and read back.
        let created_tags = json!([{"key": key, "value": "created"}]);
        let (rule_arn, revision) =
            rule_with_tags(&router, "ordinary-key", &created_tags, &case).await;
        let tags_now = stored_tags(&router, &rule_arn, &case).await;
        assert_eq!(
            tags_now, created_tags,
            "case {case:?}: a create must store the key, got {tags_now}"
        );

        // Tagged with it again: the value is replaced in place.
        call_ok(
            &router,
            "TagResource",
            &json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision,
                "tags": [{"key": key, "value": "tagged"}, {"key": "team", "value": "platform"}]
            }),
            &case,
        )
        .await;
        let tags_now = stored_tags(&router, &rule_arn, &case).await;
        assert_eq!(
            tags_now,
            json!([{"key": key, "value": "tagged"}, {"key": "team", "value": "platform"}]),
            "case {case:?}: tagging must apply to the key, got {tags_now}"
        );

        // And removed by name.
        call_ok(
            &router,
            "UntagResource",
            &json!({
                "resourceArn": &rule_arn,
                "ruleRevision": revision,
                "tagKeys": [key]
            }),
            &case,
        )
        .await;
        let tags_now = stored_tags(&router, &rule_arn, &case).await;
        assert_eq!(
            tags_now,
            json!([{"key": "team", "value": "platform"}]),
            "case {case:?}: untagging must remove the key, got {tags_now}"
        );
    }
}

/// The in-process predicate agrees with the wire on both sides of the boundary,
/// over keys built from the service's own prefix constant: a key opens the
/// reserved namespace only when the prefix is its first characters, whatever
/// their case.
#[test]
fn the_reserved_namespace_is_the_prefix_at_the_start_of_the_key() {
    for (case, key) in reserved_keys() {
        assert!(
            state::tag_key_is_reserved(&key),
            "case {case:?}: expected {key:?} to be reserved, got false"
        );
    }
    for (case, key) in ordinary_keys() {
        assert!(
            !state::tag_key_is_reserved(&key),
            "case {case:?}: expected {key:?} to be an ordinary key, got true"
        );
    }
    // A key shorter than the prefix cannot open with it, and the empty key is
    // not in the namespace either.
    for (case, key) in [("empty", ""), ("one-character", "a")] {
        assert!(
            !state::tag_key_is_reserved(key),
            "case {case:?}: expected {key:?} to be an ordinary key, got true"
        );
    }
}

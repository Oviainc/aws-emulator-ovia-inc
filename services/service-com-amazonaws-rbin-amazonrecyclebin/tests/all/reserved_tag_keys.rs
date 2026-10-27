// SPDX-License-Identifier: CC0-1.0

//! The tag-key namespace AWS keeps for itself, over every door a caller names a tag key
//! of its own through.
//!
//! RULE: a tagging API refuses, with its parameter-fault error and without writing
//! anything, a request that would create, change or remove a tag whose key claims the
//! `aws:` namespace; a key that merely RESEMBLES it is the caller's own and round-trips
//! like any other. The namespace is reserved however it is cased.
//!
//! The doors are `CreateRule$Tags`, `TagResource$Tags` and `UntagResource$TagKeys` — the
//! three members through which a caller names a key of its own. A rule's resource
//! SELECTION (`ResourceTags`, `ExcludeResourceTags`) is not covered: those members name
//! tags the retained resources already carry, so selecting on one creates no tag.
//!
//! The model admits a reserved key — `TagKey` is `@length(1, 128)` with pattern
//! `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, which allows a colon anywhere — so the boundary
//! validator cannot refuse one and every assertion here is about what the handlers do.
//!
//! The tables span the class the rule generalises over rather than one key: the casing of
//! the prefix, the prefix alone and with a tail, a key at the modelled maximum length,
//! and — on the accepted side — the prefix without its separator, the prefix away from
//! the start of the key, and a key whose fourth byte falls inside a multi-byte character.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{keys, list_tags, pairs, tag_resource, untag_resource};
use crate::retention_rule_lifecycle::{create_rule, list_rules, retention, router};

/// The reserved prefix, and the member paths a refusal names, read off the handler-side
/// constants so the suite cannot drift from what the service enforces.
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    is_reserved_tag_key, RESERVED_TAG_KEY_PREFIX, TAGS_MEMBER, TAG_KEYS_MEMBER,
};

/// `TagKey` is `@length(min: 1, max: 128)`.
/// Source: smithy_query(query="shape", shape="Tag") — member `Key`.
const MAX_TAG_KEY_LEN: usize = 128;

/// The `ValidationExceptionReason` a parameter-value fault reports.
/// Source: smithy_query(query="shape", shape="ValidationExceptionReason").
const INVALID_PARAMETER_VALUE: &str = "INVALID_PARAMETER_VALUE";

/// The resource type the fixture rules retain; `ListRules` requires one.
const RESOURCE_TYPE: &str = "EBS_SNAPSHOT";

/// The wire error code of an error response (`__type`), or `"<none>"`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// An error body's `Message`, or `"<none>"`.
fn message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// Keys that CLAIM the reserved namespace, so no caller may create, change or remove one.
///
/// The rows differ along the casing of the prefix, what follows it (nothing, a system
/// tag's own path, a key at the modelled maximum length) — never along a name that only
/// matters to one recorded request.
fn reserved_keys() -> Vec<(&'static str, String)> {
    // The prefix plus a tail that brings the key to exactly `MAX_TAG_KEY_LEN`, the
    // longest key the model admits.
    let longest = format!(
        "{RESERVED_TAG_KEY_PREFIX}{}",
        "k".repeat(MAX_TAG_KEY_LEN - RESERVED_TAG_KEY_PREFIX.len())
    );
    vec![
        (
            "a system tag's key",
            "aws:cloudformation:stack-name".to_string(),
        ),
        ("the reserved prefix alone", "aws:".to_string()),
        ("the reserved prefix in upper case", "AWS:team".to_string()),
        ("the reserved prefix in mixed case", "Aws:Team".to_string()),
        (
            "a reserved key at the maximum modelled key length",
            longest,
        ),
    ]
}

/// Keys that only RESEMBLE the reserved namespace, so they are the caller's own.
///
/// The rows differ along WHERE and HOW the resemblance falls: the prefix without its
/// separator, with another separator, with more letters, away from the start of the key,
/// and a key whose fourth byte — where the prefix's colon would sit — is inside a
/// multi-byte character.
fn ordinary_keys() -> Vec<(&'static str, String)> {
    vec![
        ("the prefix without its separator", "aws".to_string()),
        ("the prefix with more letters after it", "awsome".to_string()),
        ("the prefix with another separator", "aws-team".to_string()),
        (
            "the prefix away from the start of the key",
            "team:aws:owner".to_string(),
        ),
        (
            "a multi-byte character where the separator would be",
            "awsλ:team".to_string(),
        ),
        (
            "the prefix after a space the key pattern admits",
            " aws:team".to_string(),
        ),
    ]
}

/// A rule carrying `tags`, as `(Identifier, RuleArn)`.
async fn rule_with_tags(
    router: &MultiServiceRouter,
    case: &str,
    tags: &[(String, String)],
) -> (String, String) {
    let (status, created) = create_rule(
        router,
        json!({
            "ResourceType": RESOURCE_TYPE,
            "RetentionPeriod": retention(7),
            "Description": case,
            "Tags": tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect::<Vec<_>>(),
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: fixture CreateRule returned {status} / {created:?}"
    );
    (
        created["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no Identifier"))
            .to_string(),
        created["RuleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no RuleArn"))
            .to_string(),
    )
}

/// The tags `ListTagsForResource` reports for one rule, as `(Key, Value)` pairs sorted so
/// the comparison does not depend on an order the service does not document.
async fn stored_tags(router: &MultiServiceRouter, case: &str, rule_arn: &str) -> Vec<(String, String)> {
    let (status, listed) = list_tags(router, rule_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource returned {status} / {listed:?}"
    );
    let mut out: Vec<(String, String)> = listed["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Tags to be an array, got {listed:?}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

/// How many retention rules this Region holds.
async fn rule_count(router: &MultiServiceRouter, case: &str) -> usize {
    let (status, listed) = list_rules(router, json!({ "ResourceType": RESOURCE_TYPE })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: fixture ListRules returned {status} / {listed:?}"
    );
    listed["Rules"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Rules to be an array, got {listed:?}"))
        .len()
}

/// Every refusal of this class: HTTP 400, the modelled `ValidationException`, the
/// parameter-value fault class, and a message naming the member that carried the key and
/// the prefix that key may not claim. Returns the message, so a caller can assert that
/// every row of a table was refused with the SAME sentence.
fn assert_reserved_key_refusal(case: &str, member: &str, status: u16, body: &Value) -> String {
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
        body.get("Reason").and_then(Value::as_str),
        Some(INVALID_PARAMETER_VALUE),
        "case {case:?}: expected Reason {INVALID_PARAMETER_VALUE}, got {:?}",
        body.get("Reason")
    );
    let stated = message(body);
    assert!(
        stated.contains(&format!("'{member}'")),
        "case {case:?}: expected the message to name the {member:?} member, got {stated:?}"
    );
    assert!(
        stated.contains(RESERVED_TAG_KEY_PREFIX),
        "case {case:?}: expected the message to state the reserved prefix \
         {RESERVED_TAG_KEY_PREFIX:?}, got {stated:?}"
    );
    stated
}

/// RULE: the classification is about the NAMESPACE a key claims, not about one spelling —
/// so the handler-side predicate and the tables below agree on every row.
#[test]
fn the_reserved_namespace_is_claimed_by_the_prefix_in_any_casing_alone() {
    for (case, key) in reserved_keys() {
        assert!(
            is_reserved_tag_key(&key),
            "case {case:?}: expected {key:?} to claim the reserved namespace"
        );
    }
    for (case, key) in ordinary_keys() {
        assert!(
            !is_reserved_tag_key(&key),
            "case {case:?}: expected {key:?} to be the caller's own key"
        );
    }
}

/// RULE: `TagResource` refuses a reserved key and assigns nothing — not even the
/// well-formed tag that travelled beside it.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_key_and_assigns_nothing() {
    let router = router();
    let held = pairs(&[("env", "prod")]);
    let (_, rule_arn) = rule_with_tags(&router, "tagged beside a reserved key", &held).await;

    let mut refusals: Vec<(&str, String)> = Vec::new();
    for (case, key) in reserved_keys() {
        let assigned = vec![
            (key.clone(), "x".to_string()),
            ("Also".to_string(), "valid".to_string()),
        ];
        let (status, body) = tag_resource(&router, &rule_arn, &assigned).await;
        refusals.push((
            case,
            assert_reserved_key_refusal(case, TAGS_MEMBER, status, &body),
        ));
        // Nothing of the refused call is stored: neither the reserved key nor the
        // acceptable one beside it.
        assert_eq!(
            stored_tags(&router, case, &rule_arn).await,
            held,
            "case {case:?}: expected the rule's tags to be left untouched"
        );
    }
    assert_same_sentence(&refusals);
}

/// RULE: `UntagResource` refuses a reserved key and removes nothing — a caller may not
/// delete a system tag, and the keys it does own travel unharmed beside it.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_tag_key_and_removes_nothing() {
    let router = router();
    let held = pairs(&[("env", "prod")]);
    let (_, rule_arn) = rule_with_tags(&router, "untagged beside a reserved key", &held).await;

    let mut refusals: Vec<(&str, String)> = Vec::new();
    for (case, key) in reserved_keys() {
        let named = vec![key.clone(), "env".to_string()];
        let (status, body) = untag_resource(&router, &rule_arn, &named).await;
        refusals.push((
            case,
            assert_reserved_key_refusal(case, TAG_KEYS_MEMBER, status, &body),
        ));
        assert_eq!(
            stored_tags(&router, case, &rule_arn).await,
            held,
            "case {case:?}: expected the rule to keep every tag it held"
        );
    }
    assert_same_sentence(&refusals);
}

/// RULE: `CreateRule` refuses a reserved key in `Tags` and mints no rule — the same
/// namespace rule, on the other door a caller names its own tag keys through.
#[tokio::test]
async fn create_rule_refuses_a_reserved_tag_key_and_mints_nothing() {
    let router = router();
    let before = rule_count(&router, "before any refused create").await;

    let mut refusals: Vec<(&str, String)> = Vec::new();
    for (case, key) in reserved_keys() {
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": RESOURCE_TYPE,
                "RetentionPeriod": retention(7),
                "Tags": [
                    { "Key": key, "Value": "x" },
                    { "Key": "Also", "Value": "valid" },
                ],
            }),
        )
        .await;
        refusals.push((
            case,
            assert_reserved_key_refusal(case, TAGS_MEMBER, status, &body),
        ));
        assert_eq!(
            rule_count(&router, case).await,
            before,
            "case {case:?}: expected the refused create to mint no rule"
        );
    }
    assert_same_sentence(&refusals);
}

/// RULE: a key that only RESEMBLES the reserved namespace is the caller's own, so it is
/// created, read back and removed like any other — through every one of the three doors.
#[tokio::test]
async fn a_key_that_only_resembles_the_reserved_prefix_round_trips() {
    let router = router();

    for (case, key) in ordinary_keys() {
        // Created at CreateRule time, and reported by the tag reader.
        let created = vec![(key.clone(), "at-create".to_string())];
        let (_, rule_arn) = rule_with_tags(&router, case, &created).await;
        assert_eq!(
            stored_tags(&router, case, &rule_arn).await,
            created,
            "case {case:?}: expected CreateRule to assign the key"
        );

        // Changed by TagResource, which upserts it.
        let (status, body) = tag_resource(&router, &rule_arn, &pairs(&[(&key, "changed")])).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected TagResource to accept {key:?}, got {status} / {body:?}"
        );
        assert_eq!(
            stored_tags(&router, case, &rule_arn).await,
            vec![(key.clone(), "changed".to_string())],
            "case {case:?}: expected the new value to be stored"
        );

        // And removed by UntagResource.
        let (status, body) = untag_resource(&router, &rule_arn, &keys(&[&key])).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected UntagResource to accept {key:?}, got {status} / {body:?}"
        );
        assert!(
            stored_tags(&router, case, &rule_arn).await.is_empty(),
            "case {case:?}: expected the key to be unassigned"
        );
    }
}

/// Every row of a refusal table was answered with the same sentence: the clause states
/// the member and the reserved prefix, and echoes nothing of the key the request sent.
fn assert_same_sentence(refusals: &[(&str, String)]) {
    let Some((first_case, first)) = refusals.first() else {
        panic!("a refusal table must have at least one row");
    };
    for (case, stated) in refusals {
        assert_eq!(
            stated, first,
            "case {case:?}: expected the same sentence as case {first_case:?} \
             ({first:?}), got {stated:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the BODY this service answers a quota refusal with.
//!
//! The rule asserted here: a request past a bound this service holds a retention rule
//! to is refused with HTTP 402 `ServiceQuotaExceededException` whose `Reason` AND
//! `Message` are both the sole reason value the model declares for that error
//! (smithy_query(query="shape", shape="ServiceQuotaExceededExceptionReason")) — the
//! token alone, with nothing of the request and no bound of the service rendered into
//! it — so every quota refusal of this service answers one body, whichever bound was
//! reached and whichever operation the request arrived through.
//!
//! Every row compares the wire body with the body `crate::state` composes for a quota
//! refusal, which is what pins the handlers to the service's single composer: a handler
//! that worded a refusal of its own would answer a body no constructor produces.
//!
//! The tables span what the rule generalises over — the operation the request reaches
//! the quota through, how far past the bound it is (one tag, several, a request naming a
//! key the rule already holds), and the FORM of the tags (plain, non-ASCII, a key at the
//! published maximum length, a key the request repeats) — rather than one recorded
//! request. Each row drives the service through the real router
//! (`MultiServiceRouter::dispatch`), exactly as a signed SDK request would.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::error_response_bodies::{error_code, message};
use crate::resource_tags::{list_tags, tag_resource};
use crate::retention_rule_lifecycle::{
    create_rule, delete_rule, get_rule, list_of, list_rules, retention, router, supported_retention,
    supported_retention_max, RESOURCE_TYPES,
};
use service_com_amazonaws_rbin_amazonrecyclebin::ops::types::{
    ServiceQuotaExceededException, SERVICE_QUOTA_EXCEEDED_EXCEPTION_REASON_VALUES,
};
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    quota_exceeded, MAX_RULES, MAX_TAGS_PER_RULE, SERVICE_QUOTA_REASON_EXCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

/// `TagKey` is `@length(min: 1, max: 128)`.
/// Source: smithy_query(query="shape", shape="Tag") — member `Key`.
const MAX_TAG_KEY_LEN: usize = 128;

/// The `(key, value)` pairs one request carries, as the tag helpers take them.
type TagPairs = Vec<(String, String)>;

/// One tags-per-resource row: a set of tags past the published quota, in some form the
/// rule has to hold for, and how many tags a rule carrying them would be left holding.
#[derive(Debug, Clone)]
struct TagRow {
    case: &'static str,
    /// The tags, in the order a request names them — possibly naming one key twice.
    tags: TagPairs,
}

impl TagRow {
    /// The row split for the `TagResource` path: the tags a rule is CREATED with — one
    /// for each of the first [`MAX_TAGS_PER_RULE`] distinct keys, so the rule starts
    /// exactly at its quota — and the tags the refused assignment then names, which are
    /// the remaining keys plus any key the rule already holds.
    fn split_at_quota(&self) -> (TagPairs, TagPairs) {
        let mut held_keys: BTreeSet<&str> = BTreeSet::new();
        let (mut held, mut assigned) = (TagPairs::new(), TagPairs::new());
        for pair in &self.tags {
            if !held_keys.contains(pair.0.as_str()) && held_keys.len() < MAX_TAGS_PER_RULE {
                held_keys.insert(pair.0.as_str());
                held.push(pair.clone());
            } else {
                assigned.push(pair.clone());
            }
        }
        (held, assigned)
    }
}

/// `n` plain tags, `(key0, value0) … (key{n-1}, value{n-1})`.
fn plain_tags(n: usize) -> TagPairs {
    (0..n)
        .map(|nth| (format!("key{nth}"), format!("value{nth}")))
        .collect()
}

/// The tag sets under test: each one past [`MAX_TAGS_PER_RULE`], and each a different
/// form of the thing the rule generalises over.
fn tag_rows() -> Vec<TagRow> {
    // One key past the quota, and a key at the published maximum length among them.
    let mut at_max_key_len = plain_tags(MAX_TAGS_PER_RULE);
    at_max_key_len.push(("k".repeat(MAX_TAG_KEY_LEN), "longest".to_string()));
    // One key past the quota, with the request naming one of those keys twice: a rule
    // holds one tag per key, so the repeat is not a tag of its own.
    let mut with_a_repeated_key = plain_tags(MAX_TAGS_PER_RULE + 1);
    with_a_repeated_key.push(("key0".to_string(), "rewritten".to_string()));
    vec![
        TagRow {
            case: "one tag past the quota",
            tags: plain_tags(MAX_TAGS_PER_RULE + 1),
        },
        TagRow {
            case: "five tags past the quota",
            tags: plain_tags(MAX_TAGS_PER_RULE + 5),
        },
        TagRow {
            case: "twice the quota's worth of tags",
            tags: plain_tags(2 * MAX_TAGS_PER_RULE),
        },
        TagRow {
            case: "a key at the published maximum length past the quota",
            tags: at_max_key_len,
        },
        TagRow {
            case: "a key the request names twice, one key past the quota",
            tags: with_a_repeated_key,
        },
        TagRow {
            case: "non-ASCII keys and values past the quota",
            tags: (0..MAX_TAGS_PER_RULE + 1)
                .map(|nth| (format!("clé{nth}"), format!("naïve 日本 {nth}")))
                .collect(),
        },
    ]
}

/// The `TagList` wire form of `(key, value)` pairs.
fn tag_list(tags: &TagPairs) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// A `CreateRule` body carrying `tags`.
fn create_body(tags: &TagPairs) -> Value {
    json!({
        "ResourceType": RESOURCE_TYPES[0],
        "RetentionPeriod": retention(7),
        "Tags": tag_list(tags),
    })
}

/// Assert the body `crate::state` composes for a quota refusal is the token-only body
/// this suite words for it, so the composer is held to the text and not merely to
/// itself: `Message` and `Reason` alike are the modelled reason token.
fn assert_composed(case: &str, composed: &ServiceQuotaExceededException) {
    assert_eq!(
        composed.message.as_deref(),
        Some(SERVICE_QUOTA_REASON_EXCEEDED),
        "case {case:?}: expected the composed Message {SERVICE_QUOTA_REASON_EXCEEDED:?}, got {:?}",
        composed.message
    );
    assert_eq!(
        composed.reason.as_deref(),
        Some(SERVICE_QUOTA_REASON_EXCEEDED),
        "case {case:?}: expected the modelled Reason, got {:?}",
        composed.reason
    );
}

/// Assert a response IS the quota refusal `expected` composed in `crate::state`: HTTP
/// 402, the modelled error code, and the very `Message` and `Reason` that body carries.
fn assert_quota_body(
    case: &str,
    status: u16,
    body: &Value,
    expected: &ServiceQuotaExceededException,
) {
    assert_eq!(
        status, 402,
        "case {case:?}: expected HTTP 402, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ServiceQuotaExceededException",
        "case {case:?}: expected ServiceQuotaExceededException, got {body:?}"
    );
    assert_eq!(
        Some(message(body).as_str()),
        expected.message.as_deref(),
        "case {case:?}: expected Message {:?}, got {:?}",
        expected.message,
        message(body)
    );
    assert_eq!(
        body["Reason"].as_str(),
        expected.reason.as_deref(),
        "case {case:?}: expected Reason {:?}, got {:?}",
        expected.reason,
        body["Reason"]
    );
    // The reason the body carries is the one value the model declares for this error, so
    // a caller branching on it reads a modelled value.
    assert!(
        body["Reason"]
            .as_str()
            .is_some_and(|reason| SERVICE_QUOTA_EXCEEDED_EXCEPTION_REASON_VALUES
                .contains(&reason)),
        "case {case:?}: the Reason must be one the model declares \
         ({SERVICE_QUOTA_EXCEEDED_EXCEPTION_REASON_VALUES:?}), got {:?}",
        body["Reason"]
    );
}

/// Create a rule carrying `tags`, assert it was accepted, and answer its `RuleArn`.
async fn rule_holding(router: &MultiServiceRouter, case: &str, tags: &TagPairs) -> String {
    let (status, created) = create_rule(router, create_body(tags)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: fixture CreateRule must be accepted, got {status} / {created:?}"
    );
    assert_eq!(
        list_of(&created, "Tags").len(),
        tags.len(),
        "case {case:?}: the fixture rule must hold the {} tags it was created with, got {:?}",
        tags.len(),
        created["Tags"]
    );
    created["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no RuleArn: {created:?}"))
        .to_string()
}

#[tokio::test]
async fn create_rule_and_tag_resource_answer_one_tags_per_resource_quota_body() {
    for row in tag_rows() {
        // The CREATE door: the whole set in one `CreateRule`, which would mint a rule
        // holding more tags than the quota allows.
        let router = router();
        let case = format!("{}: CreateRule", row.case);
        let created = quota_exceeded();
        assert_composed(&case, &created);
        let (status, body) = create_rule(&router, create_body(&row.tags)).await;
        assert_quota_body(&case, status, &body, &created);
        assert_eq!(
            body["Identifier"].as_str(),
            None,
            "case {case:?}: a refused create must mint no rule, got {body:?}"
        );
        let (status, listed) =
            list_rules(&router, json!({ "ResourceType": RESOURCE_TYPES[0] })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListRules, got {status} / {listed:?}"
        );
        assert_eq!(
            list_of(&listed, "Rules").len(),
            0,
            "case {case:?}: a refused create must leave the Region empty, got {listed:?}"
        );

        // The ASSIGNMENT door: a rule already at its quota, and the rest of the set
        // assigned to it — the same bound reached from the other side.
        let (held, assigned) = row.split_at_quota();
        let case = format!("{}: TagResource", row.case);
        let rule_arn = rule_holding(&router, &case, &held).await;
        let addressed = quota_exceeded();
        assert_composed(&case, &addressed);
        let (status, body) = tag_resource(&router, &rule_arn, &assigned).await;
        assert_quota_body(&case, status, &body, &addressed);
        let (status, tags) = list_tags(&router, &rule_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} / {tags:?}"
        );
        assert_eq!(
            list_of(&tags, "Tags").len(),
            MAX_TAGS_PER_RULE,
            "case {case:?}: a refused assignment must store nothing, got {:?}",
            tags["Tags"]
        );

        // The two doors answer the SAME body: the refusal names the fault class and
        // nothing of the request, so neither handler words the quota its own way and
        // neither echoes back the tags, the count or the bound.
        for (door, composed) in [("CreateRule", &created), ("TagResource", &addressed)] {
            let stated = composed.message.clone().unwrap_or_default();
            assert_eq!(
                stated, SERVICE_QUOTA_REASON_EXCEEDED,
                "case {:?}: {door} must state the reason token alone, got {stated:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn create_rule_answers_one_rules_per_region_quota_body() {
    let router = router();
    // Fill the Region to its published quota of retention rules.
    for nth in 0..MAX_RULES {
        let resource_type = RESOURCE_TYPES[nth % RESOURCE_TYPES.len()];
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(supported_retention(resource_type, nth)),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case \"rule {nth} of the quota\": expected HTTP 201, got {status} / {body:?}"
        );
    }

    // Every request past it is refused with the one body `crate::state` composes for a
    // quota refusal, whatever the request itself asks for: the refusal states the fault
    // class, not the request and not the bound.
    let expected = quota_exceeded();
    assert_composed("the rules-per-Region bound", &expected);
    let over_quota = [
        (
            "a plain rule past the quota",
            json!({ "ResourceType": RESOURCE_TYPES[0], "RetentionPeriod": retention(7) }),
        ),
        (
            "a rule with tags past the quota",
            json!({
                "ResourceType": RESOURCE_TYPES[1],
                "RetentionPeriod": retention(supported_retention_max(RESOURCE_TYPES[1])),
                "Tags": tag_list(&plain_tags(3)),
                "Description": "one past the quota",
            }),
        ),
        (
            "a tag-level rule past the quota",
            json!({
                "ResourceType": RESOURCE_TYPES[2],
                "RetentionPeriod": retention(1),
                "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
            }),
        ),
    ];
    for (case, body) in over_quota {
        let (status, refused) = create_rule(&router, body).await;
        assert_quota_body(case, status, &refused, &expected);
    }

    // Reaching another bound answers the same body: a quota refusal names the fault
    // class, so a caller tells quota refusals from other refusals by the error code and
    // reason, never by prose.
    let tags = quota_exceeded();
    assert_eq!(
        expected.message, tags.message,
        "case \"two bounds\": every quota refusal must state the one token, got {:?} and {:?}",
        expected.message, tags.message
    );

    // Freeing one slot admits one rule again, and the rule it admits is readable — so
    // the refusals above are a live count over the store, not a latch.
    let (status, listed) = list_rules(
        &router,
        json!({ "ResourceType": RESOURCE_TYPES[0], "MaxResults": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"list one rule to free a slot\": expected HTTP 200, got {status} / {listed:?}"
    );
    let victim = list_of(&listed, "Rules")
        .first()
        .and_then(|summary| summary["Identifier"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("case \"list one rule to free a slot\": no rule listed"));
    let (status, _) = delete_rule(&router, &victim).await;
    assert_eq!(
        status, 204,
        "case \"free one slot\": expected HTTP 204 from DeleteRule, got {status}"
    );
    let (status, created) = create_rule(
        &router,
        json!({ "ResourceType": RESOURCE_TYPES[0], "RetentionPeriod": retention(7) }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"the rule into the freed slot\": expected HTTP 201, got {status} / {created:?}"
    );
    let identifier = created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"the rule into the freed slot\": no Identifier"));
    let (status, fetched) = get_rule(&router, identifier).await;
    assert_eq!(
        status, 200,
        "case \"the rule into the freed slot\": expected HTTP 200 from GetRule, \
         got {status} / {fetched:?}"
    );
}

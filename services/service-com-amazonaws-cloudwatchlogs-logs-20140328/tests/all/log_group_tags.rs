// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the deprecated log-group tagging trio `TagLogGroup`,
//! `ListTagsLogGroup` and `UntagLogGroup`.
//!
//! The rule under test: a log group holds ONE tag set, and these three operations name it by
//! log group name. `TagLogGroup` MERGES the request's tags into the set the group already
//! holds — a key it does not carry is added, a key it does carry takes the new value, every
//! other key is left where it is — `UntagLogGroup` removes exactly the keys it names, and
//! `ListTagsLogGroup` reports that one stored set without changing it. It is the SAME set the
//! generic tagging API (`TagResource` / `UntagResource` / `ListTagsForResource`) reads and
//! writes, so a tag written through either door is read back through both. A log group the
//! account does not hold is refused `ResourceNotFoundException`; a tag key in the namespace
//! Amazon Web Services reserves for its own tags is refused `InvalidParameterException` and
//! assigns nothing; and a merge past the documented 50-tag quota is refused
//! `TooManyTagsException`, leaving the stored set exactly as it was.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass, and what one operation
//! writes is read back through another.
//!
//! The tables span what the rule generalises over rather than one request: tag keys and
//! values in the character classes the model's `Tags` shape admits, at its published length
//! bounds and at the empty value; one tag per call and several at once; adding, replacing and
//! removing; removal of a key the group never carried and of several at once; tag counts from
//! zero through the published quota and one past it; reserved and merely reserved-looking tag
//! keys; and every log-group name the three operations refuse.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    ComAmazonawsCloudwatchlogsLogs20140328State, MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX,
};

use crate::harness::Logs;

/// The deprecated operation that writes a log group's tags.
const TAG: &str = "TagLogGroup";

/// The deprecated operation that removes them.
const UNTAG: &str = "UntagLogGroup";

/// The deprecated operation that reports them.
const LIST: &str = "ListTagsLogGroup";

/// A `TagLogGroup` body naming one log group and the tags to merge into it.
fn tag_body(log_group_name: &str, tags: &[(String, String)]) -> Value {
    let map: Map<String, Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    json!({"logGroupName": log_group_name, "tags": Value::Object(map)})
}

/// An `UntagLogGroup` body naming one log group and the tag KEYS to remove from it.
fn untag_body(log_group_name: &str, keys: &[&str]) -> Value {
    json!({"logGroupName": log_group_name, "tags": keys})
}

/// The tag map in a response body, as a key-ordered map.
fn tag_map(case: &str, operation: &str, response: &Value) -> BTreeMap<String, String> {
    let tags = response
        .get("tags")
        .unwrap_or_else(|| panic!("case {case}: {operation} must report a tag set, got {response}"));
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case}: tags must be a map, got {tags}"))
        .iter()
        .map(|(key, value)| {
            let value = value.as_str().unwrap_or_else(|| {
                panic!("case {case}: the value of tag {key} must be a string, got {value}")
            });
            (key.clone(), value.to_string())
        })
        .collect()
}

/// The tag set the deprecated reader reports for one log group.
async fn reported_tags(logs: &Logs, log_group_name: &str) -> BTreeMap<String, String> {
    let response = logs
        .ok(LIST, &json!({"logGroupName": log_group_name}))
        .await;
    tag_map(log_group_name, LIST, &response)
}

/// The tag set the GENERIC reader reports for the same log group, by ARN — the cross-check
/// that both doors see one stored set.
async fn reported_tags_by_arn(logs: &Logs, arn: &str) -> BTreeMap<String, String> {
    let response = logs
        .ok("ListTagsForResource", &json!({"resourceArn": arn}))
        .await;
    tag_map(arn, "ListTagsForResource", &response)
}

/// A created log group, and the ARN form the generic tagging API's `resourceArn` admits.
async fn tagged_group(logs: &Logs, name: &str) -> String {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name}))
        .await;
    logs.group_arns(name).await.1
}

/// One row per class of tag the model admits.
///
/// The classes are the model's own, from `smithy_query(query="shape", shape="Tags")`: a key
/// is a `TagKey` (`@length(min: 1, max: 128)`, `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`) and a value
/// a `TagValue` (`@length(min: 0, max: 256)`, the same character class with `*`). So the rows
/// span ASCII letters, letters outside ASCII (`\p{L}`), a separator (`\p{Z}`), digits inside
/// and outside ASCII (`\p{N}`), the punctuation characters the class names, the empty value
/// at the published minimum, and a key and a value at their published maxima.
fn tag_cases() -> Vec<(String, String)> {
    vec![
        ("Team".to_string(), "observability".to_string()),
        ("Owner".to_string(), "team platform".to_string()),
        ("Projekt".to_string(), "Ünïcödé-Wërt".to_string()),
        ("ログ".to_string(), "日本語ログ".to_string()),
        ("cost.center:unit/7".to_string(), "+=.:/-@_".to_string()),
        ("Release".to_string(), "٧ 2026".to_string()),
        ("Unset".to_string(), String::new()),
        ("t".repeat(128), "w".repeat(256)),
    ]
}

#[tokio::test]
async fn tagging_appends_each_new_key_and_the_reader_reports_every_stored_tag() {
    let logs = Logs::new();
    let group = "/aws/emulator/log-group-tags-appended";
    let arn = tagged_group(&logs, group).await;

    // A group that has never been tagged carries the empty tag set.
    let initial = reported_tags(&logs, group).await;
    assert!(
        initial.is_empty(),
        "case untagged: a log group with no tags reports an empty tag set, got {initial:?}"
    );

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in tag_cases() {
        logs.ok(TAG, &tag_body(group, &[(key.clone(), value.clone())]))
            .await;
        expected.insert(key.clone(), value.clone());

        let reported = reported_tags(&logs, group).await;
        assert_eq!(
            reported, expected,
            "case {key:?}={value:?}: a new tag key is appended to the set the group already \
             holds, expected {expected:?}, got {reported:?}"
        );

        // One group, one tag set: the generic reader reports the very same thing.
        let through_generic = reported_tags_by_arn(&logs, &arn).await;
        assert_eq!(
            through_generic, expected,
            "case {key:?}={value:?}: both readers report ONE stored tag set, expected \
             {expected:?}, got {through_generic:?}"
        );
    }
}

#[tokio::test]
async fn one_call_merges_every_tag_it_names_and_a_replacement_moves_no_other_key() {
    let logs = Logs::new();
    let group = "/aws/emulator/log-group-tags-merged";
    tagged_group(&logs, group).await;

    let cases = tag_cases();
    logs.ok(TAG, &tag_body(group, &cases)).await;
    let mut expected: BTreeMap<String, String> = cases.iter().cloned().collect();
    let reported = reported_tags(&logs, group).await;
    assert_eq!(
        reported, expected,
        "case {} tags in one call: every tag the call names is stored, expected {expected:?}, \
         got {reported:?}",
        cases.len()
    );

    // (key, replacement value) — the replacements cross the same classes as the table above,
    // including both directions of the empty value and the longest value.
    let replacements: Vec<(String, String)> = vec![
        ("Team".to_string(), "platform".to_string()),
        ("ログ".to_string(), "にほんご".to_string()),
        ("Unset".to_string(), "set".to_string()),
        ("Owner".to_string(), String::new()),
        ("t".repeat(128), "w".to_string()),
    ];
    for (key, replacement) in replacements {
        logs.ok(TAG, &tag_body(group, &[(key.clone(), replacement.clone())]))
            .await;
        expected.insert(key.clone(), replacement.clone());
        let reported = reported_tags(&logs, group).await;
        assert_eq!(
            reported, expected,
            "case {key:?}->{replacement:?}: the new value replaces the previous one and no \
             other key moves, expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn untagging_removes_exactly_the_keys_it_names() {
    let logs = Logs::new();
    let group = "/aws/emulator/log-group-tags-removed";
    tagged_group(&logs, group).await;

    let cases = tag_cases();
    logs.ok(TAG, &tag_body(group, &cases)).await;
    let mut expected: BTreeMap<String, String> = cases.iter().cloned().collect();

    let longest = "t".repeat(128);
    // Each row is a removal the rule covers: one key, several at once, a key the group never
    // carried (nothing to remove rather than a failure), a key at the published maximum
    // length, and a non-ASCII key.
    let removals: Vec<Vec<&str>> = vec![
        vec!["Team"],
        vec!["Owner", "Projekt"],
        vec!["NeverCarried"],
        vec![longest.as_str()],
        vec!["ログ"],
    ];
    for keys in removals {
        logs.ok(UNTAG, &untag_body(group, &keys)).await;
        for key in &keys {
            expected.remove(*key);
        }
        let reported = reported_tags(&logs, group).await;
        assert_eq!(
            reported, expected,
            "case remove {keys:?}: only the keys the call names leave the set, expected \
             {expected:?}, got {reported:?}"
        );

        // Removing the same keys again reaches the same set.
        logs.ok(UNTAG, &untag_body(group, &keys)).await;
        let repeated = reported_tags(&logs, group).await;
        assert_eq!(
            repeated, expected,
            "case remove {keys:?} twice: repeating the removal reaches the same set, expected \
             {expected:?}, got {repeated:?}"
        );
    }
}

/// One write or removal through ONE tagging door, and the tag set the log group holds after
/// it — read back through BOTH readers.
struct Step {
    operation: &'static str,
    body: Value,
    after: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn the_deprecated_trio_and_the_generic_tagging_api_share_one_tag_set() {
    let logs = Logs::new();
    let group = "/aws/emulator/log-group-tags-one-set";
    let arn = tagged_group(&logs, group).await;

    // (the operation that writes or removes, its body, the set the group then holds). Each
    // row is written through ONE door and read back through BOTH.
    let steps = vec![
        Step {
            operation: TAG,
            body: tag_body(group, &[("Deprecated".to_string(), "door".to_string())]),
            after: vec![("Deprecated", "door")],
        },
        Step {
            operation: "TagResource",
            body: json!({"resourceArn": arn, "tags": {"Generic": "door"}}),
            after: vec![("Deprecated", "door"), ("Generic", "door")],
        },
        Step {
            operation: "TagResource",
            body: json!({"resourceArn": arn, "tags": {"Deprecated": "replaced"}}),
            after: vec![("Deprecated", "replaced"), ("Generic", "door")],
        },
        Step {
            operation: UNTAG,
            body: untag_body(group, &["Generic"]),
            after: vec![("Deprecated", "replaced")],
        },
        Step {
            operation: "UntagResource",
            body: json!({"resourceArn": arn, "tagKeys": ["Deprecated"]}),
            after: vec![],
        },
    ];

    for step in steps {
        let (operation, body) = (step.operation, step.body);
        logs.ok(operation, &body).await;
        let expected: BTreeMap<String, String> = step
            .after
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        let deprecated = reported_tags(&logs, group).await;
        assert_eq!(
            deprecated, expected,
            "case {operation} {body}: {LIST} reports the one stored set, expected {expected:?}, \
             got {deprecated:?}"
        );
        let generic = reported_tags_by_arn(&logs, &arn).await;
        assert_eq!(
            generic, expected,
            "case {operation} {body}: the generic reader reports the SAME set, expected \
             {expected:?}, got {generic:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_key_in_the_reserved_namespace_is_refused_and_assigns_nothing() {
    let logs = Logs::new();
    let group = "/aws/emulator/log-group-tags-reserved";
    let arn = tagged_group(&logs, group).await;

    // (tag key, is it the caller's to assign). The reserved prefix is AWS's own
    // (`RESERVED_TAG_KEY_PREFIX`), and it is the PREFIX that is reserved: a key that merely
    // starts with those letters, or carries them anywhere but at the start, is the caller's.
    let cases: Vec<(&str, bool)> = vec![
        ("aws:cloudformation:stack-name", false),
        ("aws:createdBy", false),
        // What is reserved is the prefix, not one spelling of it.
        ("AWS:Billing", false),
        ("Aws:billing", false),
        // The prefix alone is still in the reserved namespace.
        (RESERVED_TAG_KEY_PREFIX, false),
        // Reserved-LOOKING keys the caller may assign: no colon after the letters, the
        // letters inside the key rather than at its start, and a longer word starting with
        // them.
        ("awsome.key", true),
        ("AWSBilling", true),
        ("team:aws:owner", true),
        ("sawstooth", true),
    ];

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (key, is_callers) in cases {
        let value = "value";
        let body = tag_body(group, &[(key.to_string(), value.to_string())]);
        if is_callers {
            logs.ok(TAG, &body).await;
            expected.insert(key.to_string(), value.to_string());
        } else {
            let (code, message) = logs.refusal(TAG, &body).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {key:?}: a reserved tag key is refused as a bad parameter, got {code}"
            );
            assert!(
                message.to_ascii_lowercase().contains("reserved"),
                "case {key:?}: the refusal states that the namespace is reserved, got {message:?}"
            );

            // The generic door refuses the same key the same way: neither admits what the
            // other refuses, and one rule has one refusal.
            let (generic_code, generic_message) = logs
                .refusal(
                    "TagResource",
                    &json!({"resourceArn": arn, "tags": {key: value}}),
                )
                .await;
            assert_eq!(
                (generic_code.as_str(), generic_message.as_str()),
                (code.as_str(), message.as_str()),
                "case {key:?}: both doors refuse a reserved tag key alike, expected \
                 {code}/{message:?}, got {generic_code}/{generic_message:?}"
            );
        }

        let reported = reported_tags(&logs, group).await;
        assert_eq!(
            reported, expected,
            "case {key:?}: a refused call assigns nothing and an accepted one assigns exactly \
             its tag, expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_merge_past_the_tag_quota_is_refused_and_keeps_the_stored_set() {
    let logs = Logs::new();
    let group = "/aws/emulator/log-group-tags-at-the-quota";
    tagged_group(&logs, group).await;

    // Fill the group to the documented quota in two calls, so the quota is reached by
    // MERGING rather than by one request.
    let quota = MAX_TAGS_PER_RESOURCE;
    let all: Vec<(String, String)> = (0..quota)
        .map(|index| (format!("key{index:03}"), format!("value{index:03}")))
        .collect();
    let (first_half, second_half) = all.split_at(quota / 2);
    logs.ok(TAG, &tag_body(group, first_half)).await;
    logs.ok(TAG, &tag_body(group, second_half)).await;

    let mut expected: BTreeMap<String, String> = all.iter().cloned().collect();
    let reported = reported_tags(&logs, group).await;
    assert_eq!(
        reported, expected,
        "case {quota} tags: a log group holds the whole quota, expected {expected:?}, \
         got {reported:?}"
    );

    // At the quota, re-tagging a key the group already carries adds nothing to the total, so
    // it is accepted.
    let (existing_key, _) = all[0].clone();
    logs.ok(
        TAG,
        &tag_body(group, &[(existing_key.clone(), "replaced".to_string())]),
    )
    .await;
    expected.insert(existing_key.clone(), "replaced".to_string());
    let reported = reported_tags(&logs, group).await;
    assert_eq!(
        reported, expected,
        "case replace {existing_key} at the quota: replacing a value does not grow the set, \
         expected {expected:?}, got {reported:?}"
    );

    // One more NEW key would push the group past the quota.
    let code = logs
        .err(
            TAG,
            &tag_body(group, &[("OneTooMany".to_string(), "x".to_string())]),
        )
        .await;
    assert_eq!(
        code, "TooManyTagsException",
        "case {} tags: a merge past the quota is refused, got {code}",
        quota + 1
    );
    let reported = reported_tags(&logs, group).await;
    assert_eq!(
        reported, expected,
        "case refused merge: a refused call assigns no tag, expected {expected:?}, \
         got {reported:?}"
    );

    // Removing a key makes room again, so the quota is on the set the group ends up holding
    // rather than on the number of calls.
    logs.ok(UNTAG, &untag_body(group, &[existing_key.as_str()]))
        .await;
    expected.remove(&existing_key);
    logs.ok(
        TAG,
        &tag_body(group, &[("RoomAgain".to_string(), "x".to_string())]),
    )
    .await;
    expected.insert("RoomAgain".to_string(), "x".to_string());
    let reported = reported_tags(&logs, group).await;
    assert_eq!(
        reported, expected,
        "case room again: with a key removed the next new key fits, expected {expected:?}, \
         got {reported:?}"
    );
}

#[tokio::test]
async fn a_log_group_the_account_does_not_hold_is_refused_by_each_operation() {
    let logs = Logs::new();
    tagged_group(&logs, "/aws/emulator/log-group-tags-exists").await;

    // Log group names inside the member's own `@pattern`
    // (`^[\.\-_/#A-Za-z0-9]+$`, from `smithy_query(query="operation",
    // operation="TagLogGroup")`) that this account holds no group under: a plain name, a
    // slashed name, a name differing only in case from one that exists, and one at the
    // published maximum length.
    let longest = "g".repeat(512);
    let absent = [
        "never-created",
        "/aws/emulator/never-created",
        "/AWS/EMULATOR/LOG-GROUP-TAGS-EXISTS",
        longest.as_str(),
    ];

    for name in absent {
        for (operation, body) in [
            (
                TAG,
                tag_body(name, &[("Key".to_string(), "value".to_string())]),
            ),
            (UNTAG, untag_body(name, &["Key"])),
            (LIST, json!({"logGroupName": name})),
        ] {
            let code = logs.err(operation, &body).await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {name:?}: {operation} must refuse a log group the account does not hold \
                 with ResourceNotFoundException, got {code}"
            );
        }
    }
}

#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let logs = Logs::new();
    let group = "/aws/emulator/log-group-tags-are-read";
    tagged_group(&logs, group).await;
    logs.ok(
        TAG,
        &tag_body(group, &[("Team".to_string(), "observability".to_string())]),
    )
    .await;

    let first = logs.ok(LIST, &json!({"logGroupName": group})).await;
    let second = logs.ok(LIST, &json!({"logGroupName": group})).await;
    assert_eq!(
        second, first,
        "case repeated read: reporting twice must report the same thing, expected {first}, \
         got {second}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored: Vec<(String, String)> = state
        .lock()
        .log_groups
        .get(group)
        .map(|record| {
            record
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        stored,
        vec![("Team".to_string(), "observability".to_string())],
        "case repeated read: the store still holds exactly the tags that were written, \
         got {stored:?}"
    );
}

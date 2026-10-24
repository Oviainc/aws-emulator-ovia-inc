// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UntagResource` against `ListTagsForResource`: the tag keys a
//! CloudWatch Logs resource stops carrying.
//!
//! The rule under test: `UntagResource` names its resource by ARN and removes from that
//! resource's ONE stored tag set exactly the keys the request lists — a key the resource
//! carries is dropped whatever its value, a key it does not carry is nothing to remove, and
//! every key the request does not name keeps its value — so the call is idempotent and
//! `ListTagsForResource` reports the remaining set. It resolves `resourceArn` the way the
//! other two tagging operations do: a value that is not an ARN or names no taggable
//! CloudWatch Logs resource is `InvalidParameterException`, a taggable kind the account holds
//! no instance of is `ResourceNotFoundException`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass, and what one operation
//! removes is read back through another.
//!
//! The table spans what the rule generalises over rather than one request: keys and values in
//! every character class the model's `Tags` shape admits, at both published length bounds and
//! at the empty value; one key per call, several at once, none at all (`tagKeys`'s published
//! minimum) and the whole set; keys the resource does not carry, alone and mixed with keys it
//! does; a log group and a delivery as the resource; the quota boundary before and after a
//! removal; and every ARN shape the operation refuses.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    delivery_arn, log_group_arn, ComAmazonawsCloudwatchlogsLogs20140328State, DeliveryRecord,
    MAX_TAGS_PER_RESOURCE,
};

use crate::harness::{Logs, REGION};

/// The operation that writes tags.
const TAG: &str = "TagResource";

/// The operation that removes them.
const UNTAG: &str = "UntagResource";

/// The operation that reports them.
const LIST: &str = "ListTagsForResource";

/// A `TagResource` body naming one resource and the tags to merge into it.
fn tag_body(resource_arn: &str, tags: &[(String, String)]) -> Value {
    let map: Map<String, Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    json!({"resourceArn": resource_arn, "tags": Value::Object(map)})
}

/// An `UntagResource` body naming one resource and the tag keys to remove from it.
fn untag_body(resource_arn: &str, tag_keys: &[String]) -> Value {
    json!({"resourceArn": resource_arn, "tagKeys": tag_keys})
}

/// The tag set the service reports for one resource, as a key-ordered map.
async fn reported_tags(logs: &Logs, resource_arn: &str) -> BTreeMap<String, String> {
    let response = logs.ok(LIST, &json!({"resourceArn": resource_arn})).await;
    let tags = response.get("tags").unwrap_or_else(|| {
        panic!("case {resource_arn}: {LIST} must report a tag set, got {response}")
    });
    tags.as_object()
        .unwrap_or_else(|| panic!("case {resource_arn}: tags must be a map, got {tags}"))
        .iter()
        .map(|(key, value)| {
            let value = value.as_str().unwrap_or_else(|| {
                panic!("case {resource_arn}: the value of tag {key} must be a string, got {value}")
            });
            (key.clone(), value.to_string())
        })
        .collect()
}

/// One row per class of tag the model admits.
///
/// The classes are the model's own, from `smithy_query(query="shape", shape="Tags")`: a key
/// is a `TagKey` (`@length(min: 1, max: 128)`,
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`) and a value a `TagValue` (`@length(min: 0, max: 256)`,
/// the same character class with `*`). So the rows span ASCII letters, letters outside ASCII
/// (`\p{L}`), a separator (`\p{Z}`), digits inside and outside ASCII (`\p{N}`), every
/// punctuation character the class names, the empty value at the published minimum, and a key
/// and a value at their published maxima. Removal must not care which class a key or a value
/// falls in.
fn tag_cases() -> Vec<(String, String)> {
    vec![
        ("Environment".to_string(), "production".to_string()),
        ("Owner".to_string(), "team platform".to_string()),
        ("Project".to_string(), "日本語ログ".to_string()),
        ("Köln".to_string(), "Ünïcödé-Wërt".to_string()),
        ("cost.center:unit/42".to_string(), "+=.:/-@_".to_string()),
        ("Release".to_string(), "٤٢ 2026".to_string()),
        ("NoValue".to_string(), String::new()),
        ("k".repeat(128), "v".repeat(256)),
    ]
}

/// A created log group and the `logGroupArn` form of its ARN — the form
/// `AmazonResourceName`'s own `@pattern` (`^[\w+=/:,.@-]*$`) admits.
async fn tagged_group(logs: &Logs, name: &str) -> String {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name}))
        .await;
    logs.group_arns(name).await.1
}

#[tokio::test]
async fn removing_one_key_drops_only_that_key_whatever_class_it_falls_in() {
    let logs = Logs::new();
    let arn = tagged_group(&logs, "/aws/emulator/untag-one-at-a-time").await;

    let cases = tag_cases();
    logs.ok(TAG, &tag_body(&arn, &cases)).await;
    let mut expected: BTreeMap<String, String> = cases.iter().cloned().collect();

    for (key, value) in &cases {
        logs.ok(UNTAG, &untag_body(&arn, std::slice::from_ref(key)))
            .await;
        expected.remove(key);

        let reported = reported_tags(&logs, &arn).await;
        assert_eq!(
            reported, expected,
            "case remove {key:?} (value {value:?}): only that key leaves the stored set, \
             expected {expected:?}, got {reported:?}"
        );
    }

    assert!(
        expected.is_empty(),
        "case remove every key: the table must end with an empty set, got {expected:?}"
    );
}

#[tokio::test]
async fn one_call_removes_every_key_it_names_and_leaves_the_rest() {
    let logs = Logs::new();
    let arn = tagged_group(&logs, "/aws/emulator/untag-several-at-once").await;

    let cases = tag_cases();
    logs.ok(TAG, &tag_body(&arn, &cases)).await;

    // The keys this call removes: a contiguous half of the table, spanning several classes.
    let removed: Vec<String> = cases
        .iter()
        .take(cases.len() / 2)
        .map(|(key, _)| key.clone())
        .collect();
    logs.ok(UNTAG, &untag_body(&arn, &removed)).await;

    let expected: BTreeMap<String, String> = cases
        .iter()
        .filter(|(key, _)| !removed.contains(key))
        .cloned()
        .collect();
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported, expected,
        "case remove {removed:?} in one call: every key named leaves and every other key \
         stays, expected {expected:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn a_removal_is_idempotent_and_a_key_the_resource_never_carried_is_a_no_op() {
    let logs = Logs::new();
    let arn = tagged_group(&logs, "/aws/emulator/untag-idempotent").await;

    let cases = tag_cases();
    logs.ok(TAG, &tag_body(&arn, &cases)).await;
    let stored: BTreeMap<String, String> = cases.iter().cloned().collect();

    // Each row is a `tagKeys` list that must leave the stored set exactly as it is: no keys
    // at all (`tagKeys`'s published minimum of 0), keys no resource here carries (one, and
    // the longest the model admits), and a key that was already removed.
    let absent_long_key = "a".repeat(128);
    let no_op_rows: Vec<Vec<String>> = vec![
        vec![],
        vec!["NeverTagged".to_string()],
        vec![absent_long_key.clone()],
        vec!["NeverTagged".to_string(), absent_long_key],
    ];
    for keys in &no_op_rows {
        logs.ok(UNTAG, &untag_body(&arn, keys)).await;
        let reported = reported_tags(&logs, &arn).await;
        assert_eq!(
            reported, stored,
            "case remove {keys:?}: a key the resource does not carry removes nothing, \
             expected {stored:?}, got {reported:?}"
        );
    }

    // Removing the same key twice reaches the same set as removing it once, and a mixed list
    // of carried and absent keys removes exactly the carried ones.
    let (carried, _) = cases[0].clone();
    let mut expected = stored.clone();
    logs.ok(UNTAG, &untag_body(&arn, std::slice::from_ref(&carried)))
        .await;
    expected.remove(&carried);
    let after_first = reported_tags(&logs, &arn).await;
    assert_eq!(
        after_first, expected,
        "case remove {carried:?}: the key leaves the stored set, expected {expected:?}, \
         got {after_first:?}"
    );

    for repeat in 1..=2 {
        logs.ok(
            UNTAG,
            &untag_body(&arn, &[carried.clone(), "NeverTagged".to_string()]),
        )
        .await;
        let reported = reported_tags(&logs, &arn).await;
        assert_eq!(
            reported, expected,
            "case remove {carried:?} again (repeat {repeat}): repeating a removal reaches the \
             same set, expected {expected:?}, got {reported:?}"
        );
    }

    // What was removed can be written again, and reads back with its new value.
    logs.ok(TAG, &tag_body(&arn, &[(carried.clone(), "again".to_string())]))
        .await;
    expected.insert(carried.clone(), "again".to_string());
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported, expected,
        "case re-tag {carried:?}: a removed key can be written again, expected {expected:?}, \
         got {reported:?}"
    );
}

#[tokio::test]
async fn removing_every_key_leaves_the_resource_with_an_empty_tag_set() {
    let logs = Logs::new();
    let group = "/aws/emulator/untag-everything";
    let arn = tagged_group(&logs, group).await;

    let cases = tag_cases();
    logs.ok(TAG, &tag_body(&arn, &cases)).await;
    let keys: Vec<String> = cases.iter().map(|(key, _)| key.clone()).collect();
    logs.ok(UNTAG, &untag_body(&arn, &keys)).await;

    let reported = reported_tags(&logs, &arn).await;
    assert!(
        reported.is_empty(),
        "case remove all {} keys: the resource carries no tag, got {reported:?}",
        keys.len()
    );

    // The same emptied set is what the log group's own reporter filters on: `logGroupTags`
    // selects "log groups that have specific tags", so a removed key selects nothing.
    for filter in [
        json!([{"key": "Environment", "values": ["production"]}]),
        json!([{"key": "Environment"}]),
    ] {
        let page = logs
            .ok("ListLogGroups", &json!({"logGroupTags": filter}))
            .await;
        let names: Vec<String> = page
            .get("logGroups")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {filter}: ListLogGroups must report logGroups, got {page}")
            })
            .iter()
            .filter_map(|entry| entry.get("logGroupName").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect();
        assert!(
            names.is_empty(),
            "case {filter}: a removed tag selects no log group, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_removal_touches_only_the_resource_its_arn_names() {
    let logs = Logs::new();

    // Two log groups and one delivery, each carrying the same keys, so a removal that
    // reached the wrong record would show.
    let first = "/aws/emulator/untag-scoped-first";
    let second = "/aws/emulator/untag-scoped-second";
    let first_arn = tagged_group(&logs, first).await;
    let second_arn = tagged_group(&logs, second).await;

    // `CreateDelivery` is still a scaffold stub
    // (`issues/things-that-do-not-work/...-logs-createdelivery-stub.md`), so the delivery is
    // seeded through the SAME constructor that operation will call; everything asserted
    // below goes over the wire.
    let delivery_id = "DLV000222";
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        store.deliveries.insert(
            delivery_id.to_string(),
            DeliveryRecord::new(
                delivery_id,
                REGION,
                "vpc-flow-logs",
                "arn:aws:s3:::vended-logs-bucket",
            ),
        );
    }
    let delivery_arn_value = delivery_arn(REGION, delivery_id);

    let cases = tag_cases();
    let all: BTreeMap<String, String> = cases.iter().cloned().collect();
    for arn in [&first_arn, &second_arn, &delivery_arn_value] {
        logs.ok(TAG, &tag_body(arn, &cases)).await;
    }

    // (the resource the call names, the resources that must keep every tag)
    let rows: Vec<(&String, Vec<&String>)> = vec![
        (&first_arn, vec![&second_arn, &delivery_arn_value]),
        (&delivery_arn_value, vec![&second_arn]),
    ];
    for (target, untouched) in rows {
        let removed: Vec<String> = cases.iter().map(|(key, _)| key.clone()).collect();
        logs.ok(UNTAG, &untag_body(target, &removed)).await;

        let reported = reported_tags(&logs, target).await;
        assert!(
            reported.is_empty(),
            "case {target}: the resource the ARN names loses its tags, got {reported:?}"
        );
        for other in untouched {
            let kept = reported_tags(&logs, other).await;
            assert_eq!(
                kept, all,
                "case {target}: removing from one resource leaves {other} untouched, \
                 expected {all:?}, got {kept:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_removal_frees_room_under_the_tag_quota() {
    let logs = Logs::new();
    let arn = tagged_group(&logs, "/aws/emulator/untag-at-the-quota").await;

    // Fill the resource to the documented quota (`MAX_TAGS_PER_RESOURCE`, from
    // `TagResource`: "You can associate as many as 50 tags with a CloudWatch Logs resource").
    let quota = MAX_TAGS_PER_RESOURCE;
    let all: Vec<(String, String)> = (0..quota)
        .map(|index| (format!("key{index:03}"), format!("value{index:03}")))
        .collect();
    logs.ok(TAG, &tag_body(&arn, &all)).await;

    // At the quota a NEW key is refused.
    let code = logs
        .err(
            TAG,
            &tag_body(&arn, &[("OneTooMany".to_string(), "x".to_string())]),
        )
        .await;
    assert_eq!(
        code, "TooManyTagsException",
        "case {} tags: a merge past the quota is refused, got {code}",
        quota + 1
    );

    // Removing one key leaves room for exactly one more, so the quota counts the stored set
    // and nothing else.
    let (dropped, _) = all[0].clone();
    logs.ok(UNTAG, &untag_body(&arn, std::slice::from_ref(&dropped)))
        .await;
    let mut expected: BTreeMap<String, String> = all.iter().cloned().collect();
    expected.remove(&dropped);
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported.len(),
        quota - 1,
        "case remove {dropped}: the stored set shrinks by one, expected {} tags, got {}",
        quota - 1,
        reported.len()
    );
    assert_eq!(
        reported, expected,
        "case remove {dropped}: only that key left, expected {expected:?}, got {reported:?}"
    );

    logs.ok(
        TAG,
        &tag_body(&arn, &[("OneTooMany".to_string(), "x".to_string())]),
    )
    .await;
    expected.insert("OneTooMany".to_string(), "x".to_string());
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported, expected,
        "case re-fill to the quota: the freed room admits one new key, expected \
         {expected:?}, got {reported:?}"
    );

    // And the resource is back at the quota, so the next new key is refused again.
    let code = logs
        .err(
            TAG,
            &tag_body(&arn, &[("AndOneMore".to_string(), "x".to_string())]),
        )
        .await;
    assert_eq!(
        code, "TooManyTagsException",
        "case {} tags again: the quota still binds after a removal, got {code}",
        quota + 1
    );
}

#[tokio::test]
async fn an_arn_naming_no_stored_taggable_resource_is_refused() {
    let logs = Logs::new();
    let group = "/aws/emulator/untag-refusals";
    tagged_group(&logs, group).await;

    // (resourceArn, the refusal). Each row is an ARN shape the operation must tell apart: a
    // taggable kind the account holds no instance of is a missing RESOURCE; anything that is
    // not a taggable CloudWatch Logs resource is a bad PARAMETER.
    let absent_group = log_group_arn(REGION, "/aws/emulator/never-created", false);
    // The `LogGroup$arn` form carries a trailing `:*`, which `AmazonResourceName`'s
    // `@pattern` (`^[\w+=/:,.@-]*$`) does not admit, so it is a bad parameter rather than a
    // missing resource — for the group that EXISTS as much as for one that does not.
    let existing_group_wildcard = log_group_arn(REGION, group, true);
    let absent_group_wildcard = log_group_arn(REGION, "/aws/emulator/never-created", true);
    // A destination is a taggable kind: `PutDestination` is a stub, so the account holds
    // none, which is a missing resource rather than a bad parameter.
    let destination = format!(
        "arn:aws:logs:{REGION}:{}:destination:vended-logs",
        engine::emulator_account_id()
    );
    let delivery = delivery_arn(REGION, "DLVNOTSTORED");
    let log_stream = format!(
        "arn:aws:logs:{REGION}:{}:log-group:{group}:log-stream:one",
        engine::emulator_account_id()
    );
    let query_result = format!(
        "arn:aws:logs:{REGION}:{}:query-result:*",
        engine::emulator_account_id()
    );
    let cases: Vec<(&str, &str)> = vec![
        (absent_group.as_str(), "ResourceNotFoundException"),
        (existing_group_wildcard.as_str(), "InvalidParameterException"),
        (absent_group_wildcard.as_str(), "InvalidParameterException"),
        (destination.as_str(), "ResourceNotFoundException"),
        (delivery.as_str(), "ResourceNotFoundException"),
        (log_stream.as_str(), "InvalidParameterException"),
        (query_result.as_str(), "InvalidParameterException"),
        (
            "arn:aws:s3:::vended-logs-bucket",
            "InvalidParameterException",
        ),
        ("not-an-arn", "InvalidParameterException"),
    ];

    for (resource_arn, expected) in cases {
        let code = logs
            .err(UNTAG, &untag_body(resource_arn, &["Key".to_string()]))
            .await;
        assert_eq!(
            code, expected,
            "case {resource_arn}: {UNTAG} must refuse with {expected}, got {code}"
        );

        // The same ARN is refused with the same code when the request names no key at all,
        // so the resource is resolved before the key list is applied.
        let empty_code = logs.err(UNTAG, &untag_body(resource_arn, &[])).await;
        assert_eq!(
            empty_code, expected,
            "case {resource_arn} with no tagKeys: {UNTAG} must refuse with {expected}, \
             got {empty_code}"
        );
    }
}

#[tokio::test]
async fn a_request_past_the_modelled_tag_key_maximum_is_refused_at_the_boundary() {
    let logs = Logs::new();
    let arn = tagged_group(&logs, "/aws/emulator/untag-over-length").await;
    logs.ok(
        TAG,
        &tag_body(&arn, &[("Keep".to_string(), "me".to_string())]),
    )
    .await;

    // `tagKeys` is `@length(min: 0, max: 50)`, from
    // `smithy_query(query="operation", operation="UntagResource")`. One past that maximum is
    // refused before the handler runs, and the stored set does not move.
    let over_length: Vec<String> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| format!("key{index:03}"))
        .collect();
    let code = logs.err(UNTAG, &untag_body(&arn, &over_length)).await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {} tagKeys: more than the modelled maximum is refused with this operation's \
         own refusal code, got {code}",
        over_length.len()
    );

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    expected.insert("Keep".to_string(), "me".to_string());
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported, expected,
        "case refused removal: a refused call removes no tag, expected {expected:?}, \
         got {reported:?}"
    );
}

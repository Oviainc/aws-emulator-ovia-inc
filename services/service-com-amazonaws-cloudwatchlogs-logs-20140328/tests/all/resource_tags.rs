// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TagResource` and `ListTagsForResource`: the tag set a CloudWatch
//! Logs resource carries.
//!
//! The rule under test: a tagging operation names its resource by ARN; `TagResource` MERGES
//! the request's tags into the tag set that resource already holds — a key the resource does
//! not carry is added, a key it does carry takes the new value, every other key is left
//! where it is — and `ListTagsForResource` reports that one stored set, whatever characters
//! the model's `Tags` map admits in a key or a value. An ARN naming a taggable resource the
//! account does not hold is refused `ResourceNotFoundException`, one naming no taggable
//! CloudWatch Logs resource at all `InvalidParameterException`, and a merge that would push
//! the resource past the documented 50-tag quota `TooManyTagsException`, leaving the stored
//! set exactly as it was.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass, and the tags written
//! through one operation are read back through another.
//!
//! The table spans what the rule generalises over rather than one request: tag keys and
//! values in every character class the model's `Tags` shape admits (ASCII, non-ASCII
//! letters, separators, digits and each punctuation character of the class) at both
//! published length bounds and at the empty value; one tag per call and several at once; the
//! two ARN forms a log group carries; a log group and a delivery as the tagged resource; tag
//! counts from zero through the published quota and one past it; and every ARN shape the two
//! operations refuse.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    delivery_arn, log_group_arn, ComAmazonawsCloudwatchlogsLogs20140328State, DeliveryRecord,
    MAX_TAGS_PER_RESOURCE,
};

use crate::harness::{Logs, REGION};

/// The operation that writes tags.
const TAG: &str = "TagResource";

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
/// The classes are the model's own, from
/// `smithy_query(query="shape", shape="Tags")`: a key is a `TagKey`
/// (`@length(min: 1, max: 128)`, `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`) and a value a `TagValue`
/// (`@length(min: 0, max: 256)`, the same character class with `*`). So the rows span ASCII
/// letters, letters outside ASCII (`\p{L}`), a separator (`\p{Z}`), digits inside and
/// outside ASCII (`\p{N}`), every punctuation character the class names, the empty value at
/// the published minimum, and a key and a value at their published maxima.
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

/// A created log group and its two ARN forms: `arn` (with the trailing `:*`) and
/// `logGroupArn` (without it).
async fn tagged_group(logs: &Logs, name: &str) -> (String, String) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name}))
        .await;
    logs.group_arns(name).await
}

#[tokio::test]
async fn tagging_appends_each_new_key_and_listing_reports_every_stored_tag() {
    let logs = Logs::new();
    let group = "/aws/emulator/tags-appended";
    let (_, arn) = tagged_group(&logs, group).await;

    // A resource that has never been tagged carries the empty tag set.
    let initial = reported_tags(&logs, &arn).await;
    assert!(
        initial.is_empty(),
        "case untagged: a resource with no tags reports an empty tag set, got {initial:?}"
    );

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in tag_cases() {
        logs.ok(TAG, &tag_body(&arn, &[(key.clone(), value.clone())]))
            .await;
        expected.insert(key.clone(), value.clone());

        let reported = reported_tags(&logs, &arn).await;
        assert_eq!(
            reported, expected,
            "case {key:?}={value:?}: a new tag key is appended to the set the resource \
             already holds, expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn one_call_merges_every_tag_it_names() {
    let logs = Logs::new();
    let group = "/aws/emulator/tags-merged-at-once";
    let (_, arn) = tagged_group(&logs, group).await;

    let cases = tag_cases();
    logs.ok(TAG, &tag_body(&arn, &cases)).await;

    let expected: BTreeMap<String, String> = cases.iter().cloned().collect();
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported, expected,
        "case {} tags in one call: every tag the call names is stored, expected {expected:?}, \
         got {reported:?}",
        cases.len()
    );
}

#[tokio::test]
async fn tagging_a_key_the_resource_already_carries_replaces_only_that_value() {
    let logs = Logs::new();
    let group = "/aws/emulator/tags-replaced";
    let (_, arn) = tagged_group(&logs, group).await;

    // The tags every case must leave untouched.
    let untouched = [
        ("Keep".to_string(), "me".to_string()),
        ("AlsoKeep".to_string(), "日本語".to_string()),
    ];
    logs.ok(TAG, &tag_body(&arn, &untouched)).await;

    // (key, first value, replacement) — the replacement crosses the same classes as
    // [`tag_cases`], including both directions of the empty value and the longest value.
    let cases: Vec<(String, String, String)> = vec![
        (
            "Environment".to_string(),
            "staging".to_string(),
            "production".to_string(),
        ),
        (
            "Project".to_string(),
            "日本語ログ".to_string(),
            "にほんご".to_string(),
        ),
        (
            "Emptied".to_string(),
            "value".to_string(),
            String::new(),
        ),
        (
            "Filled".to_string(),
            String::new(),
            "value".to_string(),
        ),
        (
            "k".repeat(128),
            "v".to_string(),
            "v".repeat(256),
        ),
    ];

    let mut expected: BTreeMap<String, String> = untouched.iter().cloned().collect();
    for (key, first, replacement) in cases {
        logs.ok(TAG, &tag_body(&arn, &[(key.clone(), first.clone())]))
            .await;
        expected.insert(key.clone(), first.clone());
        let after_first = reported_tags(&logs, &arn).await;
        assert_eq!(
            after_first, expected,
            "case {key:?}={first:?}: the first value is stored, expected {expected:?}, \
             got {after_first:?}"
        );

        logs.ok(TAG, &tag_body(&arn, &[(key.clone(), replacement.clone())]))
            .await;
        expected.insert(key.clone(), replacement.clone());
        let after_replacement = reported_tags(&logs, &arn).await;
        assert_eq!(
            after_replacement, expected,
            "case {key:?}={first:?}->{replacement:?}: the new value replaces the previous one \
             and no other key moves, expected {expected:?}, got {after_replacement:?}"
        );
    }
}

#[tokio::test]
async fn the_tag_set_is_named_by_the_log_group_arn_the_member_admits() {
    let logs = Logs::new();
    let group = "/aws/emulator/tags-by-arn";

    // Tags assigned when the group was created are the same stored set this pair reads.
    logs.ok(
        "CreateLogGroup",
        &json!({"logGroupName": group, "tags": {"CreatedWith": "create-log-group"}}),
    )
    .await;
    let (wildcard_arn, plain_arn) = logs.group_arns(group).await;

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    expected.insert("CreatedWith".to_string(), "create-log-group".to_string());
    let reported = reported_tags(&logs, &plain_arn).await;
    assert_eq!(
        reported, expected,
        "case {plain_arn}: the tags a create assigned are reported, expected {expected:?}, \
         got {reported:?}"
    );

    // What `TagResource` writes joins that same set, and `ListTagsForResource` reads it
    // back.
    logs.ok(
        TAG,
        &tag_body(&plain_arn, &[("TaggedWith".to_string(), TAG.to_string())]),
    )
    .await;
    expected.insert("TaggedWith".to_string(), TAG.to_string());
    let reported = reported_tags(&logs, &plain_arn).await;
    assert_eq!(
        reported, expected,
        "case {plain_arn}: a tag written joins the stored set, expected {expected:?}, \
         got {reported:?}"
    );

    // The OTHER ARN form a log group carries — `LogGroup$arn`, with the trailing `:*` — is
    // not a value this member admits: `AmazonResourceName`'s own `@pattern`
    // (`^[\w+=/:,.@-]*$`, from `smithy_query(query="shape", shape="AmazonResourceName")`)
    // has no `*` in its character class, so the boundary refuses it before any resource is
    // resolved.
    for operation in [TAG, LIST] {
        let body = if operation == TAG {
            tag_body(&wildcard_arn, &[("Key".to_string(), "value".to_string())])
        } else {
            json!({"resourceArn": wildcard_arn})
        };
        let code = logs.err(operation, &body).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {wildcard_arn}: {operation} must refuse an ARN outside the member's pattern, \
             got {code}"
        );
    }
}

#[tokio::test]
async fn a_merge_past_the_tag_quota_is_refused_and_keeps_the_stored_set() {
    let logs = Logs::new();
    let group = "/aws/emulator/tags-at-the-quota";
    let (_, arn) = tagged_group(&logs, group).await;

    // Fill the resource to the documented quota in two calls, so the quota is reached by
    // MERGING rather than by one request.
    let quota = MAX_TAGS_PER_RESOURCE;
    let all: Vec<(String, String)> = (0..quota)
        .map(|index| (format!("key{index:03}"), format!("value{index:03}")))
        .collect();
    let (first_half, second_half) = all.split_at(quota / 2);
    logs.ok(TAG, &tag_body(&arn, first_half)).await;
    logs.ok(TAG, &tag_body(&arn, second_half)).await;

    let mut expected: BTreeMap<String, String> = all.iter().cloned().collect();
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported, expected,
        "case {quota} tags: a resource holds the whole quota, expected {expected:?}, \
         got {reported:?}"
    );

    // At the quota, re-tagging a key the resource already carries adds nothing to the
    // total, so it is accepted.
    let (existing_key, _) = all[0].clone();
    logs.ok(
        TAG,
        &tag_body(&arn, &[(existing_key.clone(), "replaced".to_string())]),
    )
    .await;
    expected.insert(existing_key.clone(), "replaced".to_string());
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported, expected,
        "case replace {existing_key} at the quota: replacing a value does not grow the set, \
         expected {expected:?}, got {reported:?}"
    );

    // One more NEW key would push the resource past the quota.
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
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported, expected,
        "case refused merge: a refused call assigns no tag, expected {expected:?}, \
         got {reported:?}"
    );

    // A single request carrying more than the modelled `Tags` maximum is refused at the
    // boundary, with this operation's own refusal code.
    let over_length: Vec<(String, String)> = (0..=quota)
        .map(|index| (format!("over{index:03}"), "x".to_string()))
        .collect();
    let code = logs.err(TAG, &tag_body(&arn, &over_length)).await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {} tags in one request: more than the modelled maximum is refused, got {code}",
        over_length.len()
    );
}

#[tokio::test]
async fn a_delivery_carries_its_own_tag_set() {
    let logs = Logs::new();

    // `CreateDelivery` is still a scaffold stub
    // (`issues/things-that-do-not-work/...-logs-createdelivery-stub.md`), so the delivery is
    // seeded through the SAME constructor that operation will call; everything asserted
    // below goes over the wire.
    let delivery_id = "DLV000111";
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
    let arn = delivery_arn(REGION, delivery_id);

    let initial = reported_tags(&logs, &arn).await;
    assert!(
        initial.is_empty(),
        "case untagged delivery: an untagged delivery reports an empty tag set, got {initial:?}"
    );

    let cases = tag_cases();
    logs.ok(TAG, &tag_body(&arn, &cases)).await;
    let expected: BTreeMap<String, String> = cases.iter().cloned().collect();
    let reported = reported_tags(&logs, &arn).await;
    assert_eq!(
        reported, expected,
        "case delivery {delivery_id}: a delivery's tags round-trip, expected {expected:?}, \
         got {reported:?}"
    );

    // The same stored set is what the resource's own reporter reports.
    let page = logs.ok("DescribeDeliveries", &json!({})).await;
    let through_describe: BTreeMap<String, String> = page
        .get("deliveries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeDeliveries must report deliveries, got {page}"))
        .iter()
        .find(|delivery| delivery.get("id").and_then(Value::as_str) == Some(delivery_id))
        .and_then(|delivery| delivery.get("tags"))
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case delivery {delivery_id}: it must report tags, got {page}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        through_describe, expected,
        "case delivery {delivery_id}: one resource holds ONE tag set, expected {expected:?}, \
         got {through_describe:?}"
    );
}

#[tokio::test]
async fn tags_written_through_tag_resource_are_what_the_log_group_list_filters_on() {
    let logs = Logs::new();
    let tagged = "/aws/emulator/tags-filtered-in";
    let untagged = "/aws/emulator/tags-filtered-out";
    let (_, tagged_arn) = tagged_group(&logs, tagged).await;
    tagged_group(&logs, untagged).await;

    logs.ok(
        TAG,
        &tag_body(
            &tagged_arn,
            &[("Project".to_string(), "日本語ログ".to_string())],
        ),
    )
    .await;

    // `logGroupTags` selects "log groups that have specific tags", so it must select on the
    // set `TagResource` wrote — key with a value, key with any value, and a value no group
    // carries.
    let cases: Vec<(Value, Vec<&str>)> = vec![
        (json!([{"key": "Project", "values": ["日本語ログ"]}]), vec![tagged]),
        (json!([{"key": "Project"}]), vec![tagged]),
        (json!([{"key": "Project", "values": ["other"]}]), vec![]),
        (json!([{"key": "Absent"}]), vec![]),
    ];
    for (filter, expected) in cases {
        let page = logs
            .ok("ListLogGroups", &json!({"logGroupTags": filter}))
            .await;
        let names: Vec<String> = page
            .get("logGroups")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {filter}: ListLogGroups must report logGroups, got {page}"))
            .iter()
            .filter_map(|group| group.get("logGroupName").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            names, expected,
            "case {filter}: the tag filter selects on the stored tag set, expected \
             {expected:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let logs = Logs::new();
    let group = "/aws/emulator/tags-are-read";
    let (_, arn) = tagged_group(&logs, group).await;
    logs.ok(
        TAG,
        &tag_body(&arn, &[("Environment".to_string(), "production".to_string())]),
    )
    .await;

    let first = logs.ok(LIST, &json!({"resourceArn": arn})).await;
    let second = logs.ok(LIST, &json!({"resourceArn": arn})).await;
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
        vec![("Environment".to_string(), "production".to_string())],
        "case repeated read: the store still holds exactly the tags that were written, \
         got {stored:?}"
    );
}

#[tokio::test]
async fn an_arn_naming_no_stored_taggable_resource_is_refused() {
    let logs = Logs::new();
    let group = "/aws/emulator/tags-refusals";
    tagged_group(&logs, group).await;

    // (resourceArn, the refusal both operations answer). Each row is an ARN shape the pair
    // must tell apart: a taggable kind the account holds no instance of is a missing
    // RESOURCE; anything that is not a taggable CloudWatch Logs resource is a bad PARAMETER.
    let absent_group = log_group_arn(REGION, "/aws/emulator/never-created", false);
    let absent_group_wildcard = log_group_arn(REGION, "/aws/emulator/never-created", true);    let destination = format!(
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
        // The `LogGroup$arn` form carries a trailing `:*`, which `AmazonResourceName`'s
        // `@pattern` does not admit, so it is a bad parameter rather than a missing
        // resource.
        (absent_group_wildcard.as_str(), "InvalidParameterException"),
        // A destination is a taggable kind: this account was never put one under that
        // name, which is a missing resource rather than a bad parameter.
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
        let tag_code = logs
            .err(
                TAG,
                &tag_body(resource_arn, &[("Key".to_string(), "value".to_string())]),
            )
            .await;
        assert_eq!(
            tag_code, expected,
            "case {resource_arn}: {TAG} must refuse with {expected}, got {tag_code}"
        );

        let list_code = logs.err(LIST, &json!({"resourceArn": resource_arn})).await;
        assert_eq!(
            list_code, expected,
            "case {resource_arn}: {LIST} must refuse with {expected}, got {list_code}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) tag surface, end to end over the wire:
//! `TagResource`, `ListTagsForResource` and `UntagResource`, against the rules AWS
//! documents for them.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.
//!
//! The rules under test:
//!
//! * `TagResource` "[a]ssociates tags with the specified AWS resource. Tags are
//!   key:value pairs" — so the stored tag set is keyed by `Key`: a key the resource
//!   already carries takes the value the new request gives it, and a key repeated inside
//!   one request keeps the last value given for it.
//! * "You can use this action to tag the AWS resources that you manage through AWS WAF
//!   Classic: web ACLs, rule groups, and rules" — each of those kinds carries its own
//!   tag set, reached through its own ARN, and an ARN that names no resource of a
//!   taggable kind this service holds names no tag set: both operations refuse it
//!   with the modelled `WAFNonexistentItemException`.
//! * "up to 50 tags for a resource" — a request whose resulting tag set would exceed
//!   that quota is refused with the modelled `WAFLimitsExceededException` and changes
//!   nothing.
//! * `ListTagsForResource` "[r]etrieves the tags associated with the specified AWS
//!   resource", paginated by the modelled `Limit`/`NextMarker` pair — and it reports its
//!   pagination state POSITIVELY: `NextMarker` is present on every successful response,
//!   naming the key the next page resumes at while one remains and EMPTY once the listing
//!   is complete, never absent.
//! * `UntagResource` removes the tags `TagKeys` names from that same set, so a key it
//!   names that the resource does not carry leaves the set as it was.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_TAGS_PER_RESOURCE, MESSAGE_LIMITS_EXCEEDED, MESSAGE_NONEXISTENT_ITEM,
};

use crate::wire::{call, change_token, error_message, expect_error, ok_call, router, router_on};

/// The account id every ARN in this suite carries. This emulator serves a single
/// account, so the field's value identifies nothing — see
/// [`the_arn_fields_that_identify_a_record`].
const ACCOUNT: &str = "000000000000";

/// The region the wire harness signs its requests for.
const REGION: &str = "us-east-1";

/// An id this service never minted.
const UNKNOWN_ID: &str = "11111111-2222-4333-8444-555555555555";

/// One row of a tag table: the case label, the `Tags` the request carries, and the tag
/// set the resource must carry afterwards.
type TagCase<'a> = (&'a str, Value, &'a [(&'a str, &'a str)]);

/// The ARN of the `Rule` with id `rule_id`, in the AWS ARN format
/// `arn:<partition>:<service>:<region>:<account-id>:<resource-type>/<id>`.
fn rule_arn(rule_id: &str) -> String {
    format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:rule/{rule_id}")
}

/// Create a `Rule` (optionally with `Tags`) and return its `RuleId`.
async fn create_rule(router: &MultiServiceRouter, name: &str, tags: Option<Value>) -> String {
    let token = change_token(router).await;
    let mut request = json!({"Name": name, "MetricName": "tagMetric", "ChangeToken": token});
    if let Some(tags) = tags {
        request["Tags"] = tags;
    }
    let body = ok_call(router, "CreateRule", &request).await;
    body["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRule returned no RuleId: {body}"))
        .to_string()
}

/// `TagResource`, expecting 200.
async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &Value) -> Value {
    ok_call(
        router,
        "TagResource",
        &json!({"ResourceARN": arn, "Tags": tags}),
    )
    .await
}

/// One page of `ListTagsForResource`, expecting 200. `limit` of 0 is the model's
/// declared default (the service's own page size).
async fn list_tags_page(
    router: &MultiServiceRouter,
    arn: &str,
    limit: u32,
    marker: Option<&str>,
) -> Value {
    let mut request = json!({"ResourceARN": arn, "Limit": limit});
    if let Some(marker) = marker {
        request["NextMarker"] = json!(marker);
    }
    ok_call(router, "ListTagsForResource", &request).await
}

/// The `TagList` of a `ListTagsForResource` page, as the `(Key, Value)` pairs it
/// carries in the order it carries them.
fn tag_pairs(body: &Value) -> Vec<(String, String)> {
    body["TagInfoForResource"]["TagList"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTagsForResource returned no TagList: {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every tag a resource carries, read back through `ListTagsForResource` (one page: a
/// resource carries at most [`MAX_TAGS_PER_RESOURCE`] tags, below the service's page
/// size).
async fn stored_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let body = list_tags_page(router, arn, 0, None).await;
    assert_eq!(
        body["TagInfoForResource"]["ResourceARN"].as_str(),
        Some(arn),
        "ListTagsForResource must report the ARN of the resource it read: {body}"
    );
    assert_eq!(
        body["NextMarker"].as_str(),
        Some(""),
        "a tag set that fits one page reports a complete listing's empty NextMarker: {body}"
    );
    tag_pairs(&body).into_iter().collect()
}

/// `TagResource` stores the tag set it is given and `ListTagsForResource` reads it back,
/// over a table of tag sets differing in count, key and value length, the characters
/// they use, and whether a key repeats inside the request.
#[tokio::test]
async fn tag_resource_persists_each_tag_set_and_list_reads_it_back() {
    let router = router();
    let max_key = "k".repeat(128);
    let max_value = "v".repeat(256);
    let full_set: Vec<Value> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| json!({"Key": format!("key-{index:02}"), "Value": format!("value-{index}")}))
        .collect();
    let full_expected: Vec<(&str, &str)> = Vec::new();

    let cases: &[TagCase<'_>] = &[
        // (label, Tags, the tag set the resource must carry afterwards). `TagKey` is
        // len 1..128 pattern \S; `TagValue` is len 0..256 pattern .* — both from the
        // model's `Tag` members.
        (
            "one tag",
            json!([{"Key": "customer", "Value": "example-corp"}]),
            &[("customer", "example-corp")],
        ),
        (
            "several tags, given out of key order",
            json!([
                {"Key": "zone", "Value": "b"},
                {"Key": "customer", "Value": "example-corp"},
                {"Key": "env", "Value": "prod"}
            ]),
            &[
                ("customer", "example-corp"),
                ("env", "prod"),
                ("zone", "b"),
            ],
        ),
        (
            "an empty value (TagValue's minimum length)",
            json!([{"Key": "empty", "Value": ""}]),
            &[("empty", "")],
        ),
        (
            "one character in key and value",
            json!([{"Key": "k", "Value": "v"}]),
            &[("k", "v")],
        ),
        (
            "a key and a value at their maximum lengths",
            json!([{"Key": max_key.clone(), "Value": max_value.clone()}]),
            &[(max_key.as_str(), max_value.as_str())],
        ),
        (
            "values carrying spaces, punctuation and non-ASCII characters",
            json!([
                {"Key": "owner", "Value": "Ops Team / EU"},
                {"Key": "ville", "Value": "Münchenstraße"}
            ]),
            &[("owner", "Ops Team / EU"), ("ville", "Münchenstraße")],
        ),
        (
            "a key repeated inside one request keeps the last value given",
            json!([
                {"Key": "stage", "Value": "first"},
                {"Key": "stage", "Value": "second"},
                {"Key": "stage", "Value": "third"}
            ]),
            &[("stage", "third")],
        ),
        (
            "the per-resource quota's worth of tags",
            json!(full_set),
            // Checked against the generated set below rather than spelled out here.
            full_expected.as_slice(),
        ),
    ];

    for (label, tags, expected) in cases {
        let rule_id = create_rule(&router, "tagged-rule", None).await;
        let arn = rule_arn(&rule_id);
        assert!(
            stored_tags(&router, &arn).await.is_empty(),
            "case {label}: a Rule created without Tags carries none"
        );

        let tagged = tag_resource(&router, &arn, tags).await;
        assert_eq!(
            tagged,
            json!({}),
            "case {label}: TagResource answers an empty response, got {tagged}"
        );

        let expected: BTreeMap<String, String> = if expected.is_empty() {
            (0..MAX_TAGS_PER_RESOURCE)
                .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
                .collect()
        } else {
            expected
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect()
        };
        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, expected,
            "case {label}: ListTagsForResource must read back the stored tag set, got {actual:?}"
        );
    }
}

/// A second `TagResource` call merges into the tag set the resource already carries:
/// a new key is added, a key it already carries is re-valued, and no other key moves —
/// including the keys `CreateRule` stored.
#[tokio::test]
async fn tag_resource_merges_into_the_tag_set_the_resource_already_carries() {
    let router = router();
    let rule_id = create_rule(
        &router,
        "merged-rule",
        Some(json!([
            {"Key": "customer", "Value": "example-corp"},
            {"Key": "env", "Value": "staging"}
        ])),
    )
    .await;
    let arn = rule_arn(&rule_id);

    // The tags `CreateRule` stored and the tags `TagResource` writes are ONE set.
    let at_create = stored_tags(&router, &arn).await;
    assert_eq!(
        at_create,
        BTreeMap::from([
            ("customer".to_string(), "example-corp".to_string()),
            ("env".to_string(), "staging".to_string()),
        ]),
        "ListTagsForResource must report the tags CreateRule stored, got {at_create:?}"
    );

    let steps: &[TagCase<'_>] = &[
        (
            "a key the resource does not carry is added",
            json!([{"Key": "zone", "Value": "b"}]),
            &[
                ("customer", "example-corp"),
                ("env", "staging"),
                ("zone", "b"),
            ],
        ),
        (
            "a key the resource carries takes the new value",
            json!([{"Key": "env", "Value": "prod"}]),
            &[("customer", "example-corp"), ("env", "prod"), ("zone", "b")],
        ),
        (
            "the same value again leaves the set as it was",
            json!([{"Key": "env", "Value": "prod"}]),
            &[("customer", "example-corp"), ("env", "prod"), ("zone", "b")],
        ),
        (
            "one request adds and re-values at once",
            json!([
                {"Key": "customer", "Value": "other-corp"},
                {"Key": "owner", "Value": "ops"}
            ]),
            &[
                ("customer", "other-corp"),
                ("env", "prod"),
                ("owner", "ops"),
                ("zone", "b"),
            ],
        ),
    ];

    for (label, tags, expected) in steps {
        tag_resource(&router, &arn, tags).await;
        let expected: BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, expected,
            "case {label}: the merged tag set must be {expected:?}, got {actual:?}"
        );
    }

    // The record itself is untouched by tagging: GetRule still reads it back.
    let fetched = ok_call(&router, "GetRule", &json!({"RuleId": rule_id})).await;
    assert_eq!(
        fetched["Rule"]["Name"].as_str(),
        Some("merged-rule"),
        "tagging must leave the Rule record as it was, got {fetched}"
    );
}

/// A request whose resulting tag set would exceed the per-resource quota is refused with
/// the modelled `WAFLimitsExceededException` and changes nothing — while a request that
/// only re-values keys the resource already carries stays within the quota and succeeds.
#[tokio::test]
async fn tagging_beyond_the_per_resource_quota_is_refused_and_changes_nothing() {
    let router = router();
    let rule_id = create_rule(&router, "quota-rule", None).await;
    let arn = rule_arn(&rule_id);

    let full: Vec<Value> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| json!({"Key": format!("key-{index:02}"), "Value": format!("value-{index}")}))
        .collect();
    tag_resource(&router, &arn, &json!(full)).await;
    let at_quota = stored_tags(&router, &arn).await;
    assert_eq!(
        at_quota.len(),
        MAX_TAGS_PER_RESOURCE,
        "the resource must carry the quota's worth of tags, got {}",
        at_quota.len()
    );

    // Each row would leave MORE than the quota's worth of keys on the resource.
    let over: &[(&str, Value)] = &[
        ("one key past the quota", json!([{"Key": "extra", "Value": "x"}])),
        (
            "several new keys at once",
            json!([{"Key": "extra-a", "Value": "x"}, {"Key": "extra-b", "Value": "y"}]),
        ),
        (
            "a new key alongside one the resource already carries",
            json!([{"Key": "key-00", "Value": "changed"}, {"Key": "extra", "Value": "x"}]),
        ),
        (
            "a whole second quota's worth of keys",
            json!((0..MAX_TAGS_PER_RESOURCE)
                .map(|index| json!({"Key": format!("other-{index:02}"), "Value": "x"}))
                .collect::<Vec<Value>>()),
        ),
    ];
    for (label, tags) in over {
        let body = expect_error(
            &router,
            "TagResource",
            &json!({"ResourceARN": arn, "Tags": tags}),
            "WAFLimitsExceededException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_LIMITS_EXCEEDED,
            "case {label}: expected the limits-exceeded sentence, got {body}"
        );
        let after = stored_tags(&router, &arn).await;
        assert_eq!(
            after, at_quota,
            "case {label}: a refused TagResource must change nothing, got {after:?}"
        );
    }

    // At the quota, re-valuing keys the resource already carries adds no entry, so it is
    // within the quota and succeeds.
    tag_resource(
        &router,
        &arn,
        &json!([{"Key": "key-00", "Value": "revalued"}, {"Key": "key-49", "Value": "revalued"}]),
    )
    .await;
    let revalued = stored_tags(&router, &arn).await;
    assert_eq!(
        revalued.len(),
        MAX_TAGS_PER_RESOURCE,
        "re-valuing keys must not grow the tag set, got {}",
        revalued.len()
    );
    assert_eq!(
        revalued.get("key-00").map(String::as_str),
        Some("revalued"),
        "the re-valued key must carry the new value, got {revalued:?}"
    );
}

/// An ARN that names no resource of a taggable kind this service holds is refused by
/// both operations with the modelled `WAFNonexistentItemException`, and the refused
/// `TagResource` writes nothing.
#[tokio::test]
async fn arns_that_name_no_taggable_resource_are_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(engine);

    // A resource of a kind AWS WAF Classic carries no tags for ("web ACLs, rule groups,
    // and rules" are the taggable kinds), so its ARN names no tag set even though the
    // record exists.
    let ip_set_token = change_token(&router).await;
    let ip_set = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "untaggable-kind", "ChangeToken": ip_set_token}),
    )
    .await;
    let ip_set_id = ip_set["IPSet"]["IPSetId"]
        .as_str()
        .expect("CreateIPSet returned an IPSetId")
        .to_string();

    let rule_id = create_rule(&router, "arn-rule", Some(json!([{"Key": "k", "Value": "v"}]))).await;
    let arn = rule_arn(&rule_id);

    let cases: &[(&str, String)] = &[
        (
            "a rule id this service never minted",
            rule_arn(UNKNOWN_ID),
        ),
        (
            "a kind that carries no tags, with an id this service did mint",
            format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:ipset/{ip_set_id}"),
        ),
        (
            "another service's ARN namespace",
            format!("arn:aws:waf:{REGION}:{ACCOUNT}:rule/{rule_id}"),
        ),
        (
            "no resource type in the resource field",
            format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:{rule_id}"),
        ),
        (
            "a resource type with no id after it",
            format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:rule/"),
        ),
        (
            "a resource type in another case",
            format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:Rule/{rule_id}"),
        ),
        (
            "a field too many",
            format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:rule/{rule_id}:v2"),
        ),
        (
            "fields missing from the ARN",
            format!("arn:aws:waf-regional:rule/{rule_id}"),
        ),
        ("a bare resource id, not an ARN", rule_id.clone()),
        ("the empty-ish ARN prefix alone", "arn".to_string()),
        (
            "a web ACL id this service never minted",
            format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:webacl/{UNKNOWN_ID}"),
        ),
        (
            "a rule group id this service never minted",
            format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:rulegroup/{UNKNOWN_ID}"),
        ),
        (
            "a rate-based rule id this service never minted",
            format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:ratebasedrule/{UNKNOWN_ID}"),
        ),
    ];

    for (label, candidate) in cases {
        for operation in ["TagResource", "ListTagsForResource", "UntagResource"] {
            let request = match operation {
                "TagResource" => {
                    json!({"ResourceARN": candidate, "Tags": [{"Key": "k", "Value": "v"}]})
                }
                "UntagResource" => json!({"ResourceARN": candidate, "TagKeys": ["k"]}),
                _ => json!({"ResourceARN": candidate}),
            };
            let body = expect_error(
                &router,
                operation,
                &request,
                "WAFNonexistentItemException",
                label,
            )
            .await;
            assert_eq!(
                error_message(&body),
                MESSAGE_NONEXISTENT_ITEM,
                "case {label}: {operation} must answer the nonexistent-item sentence, got {body}"
            );
        }
    }

    // None of those refusals touched the tag set of the resource that does exist.
    let after = stored_tags(&router, &arn).await;
    assert_eq!(
        after,
        BTreeMap::from([("k".to_string(), "v".to_string())]),
        "a refused request must leave the stored tag set as it was, got {after:?}"
    );
}

/// The ARN field that identifies a record here is the resource id: this emulator serves
/// a single account and stores no region on a resource, so the partition, region and
/// account fields are required to be present but their values are not matched. Two ARNs
/// differing only in those fields name the same resource.
#[tokio::test]
async fn the_arn_fields_that_identify_a_record() {
    let router = router();
    let rule_id = create_rule(&router, "arn-fields-rule", None).await;

    let spellings = [
        ("the region the request is signed for", rule_arn(&rule_id)),
        (
            "another region",
            format!("arn:aws:waf-regional:eu-west-1:{ACCOUNT}:rule/{rule_id}"),
        ),
        (
            "another account id",
            format!("arn:aws:waf-regional:{REGION}:210987654321:rule/{rule_id}"),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:waf-regional:{REGION}:{ACCOUNT}:rule/{rule_id}"),
        ),
    ];

    for (label, arn) in &spellings {
        tag_resource(
            &router,
            arn,
            &json!([{"Key": "written-through", "Value": label}]),
        )
        .await;
    }

    // One tag set, written through four spellings of the same resource's ARN, and read
    // back through each of them.
    let expected = BTreeMap::from([(
        "written-through".to_string(),
        spellings
            .last()
            .map(|(label, _)| (*label).to_string())
            .expect("the table is not empty"),
    )]);
    for (label, arn) in &spellings {
        let actual = stored_tags(&router, arn).await;
        assert_eq!(
            actual, expected,
            "case {label}: every spelling must reach one tag set, got {actual:?}"
        );
    }
}

/// `ListTagsForResource` pages through the tag set in one stable order, hands back the
/// marker the next page resumes at, and changes nothing a later caller sees.
#[tokio::test]
async fn listing_tags_pages_through_the_tag_set_and_changes_nothing() {
    let router = router();
    let rule_id = create_rule(&router, "paged-rule", None).await;
    let arn = rule_arn(&rule_id);

    let tags: Vec<Value> = ["zone", "owner", "env", "customer", "stage", "cost", "team"]
        .iter()
        .enumerate()
        .map(|(index, key)| json!({"Key": *key, "Value": format!("value-{index}")}))
        .collect();
    tag_resource(&router, &arn, &json!(tags)).await;
    let whole = stored_tags(&router, &arn).await;
    assert_eq!(
        whole.len(),
        7,
        "the resource must carry every tag written, got {whole:?}"
    );

    // `Limit`'s modelled @range is 0..100; 0 asks for the service's own page size.
    for limit in [1_u32, 2, 3, 7, 8, 100] {
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let body = list_tags_page(&router, &arn, limit, marker.as_deref()).await;
            let page = tag_pairs(&body);
            pages += 1;
            assert!(
                page.len() <= limit as usize,
                "case Limit={limit}: a page must hold at most {limit} tags, got {page:?}"
            );
            walked.extend(page);
            // The marker is on every page; a page that ends the listing carries it
            // empty, which is what ends the walk.
            marker = Some(
                body["NextMarker"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case Limit={limit}: every page carries NextMarker, got {body}")
                    })
                    .to_string(),
            )
            .filter(|marker| !marker.is_empty());
            if marker.is_none() {
                break;
            }
            assert!(
                pages <= 8,
                "case Limit={limit}: paging must terminate, walked {walked:?}"
            );
        }

        let mut keys: Vec<&String> = walked.iter().map(|(key, _)| key).collect();
        let sorted = {
            let mut sorted = keys.clone();
            sorted.sort();
            sorted
        };
        assert_eq!(
            keys, sorted,
            "case Limit={limit}: the walk must be in one stable order, got {keys:?}"
        );
        keys.dedup();
        assert_eq!(
            keys.len(),
            walked.len(),
            "case Limit={limit}: no tag may repeat across pages, got {walked:?}"
        );
        let collected: BTreeMap<String, String> = walked.into_iter().collect();
        assert_eq!(
            collected, whole,
            "case Limit={limit}: paging must yield the whole tag set, got {collected:?}"
        );
    }

    // A read leaves the tag set as it was for the next caller.
    let again = stored_tags(&router, &arn).await;
    assert_eq!(
        again, whole,
        "listing tags must change nothing a later caller sees, got {again:?}"
    );

    // A marker no tag carries resumes at the first key after it, and one past every key
    // yields an empty page — the same walk, started later.
    let body = list_tags_page(&router, &arn, 0, Some("d")).await;
    assert_eq!(
        tag_pairs(&body)
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<String>>(),
        vec![
            "env".to_string(),
            "owner".to_string(),
            "stage".to_string(),
            "team".to_string(),
            "zone".to_string()
        ],
        "a marker between two keys resumes at the next one, got {body}"
    );
    let past_the_end = list_tags_page(&router, &arn, 0, Some("zzz")).await;
    assert!(
        tag_pairs(&past_the_end).is_empty(),
        "a marker past every key yields an empty page, got {past_the_end}"
    );
    assert_eq!(
        past_the_end["NextMarker"].as_str(),
        Some(""),
        "an empty page is a complete listing: its NextMarker is present and empty, \
         got {past_the_end}"
    );
}

/// `UntagResource` removes exactly the keys it names from the resource's tag set, over a
/// table of key lists differing in count, whether each key is one the resource carries,
/// and whether a key repeats — and `ListTagsForResource` reads the result back.
#[tokio::test]
async fn untag_resource_removes_the_keys_it_names_and_leaves_the_rest() {
    let router = router();
    let full = json!([
        {"Key": "customer", "Value": "example-corp"},
        {"Key": "env", "Value": "prod"},
        {"Key": "owner", "Value": "ops"},
        {"Key": "zone", "Value": "b"}
    ]);

    // (label, TagKeys, the keys the resource must still carry afterwards)
    let cases: &[(&str, Value, &[&str])] = &[
        (
            "one key the resource carries",
            json!(["env"]),
            &["customer", "owner", "zone"],
        ),
        (
            "several keys at once",
            json!(["customer", "zone"]),
            &["env", "owner"],
        ),
        (
            "a key the resource does not carry leaves the set as it was",
            json!(["absent"]),
            &["customer", "env", "owner", "zone"],
        ),
        (
            "a key it carries alongside one it does not",
            json!(["owner", "absent"]),
            &["customer", "env", "zone"],
        ),
        (
            "the same key twice removes that one entry",
            json!(["owner", "owner"]),
            &["customer", "env", "zone"],
        ),
        ("every key the resource carries", json!(["customer", "env", "owner", "zone"]), &[]),
    ];

    for (label, tag_keys, remaining) in cases {
        let rule_id = create_rule(&router, "untagged-rule", None).await;
        let arn = rule_arn(&rule_id);
        tag_resource(&router, &arn, &full).await;

        let untagged = ok_call(
            &router,
            "UntagResource",
            &json!({"ResourceARN": arn, "TagKeys": tag_keys}),
        )
        .await;
        assert_eq!(
            untagged,
            json!({}),
            "case {label}: UntagResource answers an empty response, got {untagged}"
        );

        let after = stored_tags(&router, &arn).await;
        let keys: Vec<&str> = after.keys().map(String::as_str).collect();
        assert_eq!(
            keys, *remaining,
            "case {label}: the remaining tag set must be {remaining:?}, got {after:?}"
        );
        // The values of the keys that stayed are untouched.
        for key in *remaining {
            let expected = full
                .as_array()
                .expect("the tag table is a list")
                .iter()
                .find(|tag| tag["Key"].as_str() == Some(*key))
                .and_then(|tag| tag["Value"].as_str())
                .expect("every remaining key was written");
            assert_eq!(
                after.get(*key).map(String::as_str),
                Some(expected),
                "case {label}: {key} must keep the value it was given, got {after:?}"
            );
        }

        // Re-tagging after an untag puts a removed key back.
        tag_resource(&router, &arn, &json!([{"Key": "env", "Value": "restored"}])).await;
        let restored = stored_tags(&router, &arn).await;
        assert_eq!(
            restored.get("env").map(String::as_str),
            Some("restored"),
            "case {label}: TagResource must write a removed key back, got {restored:?}"
        );
    }
}

/// Each of the kinds AWS WAF Classic documents as taggable — "web ACLs, rule groups,
/// and rules", a rate-based rule being a rule — carries its OWN tag set, reached through
/// its own ARN: tagging one leaves the others as they were, and a web ACL is reached
/// through the very `WebACLArn` the service minted for it.
#[tokio::test]
async fn every_taggable_kind_carries_its_own_tag_set() {
    let router = router();

    let rule_arn_value = {
        let rule_id = create_rule(&router, "kinds-rule", None).await;
        rule_arn(&rule_id)
    };

    let web_acl_token = change_token(&router).await;
    let web_acl = ok_call(
        &router,
        "CreateWebACL",
        &json!({
            "Name": "kinds-web-acl",
            "MetricName": "kindsWebAcl",
            "DefaultAction": {"Type": "ALLOW"},
            "ChangeToken": web_acl_token
        }),
    )
    .await;
    // The ARN the service minted for the web ACL, used verbatim as a client would.
    let web_acl_arn = web_acl["WebACL"]["WebACLArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWebACL returned no WebACLArn: {web_acl}"))
        .to_string();

    let rule_group_token = change_token(&router).await;
    let rule_group = ok_call(
        &router,
        "CreateRuleGroup",
        &json!({
            "Name": "kinds-rule-group",
            "MetricName": "kindsRuleGroup",
            "ChangeToken": rule_group_token
        }),
    )
    .await;
    let rule_group_id = rule_group["RuleGroup"]["RuleGroupId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRuleGroup returned no RuleGroupId: {rule_group}"))
        .to_string();
    let rule_group_arn = format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:rulegroup/{rule_group_id}");

    // A rate-based rule is a `Rule` with a rate limit, and AWS names it with its own ARN
    // resource type. `RateLimit`'s modelled `@range` is 100..2000000000.
    let rate_based_token = change_token(&router).await;
    let rate_based = ok_call(
        &router,
        "CreateRateBasedRule",
        &json!({
            "Name": "kinds-rate-based-rule",
            "MetricName": "kindsRateBasedRule",
            "RateKey": "IP",
            "RateLimit": 1_000,
            "ChangeToken": rate_based_token
        }),
    )
    .await;
    let rate_based_id = rate_based["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRateBasedRule returned no RuleId: {rate_based}"))
        .to_string();
    let rate_based_arn =
        format!("arn:aws:waf-regional:{REGION}:{ACCOUNT}:ratebasedrule/{rate_based_id}");

    let kinds = [
        ("a rule", rule_arn_value, "rule-tag"),
        ("a web ACL", web_acl_arn, "web-acl-tag"),
        ("a rule group", rule_group_arn, "rule-group-tag"),
        ("a rate-based rule", rate_based_arn, "rate-based-rule-tag"),
    ];

    for (label, arn, key) in &kinds {
        tag_resource(&router, arn, &json!([{"Key": key, "Value": label}])).await;
    }
    for (label, arn, key) in &kinds {
        let actual = stored_tags(&router, arn).await;
        assert_eq!(
            actual,
            BTreeMap::from([((*key).to_string(), (*label).to_string())]),
            "case {label}: each kind must carry only its own tag set, got {actual:?}"
        );
    }

    // Untagging one kind leaves the other two as they were.
    let (_, untagged_arn, untagged_key) = &kinds[1];
    ok_call(
        &router,
        "UntagResource",
        &json!({"ResourceARN": untagged_arn, "TagKeys": [untagged_key]}),
    )
    .await;
    for (label, arn, key) in &kinds {
        let actual = stored_tags(&router, arn).await;
        let expected: BTreeMap<String, String> = if arn == untagged_arn {
            BTreeMap::new()
        } else {
            BTreeMap::from([((*key).to_string(), (*label).to_string())])
        };
        assert_eq!(
            actual, expected,
            "case {label}: untagging one resource must leave the others as they were, \
             got {actual:?}"
        );
    }
}

/// A resource that carries no tags answers with an empty `TagList` rather than a
/// refusal: the resource exists, its tag set is empty.
#[tokio::test]
async fn a_resource_with_no_tags_reports_an_empty_tag_list() {
    let router = router();
    let rule_id = create_rule(&router, "untagged-rule", None).await;
    let arn = rule_arn(&rule_id);

    let (status, body) = call(
        &router,
        "ListTagsForResource",
        &json!({"ResourceARN": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListTagsForResource on an untagged resource must answer 200, got {status}: {body}"
    );
    assert_eq!(
        body["TagInfoForResource"]["ResourceARN"].as_str(),
        Some(arn.as_str()),
        "the reported ARN must name the resource read, got {body}"
    );
    assert!(
        tag_pairs(&body).is_empty(),
        "an untagged resource carries no tags, got {body}"
    );
    assert_eq!(
        body["NextMarker"].as_str(),
        Some(""),
        "an empty tag set is a complete listing: its NextMarker is present and empty, \
         got {body}"
    );
}

/// `ListTagsForResource` reports its pagination state positively: the `NextMarker` key is
/// on EVERY successful response — the key the next page resumes at while a further page
/// remains, and empty exactly when the listing is complete.
///
/// The recorded unit is one resource with one tag read in a single page; the table below
/// is the class that rule covers, its rows differing in how many tags the resource
/// carries, the `Limit` each page asks for (the modelled `@range` 0..100, 0 being the
/// declared default that asks for the service's own page size) and whether the page is
/// the last, a truncated one, or exactly as full as the remaining tags.
#[tokio::test]
async fn every_tag_listing_page_reports_a_marker_empty_only_when_complete() {
    let router = router();

    // Rows: (tag count, Limit) spanning an empty tag set, a single tag, a page exactly as
    // full as the tag set, a page smaller than it, and the default page size.
    for (tag_count, limit) in [
        (0_usize, 0_u32),
        (0, 1),
        (1, 0),
        (1, 1),
        (1, 2),
        (3, 1),
        (3, 2),
        (3, 3),
        (3, 4),
        (3, 0),
        (9, 4),
        (9, 100),
    ] {
        let case = format!("{tag_count} tags, Limit={limit}");
        let rule_id = create_rule(&router, &format!("marker-{tag_count}-{limit}"), None).await;
        let arn = rule_arn(&rule_id);
        let keys: Vec<String> = (0..tag_count).map(|index| format!("key-{index}")).collect();
        if tag_count > 0 {
            let tags: Vec<Value> = keys
                .iter()
                .map(|key| json!({"Key": key, "Value": "v"}))
                .collect();
            tag_resource(&router, &arn, &json!(tags)).await;
        }

        // `Limit` of 0 asks for the service's own page size, which holds every tag here.
        let page_size = if limit == 0 {
            tag_count
        } else {
            (limit as usize).min(tag_count)
        };
        let mut seen = 0_usize;
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let body = list_tags_page(&router, &arn, limit, marker.as_deref()).await;
            pages += 1;
            let page = tag_pairs(&body);
            seen += page.len();
            let reported = body["NextMarker"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: every successful page carries NextMarker, got {body}")
            });

            let complete = seen >= tag_count;
            if complete {
                assert_eq!(
                    reported, "",
                    "case {case:?}: page {pages} completes the listing, so its marker is \
                     empty, got {reported:?} ({body})"
                );
                break;
            }
            // A further page remains, so the marker names the key it resumes at: the
            // first key after this page.
            assert_eq!(
                reported,
                keys[seen].as_str(),
                "case {case:?}: page {pages} must resume at {:?}, got {reported:?} ({body})",
                keys[seen]
            );
            assert_eq!(
                page.len(),
                page_size,
                "case {case:?}: a page that leaves a marker must be full, got {page:?}"
            );
            marker = Some(reported.to_string());
            assert!(
                pages <= tag_count + 1,
                "case {case:?}: paging must terminate, saw {pages} pages"
            );
        }
        assert_eq!(
            seen, tag_count,
            "case {case:?}: the walk must yield every tag, got {seen}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForDeliveryStream` over the `awsJson1_1` wire: what a stream's stored tags
//! are reported as, in key order, and how `ExclusiveStartTagKey` / `Limit` page them.
//!
//! The rule under test: the operation reports the tags the named stream holds in state,
//! ordered by key, starting after `ExclusiveStartTagKey` when one is given, at most
//! `Limit` of them, with `HasMoreTags` true exactly when a further tag was left over —
//! and `ResourceNotFoundException` when no stream of that name exists. Each test spans a
//! SET of tag sets, limits and start keys; the recorded "tags at creation" case is one row.
//!
//! The harness (`Emulator`, `s3_configuration`, `error_code`) is the lifecycle suite's, so
//! every request here goes through the same real router.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{
    error_code, s3_configuration, Emulator, PAST_THE_TRANSITION,
};

/// The modelled ceiling shared by `Limit`'s range, the tag list a create may carry and
/// the `ListTagsForDeliveryStreamOutputTagList` a response may hold
/// (`smithy_query(query="shape",shape="TagKey,ListTagsForDeliveryStreamOutputTagList")`).
pub(crate) const MAX_TAGS: usize = 50;

/// Create a stream carrying `tags`, in the order the request lists them.
pub(crate) async fn create_tagged(emulator: &Emulator, name: &str, tags: &[(String, String)]) {
    let mut body = json!({
        "DeliveryStreamName": name,
        "S3DestinationConfiguration": s3_configuration(),
    });
    // `Tags` is a list of 1..50, so "no tags" is the member being absent, not an empty list.
    if !tags.is_empty() {
        body["Tags"] = Value::Array(
            tags.iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        );
    }
    let (status, response) = emulator.call("CreateDeliveryStream", body).await;
    assert_eq!(status, 200, "create {name} with {tags:?}: got {response:?}");
}

/// Ask for a page of `name`'s tags.
pub(crate) async fn list_tags(
    emulator: &Emulator,
    name: &str,
    start_after: Option<&str>,
    limit: Option<usize>,
) -> (u16, Value) {
    let mut body = json!({ "DeliveryStreamName": name });
    if let Some(key) = start_after {
        body["ExclusiveStartTagKey"] = json!(key);
    }
    if let Some(limit) = limit {
        body["Limit"] = json!(limit);
    }
    emulator.call("ListTagsForDeliveryStream", body).await
}

/// The `(Key, Value)` pairs a response listed, in the order it listed them.
pub(crate) fn listed(response: &Value) -> Vec<(String, String)> {
    response["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tags a stream created with `given` ends up holding: key order, and a key given
/// twice folding to the value given last.
fn expected_tags(given: &[(String, String)]) -> Vec<(String, String)> {
    let mut folded: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (key, value) in given {
        folded.insert(key.clone(), value.clone());
    }
    folded.into_iter().collect()
}

/// `count` distinct tags, handed over in DESCENDING key order so a response in key order
/// cannot be insertion order by accident.
pub(crate) fn generated_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .rev()
        .map(|ordinal| (format!("key-{ordinal:02}"), format!("value-{ordinal:02}")))
        .collect()
}

pub(crate) fn owned(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn a_streams_tags_are_reported_in_key_order_whatever_set_it_was_created_with() {
    // Keys and values exercising the modelled `TagKey` / `TagValue` character classes
    // (`smithy_query(query="shape",shape="TagKey,TagValue")`:
    // `[\p{L}\p{Z}\p{N}_.:\/=+\-@%]`, and `TagValue` may be empty).
    let patterned = owned(&[
        ("dotted.key", "1.0"),
        ("colon:key", "a:b"),
        ("slash/key", "a/b"),
        ("equals=key", "a=b"),
        ("plus+key", "a+b"),
        ("dash-key", "a-b"),
        ("at@key", "a@b"),
        ("percent%key", "50%"),
        ("spaced key", "a value"),
        ("ключ", "значение"),
        ("键", "值"),
        ("digits123", "456"),
        ("under_score", "_"),
    ]);

    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("no tags at all", Vec::new()),
        ("one tag", owned(&[("env", "prod")])),
        (
            "keys handed over out of order",
            owned(&[("zone", "c"), ("app", "api"), ("env", "prod")]),
        ),
        ("an empty value", owned(&[("unset", "")])),
        (
            "the same key twice",
            owned(&[("env", "staging"), ("env", "prod")]),
        ),
        ("keys spanning the modelled character class", patterned),
        ("the modelled maximum of fifty tags", generated_tags(MAX_TAGS)),
        ("one tag short of the maximum", generated_tags(MAX_TAGS - 1)),
    ];

    for (index, (case, tags)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("tagged-stream-{index}");
        create_tagged(&emulator, &name, &tags).await;

        let (status, response) = list_tags(&emulator, &name, None, None).await;
        assert_eq!(status, 200, "case {case:?}: got {response:?}");
        let expected = expected_tags(&tags);
        assert_eq!(
            listed(&response),
            expected,
            "case {case:?}: expected {expected:?} in key order, got {response:?}"
        );
        // Every set here fits the modelled maximum, so no call leaves a tag over.
        assert_eq!(
            response["HasMoreTags"].as_bool(),
            Some(false),
            "case {case:?}: expected HasMoreTags false for {} tag(s), got {response:?}",
            expected.len()
        );
    }
}

#[tokio::test]
async fn paging_by_limit_and_start_key_walks_every_tag_exactly_once() {
    // (total tags on the stream, the `Limit` each page asks for).
    let cases: &[(usize, usize)] = &[
        (1, 1),
        (7, 1),
        (7, 2),
        (7, 3),
        (7, 7),
        (7, MAX_TAGS),
        (MAX_TAGS, 1),
        (MAX_TAGS, 7),
        (MAX_TAGS, MAX_TAGS),
    ];

    for (total, limit) in cases.iter().copied() {
        let case = format!("{total} tag(s), Limit {limit}");
        let emulator = Emulator::new();
        let tags = generated_tags(total);
        create_tagged(&emulator, "paged-stream", &tags).await;
        let expected = expected_tags(&tags);

        let mut seen: Vec<(String, String)> = Vec::new();
        let mut start: Option<String> = None;
        // At most one page per tag plus one: a page that reports more tags always
        // advances, so this bound is only a guard against a loop that does not.
        for _ in 0..=total {
            let (status, page) = list_tags(
                &emulator,
                "paged-stream",
                start.as_deref(),
                Some(limit),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: got {page:?}");
            let got = listed(&page);
            let left = total - seen.len();
            assert_eq!(
                got.len(),
                limit.min(left),
                "case {case:?}: a page after {} tag(s) holds min(Limit, remaining), got {page:?}",
                seen.len()
            );
            seen.extend(got);
            let has_more = page["HasMoreTags"].as_bool();
            assert_eq!(
                has_more,
                Some(seen.len() < total),
                "case {case:?}: HasMoreTags reports whether a tag is left after {} of {total}, \
                 got {page:?}",
                seen.len()
            );
            if has_more != Some(true) {
                break;
            }
            // "To list the remaining tags, set ExclusiveStartTagKey to the last key in the
            // response."
            start = seen.last().map(|(key, _)| key.clone());
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: paging visits every tag once, in key order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_start_key_resumes_after_it_whether_or_not_a_tag_carries_that_key() {
    let emulator = Emulator::new();
    let tags = owned(&[("alpha", "1"), ("delta", "2"), ("gamma", "3")]);
    create_tagged(&emulator, "resumed-stream", &tags).await;

    // (the start key, the keys that come after it).
    let cases: &[(&str, &[&str])] = &[
        ("alpha", &["delta", "gamma"]),
        ("delta", &["gamma"]),
        ("gamma", &[]),
        // Keys no tag carries: one before every key, one between two, one after all.
        ("aaa", &["alpha", "delta", "gamma"]),
        ("beta", &["delta", "gamma"]),
        ("zeta", &[]),
    ];

    for (start, expected_keys) in cases.iter().copied() {
        let (status, response) = list_tags(&emulator, "resumed-stream", Some(start), None).await;
        assert_eq!(status, 200, "start {start:?}: got {response:?}");
        let keys: Vec<String> = listed(&response)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(
            keys, expected_keys,
            "start {start:?}: expected {expected_keys:?}, got {response:?}"
        );
        assert_eq!(
            response["HasMoreTags"].as_bool(),
            Some(false),
            "start {start:?}: no tag is left over without a Limit, got {response:?}"
        );
    }
}

#[tokio::test]
async fn a_limit_outside_its_modelled_range_is_refused_and_each_bound_is_accepted() {
    let emulator = Emulator::new();
    create_tagged(&emulator, "bounded-stream", &generated_tags(MAX_TAGS)).await;

    // `Limit` is modelled `range 1..50`, so the boundary validator refuses a value past
    // either bound before the handler runs — a 400 `ValidationException`, never a 501.
    for limit in [0_usize, MAX_TAGS + 1] {
        let (status, refused) = list_tags(&emulator, "bounded-stream", None, Some(limit)).await;
        assert_eq!(status, 400, "Limit {limit}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "Limit {limit} is outside the modelled range, got {refused:?}"
        );
    }

    // Each published bound is accepted, and reports that many tags.
    for limit in [1_usize, MAX_TAGS] {
        let (status, response) = list_tags(&emulator, "bounded-stream", None, Some(limit)).await;
        assert_eq!(status, 200, "Limit {limit}: got {response:?}");
        assert_eq!(
            listed(&response).len(),
            limit,
            "Limit {limit}: expected that many tags, got {response:?}"
        );
        assert_eq!(
            response["HasMoreTags"].as_bool(),
            Some(limit < MAX_TAGS),
            "Limit {limit} of {MAX_TAGS} tags: got {response:?}"
        );
    }
}

#[tokio::test]
async fn listing_the_tags_of_a_stream_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    create_tagged(&emulator, "present-stream", &owned(&[("env", "prod")])).await;

    // Never created, and a name that differs from a stream that does exist.
    for name in ["absent-stream", "present-stream-2"] {
        let (status, refused) = list_tags(&emulator, name, None, None).await;
        assert_eq!(status, 400, "stream {name:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "stream {name:?}: got {refused:?}"
        );
    }

    // A stream's tags are reported while it is still CREATING, and stop being reported
    // once the clock has made its deletion due.
    let (status, response) = list_tags(&emulator, "present-stream", None, None).await;
    assert_eq!(status, 200, "a CREATING stream reports its tags: {response:?}");
    assert_eq!(
        listed(&response),
        owned(&[("env", "prod")]),
        "a CREATING stream reports the tags it was created with, got {response:?}"
    );

    // A `CREATING` stream cannot be deleted, so wait for the creation the clock owes.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (status, deleted) = emulator
        .call(
            "DeleteDeliveryStream",
            json!({ "DeliveryStreamName": "present-stream" }),
        )
        .await;
    assert_eq!(status, 200, "delete: got {deleted:?}");
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let (status, refused) = list_tags(&emulator, "present-stream", None, None).await;
    assert_eq!(status, 400, "a removed stream: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "a removed stream has no tags to list, got {refused:?}"
    );
}

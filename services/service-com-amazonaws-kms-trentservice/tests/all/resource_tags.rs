// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tags a KMS key carries — `TagResource`,
//! `UntagResource` and `ListResourceTags` — driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: a key's tags are a persisted MAP from tag key to tag value,
//! owned by the key. `TagResource` writes entries into that map (a tag key the key
//! already carries takes the value of the latest write, every tag the request does
//! not name stays as it was), `UntagResource` removes the entries its tag keys name
//! and ignores the ones the key does not carry, and `ListResourceTags` reports
//! exactly what the map holds — so a value a writer accepted is the value a reader
//! returns, byte for byte, including the empty string.
//!
//! A request is refused, and writes NOTHING, when the tag list is not one the
//! service stores (a tag key named twice, a tag key under the reserved prefix), when
//! the resulting map would exceed the per-key tag quota, or when the key's own key
//! state does not admit the write.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in the tag
//! value (ASCII, empty, multi-byte, at the modelled length bound and past it), in
//! the tag key (length, case, characters, the reserved prefix and strings that only
//! resemble it), in how many tags one request carries and one key holds, in which
//! tag keys a removal names, in the key state the key is in, and in the page size the
//! reader asks for.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    assert_empty, create_key, enabled_key, failure, ok, router, text, REGION,
};

/// A key's tags as these tests hold them: `(tag key, tag value)` pairs.
type Tags = Vec<(String, String)>;

/// A table row that writes a tag list and expects every tag the key then carries.
type EditRow = (&'static str, Tags, Tags);

/// A table row that writes a tag list and says whether the service stores it.
type StoredRow = (&'static str, Tags, bool);

/// A table row that removes tag keys and expects the tags left behind.
type RemovalRow = (
    &'static str,
    Vec<&'static str>,
    Vec<(&'static str, &'static str)>,
);

/// A table row that writes a tag list through one identifier form of a key.
type IdentifierRow<'a> = (&'static str, &'a str, Tags);

/// A tag list as the wire carries it, from `(tag key, tag value)` pairs.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(tag_key, tag_value)| json!({"TagKey": tag_key, "TagValue": tag_value}))
            .collect(),
    )
}

/// `(tag key, tag value)` pairs from borrowed string literals, for a table row.
fn pairs(tags: &[(&str, &str)]) -> Tags {
    tags.iter()
        .map(|(tag_key, tag_value)| (tag_key.to_string(), tag_value.to_string()))
        .collect()
}

/// Apply a tag list to a key, requiring the empty 200 the operation documents.
async fn tag(router: &MultiServiceRouter, case: &str, key_id: &str, tags: &Value) {
    let answer = ok(
        router,
        "TagResource",
        &json!({"KeyId": key_id, "Tags": tags}),
    )
    .await;
    assert_empty(case, "TagResource", &answer);
}

/// Remove tag keys from a key, requiring the empty 200 the operation documents.
async fn untag(router: &MultiServiceRouter, case: &str, key_id: &str, tag_keys: &Value) {
    let answer = ok(
        router,
        "UntagResource",
        &json!({"KeyId": key_id, "TagKeys": tag_keys}),
    )
    .await;
    assert_empty(case, "UntagResource", &answer);
}

/// Every tag `ListResourceTags` reports for `key_id`, paging to the end, in the
/// order the reader returned them.
async fn listed_tags(router: &MultiServiceRouter, case: &str, key_id: &str) -> Tags {
    let mut collected: Vec<(String, String)> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({"KeyId": key_id});
        if let Some(marker) = marker.as_deref() {
            request["Marker"] = json!(marker);
        }
        let answer = ok(router, "ListResourceTags", &request).await;
        let page = answer["Tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a Tags list in {answer}"))
            .clone();
        for entry in &page {
            collected.push((text(case, entry, "TagKey"), text(case, entry, "TagValue")));
        }
        let truncated = answer["Truncated"]
            .as_bool()
            .unwrap_or_else(|| panic!("case {case}: expected a Truncated flag in {answer}"));
        if !truncated {
            assert!(
                answer["NextMarker"].is_null(),
                "case {case}: a complete list carries no NextMarker, got {answer}"
            );
            break;
        }
        marker = Some(text(case, &answer, "NextMarker"));
    }
    collected
}

/// A key's tags, sorted, for comparison with an expected set.
async fn tags_of(router: &MultiServiceRouter, case: &str, key_id: &str) -> Tags {
    let mut tags = listed_tags(router, case, key_id).await;
    tags.sort();
    tags
}

/// Create a key and return its key id.
async fn key_id(router: &MultiServiceRouter) -> String {
    text("setup", &enabled_key(router).await, "KeyId")
}

/// What one write asks a key to carry is exactly what the reader reports back: the
/// tag value is persisted and returned byte for byte, whatever its bytes are.
///
/// The rows differ in the tag value (ASCII, the empty string the model allows,
/// multi-byte characters, punctuation, the modelled 256-character maximum), in the
/// tag key (the modelled 128-character maximum, case, strings that resemble the
/// reserved prefix without falling under it) and in how many tags one request
/// carries.
#[tokio::test]
async fn a_tag_write_is_reported_back_byte_for_byte() {
    let router = router();

    // Rows: the case, and the tag list written. The bounds come from
    // `smithy_query(query="shape",shape="Tag")`: `TagKey` is @length(1..128) and
    // `TagValue` is @length(0..256).
    let cases: Vec<(&str, Tags)> = vec![
        ("one ASCII tag", pairs(&[("env", "production")])),
        ("an empty tag value", pairs(&[("placeholder", "")])),
        (
            "a multi-byte tag value",
            pairs(&[("owner", "équipe sécurité 🔐 鍵")]),
        ),
        (
            "a multi-byte tag key and value",
            pairs(&[("clé-propriétaire", "valeur é")]),
        ),
        (
            "punctuation either side",
            pairs(&[("path/to:thing", "a=b,c/d:e_f-g.h +i")]),
        ),
        (
            "a tag value at the modelled maximum",
            vec![("long-value".to_string(), "v".repeat(256))],
        ),
        (
            "a tag key at the modelled maximum",
            vec![("k".repeat(128), "max-length key".to_string())],
        ),
        (
            "tag keys differing only in case",
            pairs(&[("Env", "upper"), ("env", "lower"), ("ENV", "shouting")]),
        ),
        (
            "strings that only resemble the reserved prefix",
            pairs(&[("awsome", "no colon"), ("myaws:team", "not a prefix"), ("aws", "no colon")]),
        ),
        (
            "several tags in one request",
            pairs(&[
                ("a", "1"),
                ("b", "2"),
                ("c", "3"),
                ("d", "4"),
                ("e", "5"),
                ("f", "6"),
                ("g", "7"),
            ]),
        ),
    ];

    for (case, written) in cases {
        let id = key_id(&router).await;
        tag(&router, case, &id, &tag_list(&written)).await;

        let mut expected = written.clone();
        expected.sort();
        let listed = tags_of(&router, case, &id).await;
        assert_eq!(
            listed, expected,
            "case {case}: expected the reader to report {expected:?}, got {listed:?}"
        );
    }
}

/// A tag the key already carries takes the value of the LATEST write, and a write
/// never disturbs a tag it does not name — so the map the key holds is the
/// accumulation of the writes, with the last writer of a tag key winning.
///
/// The rows are a SEQUENCE of writes, differing in whether they add a tag key, edit
/// one the key already carries, or both at once.
#[tokio::test]
async fn the_last_write_of_a_tag_key_is_the_value_the_key_carries() {
    let router = router();
    let id = key_id(&router).await;

    // Rows: the case, the tag list written, and every tag the key carries after it.
    let cases: Vec<EditRow> = vec![
        (
            "the first write",
            pairs(&[("env", "test"), ("owner", "alice")]),
            pairs(&[("env", "test"), ("owner", "alice")]),
        ),
        (
            "an edit of one tag key",
            pairs(&[("env", "staging")]),
            pairs(&[("env", "staging"), ("owner", "alice")]),
        ),
        (
            "an add and an edit together",
            pairs(&[("env", "production"), ("cost-centre", "42")]),
            pairs(&[
                ("cost-centre", "42"),
                ("env", "production"),
                ("owner", "alice"),
            ]),
        ),
        (
            "an edit to the empty string",
            pairs(&[("owner", "")]),
            pairs(&[("cost-centre", "42"), ("env", "production"), ("owner", "")]),
        ),
        (
            "an edit back to a non-empty value",
            pairs(&[("owner", "bob")]),
            pairs(&[("cost-centre", "42"), ("env", "production"), ("owner", "bob")]),
        ),
    ];

    for (case, written, expected) in cases {
        tag(&router, case, &id, &tag_list(&written)).await;
        let listed = tags_of(&router, case, &id).await;
        let mut expected = expected.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case}: expected the key to carry {expected:?}, got {listed:?}"
        );
    }
}

/// A tag key under the prefix the provider reserves is not a caller's to set: the
/// request is refused as a tag failure and the key's tags are left exactly as they
/// were. The reservation is on the prefix, so no capitalisation of it is accepted —
/// and a tag key that merely starts with the same letters, without the separator, is
/// an ordinary tag key.
///
/// The rows differ in the capitalisation of the prefix, in what follows it, in the
/// position of the reserved tag in the request, and in whether the request also
/// carries a tag the service would otherwise have stored.
#[tokio::test]
async fn a_tag_key_under_the_reserved_prefix_is_refused() {
    let router = router();

    // Rows: the case, the tag list written, and whether the service stores it.
    let cases: Vec<StoredRow> = vec![
        ("the bare reserved prefix", pairs(&[("aws:", "x")]), false),
        (
            "a reserved tag key",
            pairs(&[("aws:cloudformation:stack-name", "demo")]),
            false,
        ),
        ("the prefix in upper case", pairs(&[("AWS:billing", "x")]), false),
        ("the prefix in mixed case", pairs(&[("Aws:Team", "x")]), false),
        (
            "a reserved tag key beside a valid one",
            pairs(&[("env", "production"), ("aws:service", "kms")]),
            false,
        ),
        (
            "a reserved tag key first",
            pairs(&[("aws:service", "kms"), ("env", "production")]),
            false,
        ),
        (
            "the same letters without the separator",
            pairs(&[("aws", "stored"), ("awsome", "stored")]),
            true,
        ),
        (
            "the prefix in the middle of the tag key",
            pairs(&[("team-aws:billing", "stored")]),
            true,
        ),
        (
            "the prefix in the tag VALUE only",
            pairs(&[("note", "aws:reserved-looking value")]),
            true,
        ),
    ];

    for (case, written, stored) in cases {
        let id = key_id(&router).await;
        // A tag the key already carries, so a refusal has something to leave alone.
        let existing = pairs(&[("kept", "before")]);
        tag(&router, case, &id, &tag_list(&existing)).await;

        let request = json!({"KeyId": id, "Tags": tag_list(&written)});
        let mut expected = existing.clone();
        if stored {
            tag(&router, case, &id, &tag_list(&written)).await;
            expected.extend(written.clone());
        } else {
            let (status, code) = failure(&router, "TagResource", &request).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "TagException",
                "case {case}: expected TagException, got {code}"
            );
        }
        expected.sort();
        let listed = tags_of(&router, case, &id).await;
        assert_eq!(
            listed, expected,
            "case {case}: expected the key to carry {expected:?}, got {listed:?}"
        );
    }
}

/// "You cannot have more than one tag on a KMS key with the same tag key"
/// (`smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`): a list
/// naming one tag key twice is refused rather than collapsed to whichever entry came
/// last, wherever a request carries tags.
///
/// The rows differ in where the repeat sits in the list, in whether the two values
/// agree, and in the operation the list was sent to.
#[tokio::test]
async fn one_tag_key_twice_in_a_request_is_refused() {
    let router = router();

    // Rows: the case, and the tag list written.
    let cases: Vec<(&str, Tags)> = vec![
        ("the same tag key twice", pairs(&[("dup", "one"), ("dup", "two")])),
        (
            "the same tag key and value twice",
            pairs(&[("dup", "same"), ("dup", "same")]),
        ),
        (
            "a repeat after other tags",
            pairs(&[("a", "1"), ("b", "2"), ("a", "3")]),
        ),
    ];

    for (case, written) in cases {
        let id = key_id(&router).await;
        let (status, code) = failure(
            &router,
            "TagResource",
            &json!({"KeyId": id, "Tags": tag_list(&written)}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case} on TagResource: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "TagException",
            "case {case} on TagResource: expected TagException, got {code}"
        );
        let listed = tags_of(&router, case, &id).await;
        assert!(
            listed.is_empty(),
            "case {case}: a refused write must store nothing, got {listed:?}"
        );

        // The same list, carried by the operation that tags a key as it creates it.
        let (status, code) = failure(
            &router,
            "CreateKey",
            &json!({"Tags": tag_list(&written)}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case} on CreateKey: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "TagException",
            "case {case} on CreateKey: expected TagException, got {code}"
        );
    }
}

/// `UntagResource` removes exactly the tag keys it names: a tag key the key does not
/// carry removes nothing and still succeeds ("if the specified tag key isn't found
/// on the KMS key, it doesn't throw an exception",
/// `smithy_query(query="doc",shape="UntagResource")`), and every tag the request
/// does not name survives.
///
/// The rows differ in how many tag keys the removal names, in whether they are tags
/// the key carries, and in the order they are named.
#[tokio::test]
async fn a_removal_takes_only_the_tag_keys_it_names() {
    let router = router();

    // Rows: the case, the tag keys removed, and every tag left afterwards.
    let held = pairs(&[("a", "1"), ("b", "2"), ("c", "3"), ("empty", "")]);
    let cases: Vec<RemovalRow> = vec![
        (
            "no tag keys at all",
            vec![],
            vec![("a", "1"), ("b", "2"), ("c", "3"), ("empty", "")],
        ),
        (
            "one tag key the key carries",
            vec!["b"],
            vec![("a", "1"), ("c", "3"), ("empty", "")],
        ),
        (
            "several tag keys the key carries",
            vec!["a", "c"],
            vec![("b", "2"), ("empty", "")],
        ),
        (
            "a tag whose value is the empty string",
            vec!["empty"],
            vec![("a", "1"), ("b", "2"), ("c", "3")],
        ),
        (
            "a tag key the key does not carry",
            vec!["absent"],
            vec![("a", "1"), ("b", "2"), ("c", "3"), ("empty", "")],
        ),
        (
            "a mix of carried and absent tag keys",
            vec!["absent", "c", "also-absent"],
            vec![("a", "1"), ("b", "2"), ("empty", "")],
        ),
        (
            "a tag key differing only in case",
            vec!["A", "B"],
            vec![("a", "1"), ("b", "2"), ("c", "3"), ("empty", "")],
        ),
        (
            "every tag key the key carries",
            vec!["a", "b", "c", "empty"],
            vec![],
        ),
    ];

    for (case, removed, left) in cases {
        let id = key_id(&router).await;
        tag(&router, case, &id, &tag_list(&held)).await;

        untag(&router, case, &id, &json!(removed)).await;

        let mut expected = pairs(&left);
        expected.sort();
        let listed = tags_of(&router, case, &id).await;
        assert_eq!(
            listed, expected,
            "case {case}: expected the key to carry {expected:?}, got {listed:?}"
        );
    }
}

/// The per-key tag quota bounds what a key CARRIES, not what one request names: a
/// write that edits tags the key already has stays inside the quota however often it
/// is repeated, and a write that would take the key past it is refused with the
/// modelled quota error and stores not one of its tags.
#[tokio::test]
async fn the_tag_quota_bounds_what_one_key_carries() {
    use service_com_amazonaws_kms_trentservice::state::MAX_TAGS_PER_KEY;

    let router = router();
    let id = key_id(&router).await;

    let at_quota: Vec<(String, String)> = (0..MAX_TAGS_PER_KEY)
        .map(|n| (format!("key-{n}"), format!("value-{n}")))
        .collect();
    tag(&router, "filling the quota", &id, &tag_list(&at_quota)).await;

    // Rows: the case, the tag list written, and whether the service stores it. A row
    // that only edits tags the key already carries leaves the count where it is, so
    // the quota is measured on the RESULTING map and not on the request.
    let cases: Vec<StoredRow> = vec![
        (
            "editing a tag at the quota",
            vec![("key-0".to_string(), "edited".to_string())],
            true,
        ),
        (
            "rewriting every tag at the quota",
            at_quota.clone(),
            true,
        ),
        (
            "one tag past the quota",
            vec![("one-too-many".to_string(), "x".to_string())],
            false,
        ),
        (
            "an edit and one tag past the quota together",
            vec![
                ("key-1".to_string(), "edited".to_string()),
                ("one-too-many".to_string(), "x".to_string()),
            ],
            false,
        ),
    ];

    for (case, written, stored) in cases {
        let before = tags_of(&router, case, &id).await;
        let request = json!({"KeyId": id, "Tags": tag_list(&written)});
        if stored {
            tag(&router, case, &id, &tag_list(&written)).await;
            let listed = tags_of(&router, case, &id).await;
            assert_eq!(
                listed.len(),
                MAX_TAGS_PER_KEY,
                "case {case}: expected the key to still carry {MAX_TAGS_PER_KEY} tags, got \
                 {}",
                listed.len()
            );
            for (tag_key, tag_value) in &written {
                assert!(
                    listed.contains(&(tag_key.clone(), tag_value.clone())),
                    "case {case}: expected {tag_key:?}={tag_value:?} among {listed:?}"
                );
            }
        } else {
            let (status, code) = failure(&router, "TagResource", &request).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "LimitExceededException",
                "case {case}: expected LimitExceededException, got {code}"
            );
            let listed = tags_of(&router, case, &id).await;
            assert_eq!(
                listed, before,
                "case {case}: a refused write must leave the tags as they were \
                 ({before:?}), got {listed:?}"
            );
        }
    }

    // A tag key freed by a removal is room the next write can use, so the quota is a
    // bound on the map and not a tally of writes.
    untag(&router, "freeing one tag", &id, &json!(["key-0"])).await;
    tag(
        &router,
        "reusing the freed room",
        &id,
        &tag_list(&pairs(&[("one-too-many", "x")])),
    )
    .await;
    let listed = tags_of(&router, "reusing the freed room", &id).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS_PER_KEY,
        "case reusing the freed room: expected {MAX_TAGS_PER_KEY} tags, got {listed:?}"
    );
}

/// A tag's two members are bounded by the model — `TagKey` is `@length(1..128)` and
/// `TagValue` is `@length(0..256)` (`smithy_query(query="shape",shape="Tag")`) — so a
/// tag at either bound is stored and one past it is refused, with nothing written.
///
/// The rows are each published bound and one character past it, on both members.
#[tokio::test]
async fn a_tag_outside_the_modelled_lengths_is_refused() {
    let router = router();

    // Rows: the case, the tag key, the tag value, and whether the tag is stored.
    let cases: Vec<(&str, String, String, bool)> = vec![
        (
            "the shortest tag key and the empty value",
            "k".to_string(),
            String::new(),
            true,
        ),
        (
            "both members at their maximum",
            "k".repeat(128),
            "v".repeat(256),
            true,
        ),
        (
            "a tag key one character past its maximum",
            "k".repeat(129),
            "v".to_string(),
            false,
        ),
        (
            "a tag value one character past its maximum",
            "k".to_string(),
            "v".repeat(257),
            false,
        ),
        (
            "a tag key one character short of its minimum",
            String::new(),
            "v".to_string(),
            false,
        ),
    ];

    for (case, tag_key, tag_value, stored) in cases {
        let id = key_id(&router).await;
        let written = vec![(tag_key.clone(), tag_value.clone())];
        let request = json!({"KeyId": id, "Tags": tag_list(&written)});
        if stored {
            tag(&router, case, &id, &tag_list(&written)).await;
            let listed = tags_of(&router, case, &id).await;
            assert_eq!(
                listed, written,
                "case {case}: expected the key to carry the tag, got {listed:?}"
            );
        } else {
            let (status, code) = failure(&router, "TagResource", &request).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "ValidationException",
                "case {case}: expected ValidationException, got {code}"
            );
            let listed = tags_of(&router, case, &id).await;
            assert!(
                listed.is_empty(),
                "case {case}: a refused write must store nothing, got {listed:?}"
            );
        }
    }
}

/// A tag write needs a key state that admits it: a key whose deletion is scheduled
/// keeps the tags it had, so both writers are refused with the modelled state error
/// and change nothing — while the READER, which models no state error, keeps
/// reporting that key's tags.
#[tokio::test]
async fn a_tag_write_is_refused_for_a_key_pending_deletion() {
    let router = router();
    let id = key_id(&router).await;
    let held = pairs(&[("env", "production"), ("owner", "alice")]);
    tag(&router, "before scheduling", &id, &tag_list(&held)).await;
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": id, "PendingWindowInDays": 7}),
    )
    .await;

    // Rows: the operation refused, and the request it was refused for.
    let cases: Vec<(&str, Value)> = vec![
        (
            "TagResource",
            json!({"KeyId": id, "Tags": tag_list(&pairs(&[("env", "staging")]))}),
        ),
        ("UntagResource", json!({"KeyId": id, "TagKeys": ["env"]})),
    ];
    for (operation, request) in cases {
        let (status, code) = failure(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation} on a key pending deletion: expected HTTP 400, got {status} \
             ({code})"
        );
        assert_eq!(
            code, "KMSInvalidStateException",
            "case {operation} on a key pending deletion: expected KMSInvalidStateException, \
             got {code}"
        );
    }

    let mut expected = held.clone();
    expected.sort();
    let listed = tags_of(&router, "a key pending deletion", &id).await;
    assert_eq!(
        listed, expected,
        "case a key pending deletion: expected the tags to be untouched ({expected:?}), got \
         {listed:?}"
    );
}

/// The reader pages over one key's tags: every page is at most `Limit` long, the
/// pages concatenate to the whole set exactly once each, `Truncated` says whether
/// more remain, and a `NextMarker` appears only when they do.
///
/// The rows differ in the page size asked for — one, several, exactly the number of
/// tags, more than that, and both published bounds of `Limit` — and the published
/// bounds plus one past each are checked for refusal.
#[tokio::test]
async fn the_reader_pages_one_key_s_tags() {
    let router = router();
    let id = key_id(&router).await;
    let held: Vec<(String, String)> = (0..7)
        .map(|n| (format!("tag-{n}"), format!("value-{n}")))
        .collect();
    tag(&router, "setup", &id, &tag_list(&held)).await;
    let mut expected = held.clone();
    expected.sort();

    // "If you include a value, it must be between 1 and 50, inclusive"
    // (`smithy_query(query="doc",shape="ListResourceTagsRequest",member="Limit")`).
    for limit in [1usize, 2, 3, 7, 8, 50] {
        let case = format!("limit {limit}");
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut request = json!({"KeyId": id, "Limit": limit});
            if let Some(marker) = marker.as_deref() {
                request["Marker"] = json!(marker);
            }
            let answer = ok(&router, "ListResourceTags", &request).await;
            let page = answer["Tags"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a Tags list in {answer}"))
                .clone();
            assert!(
                page.len() <= limit,
                "case {case}: a page must hold at most {limit} tags, got {}",
                page.len()
            );
            for entry in &page {
                collected.push((text(&case, entry, "TagKey"), text(&case, entry, "TagValue")));
            }
            let truncated = answer["Truncated"]
                .as_bool()
                .unwrap_or_else(|| panic!("case {case}: expected a Truncated flag in {answer}"));
            assert_eq!(
                truncated,
                collected.len() < held.len(),
                "case {case}: Truncated must say whether tags remain after {} of {}, got \
                 {answer}",
                collected.len(),
                held.len()
            );
            if !truncated {
                assert!(
                    answer["NextMarker"].is_null(),
                    "case {case}: the last page carries no NextMarker, got {answer}"
                );
                break;
            }
            marker = Some(text(&case, &answer, "NextMarker"));
        }
        collected.sort();
        assert_eq!(
            collected, expected,
            "case {case}: paging must report every tag exactly once ({expected:?}), got \
             {collected:?}"
        );
    }

    // Rows: a `Limit` outside the documented 1..=50, which is refused as invalid.
    for limit in [0i64, 51, 1000] {
        let case = format!("limit {limit}");
        let (status, code) = failure(
            &router,
            "ListResourceTags",
            &json!({"KeyId": id, "Limit": limit}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case}: expected ValidationException, got {code}"
        );
    }

    // Rows: a `Marker` that is not a token this reader minted, including one minted
    // by ANOTHER list, which must not silently resume at an item of the wrong kind.
    create_key(&router, &json!({"Description": "a second key"})).await;
    let foreign = text(
        "another list's marker",
        &ok(&router, "ListKeys", &json!({"Limit": 1})).await,
        "NextMarker",
    );
    for marker in ["not-a-marker".to_string(), "00".to_string(), foreign] {
        let case = format!("marker {marker:?}");
        let (status, code) = failure(
            &router,
            "ListResourceTags",
            &json!({"KeyId": id, "Marker": marker}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidMarkerException",
            "case {case}: expected InvalidMarkerException, got {code}"
        );
    }
}

/// Tags are the KEY's, reached by any identifier that resolves to it: a write
/// addressed by key id and one addressed by key ARN land on the same map, and the
/// reader reports the same map for either form. A key's tags are its own — tagging
/// one key never touches another.
#[tokio::test]
async fn tags_belong_to_the_key_whatever_identifier_names_it() {
    let router = router();
    let first = enabled_key(&router).await;
    let first_id = text("setup", &first, "KeyId");
    let first_arn = text("setup", &first, "Arn");
    let second_id = key_id(&router).await;

    // Rows: the identifier form the write uses, and the tag it writes.
    let cases: Vec<IdentifierRow<'_>> = vec![
        ("by key id", first_id.as_str(), pairs(&[("by-id", "1")])),
        ("by key ARN", first_arn.as_str(), pairs(&[("by-arn", "2")])),
        (
            "by key id again",
            first_id.as_str(),
            pairs(&[("by-id", "edited")]),
        ),
    ];
    let mut expected: Vec<(String, String)> = Vec::new();
    for (case, identifier, written) in cases {
        tag(&router, case, identifier, &tag_list(&written)).await;
        expected.retain(|(tag_key, _)| !written.iter().any(|(written, _)| written == tag_key));
        expected.extend(written.clone());
        expected.sort();

        for form in [first_id.as_str(), first_arn.as_str()] {
            let listed = tags_of(&router, case, form).await;
            assert_eq!(
                listed, expected,
                "case {case} read by {form}: expected {expected:?}, got {listed:?}"
            );
        }
    }

    let other = tags_of(&router, "another key", &second_id).await;
    assert!(
        other.is_empty(),
        "case another key: tagging one key must not tag another, got {other:?}"
    );

    // A region the key does not live in holds no tags for it: a KMS key is regional,
    // so the same key id read through another endpoint names no key at all.
    let (status, code) = super::harness::failure_in(
        &router,
        "us-west-2",
        "ListResourceTags",
        &json!({"KeyId": first_id}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case another Region: expected HTTP 400, got {status} ({code})"
    );
    assert_eq!(
        code, "NotFoundException",
        "case another Region: expected NotFoundException, got {code}"
    );
    assert_eq!(
        REGION, "us-east-1",
        "case another Region: the case assumes the suite's Region is not us-west-2"
    );
}

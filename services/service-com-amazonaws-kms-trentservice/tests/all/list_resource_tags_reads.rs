// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the READER of a key's tags, `ListResourceTags`, driven over
//! the wire through the shared [`super::harness`].
//!
//! The sibling [`super::resource_tags`] suite drives the reader from the tag WRITERS;
//! this one drives it from the tags a key is born with and from the key itself. The
//! rule under test: `ListResourceTags` reports the tag map persisted on the key its
//! `KeyId` RESOLVES to — whatever identifier form names that key, and whatever key
//! state the key is in, since the operation models no state error — paging that map
//! by `Limit` and a `Marker` it minted for that key, and writing nothing.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in how many tags
//! the key carries (none, one, several, the per-key quota), in the tag keys and values
//! (shortest, the longest the modelled lengths allow, the empty value, multi-byte and
//! punctuated text), in the kind of key, in the identifier form the read uses, in the
//! key's state, and in the page size.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::harness::{
    alias_arn, create_alias, create_key, describe, failure, ok, router, text, REGION,
};

/// The per-key tag quota, from `service_com_amazonaws_kms_trentservice::state`, so the
/// quota row moves with the service rather than restating it.
use service_com_amazonaws_kms_trentservice::state::MAX_TAGS_PER_KEY;

/// The largest page this operation accepts: "If you include a value, it must be
/// between 1 and 50, inclusive"
/// (`smithy_query(query="doc",shape="ListResourceTagsRequest",member="Limit")`).
const MAX_LIMIT: usize = 50;

/// A tag map as a `CreateKey`/`TagResource` request carries it.
fn wire_tags(tags: &BTreeMap<String, String>) -> Value {
    Value::Array(
        tags.iter()
            .map(|(tag_key, tag_value)| json!({"TagKey": tag_key, "TagValue": tag_value}))
            .collect(),
    )
}

/// The tag map one `ListResourceTags` answer reports, so a comparison is about WHICH
/// tags came back rather than in what order, and a repeated tag key is a failure.
fn reported(case: &str, answer: &Value) -> BTreeMap<String, String> {
    let entries = answer["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Tags list in {answer}"));
    let mut map = BTreeMap::new();
    for entry in entries {
        let tag_key = text(case, entry, "TagKey");
        let tag_value = text(case, entry, "TagValue");
        assert!(
            map.insert(tag_key.clone(), tag_value).is_none(),
            "case {case}: the tag key {tag_key} must appear at most once, got {answer}"
        );
    }
    map
}

/// A tag map from `(tag key, tag value)` pairs.
fn map_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(tag_key, tag_value)| ((*tag_key).to_string(), (*tag_value).to_string()))
        .collect()
}

/// A tag map of `count` distinct tags, for the paging rows.
fn many(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("tag-{index:02}"), format!("value-{index}")))
        .collect()
}

/// Rows of the "what the key carries is what the reader reports" table: the case, the
/// rest of the `CreateKey` request, and the tags the key is created with.
///
/// The tag keys and values span `Tag`'s modelled member constraints — `TagKey`
/// `@length(1..128)` and `TagValue` `@length(0..256)`
/// (`smithy_query(query="shape",shape="Tag")`) — at both ends, and the keys span the
/// `KeySpec`/`KeyUsage` combinations this service mints.
fn create_rows() -> Vec<(&'static str, Value, BTreeMap<String, String>)> {
    vec![
        (
            "no tags at all",
            json!({"Description": "untagged"}),
            BTreeMap::new(),
        ),
        (
            "one tag",
            json!({"Description": "one tag"}),
            map_of(&[("Project", "apollo")]),
        ),
        (
            "several tags on an HMAC key",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
            map_of(&[("Project", "apollo"), ("Owner", "platform"), ("Tier", "1")]),
        ),
        (
            "the shortest tag key and the empty tag value",
            json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
            map_of(&[("a", "")]),
        ),
        (
            "both members at the modelled maximum",
            json!({"Description": "at the length bounds"}),
            [
                ("k".repeat(128), "v".repeat(256)),
                ("short".to_string(), "x".repeat(256)),
            ]
            .into_iter()
            .collect(),
        ),
        (
            "case-distinct keys, punctuation and multi-byte text",
            json!({"Description": "character classes"}),
            map_of(&[
                ("env", "prod"),
                ("Env", "PROD"),
                ("path/to:thing", "a=b,c/d:e_f-g.h +i"),
                ("clé", "équipe sécurité 鍵"),
            ]),
        ),
        (
            "a multi-Region key at the per-key quota",
            json!({"MultiRegion": true, "Description": "mrk at the quota"}),
            many(MAX_TAGS_PER_KEY),
        ),
    ]
}

/// The tags a key is CREATED with are the tags the reader reports, for every
/// identifier form that resolves to that key — and reading them changes nothing a
/// later caller sees.
#[tokio::test]
async fn tags_a_key_was_created_with_are_reported_for_every_identifier_form() {
    let router = router();

    for (index, (case, request, expected)) in create_rows().into_iter().enumerate() {
        let mut create = request.clone();
        create["Tags"] = wire_tags(&expected);
        let metadata = create_key(&router, &create).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");
        let alias = format!("alias/reader-{index}");
        create_alias(&router, case, &alias, &key_id).await;

        for (form, identifier) in [
            ("key id", key_id.clone()),
            ("key ARN", key_arn.clone()),
            ("alias name", alias.clone()),
            ("alias ARN", alias_arn(REGION, &alias)),
        ] {
            let answer = ok(&router, "ListResourceTags", &json!({"KeyId": identifier})).await;
            assert_eq!(
                reported(case, &answer),
                expected,
                "case {case} read by {form}: expected the key's {} tags, got {answer}",
                expected.len()
            );
            assert_eq!(
                answer["Truncated"],
                json!(false),
                "case {case} read by {form}: a page holding every tag must not be \
                 truncated, got {answer}"
            );
            assert!(
                answer["NextMarker"].is_null(),
                "case {case} read by {form}: an untruncated page carries no NextMarker, \
                 got {answer}"
            );
        }

        // Reading is not writing: the same tags again, and the key exactly as
        // `CreateKey` answered it.
        let again = ok(&router, "ListResourceTags", &json!({"KeyId": key_id})).await;
        assert_eq!(
            reported(case, &again),
            expected,
            "case {case}: listing tags must not change them, got {again}"
        );
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described, metadata,
            "case {case}: listing tags must leave the key untouched, got {described}"
        );
    }
}

/// A key's tags are readable in every key state: `ListResourceTags` models no
/// `KMSInvalidStateException`, so a disabled key and a key pending deletion report
/// exactly what an enabled one does.
#[tokio::test]
async fn the_reader_reports_tags_in_every_key_state() {
    let router = router();
    let expected = map_of(&[("env", "prod"), ("owner", "sre")]);
    let metadata = create_key(
        &router,
        &json!({"Description": "key states", "Tags": wire_tags(&expected)}),
    )
    .await;
    let key_id = text("setup", &metadata, "KeyId");

    // Rows: the state, and the call that puts the key into it (a read for the state
    // the key is already in).
    let states: Vec<(&str, &str, Value)> = vec![
        ("enabled", "DescribeKey", json!({"KeyId": key_id})),
        ("disabled", "DisableKey", json!({"KeyId": key_id})),
        (
            "pending deletion",
            "ScheduleKeyDeletion",
            json!({"KeyId": key_id, "PendingWindowInDays": 7}),
        ),
        (
            "deletion cancelled",
            "CancelKeyDeletion",
            json!({"KeyId": key_id}),
        ),
        ("re-enabled", "EnableKey", json!({"KeyId": key_id})),
    ];
    for (case, operation, request) in states {
        ok(&router, operation, &request).await;
        let answer = ok(&router, "ListResourceTags", &json!({"KeyId": key_id})).await;
        assert_eq!(
            reported(case, &answer),
            expected,
            "case {case}: every key state must report the key's tags, got {answer}"
        );
    }
}

/// The reader pages a key's whole tag map: no page is longer than `Limit`, a page is
/// full while tags remain, `Truncated` says whether they do, `NextMarker` appears only
/// then, and the pages together hold every tag exactly once.
///
/// The rows differ in how many tags the key carries (none, one, several, the per-key
/// quota) and in the page size asked for, including no `Limit` at all — "if you do not
/// include a value, it defaults to 50" — and the published maximum.
#[tokio::test]
async fn the_reader_pages_the_whole_tag_map() {
    let router = router();

    let sizes: Vec<(usize, Option<usize>)> = vec![
        (0, Some(1)),
        (1, Some(1)),
        (7, Some(1)),
        (7, Some(3)),
        (7, Some(7)),
        (7, None),
        (MAX_TAGS_PER_KEY, Some(1)),
        (MAX_TAGS_PER_KEY, Some(7)),
        (MAX_TAGS_PER_KEY, Some(MAX_LIMIT)),
        (MAX_TAGS_PER_KEY, None),
    ];

    for (count, limit) in sizes {
        let case = format!("{count} tags, limit {limit:?}");
        let expected = many(count);
        let metadata = create_key(
            &router,
            &json!({"Description": case, "Tags": wire_tags(&expected)}),
        )
        .await;
        let key_id = text(&case, &metadata, "KeyId");

        let page_size = limit.unwrap_or(MAX_LIMIT);
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        let mut marker: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({"KeyId": key_id});
            if let Some(limit) = limit {
                request["Limit"] = json!(limit);
            }
            if let Some(marker) = marker.as_deref() {
                request["Marker"] = json!(marker);
            }
            let answer = ok(&router, "ListResourceTags", &request).await;
            pages += 1;
            assert!(
                pages <= count + 1,
                "case {case}: the walk must terminate, got {pages} pages"
            );

            let page = reported(&case, &answer);
            let remaining = count - seen.len();
            assert_eq!(
                page.len(),
                remaining.min(page_size),
                "case {case}: expected a full page while tags remain, got {answer}"
            );
            for (tag_key, tag_value) in page {
                assert!(
                    seen.insert(tag_key.clone(), tag_value).is_none(),
                    "case {case}: the tag {tag_key} must appear on one page only"
                );
            }

            let truncated = answer["Truncated"]
                .as_bool()
                .unwrap_or_else(|| panic!("case {case}: expected a Truncated flag in {answer}"));
            assert_eq!(
                truncated,
                seen.len() < count,
                "case {case}: Truncated must say whether tags remain after {} of {count}, \
                 got {answer}",
                seen.len()
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

        assert_eq!(
            seen, expected,
            "case {case}: the pages together must hold every tag exactly once"
        );
    }
}

/// A `Marker` names the tag the previous page ended at, so a token whose tag key this
/// key does not carry — one taken from a page of a key whose tags are different, or a
/// token this reader never minted at all — resumes nowhere and is refused rather than
/// silently resuming at whichever tag now sits in that position. The token taken from
/// THIS key's own page resumes after the tag that page ended on.
#[tokio::test]
async fn a_marker_naming_a_tag_key_this_key_does_not_carry_is_refused() {
    let router = router();
    let expected = many(4);
    let metadata = create_key(
        &router,
        &json!({"Description": "markers", "Tags": wire_tags(&expected)}),
    )
    .await;
    let key_id = text("setup", &metadata, "KeyId");

    // A token taken from a page of a DIFFERENT key, whose tag keys this key carries
    // none of — the marker carries the tag key it ended at, so it names nothing here.
    let other = create_key(
        &router,
        &json!({
            "Description": "another key",
            "Tags": wire_tags(&map_of(&[("elsewhere", "1"), ("second", "2")])),
        }),
    )
    .await;
    let other_id = text("setup", &other, "KeyId");
    let other_page = ok(
        &router,
        "ListResourceTags",
        &json!({"KeyId": other_id, "Limit": 1}),
    )
    .await;
    let other_marker = text("setup", &other_page, "NextMarker");

    for (case, marker) in [
        ("a token from another key's page", other_marker),
        ("a token that is not hex at all", "not-a-marker".to_string()),
        ("hex that carries no list prefix", hex::encode("tag-00")),
    ] {
        let (status, code) = failure(
            &router,
            "ListResourceTags",
            &json!({"KeyId": key_id, "Marker": marker}),
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

    // The token minted for THIS key is accepted, and the two pages hold the whole map.
    let first = ok(
        &router,
        "ListResourceTags",
        &json!({"KeyId": key_id, "Limit": 2}),
    )
    .await;
    let marker = text("resume", &first, "NextMarker");
    let second = ok(
        &router,
        "ListResourceTags",
        &json!({"KeyId": key_id, "Limit": 2, "Marker": marker}),
    )
    .await;
    let mut walked = reported("resume", &first);
    for (tag_key, tag_value) in reported("resume", &second) {
        assert!(
            walked.insert(tag_key.clone(), tag_value).is_none(),
            "case resume: the tag {tag_key} must appear on one page only"
        );
    }
    assert_eq!(
        walked, expected,
        "case resume: the two pages together must hold every tag, got {first} then {second}"
    );
}

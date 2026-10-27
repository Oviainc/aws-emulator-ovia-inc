// SPDX-License-Identifier: CC0-1.0

//! What `TagResource` stores for a tag's key and value, end to end over the wire.
//!
//! The rule these suites assert is: a tag the request supplies is stored against
//! the resource its `ResourceARN` names and reported back by
//! `ListTagsForResource` exactly as it was given — the same key and the same value,
//! code point for code point, with no case folding, normalisation, trimming or
//! transcoding, whatever characters the modelled `TagKey` / `TagValue` pattern
//! admits — and the modelled bounds that decide which of them a request may carry
//! at all are counted in CHARACTERS, so a value of multi-byte characters is
//! admitted on the same count as an ASCII one.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the rows differ in the script of the value (Latin, Japanese, Greek, Cyrillic,
//! Hebrew, fullwidth), in the kind of character it is built from (letters, digits
//! in two numbering systems, the separators and punctuation the pattern admits),
//! in its length (the empty string at the length floor, the published ceiling, one
//! past it) and in its byte length against its character length — crossed with
//! each of the four resource kinds WAF tags and with both scopes. The harness is
//! the one in [`crate::web_acl_lifecycle`]: every call goes through the router's
//! front door, so what these tests observe is what an SDK observes.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{SCOPE_CLOUDFRONT, SCOPE_REGIONAL};

use crate::parameter_validation::error_code;
use crate::resource_tagging::{listed_tags, pairs, tags, taggable_resources};
use crate::web_acl_lifecycle::{create_ip_set, Waf};

/// Tag values spanning the class `TagValue` admits — `length 0..256` and
/// `@pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from
/// `smithy_query(query="shape",shape="TagValue")`. Every row satisfies that
/// pattern and differs from the others along what the rule generalises over: the
/// script of its letters, the numbering system of its digits, the separators and
/// punctuation it carries, and whether it is empty at all.
const VALUES: &[(&str, &str)] = &[
    ("an ASCII word", "sim2real"),
    ("the empty string, at the length floor", ""),
    ("a Japanese value", "日本語の値"),
    ("a Greek value", "Ωμέγα"),
    ("a Cyrillic value", "Привет"),
    ("a Hebrew value, written right to left", "שלום"),
    ("fullwidth Latin letters", "ＦＵＬＬＷＩＤＴＨ"),
    ("ASCII digits", "0123456789"),
    ("Arabic-Indic digits", "٣٤٥"),
    ("a value carrying the spaces the pattern admits", "two words here"),
    ("a value carrying an ideographic space", "間\u{3000}隔"),
    (
        "a value carrying every punctuation character the pattern admits",
        "_.:/=+-@",
    ),
    ("a mixed-script value", "prod-本番-Ωmega"),
];

/// The key every round-trip row below is applied under: one key, so a row's
/// listing is exactly the pair that row stored.
const PROBE_KEY: &str = "Probe";

/// A tag's key and value survive the round trip through `TagResource` into
/// `ListTagsForResource` unchanged, on every taggable resource kind and in both
/// scopes.
#[tokio::test]
async fn a_tag_value_is_reported_back_exactly_as_given() {
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let waf = Waf::new();
        for (kind, arn) in taggable_resources(&waf, scope).await {
            for (row, value) in VALUES {
                let case = format!("{kind}, {row}");
                waf.ok(
                    "TagResource",
                    json!({ "ResourceARN": &arn, "Tags": tags(PROBE_KEY, value) }),
                )
                .await;
                let listed = listed_tags(&waf, &arn).await;
                assert_eq!(
                    listed,
                    vec![(PROBE_KEY.to_string(), (*value).to_string())],
                    "case {case:?}: expected the pair the request supplied, got {listed:?}"
                );
            }
        }
    }
}

/// One request carrying the whole table at once keeps every value with its own
/// key: the collection read back is exactly the pairs supplied, in the key order
/// the listing enumerates in.
#[tokio::test]
async fn one_request_keeps_every_value_with_its_own_key() {
    let waf = Waf::new();
    let (_id, arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, "round-trip-ips").await;

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    let mut requested: Vec<Value> = Vec::new();
    for (index, (_row, value)) in VALUES.iter().enumerate() {
        let key = format!("key-{index}");
        requested.push(json!({ "Key": &key, "Value": value }));
        expected.insert(key, (*value).to_string());
    }
    waf.ok(
        "TagResource",
        json!({ "ResourceARN": &arn, "Tags": Value::Array(requested) }),
    )
    .await;

    let listed = listed_tags(&waf, &arn).await;
    assert_eq!(
        listed,
        pairs(&expected),
        "every supplied pair must be read back under its own key, got {listed:?}"
    );
}

/// A key and a value are compared literally: two keys differing only in case are
/// two tags, and a value differing only in case is a different value — "Tag keys
/// are case-sensitive" (`Tag$Key`'s own doc).
#[tokio::test]
async fn tag_keys_and_values_are_case_sensitive() {
    let waf = Waf::new();
    let (_id, arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, "case-ips").await;

    // Rows differing only in the case of the key, of the value, or of both.
    let rows: &[(&str, &str)] = &[
        ("Env", "Prod"),
        ("env", "prod"),
        ("ENV", "PROD"),
        ("Straße", "GROSS"),
    ];
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in rows {
        let case = format!("key {key:?} value {value:?}");
        waf.ok(
            "TagResource",
            json!({ "ResourceARN": &arn, "Tags": tags(key, value) }),
        )
        .await;
        expected.insert((*key).to_string(), (*value).to_string());
        let listed = listed_tags(&waf, &arn).await;
        assert_eq!(
            listed,
            pairs(&expected),
            "case {case:?}: a key differing only in case is its own tag, got {listed:?}"
        );
    }
}

/// The modelled bounds on a key and a value count CHARACTERS: a value built from
/// multi-byte characters is admitted at the published ceiling even though its
/// encoded form is longer, while one character past the ceiling — and a key
/// shorter than the floor, and a `Tags` list with no element — is refused by the
/// request boundary and stores nothing.
///
/// The bounds are the modelled ones: `TagKey` is `length 1..128`, `TagValue` is
/// `length 0..256` and `TagList` is `length 1..`
/// (`smithy_query(query="shape",shape="TagKey,TagValue,TagList")`).
#[tokio::test]
async fn the_tag_length_bounds_count_characters_not_bytes() {
    /// `TagKey`'s published character bounds.
    const KEY_MIN: usize = 1;
    const KEY_MAX: usize = 128;
    /// `TagValue`'s published character bounds.
    const VALUE_MIN: usize = 0;
    const VALUE_MAX: usize = 256;

    let waf = Waf::new();
    let (_id, arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, "bounded-ips").await;

    // Accepted: each published bound, in ASCII and in a multi-byte script.
    let accepted: Vec<(String, String, String)> = vec![
        (
            format!("a key at the floor of {KEY_MIN}"),
            "k".repeat(KEY_MIN),
            "v".to_string(),
        ),
        (
            format!("an ASCII key at the ceiling of {KEY_MAX}"),
            "k".repeat(KEY_MAX),
            "v".to_string(),
        ),
        (
            format!("a Japanese key at the ceiling of {KEY_MAX}"),
            "鍵".repeat(KEY_MAX),
            "v".to_string(),
        ),
        (
            format!("a value at the floor of {VALUE_MIN}"),
            "floor-value".to_string(),
            "v".repeat(VALUE_MIN),
        ),
        (
            format!("an ASCII value at the ceiling of {VALUE_MAX}"),
            "ascii-ceiling".to_string(),
            "v".repeat(VALUE_MAX),
        ),
        (
            format!("a Japanese value at the ceiling of {VALUE_MAX}"),
            "japanese-ceiling".to_string(),
            "値".repeat(VALUE_MAX),
        ),
    ];
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (case, key, value) in &accepted {
        waf.ok(
            "TagResource",
            json!({ "ResourceARN": &arn, "Tags": tags(key, value) }),
        )
        .await;
        expected.insert(key.clone(), value.clone());
        let listed = listed_tags(&waf, &arn).await;
        assert_eq!(
            listed,
            pairs(&expected),
            "case {case:?}: a pair within the bounds must be stored whole, got {listed:?}"
        );
    }
    // The multi-byte rows really are longer encoded than the bound they sit on, so
    // the rows above pin a CHARACTER count and not a byte count.
    assert!(
        "鍵".repeat(KEY_MAX).len() > KEY_MAX && "値".repeat(VALUE_MAX).len() > VALUE_MAX,
        "the multi-byte rows must encode longer than their character bound, got {} \
         and {} bytes",
        "鍵".repeat(KEY_MAX).len(),
        "値".repeat(VALUE_MAX).len()
    );

    // Refused by the request boundary: one character past a ceiling, below a floor,
    // and a list with no element at all. Nothing of these reaches the store.
    //
    // A bound is a MODELLED member constraint, so the refusal is the request
    // validator's own envelope and never the handler's — the same envelope every
    // other constraint failure of this service answers in, as
    // [`crate::parameter_validation`] pins it. A handler-decided tagging failure
    // keeps its own modelled code instead, which the sibling suites assert
    // (`WAFTagOperationException`, `WAFNonexistentItemException`,
    // `WAFLimitsExceededException`, `WAFInvalidParameterException`).
    let refused: Vec<(String, Value)> = vec![
        (
            format!("an ASCII key one character past {KEY_MAX}"),
            tags(&"k".repeat(KEY_MAX + 1), "v"),
        ),
        (
            format!("a Japanese key one character past {KEY_MAX}"),
            tags(&"鍵".repeat(KEY_MAX + 1), "v"),
        ),
        (
            format!("a key below the floor of {KEY_MIN}"),
            tags("", "v"),
        ),
        (
            format!("an ASCII value one character past {VALUE_MAX}"),
            tags("past-ceiling", &"v".repeat(VALUE_MAX + 1)),
        ),
        (
            format!("a Japanese value one character past {VALUE_MAX}"),
            tags("past-ceiling", &"値".repeat(VALUE_MAX + 1)),
        ),
        ("a Tags list with no element".to_string(), json!([])),
    ];
    let before = listed_tags(&waf, &arn).await;
    for (case, request_tags) in refused {
        let (status, body) = waf
            .call(
                "TagResource",
                json!({ "ResourceARN": &arn, "Tags": request_tags }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got status {status} and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a modelled-constraint failure is the request validator's \
             own refusal, got {body}"
        );
        let after = listed_tags(&waf, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused tagging must store nothing, got {after:?}"
        );
    }
}

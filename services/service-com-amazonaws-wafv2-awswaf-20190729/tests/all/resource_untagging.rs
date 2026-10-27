// SPDX-License-Identifier: CC0-1.0

//! Untagging a WAFv2 resource: `UntagResource`, end to end over the wire.
//!
//! The rule these suites assert is: `UntagResource` disassociates from the
//! resource its `ResourceARN` names exactly the tags whose keys the request lists
//! and leaves every other tag it carries, value included, as it was — a key the
//! resource does not carry removes nothing, so the call is idempotent — while a
//! key under the prefix AWS keeps for its own system tags, and an address that
//! resolves to no taggable resource, are refused and remove nothing at all.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the rows differ in HOW MANY of the resource's keys a request names (one, two,
//! all of them, none of them), in whether the keys it names are held or absent or
//! a mix of both, and in the form of the key and of its value (ASCII, a namespaced
//! key, a value in a non-Latin script, a value carrying the spaces and punctuation
//! `TagValue`'s own `@pattern` admits), crossed with each of the four resource
//! kinds WAF tags and with both scopes. The harness is the one in
//! [`crate::web_acl_lifecycle`]: every call goes through the router's front door,
//! so what these tests observe is what an SDK observes.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{SCOPE_CLOUDFRONT, SCOPE_REGIONAL};

use crate::permission_policy::error_message;
use crate::resource_tagging::{
    listed_tags, tags, taggable_resources, NONEXISTENT_ITEM, RESERVED_KEYS, RESERVED_TAG_KEY,
};
use crate::web_acl_lifecycle::{create_ip_set, Waf};

/// The tags every untagging case below starts from, as `(key, value)`.
///
/// The keys and values span the forms `TagKey` and `TagValue` admit (both
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, length 1..128 and 0..256, from
/// `smithy_query(query="shape",shape="TagKey,TagValue")`): a plain ASCII key, a
/// namespaced one, a key that merely CONTAINS `aws`, and values that are ASCII, a
/// non-Latin script, text carrying the spaces and punctuation the pattern admits,
/// and the empty string the length allows.
const SEEDED_TAGS: &[(&str, &str)] = &[
    ("Team", "sim2real"),
    ("cost.center/1", "platform=core"),
    ("aws", "not-a-system-tag"),
    ("訳", "日本語の値"),
    ("Owner", "a name with spaces_and.punctuation:ok"),
    ("Empty", ""),
];

/// A `TagKeyList` of the keys `keys` names.
fn tag_keys(keys: &[&str]) -> Value {
    Value::Array(keys.iter().map(|key| json!(key)).collect())
}

/// Apply [`SEEDED_TAGS`] to `arn`, so each row of a table starts from the same
/// collection however the row before it changed it.
async fn seed_tags(waf: &Waf, arn: &str) {
    let seeded: Vec<Value> = SEEDED_TAGS
        .iter()
        .map(|(key, value)| json!({ "Key": key, "Value": value }))
        .collect();
    waf.ok(
        "TagResource",
        json!({ "ResourceARN": arn, "Tags": Value::Array(seeded) }),
    )
    .await;
}

/// [`SEEDED_TAGS`] without the keys `removed` names, in the key order the listing
/// answers in.
fn expected_after_removing(removed: &[&str]) -> Vec<(String, String)> {
    SEEDED_TAGS
        .iter()
        .filter(|(key, _)| !removed.contains(key))
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<String, String>>()
        .into_iter()
        .collect()
}

/// A request removes exactly the keys it names and nothing else, on every taggable
/// resource kind and in both scopes; the tags that stay keep their exact values,
/// and repeating the same request answers the same success without changing
/// anything further.
#[tokio::test]
async fn untagging_removes_exactly_the_keys_the_request_names() {
    // How many of the resource's keys a request names, and whether they are held:
    // one, several, all of them, none of them, and a held key beside an absent one.
    let rows: &[(&str, &[&str])] = &[
        ("one key the resource carries", &["Team"]),
        ("several keys at once", &["Team", "Owner", "Empty"]),
        ("a key outside the Latin script", &["訳"]),
        ("a namespaced key", &["cost.center/1"]),
        ("a key the resource does not carry", &["never-applied"]),
        ("a held key beside an absent one", &["aws", "never-applied"]),
        (
            "every key the resource carries",
            &["Team", "cost.center/1", "aws", "訳", "Owner", "Empty"],
        ),
    ];

    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let waf = Waf::new();
        for (kind, arn) in taggable_resources(&waf, scope).await {
            for (row, removed) in rows {
                let case = format!("{kind}, {row}");
                seed_tags(&waf, &arn).await;
                let seeded = listed_tags(&waf, &arn).await;
                assert_eq!(
                    seeded,
                    expected_after_removing(&[]),
                    "case {case:?}: the seeded collection must be read back whole, \
                     got {seeded:?}"
                );

                waf.ok(
                    "UntagResource",
                    json!({ "ResourceARN": &arn, "TagKeys": tag_keys(removed) }),
                )
                .await;
                let after = listed_tags(&waf, &arn).await;
                assert_eq!(
                    after,
                    expected_after_removing(removed),
                    "case {case:?}: only the named keys may go, got {after:?}"
                );

                // The same request again: a key the resource no longer carries
                // removes nothing, so the second call succeeds and the collection
                // is the one the first call left.
                waf.ok(
                    "UntagResource",
                    json!({ "ResourceARN": &arn, "TagKeys": tag_keys(removed) }),
                )
                .await;
                let repeated = listed_tags(&waf, &arn).await;
                assert_eq!(
                    repeated, after,
                    "case {case:?}: untagging must be idempotent, got {repeated:?}"
                );
            }
        }
    }
}

/// A removed key is gone rather than emptied: applying it again brings it back
/// with the value the new request gives it, so a tag's value never survives its
/// key's removal.
#[tokio::test]
async fn a_removed_key_can_be_applied_again_with_a_new_value() {
    let waf = Waf::new();
    let (_id, arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, "re-tagged-ips").await;

    for (key, value) in SEEDED_TAGS {
        let case = format!("key {key:?}");
        waf.ok(
            "TagResource",
            json!({ "ResourceARN": &arn, "Tags": tags(key, value) }),
        )
        .await;
        waf.ok(
            "UntagResource",
            json!({ "ResourceARN": &arn, "TagKeys": tag_keys(&[key]) }),
        )
        .await;
        let emptied = listed_tags(&waf, &arn).await;
        assert!(
            emptied.is_empty(),
            "case {case:?}: the removed key must not be listed at all, got {emptied:?}"
        );

        waf.ok(
            "TagResource",
            json!({ "ResourceARN": &arn, "Tags": tags(key, "applied again") }),
        )
        .await;
        let reapplied = listed_tags(&waf, &arn).await;
        assert_eq!(
            reapplied,
            vec![((*key).to_string(), "applied again".to_string())],
            "case {case:?}: re-applying the key must store the new value, got {reapplied:?}"
        );
        waf.ok(
            "UntagResource",
            json!({ "ResourceARN": &arn, "TagKeys": tag_keys(&[key]) }),
        )
        .await;
    }
}

/// A key under the reserved prefix is refused on every taggable resource kind and
/// removes nothing, while a key that merely resembles the prefix is a caller's own
/// and is removed like any other.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_removes_nothing() {
    /// Keys that are a caller's own though `aws` appears in them: the reserved
    /// namespace is the literal `aws:` at the START of the key ("These tags have an
    /// aws: prefix and are reserved by AWS"), so none of these is reserved.
    const LOOKALIKE_KEYS: &[&str] = &["aws", "awsome", "team:aws:owner", "my-aws:key"];

    let waf = Waf::new();
    for (kind, arn) in taggable_resources(&waf, SCOPE_REGIONAL).await {
        for reserved in RESERVED_KEYS {
            let case = format!("{kind}, key {reserved:?}");
            seed_tags(&waf, &arn).await;

            // The reserved key alone, and the reserved key beside a key the
            // resource really carries: one reserved key refuses the whole request.
            for requested in [vec![*reserved], vec!["Team", *reserved]] {
                let refused = waf
                    .refused(
                        "UntagResource",
                        json!({ "ResourceARN": &arn, "TagKeys": tag_keys(&requested) }),
                        "WAFTagOperationException",
                        &case,
                    )
                    .await;
                assert_eq!(
                    error_message(&refused),
                    RESERVED_TAG_KEY,
                    "case {case:?}: the refusal must carry WAF's own sentence, got {refused}"
                );
                let after = listed_tags(&waf, &arn).await;
                assert_eq!(
                    after,
                    expected_after_removing(&[]),
                    "case {case:?}: a refused untagging must remove nothing, got {after:?}"
                );
            }
        }

        // A lookalike key is the caller's own: naming it is accepted, and it takes
        // away only the pair held under it.
        for key in LOOKALIKE_KEYS {
            let case = format!("{kind}, lookalike key {key:?}");
            seed_tags(&waf, &arn).await;
            waf.ok(
                "UntagResource",
                json!({ "ResourceARN": &arn, "TagKeys": tag_keys(&[key]) }),
            )
            .await;
            let after = listed_tags(&waf, &arn).await;
            assert_eq!(
                after,
                expected_after_removing(&[key]),
                "case {case:?}: a key outside the reserved namespace must be removed, \
                 got {after:?}"
            );
        }
    }
}

/// An address that resolves to no taggable resource of this account is refused —
/// malformed as the parameter failure, well formed as the nonexistent-item
/// failure — and the resources that do exist keep every tag they carried.
#[tokio::test]
async fn an_address_that_resolves_to_nothing_is_refused() {
    /// A string long enough to satisfy `ResourceArn`'s `@length(20..2048)` that is
    /// not an ARN at all, so the modelled constraints admit it and the handler is
    /// what refuses it.
    const MALFORMED_ARN: &str = "this-is-not-an-arn-but-it-is-long-enough";
    /// A well-formed wafv2 IP set ARN that no create minted.
    const MISSING_ARN: &str = "arn:aws:wafv2:us-west-2:000000000000:regional/ipset/\
         untag-absent-ips/00000000-1111-2222-3333-444444444444";

    let waf = Waf::new();
    let (_id, arn, _lock) = create_ip_set(&waf, SCOPE_REGIONAL, "addressed-untag-ips").await;
    seed_tags(&waf, &arn).await;

    for (case, address, code) in [
        (
            "a string that is not an ARN",
            MALFORMED_ARN,
            "WAFInvalidParameterException",
        ),
        (
            "an ARN no create minted",
            MISSING_ARN,
            "WAFNonexistentItemException",
        ),
    ] {
        let refused = waf
            .refused(
                "UntagResource",
                json!({ "ResourceARN": address, "TagKeys": tag_keys(&["Team"]) }),
                code,
                case,
            )
            .await;
        if code == "WAFNonexistentItemException" {
            assert_eq!(
                error_message(&refused),
                NONEXISTENT_ITEM,
                "case {case:?}: the refusal must carry WAF's own sentence, got {refused}"
            );
        }
    }

    let after = listed_tags(&waf, &arn).await;
    assert_eq!(
        after,
        expected_after_removing(&[]),
        "a refused untagging must leave the resources that do exist alone, got {after:?}"
    );
}

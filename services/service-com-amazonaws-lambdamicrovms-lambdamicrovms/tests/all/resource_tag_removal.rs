// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UntagResource`, driven over the same router the emulator boots.
//!
//! Each test states a RULE and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) and the tagging helpers are the ones
//! `microvm_lifecycle`, `resource_tagging` and `list_tags_read_path` already own, and
//! the read-back goes through BOTH tag read paths (`ListTags` and `GetMicrovmImage`).

use serde_json::Value;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_not_found_message, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::list_tags_read_path::list_tags;
use crate::microvm_image_and_instance_paths::delete_image;
use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, ACCOUNT, PAST_THE_SPAN, REGION,
};
use crate::resource_tagging::{
    image_with_tags, numbered_tags, stored_tags, tag_map, MAX_TAG_KEY_LENGTH,
};

/// One tag key as a raw `tagKeys` query value.
///
/// Every key these suites send is written from characters that are legal in a query
/// component unescaped and that the server reads back unchanged — ASCII letters and
/// digits, `-`, `.`, `_`, `~`, and the `:`, `/`, `=` and `@` the `TagKey` pattern also
/// allows — so what a test sends is what the handler sees. The assertion keeps that
/// true: a key that WOULD need percent-encoding (a space, a `+`, a non-ASCII letter)
/// fails loudly here instead of travelling mangled, and the removal rule does not
/// depend on how a key is spelled.
fn as_query_value(value: &str) -> &str {
    assert!(
        value
            .chars()
            .all(|c| c.is_ascii_alphanumeric()
                || matches!(c, '-' | '.' | '_' | '~' | ':' | '/' | '=' | '@')),
        "a tag key sent in a raw query value must need no escaping, got {value:?}"
    );
    value
}

/// `DELETE /2017-03-31/tags/{Resource}?tagKeys=…` — the tag-removal URI this model
/// binds, with the resource ARN as the path label and one repeated query parameter per
/// key (`TagKeys` is `@httpQuery("tagKeys")` on a list;
/// `smithy_query(query="operation",operation="UntagResource")`).
async fn untag_resource(
    router: &MultiServiceRouter,
    resource: &str,
    tag_keys: &[String],
) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", as_query_value(key)))
        .collect::<Vec<String>>()
        .join("&");
    let path = format!("/2017-03-31/tags/{}", as_uri_value(resource));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, "DELETE", &uri, None).await
}

/// The tag keys `keys` names, as the owned values [`untag_resource`] takes.
fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

/// The `key-NNN` keys `numbered_tags` mints for indexes `range`.
fn numbered_keys(range: std::ops::Range<usize>) -> Vec<String> {
    range.map(|index| format!("key-{index:03}")).collect()
}

/// The `numbered_tags` pairs for indexes `range`, each holding `value`.
fn numbered_tag_slice(range: std::ops::Range<usize>, value: &str) -> Value {
    Value::Object(
        range
            .map(|index| (format!("key-{index:03}"), Value::String(value.to_string())))
            .collect(),
    )
}

/// RULE: removing tags takes out of a resource's stored tag set exactly the keys the
/// request names, leaving every key it does not name with the value it had; a named key
/// the resource does not carry removes nothing — so the call is idempotent, answers the
/// modelled 204 with no body, and both tag read paths report the remaining set.
#[tokio::test]
async fn removing_tags_takes_out_exactly_the_named_keys() {
    let router = emulator();

    let longest_key = "k".repeat(MAX_TAG_KEY_LENGTH);
    let punctuated_key = "Key_.:/=-@42";
    let half = MAX_TAGS_PER_RESOURCE / 2;

    // Rows differ in how many keys are removed, in how many the resource carried, in
    // whether a named key is carried at all, in whether it is named twice, and in the
    // shape of the keys and values — the dimensions the removal rule generalises over.
    let rows: Vec<(&str, Value, Vec<String>, Value)> = vec![
        (
            "the one key the resource carries",
            tag_map(&[("env", "prod")]),
            keys(&["env"]),
            tag_map(&[]),
        ),
        (
            "one of several keys",
            tag_map(&[("env", "prod"), ("owner", "platform"), ("tier", "free")]),
            keys(&["owner"]),
            tag_map(&[("env", "prod"), ("tier", "free")]),
        ),
        (
            "several keys at once",
            tag_map(&[("env", "prod"), ("owner", "platform"), ("tier", "free")]),
            keys(&["env", "tier"]),
            tag_map(&[("owner", "platform")]),
        ),
        (
            "every key the resource carries",
            tag_map(&[("env", "prod"), ("owner", "platform")]),
            keys(&["env", "owner"]),
            tag_map(&[]),
        ),
        (
            "a key the resource does not carry removes nothing",
            tag_map(&[("env", "prod")]),
            keys(&["owner"]),
            tag_map(&[("env", "prod")]),
        ),
        (
            "a carried key beside one that is not carried",
            tag_map(&[("env", "prod"), ("owner", "platform")]),
            keys(&["owner", "absent"]),
            tag_map(&[("env", "prod")]),
        ),
        (
            "the same key named twice in one request",
            tag_map(&[("env", "prod"), ("owner", "platform")]),
            keys(&["env", "env"]),
            tag_map(&[("owner", "platform")]),
        ),
        (
            "a resource that carries no tags at all",
            tag_map(&[]),
            keys(&["env"]),
            tag_map(&[]),
        ),
        (
            "a key whose only value was the empty string",
            tag_map(&[("optional", ""), ("env", "prod")]),
            keys(&["optional"]),
            tag_map(&[("env", "prod")]),
        ),
        (
            // `TagKey` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`: any letter, any
            // separator, any number and that punctuation — so a key built from the
            // punctuation the pattern allows is a legal key, as is a value carrying
            // separators and non-ASCII letters.
            "a key carrying the punctuation the key pattern allows",
            tag_map(&[(punctuated_key, "välue with spaces 42"), ("keep", "as-is")]),
            keys(&[punctuated_key]),
            tag_map(&[("keep", "as-is")]),
        ),
        (
            "a key at its published maximum length",
            tag_map(&[(longest_key.as_str(), "v"), ("keep", "as-is")]),
            keys(&[longest_key.as_str()]),
            tag_map(&[("keep", "as-is")]),
        ),
        (
            "half the keys of a resource carrying the per-resource maximum",
            numbered_tags(MAX_TAGS_PER_RESOURCE, "capped"),
            numbered_keys(0..half),
            numbered_tag_slice(half..MAX_TAGS_PER_RESOURCE, "capped"),
        ),
        (
            "every key of a resource carrying the per-resource maximum",
            numbered_tags(MAX_TAGS_PER_RESOURCE, "capped"),
            numbered_keys(0..MAX_TAGS_PER_RESOURCE),
            tag_map(&[]),
        ),
    ];

    for (index, (case, created, removed, expected)) in rows.into_iter().enumerate() {
        let name = format!("untagged-{index}");
        let arn = image_with_tags(&router, &name, created).await;

        let (status, body) = untag_resource(&router, &arn, &removed).await;
        assert_eq!(status, 204, "case {case:?}: untag got {status} {body}");
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: the modelled 204 carries no body, got {body}"
        );

        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored tags to be {expected}, got {actual}"
        );

        // Both tag read paths read the one stored record, so they agree on the removal.
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: list got {status} {listed}");
        assert_eq!(
            listed["Tags"], expected,
            "case {case:?}: expected the listing to report {expected}, got {listed}"
        );

        // Modelled `@idempotent`: the same removal again leaves the same tag set.
        let (status, body) = untag_resource(&router, &arn, &removed).await;
        assert_eq!(status, 204, "case {case:?}: repeat got {status} {body}");
        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: a repeated removal must change nothing, got {actual}"
        );

        // The resource is addressed by ARN or by name; both reach the one record the
        // keys were removed from.
        let by_name = stored_tags(&router, &name).await;
        assert_eq!(
            by_name, expected,
            "case {case:?}: the record read by name must carry the same tags, got {by_name}"
        );
    }
}

/// RULE: a tag removal addresses the resource whose ARN the service itself minted, so a
/// well-formed taggable ARN that names no resource this account holds — never created,
/// already deleted, or the same resource name in another partition, region or account —
/// is the missing-resource condition (404), and nothing is removed.
#[tokio::test]
async fn untagging_an_unheld_resource_is_refused_and_removes_nothing() {
    let router = emulator();

    let held = image_with_tags(&router, "held", tag_map(&[("env", "prod")])).await;

    // An image whose asynchronous deletion the clock has already made due is gone, so
    // its ARN names nothing this account holds any more.
    let gone = image_with_tags(&router, "gone", tag_map(&[("env", "doomed")])).await;
    let (status, deleted) = delete_image(&router, &gone).await;
    assert_eq!(status, 200, "delete the image: got {status} {deleted}");
    tokio::time::sleep(PAST_THE_SPAN).await;

    let absent = expected_image_arn("absent");
    let other_region = format!("arn:aws:lambda:eu-west-1:{ACCOUNT}:microvm-image:held");
    let other_account = format!("arn:aws:lambda:{REGION}:111111111111:microvm-image:held");
    let other_partition =
        format!("arn:aws-us-gov:lambda:us-gov-west-1:{ACCOUNT}:microvm-image:held");
    let other_name = expected_image_arn("held-too");

    let rows: Vec<(&str, &str)> = vec![
        ("an ARN of a resource never created", absent.as_str()),
        ("an ARN whose resource has been deleted", gone.as_str()),
        (
            "the held resource's name in another region",
            other_region.as_str(),
        ),
        (
            "the held resource's name in another account",
            other_account.as_str(),
        ),
        (
            "the held resource's name in another partition",
            other_partition.as_str(),
        ),
        (
            "an ARN whose resource name differs from the held one by a suffix",
            other_name.as_str(),
        ),
    ];

    for (case, resource) in rows {
        let (status, body) = untag_resource(&router, resource, &keys(&["env"])).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            body["message"].as_str(),
            Some(image_not_found_message(resource).as_str()),
            "case {case:?}: expected the 404 to name {resource}, got {body}"
        );
    }

    let actual = stored_tags(&router, &held).await;
    assert_eq!(
        actual,
        tag_map(&[("env", "prod")]),
        "a refused removal must leave every held resource's tags alone, got {actual}"
    );
}

/// RULE: a resource whose asynchronous deletion is still in flight is a record the
/// account still holds and still reads back, so its tags are removed like any other
/// resource's — and once the clock-owed removal has taken the record, untagging it is
/// the missing-resource condition.
#[tokio::test]
async fn a_resource_whose_deletion_is_in_flight_is_still_untagged() {
    let router = emulator();

    let arn = image_with_tags(
        &router,
        "deleting",
        tag_map(&[("env", "prod"), ("owner", "platform")]),
    )
    .await;
    let (status, deleted) = delete_image(&router, &arn).await;
    assert_eq!(status, 200, "delete the resource: got {status} {deleted}");
    assert_eq!(
        deleted["state"].as_str(),
        Some("DELETING"),
        "the delete must report the resource's deletion in flight, got {deleted}"
    );

    let (status, body) = untag_resource(&router, &arn, &keys(&["owner"])).await;
    assert_eq!(
        status, 204,
        "a resource still held while deleting is untaggable, got {status} {body}"
    );
    let actual = stored_tags(&router, &arn).await;
    assert_eq!(
        actual,
        tag_map(&[("env", "prod")]),
        "the key removed while deleting must be off the record, got {actual}"
    );

    tokio::time::sleep(PAST_THE_SPAN).await;
    let (status, body) = untag_resource(&router, &arn, &keys(&["env"])).await;
    assert_eq!(
        status, 404,
        "once the removal the clock owes has happened the resource is gone, got {status} {body}"
    );
}

/// RULE: a tag removal writes to exactly the one resource its ARN names, so the tag
/// sets of every other resource the account holds are unchanged by it — including the
/// keys those resources carry under the very names the removal addressed.
#[tokio::test]
async fn untagging_one_resource_leaves_every_other_resource_alone() {
    let router = emulator();

    // Every resource is created with the same keys, so only the resource the removal
    // names can lose one; the rows differ in which keys that removal takes out.
    let created = tag_map(&[("env", "prod"), ("owner", "platform"), ("tier", "free")]);
    let rows: Vec<(&str, Vec<String>, Value)> = vec![
        (
            "no key of this resource is removed",
            keys(&["absent"]),
            created.clone(),
        ),
        (
            "one key of this resource is removed",
            keys(&["env"]),
            tag_map(&[("owner", "platform"), ("tier", "free")]),
        ),
        (
            "two keys of this resource are removed",
            keys(&["env", "owner"]),
            tag_map(&[("tier", "free")]),
        ),
        (
            "every key of this resource is removed",
            keys(&["env", "owner", "tier"]),
            tag_map(&[]),
        ),
    ];

    let mut expected: Vec<(String, &str, Value)> = Vec::new();
    for (index, (case, removed, want)) in rows.into_iter().enumerate() {
        let arn = image_with_tags(&router, &format!("isolated-{index}"), created.clone()).await;
        let (status, body) = untag_resource(&router, &arn, &removed).await;
        assert_eq!(status, 204, "case {case:?}: untag got {status} {body}");
        expected.push((arn, case, want));
    }

    for (arn, case, want) in &expected {
        let actual = stored_tags(&router, arn).await;
        assert_eq!(
            &actual, want,
            "case {case:?}: {arn} must carry only its own tags {want}, got {actual}"
        );
    }
}

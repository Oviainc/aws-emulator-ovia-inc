// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TagResource` on this service's own resources, driven over the
//! same router the emulator boots.
//!
//! Each test states a RULE and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` already owns,
//! and the read-back goes through `GetMicrovmImage`, which projects the stored `tags`.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_not_found_message, too_many_tags_message, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_image_and_instance_paths::{delete_image, get_image};
use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, image_body, ACCOUNT, PAST_THE_SPAN, REGION,
};

/// The longest `TagKey` the model accepts: `Tags`'s key is `@length(1..128)`
/// (`smithy_query(query="shape",shape="Tags")`).
pub(crate) const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest `TagValue` the model accepts, and a value may also be EMPTY: `Tags`'s
/// value is `@length(0..256)` (`smithy_query(query="shape",shape="Tags")`).
pub(crate) const MAX_TAG_VALUE_LENGTH: usize = 256;

/// `POST /2017-03-31/tags/{Resource}` — the tagging URI this model binds, with the
/// resource ARN as the path label and the tags in the body.
pub(crate) async fn tag_resource(
    router: &MultiServiceRouter,
    resource: &str,
    tags: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/2017-03-31/tags/{}", as_uri_value(resource)),
        Some(json!({"Tags": tags})),
    )
    .await
}

/// A `Tags` map of `count` distinct keys, each holding `value`.
pub(crate) fn numbered_tags(count: usize, value: &str) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("key-{index:03}"), Value::String(value.to_string())))
            .collect(),
    )
}

/// A `Tags` map as the wire shape.
pub(crate) fn tag_map(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

/// Create an image carrying `tags` and return the ARN the service minted for it.
pub(crate) async fn image_with_tags(
    router: &MultiServiceRouter,
    name: &str,
    tags: Value,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, json!({"tags": tags}))),
    )
    .await;
    assert_eq!(status, 201, "create image {name}: got {status} {created}");
    created["imageArn"].as_str().unwrap_or_default().to_string()
}

/// The `tags` `GetMicrovmImage` projects for the resource `identifier` names.
pub(crate) async fn stored_tags(router: &MultiServiceRouter, identifier: &str) -> Value {
    let (status, image) = get_image(router, identifier).await;
    assert_eq!(status, 200, "read {identifier} back: got {status} {image}");
    image["tags"].clone()
}

/// RULE: adding tags to a resource MERGES the request's key-value pairs into the
/// resource's stored tag set — a key the request carries takes the request's value,
/// every other key the resource already carries keeps its own — so the call persists
/// the union, a later read of the resource reports it, and repeating the same call
/// changes nothing.
#[tokio::test]
async fn adding_tags_merges_into_the_stored_resource() {
    let router = emulator();

    let longest_key = "k".repeat(MAX_TAG_KEY_LENGTH);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LENGTH);

    // Rows differ in how many pairs are added, in what the resource already carried,
    // and in the shape of the keys and values — the dimensions the merge rule
    // generalises over.
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "one pair onto a resource created with no tags",
            tag_map(&[]),
            tag_map(&[("env", "prod")]),
            tag_map(&[("env", "prod")]),
        ),
        (
            "several pairs at once",
            tag_map(&[]),
            tag_map(&[("env", "prod"), ("owner", "team.platform"), ("cost:center", "cc-1")]),
            tag_map(&[("env", "prod"), ("owner", "team.platform"), ("cost:center", "cc-1")]),
        ),
        (
            "a new key beside the ones the create carried",
            tag_map(&[("created-by", "suite")]),
            tag_map(&[("env", "prod")]),
            tag_map(&[("created-by", "suite"), ("env", "prod")]),
        ),
        (
            "a key the resource already carries takes the new value, the rest keep theirs",
            tag_map(&[("env", "dev"), ("keep", "as-is")]),
            tag_map(&[("env", "prod")]),
            tag_map(&[("env", "prod"), ("keep", "as-is")]),
        ),
        (
            "an empty value is a value",
            tag_map(&[("env", "dev")]),
            tag_map(&[("optional", "")]),
            tag_map(&[("env", "dev"), ("optional", "")]),
        ),
        (
            // `TagKey`/`TagValue` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`: any
            // letter, any separator, any number and that punctuation.
            "the letters, separators and punctuation the key and value patterns allow",
            tag_map(&[]),
            tag_map(&[("Ünïcode Key_.:/=+-@", "välue with spaces 42")]),
            tag_map(&[("Ünïcode Key_.:/=+-@", "välue with spaces 42")]),
        ),
        (
            "a key and a value at their published maximum lengths",
            tag_map(&[]),
            tag_map(&[(longest_key.as_str(), longest_value.as_str())]),
            tag_map(&[(longest_key.as_str(), longest_value.as_str())]),
        ),
    ];

    for (index, (case, created, added, expected)) in rows.into_iter().enumerate() {
        let name = format!("tagged-{index}");
        let arn = image_with_tags(&router, &name, created).await;

        let (status, body) = tag_resource(&router, &arn, added.clone()).await;
        assert_eq!(status, 204, "case {case:?}: tag got {status} {body}");
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

        // Documented idempotent: the same call again leaves the same tag set.
        let (status, body) = tag_resource(&router, &arn, added).await;
        assert_eq!(status, 204, "case {case:?}: repeat got {status} {body}");
        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: a repeated tag call must change nothing, got {actual}"
        );

        // The resource is addressed by ARN or by name; both reach the one record the
        // tags were written to.
        let by_name = stored_tags(&router, &name).await;
        assert_eq!(
            by_name, expected,
            "case {case:?}: the record read by name must carry the same tags, got {by_name}"
        );
    }
}

/// RULE: a tagging call addresses the resource whose ARN the service itself minted, so
/// a well-formed taggable ARN that names no resource this account holds — never
/// created, already deleted, or the same resource name in another partition, region or
/// account — is the missing-resource condition (404), and nothing is written.
#[tokio::test]
async fn tagging_an_unheld_resource_is_refused_and_writes_nothing() {
    let router = emulator();

    let held = image_with_tags(&router, "held", tag_map(&[("env", "prod")])).await;

    // An image whose asynchronous deletion the clock has already made due is gone, so
    // its ARN names nothing this account holds any more.
    let gone = image_with_tags(&router, "gone", tag_map(&[])).await;
    let (status, deleted) = delete_image(&router, &gone).await;
    assert_eq!(status, 200, "delete the image: got {status} {deleted}");
    tokio::time::sleep(PAST_THE_SPAN).await;

    let absent = expected_image_arn("absent");
    let other_region = format!("arn:aws:lambda:eu-west-1:{ACCOUNT}:microvm-image:held");
    let other_account = format!("arn:aws:lambda:{REGION}:111111111111:microvm-image:held");
    let other_partition =
        format!("arn:aws-us-gov:lambda:us-gov-west-1:{ACCOUNT}:microvm-image:held");

    let rows: Vec<(&str, &str)> = vec![
        ("an ARN of a resource never created", absent.as_str()),
        ("an ARN whose resource has been deleted", gone.as_str()),
        ("the held resource's name in another region", other_region.as_str()),
        ("the held resource's name in another account", other_account.as_str()),
        (
            "the held resource's name in another partition",
            other_partition.as_str(),
        ),
    ];

    for (case, resource) in rows {
        let (status, body) = tag_resource(&router, resource, tag_map(&[("env", "stolen")])).await;
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
        "a refused tag call must leave every held resource's tags alone, got {actual}"
    );
}

/// RULE: a resource whose asynchronous deletion is still in flight is a record the
/// account still holds and still reads back, so it is tagged like any other — and once
/// the clock-owed removal has taken the record, tagging it is the missing-resource
/// condition.
#[tokio::test]
async fn a_resource_whose_deletion_is_in_flight_is_still_tagged() {
    let router = emulator();

    let arn = image_with_tags(&router, "deleting", tag_map(&[("env", "prod")])).await;
    let (status, deleted) = delete_image(&router, &arn).await;
    assert_eq!(status, 200, "delete the resource: got {status} {deleted}");
    assert_eq!(
        deleted["state"].as_str(),
        Some("DELETING"),
        "the delete must report the resource's deletion in flight, got {deleted}"
    );

    let (status, body) = tag_resource(&router, &arn, tag_map(&[("owner", "platform")])).await;
    assert_eq!(
        status, 204,
        "a resource still held while deleting is taggable, got {status} {body}"
    );
    let actual = stored_tags(&router, &arn).await;
    assert_eq!(
        actual,
        tag_map(&[("env", "prod"), ("owner", "platform")]),
        "the tags written while deleting must be on the record, got {actual}"
    );

    tokio::time::sleep(PAST_THE_SPAN).await;
    let (status, body) = tag_resource(&router, &arn, tag_map(&[("owner", "platform")])).await;
    assert_eq!(
        status, 404,
        "once the removal the clock owes has happened the resource is gone, got {status} {body}"
    );
}

/// RULE: a tagging call writes to exactly the one resource its ARN names, so the tag
/// sets of every other resource the account holds are unchanged by it.
#[tokio::test]
async fn tagging_one_resource_leaves_every_other_resource_alone() {
    let router = emulator();

    // Rows differ in what each resource was created with and in what is added to it,
    // so no two resources end with the same tag set.
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "created with no tags, one added",
            tag_map(&[]),
            tag_map(&[("env", "prod")]),
            tag_map(&[("env", "prod")]),
        ),
        (
            "created tagged, nothing of its own overwritten",
            tag_map(&[("env", "dev")]),
            tag_map(&[("owner", "platform")]),
            tag_map(&[("env", "dev"), ("owner", "platform")]),
        ),
        (
            "created tagged, its own key overwritten",
            tag_map(&[("env", "dev"), ("tier", "free")]),
            tag_map(&[("env", "staging")]),
            tag_map(&[("env", "staging"), ("tier", "free")]),
        ),
    ];

    let mut expected: Vec<(String, Value)> = Vec::new();
    for (index, (case, created, added, want)) in rows.into_iter().enumerate() {
        let arn = image_with_tags(&router, &format!("isolated-{index}"), created).await;
        let (status, body) = tag_resource(&router, &arn, added).await;
        assert_eq!(status, 204, "case {case:?}: tag got {status} {body}");
        expected.push((arn, want));
    }

    for (arn, want) in &expected {
        let actual = stored_tags(&router, arn).await;
        assert_eq!(
            &actual, want,
            "{arn} must carry only its own tags {want}, got {actual}"
        );
    }
}

/// RULE: a resource carries at most the per-resource maximum number of tags, and the
/// maximum caps the tag set the call would LEAVE on the resource — so a call that only
/// re-values keys the resource already carries stays within it, a call that would take
/// it past the maximum is a bad request, and a refused call writes none of its pairs.
#[tokio::test]
async fn the_tag_set_of_a_resource_is_capped_at_its_published_maximum() {
    let router = emulator();

    // Rows walk the cap from both sides: at the maximum, one past it in a single call,
    // and one past it by adding to a resource already at the maximum.
    let rows: Vec<(&str, Value, Value, u16, Value)> = vec![
        (
            "exactly the maximum in one call",
            tag_map(&[]),
            numbered_tags(MAX_TAGS_PER_RESOURCE, "first"),
            204,
            numbered_tags(MAX_TAGS_PER_RESOURCE, "first"),
        ),
        (
            "one past the maximum in one call",
            tag_map(&[]),
            numbered_tags(MAX_TAGS_PER_RESOURCE + 1, "first"),
            400,
            tag_map(&[]),
        ),
        (
            "re-valuing every key a resource at the maximum already carries",
            numbered_tags(MAX_TAGS_PER_RESOURCE, "first"),
            numbered_tags(MAX_TAGS_PER_RESOURCE, "second"),
            204,
            numbered_tags(MAX_TAGS_PER_RESOURCE, "second"),
        ),
        (
            "one new key on a resource already at the maximum",
            numbered_tags(MAX_TAGS_PER_RESOURCE, "first"),
            tag_map(&[("one-too-many", "no")]),
            400,
            numbered_tags(MAX_TAGS_PER_RESOURCE, "first"),
        ),
        (
            "one new key on a resource one below the maximum",
            numbered_tags(MAX_TAGS_PER_RESOURCE - 1, "first"),
            tag_map(&[("the-last-one", "yes")]),
            204,
            {
                let mut want = numbered_tags(MAX_TAGS_PER_RESOURCE - 1, "first");
                want["the-last-one"] = Value::String("yes".to_string());
                want
            },
        ),
    ];

    for (index, (case, created, added, want_status, want_stored)) in rows.into_iter().enumerate() {
        let arn = image_with_tags(&router, &format!("capped-{index}"), created).await;

        let (status, body) = tag_resource(&router, &arn, added).await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected {want_status}, got {status} {body}"
        );
        if want_status == 400 {
            assert_eq!(
                body["message"].as_str(),
                Some(too_many_tags_message().as_str()),
                "case {case:?}: expected the refusal to state the maximum, got {body}"
            );
        }

        let actual = stored_tags(&router, &arn).await;
        assert_eq!(
            actual, want_stored,
            "case {case:?}: expected the stored tags to be {want_stored}, got {actual}"
        );
    }
}

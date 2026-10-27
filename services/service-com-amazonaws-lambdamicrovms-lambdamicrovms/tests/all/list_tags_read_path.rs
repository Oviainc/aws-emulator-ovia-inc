// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListTags`, driven over the same router the emulator boots.
//!
//! Each test states a RULE and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) and the tagging helpers are the ones
//! `microvm_lifecycle` and `resource_tagging` already own.

use serde_json::Value;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_not_found_message, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_image_and_instance_paths::{delete_image, get_image};
use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, ACCOUNT, PAST_THE_SPAN, REGION,
};
use crate::resource_tagging::{
    image_with_tags, numbered_tags, tag_map, tag_resource, MAX_TAG_KEY_LENGTH,
    MAX_TAG_VALUE_LENGTH,
};

/// `GET /2017-03-31/tags/{Resource}` — the tag-listing URI this model binds, with the
/// resource ARN as the path label and no body.
pub(crate) async fn list_tags(router: &MultiServiceRouter, resource: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/2017-03-31/tags/{}", as_uri_value(resource)),
        None,
    )
    .await
}

/// RULE: listing a resource's tags reports exactly the key-value pairs persisted on the
/// record its ARN names — as a map that is present even when the record carries none —
/// so the listing agrees with the resource's own read path over every tag edit, and
/// reporting the tags leaves the record as every later caller finds it.
#[tokio::test]
async fn listing_tags_reports_the_persisted_tag_set() {
    let router = emulator();

    let longest_key = "k".repeat(MAX_TAG_KEY_LENGTH);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LENGTH);

    // Rows differ in how many pairs the record ends up carrying, in whether they came
    // from the create or from a later tag call, and in the shape of the keys and values
    // — the dimensions the projection rule generalises over.
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "a resource carrying no tags at all",
            tag_map(&[]),
            tag_map(&[]),
            tag_map(&[]),
        ),
        (
            "only the pairs the create carried",
            tag_map(&[("env", "dev")]),
            tag_map(&[]),
            tag_map(&[("env", "dev")]),
        ),
        (
            "only the pairs a later tag call added",
            tag_map(&[]),
            tag_map(&[("env", "prod"), ("owner", "team.platform")]),
            tag_map(&[("env", "prod"), ("owner", "team.platform")]),
        ),
        (
            "the union of the create's pairs and a later call's",
            tag_map(&[("created-by", "suite")]),
            tag_map(&[("env", "prod")]),
            tag_map(&[("created-by", "suite"), ("env", "prod")]),
        ),
        (
            "a key the create carried, re-valued by a later call",
            tag_map(&[("env", "dev"), ("keep", "as-is")]),
            tag_map(&[("env", "prod")]),
            tag_map(&[("env", "prod"), ("keep", "as-is")]),
        ),
        (
            "an empty value is a value, and is reported as one",
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
        (
            "a resource carrying the per-resource maximum number of pairs",
            tag_map(&[]),
            numbered_tags(MAX_TAGS_PER_RESOURCE, "listed"),
            numbered_tags(MAX_TAGS_PER_RESOURCE, "listed"),
        ),
    ];

    for (index, (case, created, added, expected)) in rows.into_iter().enumerate() {
        let name = format!("listed-{index}");
        let arn = image_with_tags(&router, &name, created).await;

        if added != tag_map(&[]) {
            let (status, body) = tag_resource(&router, &arn, added).await;
            assert_eq!(status, 204, "case {case:?}: tag got {status} {body}");
        }

        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: list got {status} {body}");
        assert_eq!(
            body["Tags"], expected,
            "case {case:?}: expected the listed tags to be {expected}, got {body}"
        );

        // Both read paths read the one stored record, so they agree pair for pair.
        let (status, image) = get_image(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: read back got {status} {image}");
        assert_eq!(
            image["tags"], body["Tags"],
            "case {case:?}: the resource's own read path reports {}, the listing {}",
            image["tags"], body["Tags"]
        );

        // Reporting the tags is a read: a second listing, and the resource's own read
        // path after it, are unchanged by the first.
        let (status, again) = list_tags(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: second list got {status} {again}");
        assert_eq!(
            again["Tags"], expected,
            "case {case:?}: listing the tags must change nothing, got {again}"
        );
        let (status, after) = get_image(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: read after got {status} {after}");
        assert_eq!(
            after, image,
            "case {case:?}: listing the tags must leave the record alone, got {after}"
        );
    }
}

/// RULE: a tag listing addresses the resource whose ARN the service itself minted, so a
/// well-formed taggable ARN that names no resource this account holds — never created,
/// already deleted, or the same resource name in another partition, region or account —
/// is the missing-resource condition.
#[tokio::test]
async fn listing_the_tags_of_an_unheld_resource_is_refused() {
    let router = emulator();

    let held = image_with_tags(&router, "held", tag_map(&[("env", "prod")])).await;

    // A resource whose asynchronous deletion the clock has already made due is gone, so
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
    // A resource name this account holds only under ANOTHER resource type is not this
    // service's: `TaggableResource` also accepts `function:`, `layer:`,
    // `capacity-provider:` and `network-connector:`
    // (`smithy_query(query="operation",operation="ListTags")`), and a request naming one
    // of those belongs to the endpoint that mints it, so it never reaches this handler.
    let other_name_same_type = expected_image_arn("held-too");

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
            other_name_same_type.as_str(),
        ),
    ];

    for (case, resource) in rows {
        let (status, body) = list_tags(&router, resource).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(image_not_found_message(resource).as_str()),
            "case {case:?}: expected the 404 to name {resource}, got {body}"
        );
    }

    // The refusals read nothing of the held resource and change nothing on it.
    let (status, body) = list_tags(&router, &held).await;
    assert_eq!(status, 200, "the held resource still lists: {status} {body}");
    assert_eq!(
        body["Tags"],
        tag_map(&[("env", "prod")]),
        "the held resource's tags must be untouched, got {body}"
    );
}

/// RULE: a resource whose asynchronous deletion is still in flight is a record the
/// account still holds and still reads back, so its tags are listed like any other
/// resource's — and once the clock-owed removal has taken the record, listing them is
/// the missing-resource condition.
#[tokio::test]
async fn a_resource_whose_deletion_is_in_flight_still_lists_its_tags() {
    let router = emulator();

    let arn = image_with_tags(&router, "deleting", tag_map(&[("env", "prod")])).await;
    let (status, deleted) = delete_image(&router, &arn).await;
    assert_eq!(status, 200, "delete the resource: got {status} {deleted}");
    assert_eq!(
        deleted["state"].as_str(),
        Some("DELETING"),
        "the delete must report the resource's deletion in flight, got {deleted}"
    );

    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "a resource still held while deleting lists its tags, got {status} {body}"
    );
    assert_eq!(
        body["Tags"],
        tag_map(&[("env", "prod")]),
        "the tags of a record still held must be reported, got {body}"
    );

    tokio::time::sleep(PAST_THE_SPAN).await;
    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 404,
        "once the removal the clock owes has happened the resource is gone, got {status} {body}"
    );
}

/// RULE: a tag listing reports the tags of exactly the one resource its ARN names, so
/// no two resources of the account are ever reported with each other's pairs.
#[tokio::test]
async fn listing_reports_only_the_named_resource_s_tags() {
    let router = emulator();

    // Rows differ in what each resource carries, so no two end with the same tag set.
    let rows: Vec<(&str, Value)> = vec![
        ("a resource carrying nothing", tag_map(&[])),
        ("a resource carrying one pair", tag_map(&[("env", "dev")])),
        (
            "a resource carrying several pairs",
            tag_map(&[("env", "staging"), ("tier", "free")]),
        ),
        (
            "a resource carrying many pairs",
            numbered_tags(MAX_TAGS_PER_RESOURCE / 2, "isolated"),
        ),
    ];

    let mut expected: Vec<(String, &str, Value)> = Vec::new();
    for (index, (case, tags)) in rows.into_iter().enumerate() {
        let arn = image_with_tags(&router, &format!("isolated-{index}"), tags.clone()).await;
        expected.push((arn, case, tags));
    }

    for (arn, case, want) in &expected {
        let (status, body) = list_tags(&router, arn).await;
        assert_eq!(status, 200, "case {case:?}: list got {status} {body}");
        assert_eq!(
            &body["Tags"], want,
            "case {case:?}: expected {arn} to list {want}, got {body}"
        );
    }
}

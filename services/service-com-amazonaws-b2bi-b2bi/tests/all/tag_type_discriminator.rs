// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the document shape `ListTagsForResource` answers with, driven
//! over the wire (awsJson1_0, `x-amz-target: B2BI.ListTagsForResource`) through the same
//! router the emulator boots.
//!
//! RULE under test: every `Tag` object this service writes into a response body carries
//! the `awsJson1_0` `__type` discriminator naming the shape the object is an instance of,
//! beside — and in addition to — the shape's own modelled `Key` and `Value` members. The
//! discriminator belongs to the SHAPE, so it is on EVERY entry of the reported list,
//! whatever stored the tag and however many entries there are.
//!
//! Each test asserts that rule over a TABLE of inputs rather than one recorded request:
//! the rows differ along the resource kind, how the tag set was arrived at (a create-time
//! `tags` list, `TagResource` adding keys, `TagResource` replacing a value, what survives
//! an `UntagResource`), the number of entries, and the key/value spellings.

use serde_json::Value;

// One copy of the wire harness and of the tagging fixtures, shared with the other suites.
use crate::onboarding_loop::b2bi_router;
use crate::tag_resource::{b2bi_fixtures, tag_resource};
use crate::tagging::{given_tagged_resource, list_tags, tag_pairs, Kind};
use crate::untag_resource::untag_resource;

/// The shape id this service's wire names a tag object by, as its recorded
/// `ListTagsForResource` responses carry it: the id the service's own internal model
/// declares the shape under, not the `com.amazonaws.b2bi#Tag` id of the public projection
/// in `smithy-model/`. A constant of the shape — no part of it comes from a request.
///
/// Spelled out here rather than imported from the handler on purpose: this is the
/// RECORDED expectation the response is judged against, so it has to be independent of
/// whatever the implementation happens to hold.
pub(crate) const TAG_WIRE_SHAPE_ID: &str = "com.amazonaws.visby.common#Tag";

/// The members a reported tag object carries: the discriminator plus
/// `com.amazonaws.b2bi#Tag`'s own two (`smithy_query(query="shape",shape="Tag")`).
const TAG_MEMBERS: [&str; 3] = ["__type", "Key", "Value"];

/// Assert the rule on one `Tags` list: every entry is an object carrying exactly
/// [`TAG_MEMBERS`], its `__type` is [`TAG_WIRE_SHAPE_ID`], and the `(Key, Value)` pairs
/// are `expected` (sorted), so a discriminator can never be bought at the price of the
/// content.
fn assert_discriminated(case: &str, tags: &Value, expected: &[(&str, &str)]) {
    let entries = tags
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Tags must be a list, got {tags}"));

    for entry in entries {
        let members = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a Tag entry must be an object, got {entry}"));
        assert_eq!(
            entry["__type"].as_str(),
            Some(TAG_WIRE_SHAPE_ID),
            "case {case:?}: every Tag entry carries the __type discriminator, got {entry}"
        );
        let mut names: Vec<&str> = members.keys().map(String::as_str).collect();
        names.sort_unstable();
        let mut wanted = TAG_MEMBERS;
        wanted.sort_unstable();
        assert_eq!(
            names,
            wanted.to_vec(),
            "case {case:?}: a Tag entry carries the discriminator and the modelled members \
             only, got {entry}"
        );
    }

    let mut wanted: Vec<(String, String)> = expected
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    wanted.sort();
    assert_eq!(
        tag_pairs(tags),
        wanted,
        "case {case:?}: expected the pairs {wanted:?}, got {tags}"
    );
}

/// One row: the case name, the resource kind, and the tags the resource is CREATED with.
type CreatedCase<'a> = (&'a str, Kind, &'a [(&'a str, &'a str)]);

/// RULE: the tags a resource was CREATED with are reported as discriminated `Tag`
/// objects, for every taggable resource kind this service holds.
///
/// The rows span the four resource kinds, one / two / three entries, a key reused across
/// resources with a different value, a dotted and an AWS-style key spelling, and the
/// modelled `TagValue` minimum length (`len 0..256`, so an empty value is legal).
#[tokio::test]
async fn creation_time_tags_are_reported_with_the_type_discriminator() {
    let cases: &[CreatedCase<'_>] = &[
        ("profile, one tag", Kind::Profile, &[("env", "prod")]),
        (
            "profile, three tags",
            Kind::Profile,
            &[
                ("env", "staging"),
                ("team", "integration"),
                ("cost-center", "4711"),
            ],
        ),
        (
            "capability, two tags, one of them a dotted key",
            Kind::Capability,
            &[("env", "dr"), ("owner.team", "edi-platform")],
        ),
        (
            "partnership, one tag with an empty value",
            Kind::Partnership,
            &[("retain-until", "")],
        ),
        (
            "transformer, two tags",
            Kind::Transformer,
            &[("env", "prod"), ("Name", "x12-850-in")],
        ),
    ];

    let (router, profile_id, capability_id) = b2bi_fixtures().await;

    for (case, kind, tags) in cases {
        let arn = given_tagged_resource(
            &router,
            *kind,
            case,
            Some(tags),
            &profile_id,
            &capability_id,
        )
        .await;

        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource({arn}) must be 200, got {status} {body}"
        );
        assert_discriminated(case, &body["Tags"], tags);
    }
}

/// One row: the case name, the tags the profile is created with, the pairs `TagResource`
/// then applies, the keys `UntagResource` then removes, and the tag set that must survive.
type MutatedCase<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
    &'a [&'a str],
    &'a [(&'a str, &'a str)],
);

/// RULE: the discriminator is a property of the Tag SHAPE, not of the write that produced
/// the entry — so a tag added by `TagResource`, one whose value `TagResource` replaced,
/// and the remainder after `UntagResource` are each reported with it, exactly like a
/// creation-time tag.
///
/// The rows span a resource created untagged and then tagged, keys added beside existing
/// ones, a value replaced in place, one of two keys removed, every key removed (the empty
/// list, which has no entry to discriminate), and a removal naming a key the resource
/// never carried.
#[tokio::test]
async fn tags_written_and_removed_after_creation_keep_the_type_discriminator() {
    let cases: &[MutatedCase<'_>] = &[
        (
            "untagged at create, two keys added",
            &[],
            &[("env", "prod"), ("team", "integration")],
            &[],
            &[("env", "prod"), ("team", "integration")],
        ),
        (
            "one key added beside a creation-time tag",
            &[("env", "prod")],
            &[("team", "integration")],
            &[],
            &[("env", "prod"), ("team", "integration")],
        ),
        (
            "a creation-time value replaced in place",
            &[("env", "prod"), ("team", "integration")],
            &[("env", "dr")],
            &[],
            &[("env", "dr"), ("team", "integration")],
        ),
        (
            "one of two keys removed",
            &[("env", "prod")],
            &[("team", "integration")],
            &["team"],
            &[("env", "prod")],
        ),
        (
            "every key removed",
            &[("env", "prod")],
            &[("team", "integration")],
            &["env", "team"],
            &[],
        ),
        (
            "a key the resource never carried removed",
            &[("env", "prod")],
            &[],
            &["absent"],
            &[("env", "prod")],
        ),
    ];

    for (case, created_with, added, removed, expected) in cases {
        // A router (and so a store) per row: the documented five-profile account quota is
        // about the account, and this rule is about one resource's reported document.
        let router = b2bi_router();
        let arn = given_tagged_resource(
            &router,
            Kind::Profile,
            case,
            Some(created_with),
            "",
            "",
        )
        .await;

        if !added.is_empty() {
            let (status, body) = tag_resource(&router, &arn, added).await;
            assert_eq!(
                status, 200,
                "case {case:?}: TagResource({arn}) must be 200, got {status} {body}"
            );
        }
        if !removed.is_empty() {
            let (status, body) = untag_resource(&router, &arn, removed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UntagResource({arn}) must be 200, got {status} {body}"
            );
        }

        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource({arn}) must be 200, got {status} {body}"
        );
        assert!(
            body["Tags"].is_array(),
            "case {case:?}: Tags must be a list even when empty, got {}",
            body["Tags"]
        );
        assert_discriminated(case, &body["Tags"], expected);

        // Reading changes nothing about the document: a second call answers the same.
        let (status, again) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second ListTagsForResource must be 200, got {status} {again}"
        );
        assert_eq!(
            again["Tags"], body["Tags"],
            "case {case:?}: a read must not change the reported document, got {}",
            again["Tags"]
        );
    }
}

/// RULE: the discriminator does not grow with the list — a resource carrying many tags
/// reports one on each entry, up to the published `TagList` maximum
/// (`smithy_query(query="operation",operation="ListTagsForResource")`: `len 0..200`).
///
/// The rows span the list sizes 0 and 1, a middling size, and the published maximum.
#[tokio::test]
async fn every_entry_of_a_long_tag_list_carries_the_discriminator() {
    /// The published maximum of `com.amazonaws.b2bi#TagList`.
    const TAG_LIST_MAX: usize = 200;

    let router = b2bi_router();

    for size in [0usize, 1, 37, TAG_LIST_MAX] {
        let case = format!("{size} tags");
        let keys: Vec<String> = (0..size).map(|i| format!("key-{i}")).collect();
        let values: Vec<String> = (0..size).map(|i| format!("value-{i}")).collect();
        let pairs: Vec<(&str, &str)> = keys
            .iter()
            .map(String::as_str)
            .zip(values.iter().map(String::as_str))
            .collect();

        let arn = given_tagged_resource(
            &router,
            Kind::Profile,
            &case,
            Some(&pairs),
            "",
            "",
        )
        .await;

        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource({arn}) must be 200, got {status} {body}"
        );
        assert_eq!(
            body["Tags"].as_array().map(Vec::len),
            Some(size),
            "case {case:?}: expected {size} entries, got {}",
            body["Tags"]
        );
        assert_discriminated(&case, &body["Tags"], &pairs);
    }
}

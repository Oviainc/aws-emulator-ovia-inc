// SPDX-License-Identifier: CC0-1.0

//! The FORM of every entity tag this service hands out.
//!
//! RULE under test: an `ETag` this service reports is the base64 of a 32-byte digest of the
//! resource — one base64 token of [`ETAG_CHARS`] characters, padded, carrying no
//! whitespace — whichever resource it belongs to and whichever operation reports it. That
//! is the encoding MediaPackage issues entity tags in, so a token this emulator hands out
//! is interchangeable with one a client recorded, persisted or pattern-checked from the
//! real service; a digest rendered in hex would be twice as long and interchangeable with
//! neither. The model admits it: `EntityTag` is `@length` 1..256 with `@pattern ^[\S]+$`
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackagev2#EntityTag",model_path="services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json")`).
//!
//! The rule is asserted alongside what an entity tag is FOR, so the encoding is pinned on a
//! token that still works: the same resource read back reports the same token, a resource
//! that changed reports a different one, and the token the service reported is accepted as
//! the `x-amzn-update-if-match` precondition of a conditional write.
//!
//! The rows differ along what the rule generalises over: the resource KIND (channel group,
//! channel, origin endpoint, harvest job), the operation that reports the tag (create, read
//! and update), and the content the digest is taken over (a bare resource, a described one,
//! a tagged one, several manifests, a differing container type).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::channel_lifecycle::{minimal_endpoint, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};
use crate::shared_entity_tag_precondition::IF_MATCH;

/// How many bytes of digest an entity tag of this service carries: a SHA-256 digest.
const DIGEST_BYTES: usize = 32;

/// How many base64 characters [`DIGEST_BYTES`] encode to: four characters per three-byte
/// quantum, the last quantum padded out because 32 is not a multiple of 3.
const ETAG_CHARS: usize = DIGEST_BYTES.div_ceil(3) * 4;

/// The characters standard base64 encodes with, and the character it pads with.
const BASE64_ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// See [`BASE64_ALPHABET`].
const BASE64_PAD: char = '=';

/// The sizes the modelled `EntityTag` admits, which every reported tag has to be inside.
const ENTITY_TAG_MIN_CHARS: usize = 1;
/// See [`ENTITY_TAG_MIN_CHARS`].
const ENTITY_TAG_MAX_CHARS: usize = 256;

/// Assert `tag` is one base64-encoded 32-byte digest, and answer it.
fn assert_base64_digest(case: &str, tag: &str) {
    assert_eq!(
        tag.chars().count(),
        ETAG_CHARS,
        "case {case:?}: the base64 of a {DIGEST_BYTES}-byte digest is {ETAG_CHARS} characters, \
         got {} in {tag:?}",
        tag.chars().count()
    );
    assert!(
        tag.len() >= ENTITY_TAG_MIN_CHARS && tag.len() <= ENTITY_TAG_MAX_CHARS,
        "case {case:?}: the modelled EntityTag admits {ENTITY_TAG_MIN_CHARS}..\
         {ENTITY_TAG_MAX_CHARS} characters, got {} in {tag:?}",
        tag.len()
    );
    assert!(
        !tag.chars().any(char::is_whitespace),
        "case {case:?}: EntityTag's pattern admits no whitespace, got {tag:?}"
    );
    // 32 bytes leave two bits of the final quantum unused, so exactly one padding
    // character closes the token and every other character is from the alphabet.
    let (body, pad) = tag.split_at(tag.len() - 1);
    assert_eq!(
        pad,
        BASE64_PAD.to_string(),
        "case {case:?}: the base64 of {DIGEST_BYTES} bytes ends in {BASE64_PAD:?}, got {tag:?}"
    );
    for character in body.chars() {
        assert!(
            BASE64_ALPHABET.contains(character),
            "case {case:?}: {character:?} is not a base64 character, in {tag:?}"
        );
    }
}

/// The `ETag` a response reported.
fn etag_of(case: &str, body: &Value) -> String {
    body.get("ETag")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: this response reports an ETag, got {body}"))
        .to_string()
}

/// A child resource a row creates: its name, and the body it is created with.
type Child = (&'static str, fn() -> Value);

/// One resource the rows below build: the group it lives in, and the creates that make it.
struct Row {
    case: &'static str,
    group: &'static str,
    /// The channel to create under the group, with the body it is created with.
    channel: Option<Child>,
    /// The origin endpoint to create under that channel.
    endpoint: Option<Child>,
    /// Whether a harvest job is submitted against that endpoint.
    harvest: bool,
}

/// Rows spanning the resource kinds that carry an entity tag and the content its digest is
/// taken over: a bare group, a described and tagged group, a channel of each input type the
/// model admits for one that receives ingest, an endpoint with one manifest and one with
/// several, and a harvest job.
const ROWS: &[Row] = &[
    Row {
        case: "a bare channel group",
        group: "etag-bare",
        channel: None,
        endpoint: None,
        harvest: false,
    },
    Row {
        case: "a described, tagged group with an HLS channel and a TS endpoint",
        group: "etag-described",
        channel: Some(("chan", || {
            json!({"Description": "a described channel", "tags": {"team": "video"}})
        })),
        endpoint: Some(("end", || minimal_endpoint("TS"))),
        harvest: true,
    },
    Row {
        case: "a CMAF channel with an endpoint publishing several manifests",
        group: "etag-cmaf",
        channel: Some(("chan", || json!({"InputType": "CMAF"}))),
        endpoint: Some(("end", || {
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": "index"}, {"ManifestName": "backup"}],
            })
        })),
        harvest: true,
    },
];

/// Create the resources one row names, answering `(case, the ETag each create reported, the
/// path the resource is read and updated at)` for every one of them.
async fn created(emulator: &Emulator, row: &Row) -> Vec<(String, String, String)> {
    let mut reported = Vec::new();
    let (status, group) = emulator.create_group(row.group, json!({})).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateChannelGroup got {status} {group}",
        row.case
    );
    reported.push((
        format!("{}: CreateChannelGroup", row.case),
        etag_of(row.case, &group),
        format!("/channelGroup/{}", row.group),
    ));

    let Some((channel, body)) = row.channel else {
        return reported;
    };
    let (status, created) = emulator.create_channel(row.group, channel, body()).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateChannel got {status} {created}",
        row.case
    );
    reported.push((
        format!("{}: CreateChannel", row.case),
        etag_of(row.case, &created),
        format!("/channelGroup/{}/channel/{channel}/", row.group),
    ));

    let Some((endpoint, endpoint_body)) = row.endpoint else {
        return reported;
    };
    let (status, created) = emulator
        .create_endpoint(row.group, channel, endpoint, endpoint_body())
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateOriginEndpoint got {status} {created}",
        row.case
    );
    reported.push((
        format!("{}: CreateOriginEndpoint", row.case),
        etag_of(row.case, &created),
        format!(
            "/channelGroup/{}/channel/{channel}/originEndpoint/{endpoint}",
            row.group
        ),
    ));

    if row.harvest {
        let start = epoch_seconds_from_now(600.0);
        let mut submission = harvest_body(start, start + 120.0, &["index"]);
        submission["HarvestJobName"] = json!("job");
        let (status, job) = emulator
            .call(
                "POST",
                &format!(
                    "/channelGroup/{}/channel/{channel}/originEndpoint/{endpoint}/harvestJob",
                    row.group
                ),
                Some(submission),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateHarvestJob got {status} {job}",
            row.case
        );
        reported.push((
            format!("{}: CreateHarvestJob", row.case),
            etag_of(row.case, &job),
            format!(
                "/channelGroup/{}/channel/{channel}/originEndpoint/{endpoint}/harvestJob/job",
                row.group
            ),
        ));
    }
    reported
}

#[tokio::test]
async fn every_entity_tag_is_the_base64_of_a_32_byte_digest() {
    let emulator = Emulator::new();
    // Every tag any operation reported, so two different resources cannot be reported
    // under one entity tag.
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        for (case, tag, path) in created(&emulator, row).await {
            assert_base64_digest(&case, &tag);
            assert!(
                seen.insert(tag.clone()),
                "case {case:?}: two resources must not share an entity tag, got {tag:?} twice"
            );

            // A read reports the token the create did: the tag is a function of the stored
            // resource rather than of the call that reported it.
            let (status, read) = emulator.call("GET", &path, None).await;
            assert_eq!(status, 200, "case {case:?}: the read got {status} {read}");
            let read_tag = etag_of(&case, &read);
            assert_base64_digest(&format!("{case} read back"), &read_tag);
            assert_eq!(
                read_tag, tag,
                "case {case:?}: the read must report the tag the create did, got {read_tag:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_entity_tag_reported_is_the_one_a_conditional_write_is_matched_against() {
    let emulator = Emulator::new();
    let group = "etag-conditional";
    let (status, created) = emulator.create_group(group, json!({})).await;
    assert_eq!(status, 200, "CreateChannelGroup got {status} {created}");
    let created_tag = etag_of("the created group", &created);
    assert_base64_digest("the created group", &created_tag);

    // The token the service handed out is accepted as the precondition, exactly as it was
    // reported: a client echoes it back unchanged.
    let (status, updated) = emulator
        .update_group(
            group,
            json!({"Description": "changed under the precondition"}),
            &[(IF_MATCH, &created_tag)],
        )
        .await;
    assert_eq!(
        status, 200,
        "a conditional update presenting the reported tag got {status} {updated}"
    );
    let updated_tag = etag_of("the updated group", &updated);
    assert_base64_digest("the updated group", &updated_tag);
    assert_ne!(
        updated_tag, created_tag,
        "a resource that changed must report a new entity tag, got {updated_tag:?} again"
    );

    // And the tag that is no longer current is no longer the precondition: the token is
    // compared as the opaque string it is.
    let (status, refused) = emulator
        .update_group(
            group,
            json!({"Description": "a second change"}),
            &[(IF_MATCH, &created_tag)],
        )
        .await;
    assert_eq!(
        status, 409,
        "a conditional update presenting a superseded tag must be refused, got {status} {refused}"
    );
}

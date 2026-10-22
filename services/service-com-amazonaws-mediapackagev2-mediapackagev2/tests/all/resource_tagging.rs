// SPDX-License-Identifier: CC0-1.0

//! What `TagResource` does to a resource's PERSISTED tag map, driven over HTTP through
//! the same router the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. `TagResource` MERGES the submitted key-value pairs into the addressed resource's
//!    stored tag map: a key the resource does not carry is appended, a key it already
//!    carries has its value replaced, and a key the request does not name keeps the value
//!    it had. That holds at every taggable level of the hierarchy, and the merged map is
//!    what `ListTagsForResource` and the resource's own `Get` report afterwards.
//! 2. A tag write is a commit on exactly the resource addressed: it rotates that
//!    resource's ETag, leaves every other member of it alone, and leaves the tag maps of
//!    its parents untouched.
//!
//! The ARN-class refusals (`BadRequestException` for a string that is not an ARN,
//! `NotFoundException` for an ARN naming nothing stored) and the tag bounds live in
//! `tests/all/rejection_rule_and_negative_paths.rs`, which owns those classes.

use std::collections::BTreeMap;

use serde_json::{json, Value};

// The crate's shared test client, defined once in `tests/all/channel_lifecycle.rs`.
use crate::channel_lifecycle::Emulator;

/// A pair list, as the table below spells a tag map.
type Pairs = &'static [(&'static str, &'static str)];

/// `TagResource`: `POST /tags/{ResourceArn}`, `Tags` rendered as `@jsonName("tags")`.
async fn tag(emulator: &Emulator, arn: &str, tags: &[(&str, &str)]) -> (u16, Value) {
    emulator
        .call("POST", &tags_path(arn), Some(json!({"tags": tag_object(tags)})))
        .await
}

/// `ListTagsForResource`: `GET /tags/{ResourceArn}`.
async fn list_tags(emulator: &Emulator, arn: &str) -> (u16, Value) {
    emulator.call("GET", &tags_path(arn), None).await
}

/// The URI the tagging operations bind. `ResourceArn` is a NON-greedy `@httpLabel`, so an
/// SDK percent-encodes the ARN's reserved characters into one path segment and its `:` and
/// `/` never split the path. Escaped by the one label escaper this crate's tests share,
/// `rejection_rule_and_negative_paths::encoded`, so every suite addresses the same URI for
/// the same ARN.
fn tags_path(arn: &str) -> String {
    format!(
        "/tags/{}",
        crate::rejection_rule_and_negative_paths::encoded(arn)
    )
}

/// A pair list as a JSON object, the way a `TagMap` renders on the wire.
fn tag_object(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// A pair list as an order-independent map, for comparing against a response.
fn expected_tags(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A `TagMap` member of a response as an order-independent map. An absent member reads as
/// the empty map, which is what "this resource carries no tags" means.
fn reported_tags(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Which taggable level of the hierarchy a case addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    ChannelGroup,
    Channel,
    OriginEndpoint,
}

/// Every level this emulator can hold a taggable resource at.
const LEVELS: [Level; 3] = [Level::ChannelGroup, Level::Channel, Level::OriginEndpoint];

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::ChannelGroup => "channel group",
            Self::Channel => "channel",
            Self::OriginEndpoint => "origin endpoint",
        }
    }
}

/// One resource created for a case: where it sits, and how to read it back.
struct Resource {
    level: Level,
    group: String,
    channel: String,
    endpoint: String,
    arn: String,
}

impl Resource {
    async fn get(&self, emulator: &Emulator) -> (u16, Value) {
        match self.level {
            Level::ChannelGroup => emulator.get_group(&self.group).await,
            Level::Channel => emulator.get_channel(&self.group, &self.channel).await,
            Level::OriginEndpoint => {
                emulator
                    .get_endpoint(&self.group, &self.channel, &self.endpoint)
                    .await
            }
        }
    }

    /// Which member this resource's own `Get` renders its tag map under:
    /// `GetChannelGroupResponse` carries `@jsonName("tags")` on `Tags`, while
    /// `GetChannelResponse` and `GetOriginEndpointResponse` do not.
    fn tags_member(&self) -> &'static str {
        match self.level {
            Level::ChannelGroup => "tags",
            _ => "Tags",
        }
    }
}

/// Create one resource at `level`, named after `id`, with `tags` on the resource itself
/// and none on its parents. Every resource is created WITH a description, so an assertion
/// that a tag write left the other members alone compares real values rather than two
/// absences.
async fn create_resource(emulator: &Emulator, level: Level, id: &str, tags: Pairs) -> Resource {
    let group = format!("grp-{id}");
    let channel = format!("chan-{id}");
    let endpoint = format!("ep-{id}");
    let own = |owner: Level| if owner == level { tags } else { &[][..] };

    let (status, body) = emulator
        .create_group(
            &group,
            // `CreateChannelGroupRequest` renders `Tags` as `@jsonName("tags")`.
            json!({
                "Description": format!("group {id}"),
                "tags": tag_object(own(Level::ChannelGroup)),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {id:?}: CreateChannelGroup expected 200, got {status} {body}"
    );
    let mut arn = body["Arn"].as_str().unwrap_or_default().to_string();

    if level != Level::ChannelGroup {
        let (status, body) = emulator
            .create_channel(
                &group,
                &channel,
                // `CreateChannelRequest` also renders `Tags` as `@jsonName("tags")`.
                json!({
                    "Description": format!("channel {id}"),
                    "tags": tag_object(own(Level::Channel)),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {id:?}: CreateChannel expected 200, got {status} {body}"
        );
        arn = body["Arn"].as_str().unwrap_or_default().to_string();
    }

    if level == Level::OriginEndpoint {
        let (status, body) = emulator
            .create_endpoint(
                &group,
                &channel,
                &endpoint,
                json!({
                    "ContainerType": "TS",
                    "Description": format!("endpoint {id}"),
                    // At least one manifest: the service refuses an endpoint with none
                    // (`ValidationExceptionType` `NUM_MANIFESTS_LOW`).
                    "HlsManifests": [{"ManifestName": "index"}],
                    // `CreateOriginEndpointRequest` carries NO `@jsonName` on `Tags`.
                    "Tags": tag_object(tags),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {id:?}: CreateOriginEndpoint expected 200, got {status} {body}"
        );
        arn = body["Arn"].as_str().unwrap_or_default().to_string();
    }

    assert!(
        !arn.is_empty(),
        "case {id:?}: the create must report the resource's ARN"
    );
    Resource {
        level,
        group,
        channel,
        endpoint,
        arn,
    }
}

/// One row of the merge table: the tags a resource is created with, the sequence of
/// `TagResource` writes applied to it, and the map it must carry afterwards.
struct MergeCase {
    label: &'static str,
    created_with: Pairs,
    writes: &'static [Pairs],
    expected: Pairs,
}

/// The merge table. Rows differ in whether a written key is new or already present, in
/// how many writes arrive, in how many pairs each carries, and in the form of the keys and
/// values (ASCII, punctuation, non-ASCII, empty value) — `TagKey` and `TagValue` carry no
/// `@length` or `@pattern` in the model, so any string is a legal tag.
const MERGE_CASES: &[MergeCase] = &[
    MergeCase {
        label: "new keys on an untagged resource",
        created_with: &[],
        writes: &[&[("env", "prod"), ("team", "video")]],
        expected: &[("env", "prod"), ("team", "video")],
    },
    MergeCase {
        label: "an existing key is replaced and the others are kept",
        created_with: &[("env", "prod"), ("team", "video")],
        writes: &[&[("env", "staging")]],
        expected: &[("env", "staging"), ("team", "video")],
    },
    MergeCase {
        label: "a new key is appended to the create-time tags",
        created_with: &[("env", "prod")],
        writes: &[&[("costCenter", "1234")]],
        expected: &[("costCenter", "1234"), ("env", "prod")],
    },
    MergeCase {
        label: "successive writes accumulate",
        created_with: &[],
        writes: &[&[("a", "1")], &[("b", "2")], &[("c", "3")]],
        expected: &[("a", "1"), ("b", "2"), ("c", "3")],
    },
    MergeCase {
        label: "the last write wins for a repeated key",
        created_with: &[("k", "0")],
        writes: &[&[("k", "1")], &[("k", "2")], &[("k", "3")]],
        expected: &[("k", "3")],
    },
    MergeCase {
        label: "an empty tag map leaves the resource unchanged",
        created_with: &[("env", "prod")],
        writes: &[&[]],
        expected: &[("env", "prod")],
    },
    MergeCase {
        label: "keys and values of every form",
        created_with: &[("plain", "value")],
        writes: &[&[
            ("dotted.key:with/punctuation", "a b c"),
            ("ключ", "значение"),
            ("empty-value", ""),
        ]],
        expected: &[
            ("dotted.key:with/punctuation", "a b c"),
            ("empty-value", ""),
            ("plain", "value"),
            ("ключ", "значение"),
        ],
    },
    MergeCase {
        label: "a write replacing every key it finds",
        created_with: &[("a", "1"), ("b", "2")],
        writes: &[&[("a", "9"), ("b", "8")]],
        expected: &[("a", "9"), ("b", "8")],
    },
];

#[tokio::test]
async fn tag_resource_merges_its_pairs_into_the_resources_tags_at_every_level() {
    let emulator = Emulator::new();
    for (index, case) in MERGE_CASES.iter().enumerate() {
        for level in LEVELS {
            let case_label = format!("{} / {}", case.label, level.label());
            let id = format!("{index}-{}", level as usize);
            let resource = create_resource(&emulator, level, &id, case.created_with).await;

            // Before any write, the tags the create stored are what is reported — so the
            // merge below is measured against a known starting map.
            let (status, listed) = list_tags(&emulator, &resource.arn).await;
            assert_eq!(
                status, 200,
                "case {case_label:?}: ListTagsForResource expected 200, got {status} {listed}"
            );
            assert_eq!(
                reported_tags(&listed["tags"]),
                expected_tags(case.created_with),
                "case {case_label:?}: the create-time tags must be reported, got {listed}"
            );

            for (write_index, write) in case.writes.iter().enumerate() {
                let (status, body) = tag(&emulator, &resource.arn, write).await;
                assert_eq!(
                    status, 204,
                    "case {case_label:?} write {write_index}: TagResource expected its modeled \
                     204, got {status} {body}"
                );
                assert!(
                    body.is_null(),
                    "case {case_label:?} write {write_index}: a 204 carries no body, got {body}"
                );
            }

            let expected = expected_tags(case.expected);
            let (status, listed) = list_tags(&emulator, &resource.arn).await;
            assert_eq!(
                status, 200,
                "case {case_label:?}: ListTagsForResource expected 200, got {status} {listed}"
            );
            assert_eq!(
                reported_tags(&listed["tags"]),
                expected,
                "case {case_label:?}: ListTagsForResource reported {listed}"
            );

            // The round trip through the resource's own reader: tagging writes to the one
            // record the Get reads, not to a map beside it.
            let (status, read) = resource.get(&emulator).await;
            assert_eq!(
                status, 200,
                "case {case_label:?}: the resource must still read back, got {status} {read}"
            );
            assert_eq!(
                reported_tags(&read[resource.tags_member()]),
                expected,
                "case {case_label:?}: the resource's own read reported {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_tag_write_commits_the_addressed_resource_and_nothing_else() {
    let emulator = Emulator::new();
    for level in LEVELS {
        let label = level.label();
        let resource = create_resource(
            &emulator,
            level,
            &format!("etag{}", level as usize),
            &[("env", "prod")],
        )
        .await;

        let (_, before) = resource.get(&emulator).await;
        let (status, body) =
            tag(&emulator, &resource.arn, &[("env", "staging"), ("owner", "video")]).await;
        assert_eq!(
            status, 204,
            "case {label:?}: TagResource expected 204, got {status} {body}"
        );

        let (_, after) = resource.get(&emulator).await;
        assert_ne!(
            after["ETag"], before["ETag"],
            "case {label:?}: a tag write is a commit, so it must rotate the entity tag; got {} \
             both times",
            after["ETag"]
        );
        for member in ["Arn", "CreatedAt", "Description"] {
            assert_eq!(
                after[member], before[member],
                "case {label:?}: a tag write must not change {member}, got {} where {} was stored",
                after[member], before[member]
            );
        }
        assert!(
            after["Description"].is_string(),
            "case {label:?}: the comparison above needs a description to compare, got {after}"
        );

        // Tagging one resource is not tagging its parents.
        if level == Level::OriginEndpoint {
            let (_, channel) = emulator.get_channel(&resource.group, &resource.channel).await;
            assert!(
                reported_tags(&channel["Tags"]).is_empty(),
                "case {label:?}: tagging the endpoint must leave its channel untagged, got \
                 {channel}"
            );
            let (_, group) = emulator.get_group(&resource.group).await;
            assert!(
                reported_tags(&group["tags"]).is_empty(),
                "case {label:?}: tagging the endpoint must leave its group untagged, got {group}"
            );
        }
    }
}

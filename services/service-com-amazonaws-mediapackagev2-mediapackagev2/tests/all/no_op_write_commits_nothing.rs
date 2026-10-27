// SPDX-License-Identifier: CC0-1.0

//! What a write that stored NOTHING NEW does to a MediaPackageV2 resource, driven over
//! HTTP through the same router the emulator boots.
//!
//! The rule under test, asserted over a table rather than one recorded request: a write
//! commits a new version of the resource it addresses — restamping `ModifiedAt` and
//! rotating the entity tag — only when it CHANGED that resource's stored content, so a
//! write that stored exactly what was already there leaves the resource, its `ModifiedAt`
//! and its ETag alone, and the tag the caller last read stays usable as the precondition
//! of the next write it is entitled to make.
//!
//! AWS documents the ETag's PURPOSE (it is what `x-amzn-update-if-match` is matched
//! against) but not how it is computed, so this is the emulator's own contract for that
//! precondition — a tag that rotated on every write would refuse a conditional update the
//! caller is entitled to make — not a value read off the model. The rows span the three
//! update paths (`UpdateChannelGroup`, `UpdateChannel`, `UpdateOriginEndpoint`) and the two
//! tag-writing paths (`TagResource`, `UntagResource`), and differ in the KIND of content
//! re-submitted: absent, scalar, at a modelled length bound, non-ASCII, structured, and
//! list-valued.
//!
//! Each row also carries its control: the same path, given content that DOES differ,
//! commits — the tag rotates and the superseded one is then refused `ConflictException`
//! (409) / `CONFLICTING_OPERATION`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

// The crate's shared test client (and its tagging calls, declared `pub(crate)` on it in
// `resource_tags`), so this suite drives the emulator through one client rather than
// growing a second copy of the dispatch boilerplate.
use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};

/// The header the model binds every `Update*`'s `ETag` member to.
const IF_MATCH: &str = "x-amzn-update-if-match";

/// The parent chain every row hangs off. The names match the model's `ResourceName`
/// pattern `^[a-zA-Z0-9_-]+$`.
const GROUP: &str = "nop-group";
const CHANNEL: &str = "nop-channel";
const ENDPOINT: &str = "nop-endpoint";

/// Which update path a row exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Group,
    Channel,
    Endpoint,
}

impl Level {
    /// Create the resource under test with `body`.
    async fn create(self, emulator: &Emulator, body: Value) -> (u16, Value) {
        match self {
            Self::Group => emulator.create_group(GROUP, body).await,
            Self::Channel => emulator.create_channel(GROUP, CHANNEL, body).await,
            Self::Endpoint => emulator.create_endpoint(GROUP, CHANNEL, ENDPOINT, body).await,
        }
    }

    /// Update the resource under test with `body`, under the `x-amzn-update-if-match`
    /// precondition when the case presents one.
    async fn update(
        self,
        emulator: &Emulator,
        body: Value,
        if_match: Option<&str>,
    ) -> (u16, Value) {
        let headers: Vec<(&str, &str)> = if_match.map(|tag| (IF_MATCH, tag)).into_iter().collect();
        match self {
            Self::Group => emulator.update_group(GROUP, body, &headers).await,
            Self::Channel => emulator.update_channel(GROUP, CHANNEL, body, &headers).await,
            Self::Endpoint => {
                emulator
                    .update_endpoint(GROUP, CHANNEL, ENDPOINT, body, &headers)
                    .await
            }
        }
    }

    /// Read the resource under test back.
    async fn read(self, emulator: &Emulator) -> Value {
        let (status, body) = match self {
            Self::Group => emulator.get_group(GROUP).await,
            Self::Channel => emulator.get_channel(GROUP, CHANNEL).await,
            Self::Endpoint => emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await,
        };
        assert_eq!(status, 200, "the read expected 200, got {status} {body}");
        body
    }
}

/// One row of the no-op-write table.
struct NoOpCase {
    label: &'static str,
    level: Level,
    /// Channels of the same group to create before the resource under test: a multiview
    /// channel's available sources must be CMAF channels of its own group.
    sources: &'static [&'static str],
    /// The `Create*` body the resource is stored with.
    create: fn() -> Value,
    /// The `Update*` body that re-submits exactly the content the create stored.
    same: fn() -> Value,
    /// An `Update*` body that stores DIFFERENT content: the row's control.
    changed: fn() -> Value,
}

/// The smallest endpoint body plus the members a row varies, so the endpoint rows carry
/// the required container type and manifest without repeating them.
fn endpoint_body(overlay: Value) -> Value {
    let mut body = minimal_endpoint("TS");
    for (member, value) in overlay.as_object().expect("an object overlay") {
        body[member] = value.clone();
    }
    body
}

/// The rows. The length bound 1024 is `ResourceDescription`'s `@length` (0..1024), the
/// `PreferredInput` values 1..2 are `InputSwitchConfiguration$PreferredInput`'s `@range`,
/// the input types come from the model's `InputType` enum (CMAF | HLS | MULTIVIEW), the
/// layouts from its `MultiviewLayoutType` enum (LAYOUT_2EH | LAYOUT_2PL | LAYOUT_3EL |
/// LAYOUT_3PL | LAYOUT_4E | LAYOUT_4PL) and the error conditions from
/// `EndpointErrorCondition`
/// (`smithy_query(query="operation", operation="UpdateChannel,UpdateChannelGroup,UpdateOriginEndpoint")`).
fn no_op_cases() -> Vec<NoOpCase> {
    vec![
        NoOpCase {
            label: "a channel group whose description is re-sent unchanged",
            level: Level::Group,
            sources: &[],
            create: || json!({"Description": "the same words"}),
            same: || json!({"Description": "the same words"}),
            changed: || json!({"Description": "other words"}),
        },
        NoOpCase {
            label: "a channel group that has no description, and still none after",
            level: Level::Group,
            sources: &[],
            create: || json!({}),
            same: || json!({}),
            changed: || json!({"Description": "now described"}),
        },
        NoOpCase {
            label: "a channel group description at the modelled maximum length of 1024",
            level: Level::Group,
            sources: &[],
            create: || json!({"Description": "d".repeat(1024)}),
            same: || json!({"Description": "d".repeat(1024)}),
            // One character short of the bound: a different value of the same class.
            changed: || json!({"Description": "d".repeat(1023)}),
        },
        NoOpCase {
            label: "a channel group description of non-ASCII text",
            level: Level::Group,
            sources: &[],
            create: || json!({"Description": "vidéo — 4K 😀 日本"}),
            same: || json!({"Description": "vidéo — 4K 😀 日本"}),
            changed: || json!({"Description": "vidéo — 4K 😀 日本語"}),
        },
        NoOpCase {
            label: "a channel group description that is the empty string, the lower bound",
            level: Level::Group,
            sources: &[],
            create: || json!({"Description": ""}),
            same: || json!({"Description": ""}),
            // An omitted description clears it, which on an empty-string description is a
            // change from "" to absent.
            changed: || json!({}),
        },
        NoOpCase {
            label: "a channel whose description is re-sent unchanged",
            level: Level::Channel,
            sources: &[],
            create: || json!({"InputType": "HLS", "Description": "the same words"}),
            same: || json!({"Description": "the same words"}),
            changed: || json!({"Description": "other words"}),
        },
        NoOpCase {
            label: "a channel with nothing set beyond the members the service applies",
            level: Level::Channel,
            sources: &[],
            create: || json!({}),
            same: || json!({}),
            changed: || json!({"Description": "now described"}),
        },
        NoOpCase {
            label: "a CMAF channel whose switching and CMSD blocks are re-sent unchanged",
            level: Level::Channel,
            sources: &[],
            create: || {
                json!({
                    "InputType": "CMAF",
                    "Description": "switched",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": true, "PreferredInput": 2},
                    "OutputHeaderConfiguration": {"PublishMQCS": true},
                })
            },
            same: || {
                json!({
                    "Description": "switched",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": true, "PreferredInput": 2},
                    "OutputHeaderConfiguration": {"PublishMQCS": true},
                })
            },
            // Only the preferred input differs, so the change is inside a structured
            // member rather than in the description.
            changed: || {
                json!({
                    "Description": "switched",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": true, "PreferredInput": 1},
                    "OutputHeaderConfiguration": {"PublishMQCS": true},
                })
            },
        },
        NoOpCase {
            label: "a multiview channel whose sources and layouts are re-sent unchanged",
            level: Level::Channel,
            sources: &["nop-source-a", "nop-source-b"],
            create: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": ["nop-source-a"],
                        "AvailableLayouts": ["LAYOUT_2EH"],
                    },
                })
            },
            same: || {
                json!({
                    "MultiviewConfiguration": {
                        "AvailableSources": ["nop-source-a"],
                        "AvailableLayouts": ["LAYOUT_2EH"],
                    },
                })
            },
            // A list-valued change: one more source and one more layout.
            changed: || {
                json!({
                    "MultiviewConfiguration": {
                        "AvailableSources": ["nop-source-a", "nop-source-b"],
                        "AvailableLayouts": ["LAYOUT_2EH", "LAYOUT_4PL"],
                    },
                })
            },
        },
        NoOpCase {
            label: "an origin endpoint carrying nothing beyond its one manifest",
            level: Level::Endpoint,
            sources: &[],
            create: || endpoint_body(json!({})),
            same: || endpoint_body(json!({})),
            changed: || endpoint_body(json!({"Description": "now described"})),
        },
        NoOpCase {
            label: "a fully configured origin endpoint re-sent unchanged",
            level: Level::Endpoint,
            sources: &[],
            create: || {
                endpoint_body(json!({
                    "Description": "stable",
                    "StartoverWindowSeconds": 60,
                    "Segment": {"SegmentDurationSeconds": 6, "SegmentName": "seg"},
                    "ForceEndpointErrorConfiguration": {
                        "EndpointErrorConditions": ["MISSING_DRM_KEY"],
                    },
                }))
            },
            same: || {
                endpoint_body(json!({
                    "Description": "stable",
                    "StartoverWindowSeconds": 60,
                    "Segment": {"SegmentDurationSeconds": 6, "SegmentName": "seg"},
                    "ForceEndpointErrorConfiguration": {
                        "EndpointErrorConditions": ["MISSING_DRM_KEY"],
                    },
                }))
            },
            // A numeric change inside the same configuration.
            changed: || {
                endpoint_body(json!({
                    "Description": "stable",
                    "StartoverWindowSeconds": 61,
                    "Segment": {"SegmentDurationSeconds": 6, "SegmentName": "seg"},
                    "ForceEndpointErrorConfiguration": {
                        "EndpointErrorConditions": ["MISSING_DRM_KEY"],
                    },
                }))
            },
        },
    ]
}

/// How many times a row re-submits the stored content. More than one, because the defect
/// this pins is that the SECOND identical request is refused the tag the first handed
/// back: the invariant is idempotence, not just a single stable answer.
const REPEATS: usize = 3;

/// THE RULE — an update that stored what was already there commits nothing, so the tag the
/// caller read stays accepted; an update that stored something else commits, which rotates
/// the tag and makes the one it superseded a conflict.
#[tokio::test]
async fn an_update_that_stores_what_was_already_there_commits_nothing() {
    for case in no_op_cases() {
        let label = case.label;
        let level = case.level;
        let emulator = Emulator::new();

        // The parent chain of the level under test, plus the channels a multiview row's
        // sources must name. A group row's own resource IS the group, so it is created
        // below, by the row's create body, and not here.
        if level != Level::Group {
            let (status, body) = emulator.create_group(GROUP, json!({})).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the parent group create expected 200, got {status} {body}"
            );
        }
        for source in case.sources {
            let (status, created) = emulator
                .create_channel(GROUP, source, json!({"InputType": "CMAF"}))
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: CreateChannel {source} expected 200, got {status} {created}"
            );
        }
        if level == Level::Endpoint {
            let (status, created) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the parent channel create expected 200, got {status} {created}"
            );
        }

        let (status, created) = level.create(&emulator, (case.create)()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the create expected 200, got {status} {created}"
        );
        let first_tag = created["ETag"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the create must report an ETag, got {created}"))
            .to_string();

        // Everything the service stored for this resource, as the matching read reports
        // it: the whole body is the comparison, so `ModifiedAt`, the ETag and every
        // content member are pinned at once.
        let stored = level.read(&emulator).await;

        // Re-submitting the stored content, first unconditionally and then under the tag
        // the caller already holds, which is the precondition it is entitled to re-use.
        for attempt in 0..REPEATS {
            let presented = (attempt > 0).then_some(first_tag.as_str());
            let (status, answered) = level.update(&emulator, (case.same)(), presented).await;
            assert_eq!(
                status, 200,
                "case {label:?}/{attempt}: re-submitting the stored content expected 200, \
                 got {status} {answered}"
            );
            assert_eq!(
                answered["ETag"],
                json!(first_tag),
                "case {label:?}/{attempt}: a write that stored nothing new must report the \
                 stored ETag, got {}",
                answered["ETag"]
            );
            let after = level.read(&emulator).await;
            assert_eq!(
                after, stored,
                "case {label:?}/{attempt}: a write that stored nothing new must leave the \
                 resource alone, got {after} where {stored} was stored"
            );
        }

        // The control: content that DIFFERS commits, which rotates the tag — and the
        // superseded tag is then the conflict class.
        let (status, committed) = level
            .update(&emulator, (case.changed)(), Some(&first_tag))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: an update carrying the current ETag expected 200, got {status} \
             {committed}"
        );
        let second_tag = committed["ETag"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the update must report an ETag, got {committed}"))
            .to_string();
        assert_ne!(
            second_tag, first_tag,
            "case {label:?}: a write that changed the resource must rotate the ETag, got \
             {second_tag} twice"
        );
        let changed_read = level.read(&emulator).await;
        assert_eq!(
            changed_read["ETag"],
            json!(second_tag),
            "case {label:?}: the read must report the tag the committed update returned, got \
             {changed_read}"
        );
        assert!(
            modified_at(&changed_read, label) >= modified_at(&stored, label),
            "case {label:?}: ModifiedAt {} must not precede the one before the commit, {}",
            modified_at(&changed_read, label),
            modified_at(&stored, label)
        );

        let (status, refused) = level
            .update(&emulator, (case.same)(), Some(&first_tag))
            .await;
        assert_eq!(
            status, 409,
            "case {label:?}: the superseded ETag expected 409, got {status} {refused}"
        );
        assert_eq!(
            error_type(&refused),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            refused["ConflictExceptionType"],
            json!("CONFLICTING_OPERATION"),
            "case {label:?}: expected ConflictExceptionType CONFLICTING_OPERATION, got {refused}"
        );
        assert_eq!(
            level.read(&emulator).await,
            changed_read,
            "case {label:?}: a refused update must leave the resource alone"
        );
    }
}

/// A body's `ModifiedAt` as epoch seconds.
fn modified_at(body: &Value, label: &str) -> f64 {
    body["ModifiedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {label:?}: expected a ModifiedAt timestamp, got {body}"))
}

/// A pair list as the map `TagResource` takes.
fn tag_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// What one row of the tag-write table submits.
enum TagWrite {
    /// `TagResource` with these pairs.
    Assign(BTreeMap<String, String>),
    /// `UntagResource` with these keys.
    Remove(&'static [&'static str]),
}

/// THE SAME RULE on the tag-writing paths — the tags are part of each resource's content,
/// so a tag write that stored exactly the tags already there commits nothing, while one
/// that changed them commits.
///
/// The rows are the ways a tag write can store nothing new: assigning a key its current
/// value, assigning the whole stored map again, and removing keys the resource does not
/// carry — at every level of the hierarchy, since each level's tags live on a record of
/// its own.
#[tokio::test]
async fn a_tag_write_that_stores_the_tags_already_there_commits_nothing() {
    let stored_tags = tag_map(&[("env", "prod"), ("owner", "video")]);

    for level in [Level::Group, Level::Channel, Level::Endpoint] {
        let label = format!("{level:?}");
        let emulator = Emulator::new();
        if level != Level::Group {
            let (status, body) = emulator.create_group(GROUP, json!({})).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the parent group create expected 200, got {status} {body}"
            );
        }
        if level == Level::Endpoint {
            let (status, created) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the parent channel create expected 200, got {status} {created}"
            );
        }

        // `CreateChannelGroup`/`CreateChannel` take the tag map in the member the model
        // renames `tags`, `CreateOriginEndpoint` in `Tags`
        // (`smithy_query(query="operation", operation="CreateChannelGroup,CreateChannel,CreateOriginEndpoint")`).
        let create = match level {
            Level::Group | Level::Channel => json!({"tags": stored_tags}),
            Level::Endpoint => endpoint_body(json!({"Tags": stored_tags})),
        };
        let (status, created) = level.create(&emulator, create).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the create expected 200, got {status} {created}"
        );
        let arn = created["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the create must report an Arn, got {created}"))
            .to_string();
        let stored = level.read(&emulator).await;

        // (what the write is, and what it submits)
        let no_ops: [(&str, TagWrite); 4] = [
            (
                "assigning one key the value it already has",
                TagWrite::Assign(tag_map(&[("env", "prod")])),
            ),
            (
                "assigning the whole stored map again",
                TagWrite::Assign(stored_tags.clone()),
            ),
            (
                "removing a key the resource does not carry",
                TagWrite::Remove(&["absent"]),
            ),
            (
                "removing two keys the resource does not carry",
                TagWrite::Remove(&["absent", "also-absent"]),
            ),
        ];
        for (what, write) in no_ops {
            let (status, answered) = match &write {
                TagWrite::Assign(tags) => emulator.tag(&arn, tags).await,
                TagWrite::Remove(keys) => emulator.untag(&arn, keys).await,
            };
            assert_eq!(
                status, 204,
                "case {label:?}/{what:?}: the tag write expected 204, got {status} {answered}"
            );
            let after = level.read(&emulator).await;
            assert_eq!(
                after, stored,
                "case {label:?}/{what:?}: a tag write that stored nothing new must leave the \
                 resource alone, got {after} where {stored} was stored"
            );
        }

        // The control: a tag write that CHANGED the map commits, so the tag rotates.
        let (status, answered) = emulator.tag(&arn, &tag_map(&[("env", "staging")])).await;
        assert_eq!(
            status, 204,
            "case {label:?}: replacing a tag value expected 204, got {status} {answered}"
        );
        let replaced = level.read(&emulator).await;
        assert_ne!(
            replaced["ETag"], stored["ETag"],
            "case {label:?}: a tag write that changed a value must rotate the ETag, got {} \
             twice",
            replaced["ETag"]
        );

        let (status, answered) = emulator.untag(&arn, &["owner"]).await;
        assert_eq!(
            status, 204,
            "case {label:?}: removing a stored key expected 204, got {status} {answered}"
        );
        let removed = level.read(&emulator).await;
        assert_ne!(
            removed["ETag"], replaced["ETag"],
            "case {label:?}: a tag write that removed a key must rotate the ETag, got {} twice",
            removed["ETag"]
        );
    }
}

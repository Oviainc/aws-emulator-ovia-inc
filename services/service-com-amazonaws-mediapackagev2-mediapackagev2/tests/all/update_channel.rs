// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateChannel`, driven over HTTP through the same router the
//! emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. An update REPLACES the members it governs (`Description`,
//!    `InputSwitchConfiguration`, `OutputHeaderConfiguration`) — an omitted
//!    `Description` is cleared, while the two server-materialised blocks fall back to
//!    the service's own applied state (`MQCSInputSwitching` / `PublishMQCS` false) —
//!    advances `ModifiedAt`, rotates the ETag, and persists all of it, so the following
//!    `GetChannel` reports exactly what the update returned. The members it does not
//!    govern (name, ARN, `InputType`, ingest endpoints, tags, `CreatedAt`) are
//!    unchanged.
//! 2. The conditional entity tag in `x-amzn-update-if-match` admits exactly the
//!    channel's current tag: no tag is unconditional, the current tag commits, and any
//!    other tag is refused with `ConflictException` (409) /
//!    `ConflictExceptionType` `CONFLICTING_OPERATION`, leaving the channel untouched.
//! 3. An update whose parent chain does not resolve answers `ResourceNotFoundException`
//!    (404) with the `ResourceTypeNotFound` of the level that is missing.
//! 4. A multiview channel's sources and layouts are editable, and the update's view of
//!    them is the one the following read reports.

use serde_json::{json, Value};

// The crate's one emulator client, so this suite drives the router exactly as the other
// suites do rather than growing a second copy of the dispatch boilerplate.
use crate::channel_lifecycle::{error_type, Emulator};

/// The header `UpdateChannel`'s `ETag` member is bound to (`@httpHeader`).
const IF_MATCH: &str = "x-amzn-update-if-match";

/// MediaPackage's rejection sentences, spelled out here rather than read back from the
/// crate: the service's error text is fixed per rejection class and carries nothing from
/// the request, so the sentence itself is part of the contract. The stale-entity-tag one
/// is the text the behavioral corpus records for a conditional update whose
/// `x-amzn-update-if-match` tag is not current (note the typographic apostrophes).
const STALE_ETAG_MESSAGE: &str = "You\u{2019}ve specified an entity tag with your request \
    that does not match the resource\u{2019}s current entity tag. This can occur if the \
    resource has been updated by another request between the time you retrieved the ETag \
    and when you made your update request";

/// The fixed sentence for a channel group that does not exist.
const CHANNEL_GROUP_NOT_FOUND_MESSAGE: &str = "MediaPackage can't process your request \
    because we can't find your channel group. Verify your channel group name or add a \
    channel group and then try again";

/// The fixed sentence for a channel that does not exist.
const CHANNEL_NOT_FOUND_MESSAGE: &str = "MediaPackage can't process your request because \
    we can't find your channel. Verify your channel name or add a channel and then try \
    again";

/// `UpdateChannel` through the shared client, optionally carrying the conditional entity
/// tag in its header.
async fn update_channel(
    emulator: &Emulator,
    group: &str,
    channel: &str,
    body: Value,
    if_match: Option<&str>,
) -> (u16, Value) {
    let headers: Vec<(&str, &str)> = if_match.map(|tag| (IF_MATCH, tag)).into_iter().collect();
    emulator.update_channel(group, channel, body, &headers).await
}

/// A timestamp member as epoch seconds.
fn seconds(body: &Value, member: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("expected {member} to be a timestamp, got {body}"))
}

/// Create a group (or reuse one this suite already created) and one channel in it,
/// answering the created channel's body.
async fn seeded_channel(emulator: &Emulator, group: &str, channel: &str, body: Value) -> Value {
    let (status, group_body) = emulator.create_group(group, json!({})).await;
    assert!(
        // 409 is the group already being there, which is what a second channel in the
        // same group looks like.
        status == 200 || status == 409,
        "CreateChannelGroup {group} expected 200 or 409, got {status} {group_body}"
    );
    let (status, created) = emulator.create_channel(group, channel, body).await;
    assert_eq!(
        status, 200,
        "CreateChannel {channel} expected 200, got {status} {created}"
    );
    created
}

/// One row of the replace-and-read-back table.
struct ReplaceCase {
    label: &'static str,
    /// The channel the update is applied to.
    channel_body: fn() -> Value,
    /// The `UpdateChannel` request body.
    update_body: fn() -> Value,
    /// The members the update must report and the following read must report
    /// identically. `Value::Null` means the member must be absent.
    expected: fn() -> Vec<(&'static str, Value)>,
}

/// The rows. They differ in which members the update carries, and in the value, the
/// length and the character class of the description. `InputType` values come from the
/// model's `InputType` enum (HLS | CMAF | MULTIVIEW) and the 1024 bound from
/// `ResourceDescription`'s `@length` (0..1024); `PreferredInput`'s 1..2 is
/// `InputSwitchConfiguration$PreferredInput`'s `@range`.
fn replace_cases() -> Vec<ReplaceCase> {
    vec![
        ReplaceCase {
            label: "an omitted description is cleared",
            channel_body: || json!({"Description": "initial", "InputType": "HLS"}),
            update_body: || json!({}),
            expected: || vec![("Description", Value::Null)],
        },
        ReplaceCase {
            label: "a description is replaced",
            channel_body: || json!({"Description": "initial"}),
            update_body: || json!({"Description": "edited"}),
            expected: || vec![("Description", json!("edited"))],
        },
        ReplaceCase {
            label: "an empty description is stored as empty, not as absent",
            channel_body: || json!({"Description": "initial"}),
            update_body: || json!({"Description": ""}),
            expected: || vec![("Description", json!(""))],
        },
        ReplaceCase {
            label: "a description at the modelled maximum length of 1024",
            channel_body: || json!({}),
            update_body: || json!({"Description": "d".repeat(1024)}),
            expected: || vec![("Description", json!("d".repeat(1024)))],
        },
        ReplaceCase {
            label: "a non-ASCII description survives byte for byte",
            channel_body: || json!({"Description": "initial"}),
            update_body: || json!({"Description": "vidéo — 4K 😀 日本"}),
            expected: || vec![("Description", json!("vidéo — 4K 😀 日本"))],
        },
        ReplaceCase {
            label: "the MQCS settings are set on a CMAF channel",
            channel_body: || json!({"InputType": "CMAF"}),
            update_body: || {
                json!({
                    "Description": "mqcs on",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": true, "PreferredInput": 2},
                    "OutputHeaderConfiguration": {"PublishMQCS": true},
                })
            },
            expected: || {
                vec![
                    ("Description", json!("mqcs on")),
                    (
                        "InputSwitchConfiguration",
                        json!({"MQCSInputSwitching": true, "PreferredInput": 2}),
                    ),
                    ("OutputHeaderConfiguration", json!({"PublishMQCS": true})),
                ]
            },
        },
        ReplaceCase {
            label: "the MQCS settings fall back to the applied state when omitted",
            channel_body: || {
                json!({
                    "InputType": "CMAF",
                    "InputSwitchConfiguration": {"MQCSInputSwitching": true, "PreferredInput": 1},
                    "OutputHeaderConfiguration": {"PublishMQCS": true},
                })
            },
            update_body: || json!({"Description": "mqcs off"}),
            expected: || {
                vec![
                    ("Description", json!("mqcs off")),
                    // Materialised, not absent: the service reports a switching state
                    // for every channel, with the model's documented default.
                    (
                        "InputSwitchConfiguration",
                        json!({"MQCSInputSwitching": false}),
                    ),
                    ("OutputHeaderConfiguration", json!({"PublishMQCS": false})),
                ]
            },
        },
    ]
}

/// RULE 1 — an update replaces what it governs, persists it, and leaves the rest alone.
#[tokio::test]
async fn an_update_replaces_the_members_it_governs_and_the_read_reports_them() {
    let emulator = Emulator::new();

    for (index, case) in replace_cases().into_iter().enumerate() {
        let label = case.label;
        let group = format!("replace-group-{index}");
        let channel = format!("replace-channel-{index}");
        let created = seeded_channel(&emulator, &group, &channel, (case.channel_body)()).await;

        let (status, updated) =
            update_channel(&emulator, &group, &channel, (case.update_body)(), None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateChannel expected 200, got {status} {updated}"
        );

        for (member, want) in (case.expected)() {
            assert_eq!(
                updated[member], want,
                "case {label:?}: UpdateChannel {member} expected {want}, got {}",
                updated[member]
            );
        }

        // The members UpdateChannel does not take keep the values the create derived.
        for member in [
            "Arn",
            "ChannelName",
            "ChannelGroupName",
            "CreatedAt",
            "InputType",
            "IngestEndpoints",
        ] {
            assert_eq!(
                updated[member], created[member],
                "case {label:?}: UpdateChannel must not change {member}, got {} (was {})",
                updated[member], created[member]
            );
        }
        // `UpdateChannelResponse` carries `@jsonName("tags")` on the tag map, where
        // `CreateChannelResponse` renders the same member as `Tags`: the two reads differ
        // in the member NAME and must agree on the value.
        assert_eq!(
            updated["tags"], created["Tags"],
            "case {label:?}: UpdateChannel must not change the tags, got {} (was {})",
            updated["tags"], created["Tags"]
        );

        let new_etag = updated["ETag"].as_str().unwrap_or_default().to_string();
        assert!(
            !new_etag.is_empty() && !new_etag.contains(char::is_whitespace),
            "case {label:?}: expected an EntityTag (non-empty, no whitespace), got {}",
            updated["ETag"]
        );
        assert_ne!(
            updated["ETag"], created["ETag"],
            "case {label:?}: a committed update must rotate the ETag, got {new_etag} twice"
        );
        assert!(
            seconds(&updated, "ModifiedAt") >= seconds(&created, "CreatedAt"),
            "case {label:?}: ModifiedAt {} must not precede CreatedAt {}",
            seconds(&updated, "ModifiedAt"),
            seconds(&created, "CreatedAt")
        );

        let (status, read) = emulator.get_channel(&group, &channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );
        for member in [
            "Arn",
            "ChannelName",
            "ChannelGroupName",
            "CreatedAt",
            "ModifiedAt",
            "InputType",
            "IngestEndpoints",
            "Description",
            "ETag",
            "InputSwitchConfiguration",
            "OutputHeaderConfiguration",
        ] {
            assert_eq!(
                read[member], updated[member],
                "case {label:?}: GetChannel {member} must equal what the update stored \
                 ({} vs {})",
                read[member], updated[member]
            );
        }
        assert_eq!(
            read["Tags"], updated["tags"],
            "case {label:?}: GetChannel must report the same tags the update did"
        );
    }
}

/// RULE 2 — only the channel's current entity tag lets a conditional update through.
/// The rows are tags that are NOT the current one: a tag that was current until the
/// previous update, a tag belonging to another channel, and tags no resource ever had,
/// including one at `EntityTag`'s maximum length of 256.
#[tokio::test]
async fn a_conditional_update_admits_only_the_channels_current_entity_tag() {
    let emulator = Emulator::new();
    let foreign = seeded_channel(&emulator, "etag-foreign-group", "foreign", json!({})).await;
    let foreign_etag = foreign["ETag"].as_str().expect("an ETag").to_string();

    enum Stale {
        /// The tag that was current before the committed update below.
        Previous,
        /// A tag that is another channel's current tag.
        OtherChannel,
        /// A tag no resource ever had.
        Literal(String),
    }

    let cases: Vec<(&str, Stale)> = vec![
        ("the tag that the last update replaced", Stale::Previous),
        ("another channel's current tag", Stale::OtherChannel),
        ("a tag no resource ever had", Stale::Literal("0".to_string())),
        (
            "an HTTP weak validator, which is not this service's tag form",
            Stale::Literal("W/\"1\"".to_string()),
        ),
        (
            "a tag at EntityTag's maximum length of 256",
            Stale::Literal("a".repeat(256)),
        ),
    ];

    for (index, (label, stale)) in cases.into_iter().enumerate() {
        let group = format!("etag-group-{index}");
        let channel = format!("etag-channel-{index}");
        let created = seeded_channel(
            &emulator,
            &group,
            &channel,
            json!({"Description": "first"}),
        )
        .await;
        let first_etag = created["ETag"].as_str().expect("an ETag").to_string();

        // The current tag commits, and hands back the only tag that is current after it.
        let (status, committed) = update_channel(
            &emulator,
            &group,
            &channel,
            json!({"Description": "second"}),
            Some(&first_etag),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: an update carrying the current ETag expected 200, got \
             {status} {committed}"
        );
        let committed_etag = committed["ETag"].as_str().expect("an ETag").to_string();

        let stale_tag = match &stale {
            Stale::Previous => first_etag.clone(),
            Stale::OtherChannel => foreign_etag.clone(),
            Stale::Literal(tag) => tag.clone(),
        };
        assert_ne!(
            stale_tag, committed_etag,
            "case {label:?}: the test's stale tag must not be the current one"
        );

        let (status, refused) = update_channel(
            &emulator,
            &group,
            &channel,
            json!({"Description": "third"}),
            Some(&stale_tag),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {label:?}: a stale ETag expected 409, got {status} {refused}"
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
        // The rejection sentence is fixed per class: the same text whatever was asked
        // for, carrying none of the request's identifiers.
        let message = refused["Message"].as_str().unwrap_or_default();
        assert_eq!(
            message, STALE_ETAG_MESSAGE,
            "case {label:?}: expected the service's stale-entity-tag sentence, got {message:?}"
        );
        assert!(
            !message.contains(&group) && !message.contains(&channel) && !message.contains(&stale_tag),
            "case {label:?}: the sentence must name nothing from the request, got {message:?}"
        );

        // A refused update writes nothing.
        let (status, read) = emulator.get_channel(&group, &channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );
        assert_eq!(
            read["Description"],
            json!("second"),
            "case {label:?}: the refused update must not change Description, got {read}"
        );
        assert_eq!(
            read["ETag"],
            json!(committed_etag),
            "case {label:?}: the refused update must not rotate the ETag, got {read}"
        );

        // No tag at all is an unconditional update, and it commits.
        let (status, unconditional) =
            update_channel(&emulator, &group, &channel, json!({"Description": "fourth"}), None)
                .await;
        assert_eq!(
            status, 200,
            "case {label:?}: an update carrying no ETag expected 200, got {status} \
             {unconditional}"
        );
        assert_ne!(
            unconditional["ETag"],
            json!(committed_etag),
            "case {label:?}: the committed unconditional update must rotate the ETag"
        );
    }
}

/// RULE 3 — an update whose parent chain does not resolve is a 404 naming the level that
/// is missing.
#[tokio::test]
async fn an_update_of_a_resource_that_does_not_resolve_is_not_found() {
    let emulator = Emulator::new();
    seeded_channel(&emulator, "missing-group", "present", json!({})).await;
    // A channel that existed and was deleted resolves no differently from one that never
    // existed.
    seeded_channel(&emulator, "missing-group", "deleted", json!({})).await;
    let (status, body) = emulator.delete_channel("missing-group", "deleted").await;
    assert_eq!(
        status, 200,
        "DeleteChannel expected 200, got {status} {body}"
    );

    let cases = [
        ("no such channel group", "absent-group", "present", "CHANNEL_GROUP"),
        ("no such channel in the group", "missing-group", "absent", "CHANNEL"),
        ("a channel that was deleted", "missing-group", "deleted", "CHANNEL"),
    ];

    for (label, group, channel, resource_type) in cases {
        let (status, body) =
            update_channel(&emulator, group, channel, json!({"Description": "x"}), None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceTypeNotFound"],
            json!(resource_type),
            "case {label:?}: expected ResourceTypeNotFound {resource_type}, got {body}"
        );
        // Fixed per resource type, and naming nothing from the request.
        let message = body["Message"].as_str().unwrap_or_default();
        let expected = if resource_type == "CHANNEL_GROUP" {
            CHANNEL_GROUP_NOT_FOUND_MESSAGE
        } else {
            CHANNEL_NOT_FOUND_MESSAGE
        };
        assert_eq!(
            message, expected,
            "case {label:?}: expected the fixed {resource_type} sentence, got {message:?}"
        );
        assert!(
            !message.contains(group) && !message.contains(channel),
            "case {label:?}: the sentence must name nothing from the request, got {message:?}"
        );
    }
}

/// RULE 4 — a multiview channel's sources and layouts are editable, and what the update
/// reports is what the following read reports.
///
/// The rows differ in how many sources and how many layouts the configuration carries;
/// the layout values come from the model's `MultiviewLayoutType` enum (LAYOUT_2EH |
/// LAYOUT_2PL | LAYOUT_3EL | LAYOUT_3PL | LAYOUT_4E | LAYOUT_4PL) and the list's
/// `@length` is 1..6, so one row sits at that maximum. Every source is a CMAF channel of
/// the same group, which is what the model's `MultiviewConfiguration` doc requires of a
/// source.
#[tokio::test]
async fn a_multiview_channels_sources_and_layouts_are_editable() {
    let emulator = Emulator::new();
    let group = "multiview-group";
    let (status, body) = emulator.create_group(group, json!({})).await;
    assert_eq!(
        status, 200,
        "CreateChannelGroup expected 200, got {status} {body}"
    );

    for source in ["source-CMAF", "source-CMAF-2"] {
        let (status, created) = emulator
            .create_channel(group, source, json!({"InputType": "CMAF"}))
            .await;
        assert_eq!(
            status, 200,
            "CreateChannel {source} expected 200, got {status} {created}"
        );
    }

    // A multiview channel's sources and layouts are editable. The layout values come
    // from the model's MultiviewLayoutType enum (LAYOUT_2EH | LAYOUT_2PL | LAYOUT_3EL |
    // LAYOUT_3PL | LAYOUT_4E | LAYOUT_4PL). Every row names CMAF channels of this group,
    // which is what a tile source must be; the refusals for the ones that are not are
    // asserted in the multiview-configuration suite.
    let (status, multiview) = emulator
        .create_channel(
            group,
            "tiled",
            json!({
                "InputType": "MULTIVIEW",
                "MultiviewConfiguration": {
                    "AvailableSources": ["source-CMAF"],
                    "AvailableLayouts": ["LAYOUT_2EH"],
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel MULTIVIEW expected 200, got {status} {multiview}"
    );

    for (label, sources, layouts) in [
        (
            "two sources, two layouts",
            vec!["source-CMAF", "source-CMAF-2"],
            vec!["LAYOUT_2EH", "LAYOUT_4E"],
        ),
        (
            "one source, the maximum of six layouts",
            vec!["source-CMAF-2"],
            vec![
                "LAYOUT_2EH",
                "LAYOUT_2PL",
                "LAYOUT_3EL",
                "LAYOUT_3PL",
                "LAYOUT_4E",
                "LAYOUT_4PL",
            ],
        ),
    ] {
        let (status, updated) = update_channel(
            &emulator,
            group,
            "tiled",
            json!({
                "MultiviewConfiguration": {
                    "AvailableSources": sources,
                    "AvailableLayouts": layouts,
                }
            }),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateChannel expected 200, got {status} {updated}"
        );
        assert_eq!(
            updated["MultiviewConfiguration"],
            json!({"AvailableSources": sources, "AvailableLayouts": layouts}),
            "case {label:?}: the update must report the sources and layouts it stored, got {}",
            updated["MultiviewConfiguration"]
        );
        let (status, read) = emulator.get_channel(group, "tiled").await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );
        assert_eq!(
            read["MultiviewConfiguration"], updated["MultiviewConfiguration"],
            "case {label:?}: GetChannel must report the stored multiview configuration"
        );
    }

}

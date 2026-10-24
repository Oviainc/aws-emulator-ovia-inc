// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the derived, read-only `AttachedMultiviewChannels` member,
//! driven over HTTP through the same router the emulator boots.
//!
//! The rule under test: a response member that is a derived projection of stored state
//! is computed from the store by EVERY operation that declares it, so every reader of
//! one channel reports the same value for it at the same instant. For this member the
//! projection is the inverse of the group's multiview source lists — the channels of the
//! group whose `MultiviewConfiguration.AvailableSources` name this channel, in name
//! order, and absent when none do (`AttachedMultiviewChannelList`, documented read-only
//! on `CreateChannelResponse`, `GetChannelResponse`, `UpdateChannelResponse` and
//! `ChannelListConfiguration`). An update neither adds nor removes an attachment, so an
//! `UpdateChannel` response reports the attachments the channel already has.
//!
//! Each test asserts that over a SET of inputs: channels differing in how many multiview
//! channels attach them (none, one, two), updates differing in which members they carry,
//! and a source list that is itself edited so the attachments move between channels.

use serde_json::{json, Value};

use crate::channel_lifecycle::Emulator;

/// `GET /channelGroup/{group}/channel` — the `ListChannels` binding, with a page size
/// large enough to hold every channel these tests create.
async fn list_channels(emulator: &Emulator, group: &str) -> Value {
    let (status, body) = emulator
        .call(
            "GET",
            &format!("/channelGroup/{group}/channel?maxResults=100"),
            None,
        )
        .await;
    assert_eq!(status, 200, "ListChannels expected 200, got {status} {body}");
    body
}

/// The `AttachedMultiviewChannels` of one channel's `ListChannels` item.
fn listed_attachments(listing: &Value, channel: &str) -> Value {
    let items = listing["Items"].as_array().cloned().unwrap_or_default();
    let item = items
        .iter()
        .find(|item| item["ChannelName"] == json!(channel))
        .unwrap_or_else(|| panic!("channel {channel:?} missing from the listing {listing}"));
    item["AttachedMultiviewChannels"].clone()
}

/// Create a channel group, failing the test if it is not accepted.
async fn seed_group(emulator: &Emulator, group: &str) {
    let (status, body) = emulator.create_group(group, json!({})).await;
    assert_eq!(
        status, 200,
        "CreateChannelGroup {group} expected 200, got {status} {body}"
    );
}

/// Create one channel, answering its create response.
async fn seed_channel(emulator: &Emulator, group: &str, name: &str, body: Value) -> Value {
    let (status, created) = emulator.create_channel(group, name, body).await;
    assert_eq!(
        status, 200,
        "CreateChannel {name} expected 200, got {status} {created}"
    );
    created
}

/// RULE — every reader of a channel reports the same derived attachment list, including
/// `UpdateChannel`.
///
/// The rows differ in the number of attaching multiview channels (0, 1, 2), in whether
/// the channel is itself a multiview channel, and in which members the update request
/// carries (nothing at all, a description, the channel's own multiview configuration) —
/// none of which the derived member depends on.
#[tokio::test]
async fn every_channel_reader_reports_the_same_derived_attachment_list() {
    let emulator = Emulator::new();
    let group = "attach-group";
    seed_group(&emulator, group).await;

    // Three CMAF sources, attached by none, one and two multiview channels respectively.
    // `InputType` values come from the model's `InputType` enum (CMAF | HLS | MULTIVIEW).
    for source in ["src-none", "src-one", "src-two"] {
        seed_channel(&emulator, group, source, json!({"InputType": "CMAF"})).await;
    }
    // `AvailableLayouts` values come from the model's `MultiviewLayoutType` enum.
    seed_channel(
        &emulator,
        group,
        "mv-alpha",
        json!({
            "InputType": "MULTIVIEW",
            "MultiviewConfiguration": {
                "AvailableSources": ["src-one", "src-two"],
                "AvailableLayouts": ["LAYOUT_2EH"],
            },
        }),
    )
    .await;
    seed_channel(
        &emulator,
        group,
        "mv-beta",
        json!({
            "InputType": "MULTIVIEW",
            "MultiviewConfiguration": {
                "AvailableSources": ["src-two"],
                "AvailableLayouts": ["LAYOUT_4E", "LAYOUT_3PL"],
            },
        }),
    )
    .await;

    // (label, channel, the update body it is edited with, the expected derived member)
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "no multiview channel names it: the member is absent",
            "src-none",
            json!({"Description": "edited"}),
            Value::Null,
        ),
        (
            "one multiview channel names it, and the update carries no members at all",
            "src-one",
            json!({}),
            json!(["mv-alpha"]),
        ),
        (
            "two multiview channels name it, reported in name order",
            "src-two",
            json!({"Description": "edited"}),
            json!(["mv-alpha", "mv-beta"]),
        ),
        (
            "a multiview channel editing its own sources is not thereby attached",
            "mv-beta",
            json!({
                "MultiviewConfiguration": {
                    "AvailableSources": ["src-two"],
                    "AvailableLayouts": ["LAYOUT_4E"],
                }
            }),
            Value::Null,
        ),
    ];

    for (label, channel, update_body, expected) in cases {
        let (status, read) = emulator.get_channel(group, channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read}"
        );
        assert_eq!(
            read["AttachedMultiviewChannels"], expected,
            "case {label:?}: GetChannel AttachedMultiviewChannels expected {expected}, got {:?}",
            read["AttachedMultiviewChannels"]
        );

        let (status, updated) = emulator
            .update_channel(group, channel, update_body, &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateChannel expected 200, got {status} {updated}"
        );
        assert_eq!(
            updated["AttachedMultiviewChannels"], expected,
            "case {label:?}: UpdateChannel AttachedMultiviewChannels expected {expected}, \
             got {:?}",
            updated["AttachedMultiviewChannels"]
        );

        // The update changed nothing about the attachments, so the reads after it agree
        // with the update's own view of them.
        let (status, reread) = emulator.get_channel(group, channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel after the update expected 200, got {status} {reread}"
        );
        assert_eq!(
            reread["AttachedMultiviewChannels"], updated["AttachedMultiviewChannels"],
            "case {label:?}: GetChannel after the update expected {:?}, got {:?}",
            updated["AttachedMultiviewChannels"], reread["AttachedMultiviewChannels"]
        );
        let listing = list_channels(&emulator, group).await;
        assert_eq!(
            listed_attachments(&listing, channel),
            updated["AttachedMultiviewChannels"],
            "case {label:?}: ListChannels expected {:?}, got {:?}",
            updated["AttachedMultiviewChannels"],
            listed_attachments(&listing, channel)
        );
    }
}

/// RULE, continued — the member is DERIVED, so editing a multiview channel's source list
/// moves the attachment, and the very next update of each affected channel reports the
/// new derivation rather than a value captured earlier.
///
/// The rows walk a source list through sizes 1, 2 and 0-for-one-channel, so each channel
/// is observed both attached and unattached.
#[tokio::test]
async fn editing_a_multiview_source_list_moves_the_reported_attachment() {
    let emulator = Emulator::new();
    let group = "attach-move-group";
    seed_group(&emulator, group).await;
    for source in ["first", "second"] {
        seed_channel(&emulator, group, source, json!({"InputType": "CMAF"})).await;
    }
    seed_channel(
        &emulator,
        group,
        "tiled",
        json!({
            "InputType": "MULTIVIEW",
            "MultiviewConfiguration": {
                "AvailableSources": ["first"],
                "AvailableLayouts": ["LAYOUT_2EH"],
            },
        }),
    )
    .await;

    // (label, the multiview channel's new source list, first's expectation, second's)
    let cases: Vec<(&str, Vec<&str>, Value, Value)> = vec![
        (
            "the attachment moves to the only source now named",
            vec!["second"],
            Value::Null,
            json!(["tiled"]),
        ),
        (
            "both sources are named, so both report the attachment",
            vec!["first", "second"],
            json!(["tiled"]),
            json!(["tiled"]),
        ),
        (
            "the list shrinks back to one source",
            vec!["first"],
            json!(["tiled"]),
            Value::Null,
        ),
    ];

    for (label, sources, first_expected, second_expected) in cases {
        let (status, body) = emulator
            .update_channel(
                group,
                "tiled",
                json!({
                    "MultiviewConfiguration": {
                        "AvailableSources": sources,
                        "AvailableLayouts": ["LAYOUT_2EH"],
                    }
                }),
                &[],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateChannel tiled expected 200, got {status} {body}"
        );

        for (channel, expected) in [("first", &first_expected), ("second", &second_expected)] {
            let (status, updated) = emulator
                .update_channel(group, channel, json!({"Description": label}), &[])
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: UpdateChannel {channel} expected 200, got {status} {updated}"
            );
            assert_eq!(
                updated["AttachedMultiviewChannels"], *expected,
                "case {label:?}: UpdateChannel {channel} AttachedMultiviewChannels expected \
                 {expected}, got {:?}",
                updated["AttachedMultiviewChannels"]
            );
            let (status, read) = emulator.get_channel(group, channel).await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetChannel {channel} expected 200, got {status} {read}"
            );
            assert_eq!(
                read["AttachedMultiviewChannels"], updated["AttachedMultiviewChannels"],
                "case {label:?}: GetChannel {channel} expected {:?}, got {:?}",
                updated["AttachedMultiviewChannels"], read["AttachedMultiviewChannels"]
            );
        }
    }
}

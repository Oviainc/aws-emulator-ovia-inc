// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH channels `ResetChannelState` acts on, driven over HTTP
//! through the same router the emulator boots and reusing the client in
//! [`crate::channel_lifecycle`].
//!
//! The rules under test, each asserted over a SET of channels rather than one recorded
//! request:
//!
//! 1. A reset refreshes the addressed channel's INGEST stream, so a channel that receives
//!    no ingest of its own — the state the model's `InputType` doc describes for
//!    `MULTIVIEW` ("The channel receives no ingest of its own. Instead, it composites
//!    video from the source channels in its MultiviewConfiguration") — is refused with
//!    `ValidationException` (400) and `ValidationExceptionType`
//!    `MULTIVIEW_RESET_NOT_ALLOWED`, the type the model names for this case, and records
//!    nothing; a channel of every other input type the model declares is reset.
//! 2. The refusal is keyed on the ADDRESSED channel's own input type, not on the group it
//!    lives in nor on being named as a tile source: the source channels of a refused
//!    multiview channel reset normally, and the refused channel still reports no reset
//!    afterwards.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};

/// The channel group every row below is created in.
const GROUP: &str = "reset-input-types";

/// The CMAF channels the multiview rows list as tile sources:
/// `MultiviewConfiguration$AvailableSources`' doc requires each source to be in the same
/// channel group and to have an `InputType` of `CMAF`.
const SOURCES: [&str; 2] = ["source-one", "source-two"];

/// Layout values, from the model's `MultiviewLayoutType` enum (LAYOUT_2EH | LAYOUT_2PL |
/// LAYOUT_3EL | LAYOUT_3PL | LAYOUT_4E | LAYOUT_4PL). The rows below use entries the
/// reported case did not cite as well as the whole catalogue, which `AvailableLayouts`'
/// `@length` of 1..6 also makes the longest legal list.
const LAYOUTS: [&str; 6] = [
    "LAYOUT_2EH",
    "LAYOUT_2PL",
    "LAYOUT_3EL",
    "LAYOUT_3PL",
    "LAYOUT_4E",
    "LAYOUT_4PL",
];

/// The `ValidationExceptionType` of an error envelope.
fn validation_type(body: &Value) -> &str {
    body.get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `Message` of an error envelope.
fn message(body: &Value) -> &str {
    body.get("Message").and_then(Value::as_str).unwrap_or("")
}

/// Whether a response carries a member at all, treating an explicit `null` as absent.
fn absent(body: &Value, member: &str) -> bool {
    body.get(member).is_none_or(Value::is_null)
}

/// One row: a channel to create, and whether a reset of it is refused for having no
/// ingest of its own.
struct Row {
    label: &'static str,
    channel: &'static str,
    body: fn() -> Value,
    refused: bool,
}

/// The rows span the model's whole `InputType` enum (CMAF | HLS | MULTIVIEW) plus the
/// absent case the member's doc defaults to HLS, and vary the multiview channels along
/// source count, layout count and the members a reset is indifferent to.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "input type absent (defaults to HLS)",
            channel: "plain",
            body: || json!({}),
            refused: false,
        },
        Row {
            label: "HLS input, described and tagged",
            channel: "hls-channel",
            body: || json!({"InputType": "HLS", "Description": "ingests HLS", "tags": {"k": "v"}}),
            refused: false,
        },
        Row {
            label: "CMAF input",
            channel: "cmaf-channel",
            body: || json!({"InputType": "CMAF"}),
            refused: false,
        },
        Row {
            label: "MULTIVIEW: one source, one layout the report did not cite",
            channel: "mv-one",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": [SOURCES[1]],
                        "AvailableLayouts": [LAYOUTS[4]],
                    },
                })
            },
            refused: true,
        },
        Row {
            label: "MULTIVIEW: every source, the whole layout catalogue",
            channel: "mv-every",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": SOURCES,
                        "AvailableLayouts": LAYOUTS,
                    },
                })
            },
            refused: true,
        },
        Row {
            label: "MULTIVIEW: described and tagged",
            channel: "mv-described",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "Description": "composites two sources",
                    "tags": {"a": "1", "b": "2"},
                    "MultiviewConfiguration": {
                        "AvailableSources": SOURCES,
                        "AvailableLayouts": [LAYOUTS[1], LAYOUTS[5]],
                    },
                })
            },
            refused: true,
        },
    ]
}

/// [`GROUP`] with its [`SOURCES`] in place, so the multiview rows resolve.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        status, 200,
        "setup: creating group {GROUP} must answer 200, got {status}: {body}"
    );
    for source in SOURCES {
        let (status, body) = emulator
            .create_channel(GROUP, source, json!({"InputType": "CMAF"}))
            .await;
        assert_eq!(
            status, 200,
            "setup: creating source {source} must answer 200, got {status}: {body}"
        );
    }
    emulator
}

/// Send a reset of one channel.
async fn reset(emulator: &Emulator, group: &str, channel: &str) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/channelGroup/{group}/channel/{channel}/reset"),
            None,
        )
        .await
}

#[tokio::test]
async fn a_reset_is_refused_on_a_channel_that_receives_no_ingest_of_its_own() {
    let emulator = seeded().await;

    for row in rows() {
        let label = row.label;
        let (status, created) = emulator
            .create_channel(GROUP, row.channel, (row.body)())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: creating {} must answer 200, got {status}: {created}",
            row.channel
        );

        let (status, body) = reset(&emulator, GROUP, row.channel).await;
        if row.refused {
            assert_eq!(
                status, 400,
                "case {label:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_type(&body),
                "ValidationException",
                "case {label:?}: expected a ValidationException envelope, got {body}"
            );
            assert_eq!(
                validation_type(&body),
                "MULTIVIEW_RESET_NOT_ALLOWED",
                "case {label:?}: expected the multiview reset validation type, got {body}"
            );
            // The service states every refusal in one form: the member path the rule is
            // about, then the requirement it failed.
            let expected_prefix = "Validation error encountered at request.channelName: ";
            assert!(
                message(&body).starts_with(expected_prefix),
                "case {label:?}: expected a message naming channelName, got {:?}",
                message(&body)
            );
            assert!(
                absent(&body, "ResetAt"),
                "case {label:?}: a refusal must report no ResetAt, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status}: {body}"
            );
            assert!(
                !absent(&body, "ResetAt"),
                "case {label:?}: an accepted reset must report a ResetAt, got {body}"
            );
        }

        // What the reset recorded, as the channel reads back: an instant on the channels
        // that were reset, and nothing at all on the ones that were refused.
        let (status, read) = emulator.get_channel(GROUP, row.channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: reading {} must answer 200, got {status}: {read}",
            row.channel
        );
        assert_eq!(
            absent(&read, "ResetAt"),
            row.refused,
            "case {label:?}: expected ResetAt absent={}, got {read}",
            row.refused
        );
        // A refused reset leaves the whole channel record as the create left it.
        if row.refused {
            for member in ["ETag", "ModifiedAt", "CreatedAt", "InputType", "MultiviewConfiguration"]
            {
                assert_eq!(
                    read.get(member),
                    created.get(member),
                    "case {label:?}: a refused reset must leave {member} alone: was {:?}, now {:?}",
                    created.get(member),
                    read.get(member)
                );
            }
        }
    }
}

#[tokio::test]
async fn the_sources_of_a_refused_multiview_channel_still_reset() {
    let emulator = seeded().await;
    let multiview = "mv-over-sources";
    let (status, body) = emulator
        .create_channel(
            GROUP,
            multiview,
            json!({
                "InputType": "MULTIVIEW",
                "MultiviewConfiguration": {
                    "AvailableSources": SOURCES,
                    "AvailableLayouts": [LAYOUTS[3]],
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: creating {multiview} must answer 200, got {status}: {body}"
    );

    let (status, body) = reset(&emulator, GROUP, multiview).await;
    assert_eq!(
        status, 400,
        "case {multiview:?}: expected 400, got {status}: {body}"
    );

    // Each source has ingest of its own, so each resets — the refusal is about the
    // channel the request addresses, not about the group or the multiview relationship.
    for source in SOURCES {
        let (status, body) = reset(&emulator, GROUP, source).await;
        assert_eq!(
            status, 200,
            "case {source:?}: expected 200, got {status}: {body}"
        );
        let (_, read) = emulator.get_channel(GROUP, source).await;
        assert_eq!(
            read.get("ResetAt"),
            body.get("ResetAt"),
            "case {source:?}: expected the read to report the instant the reset stored {:?}, \
             got {read}",
            body.get("ResetAt")
        );
    }

    let (_, read) = emulator.get_channel(GROUP, multiview).await;
    assert!(
        absent(&read, "ResetAt"),
        "case {multiview:?}: resetting its sources must record nothing on it, got {read}"
    );
}

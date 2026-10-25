// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ResetChannelState`, driven over HTTP through the same router
//! the emulator boots, with the client from `channel_lifecycle.rs`.
//!
//! The rules under test, each asserted over a SET of channels rather than one recorded
//! request:
//!
//! 1. Resetting a channel records the instant the call refreshed its ingest state on
//!    exactly that channel and reports it with the channel's own identity (its stored
//!    ARN and names), so the matching `GetChannel` reports that instant unchanged while
//!    a channel nobody reset reports none.
//! 2. The reset instant is the time of the LAST reset: a further reset reports a later
//!    one and the read follows it.
//! 3. A reset changes no configuration: description, input type, ingest endpoints, tags,
//!    `CreatedAt`, `ModifiedAt` and the ETag all survive it.
//! 4. A reset addressing a channel whose parent chain does not resolve answers
//!    `ResourceNotFoundException` (404) and records nothing.
//!
//! Which channels have an ingest state to reset at all is covered next door, in
//! [`crate::channel_reset_multiview`].

use std::time::Duration;

use serde_json::{json, Value};

use super::channel_lifecycle::{error_type, Emulator};

/// One row of the reset table: a channel differing from the others in name shape,
/// description, input type and tag count — the dimensions a reset must be indifferent
/// to.
struct ResetCase {
    label: &'static str,
    group: &'static str,
    channel: &'static str,
    channel_body: fn() -> Value,
}

/// The rows. `InputType` values come from the model's `InputType` enum, restricted here
/// to the types whose channels ingest a stream of their own (CMAF, HLS, and the absent
/// case the member's doc defaults to HLS).
fn reset_cases() -> Vec<ResetCase> {
    vec![
        ResetCase {
            label: "minimal: no description, no input type, no tags",
            group: "g1",
            channel: "c1",
            channel_body: || json!({}),
        },
        ResetCase {
            label: "described and tagged, HLS input",
            group: "demo-group",
            channel: "demo-channel",
            channel_body: || {
                json!({"Description": "a channel", "InputType": "HLS", "tags": {"k": "v"}})
            },
        },
        ResetCase {
            label: "long underscored names, CMAF input, three tags",
            group: "A_group_with_9_and-dashes",
            channel: "A_channel_0",
            channel_body: || {
                json!({"InputType": "CMAF", "tags": {"a": "1", "b": "2", "c": "3"}})
            },
        },
        ResetCase {
            label: "single-character names, empty description",
            group: "G",
            channel: "C",
            channel_body: || json!({"Description": ""}),
        },
    ]
}

/// A required timestamp member, as the epoch seconds restJson1 renders it to.
fn timestamp(case: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} timestamp, got {body}"))
}

/// A member a response is NOT expected to carry, treating an explicit `null` as absent.
fn absent(body: &Value, member: &str) -> bool {
    body.get(member).is_none_or(Value::is_null)
}

/// Reset one channel, asserting the call succeeded, and return its response body.
async fn reset(emulator: &Emulator, case: &str, group: &str, channel: &str) -> Value {
    let (status, body) = emulator
        .call(
            "POST",
            &format!("/channelGroup/{group}/channel/{channel}/reset"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: resetting {group}/{channel} must answer 200, got {status}: {body}"
    );
    body
}

#[tokio::test]
async fn a_reset_records_the_instant_on_the_channel_it_addresses() {
    let emulator = Emulator::new();

    for case in reset_cases() {
        let label = case.label;
        let (status, _) = emulator.create_group(case.group, json!({})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: creating group {} must answer 200, got {status}",
            case.group
        );
        let (status, created) = emulator
            .create_channel(case.group, case.channel, (case.channel_body)())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: creating channel {} must answer 200, got {status}: {created}",
            case.channel
        );
        let created_at = timestamp(label, &created, "CreatedAt");

        // Nothing has reset the channel yet, so neither the create nor the read reports
        // a reset instant.
        assert!(
            absent(&created, "ResetAt"),
            "case {label:?}: a freshly created channel must report no ResetAt, got {created}"
        );
        let (status, before) = emulator.get_channel(case.group, case.channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: reading the new channel must answer 200, got {status}: {before}"
        );
        assert!(
            absent(&before, "ResetAt"),
            "case {label:?}: an unreset channel must report no ResetAt, got {before}"
        );

        let reset_body = reset(&emulator, label, case.group, case.channel).await;

        // The response names the channel that was reset, by the identity the create
        // persisted for it.
        assert_eq!(
            reset_body.get("ChannelGroupName").and_then(Value::as_str),
            Some(case.group),
            "case {label:?}: expected ChannelGroupName {:?}, got {reset_body}",
            case.group
        );
        assert_eq!(
            reset_body.get("ChannelName").and_then(Value::as_str),
            Some(case.channel),
            "case {label:?}: expected ChannelName {:?}, got {reset_body}",
            case.channel
        );
        assert_eq!(
            reset_body.get("Arn"),
            created.get("Arn"),
            "case {label:?}: expected the channel's stored ARN {:?}, got {reset_body}",
            created.get("Arn")
        );

        let reset_at = timestamp(label, &reset_body, "ResetAt");
        assert!(
            reset_at >= created_at,
            "case {label:?}: expected a ResetAt at or after CreatedAt {created_at}, got {reset_at}"
        );

        // The read reports the very value the reset stored, and every configured value
        // survives the reset untouched.
        let (status, after) = emulator.get_channel(case.group, case.channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: reading the reset channel must answer 200, got {status}: {after}"
        );
        assert_eq!(
            timestamp(label, &after, "ResetAt"),
            reset_at,
            "case {label:?}: expected the read to report ResetAt {reset_at}, got {after}"
        );
        for member in [
            "Arn",
            "ChannelName",
            "ChannelGroupName",
            "CreatedAt",
            "ModifiedAt",
            "Description",
            "InputType",
            "IngestEndpoints",
            "ETag",
            "Tags",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {label:?}: a reset must leave {member} alone: was {:?}, now {:?}",
                before.get(member),
                after.get(member)
            );
        }
    }
}

#[tokio::test]
async fn each_reset_replaces_the_instant_with_its_own() {
    let emulator = Emulator::new();
    emulator.create_group("g", json!({})).await;
    emulator.create_channel("g", "c", json!({})).await;

    let mut previous = 0.0_f64;
    // Three resets in a row: each one must report an instant later than the last, and
    // the read must follow the latest. The waits are longer than the millisecond
    // resolution the store keeps timestamps at, so "later" is observable.
    for round in 1..=3 {
        let case = format!("reset {round} of 3");
        let body = reset(&emulator, &case, "g", "c").await;
        let reset_at = timestamp(&case, &body, "ResetAt");
        assert!(
            reset_at > previous,
            "case {case:?}: expected an instant later than {previous}, got {reset_at}"
        );
        let (_, read) = emulator.get_channel("g", "c").await;
        assert_eq!(
            timestamp(&case, &read, "ResetAt"),
            reset_at,
            "case {case:?}: expected the read to report the latest reset {reset_at}, got {read}"
        );
        previous = reset_at;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_reset_touches_no_other_channel() {
    let emulator = Emulator::new();
    // Two groups, each with channels of the same two names, so the only thing telling
    // the reset target apart from its siblings and its namesake is the address.
    for group in ["left", "right"] {
        emulator.create_group(group, json!({})).await;
        for channel in ["alpha", "beta"] {
            emulator.create_channel(group, channel, json!({})).await;
        }
    }

    reset(&emulator, "left/alpha only", "left", "alpha").await;

    for (group, channel, expect_reset) in [
        ("left", "alpha", true),
        ("left", "beta", false),
        ("right", "alpha", false),
        ("right", "beta", false),
    ] {
        let case = format!("{group}/{channel}");
        let (status, body) = emulator.get_channel(group, channel).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the channel must answer 200, got {status}: {body}"
        );
        assert_eq!(
            !absent(&body, "ResetAt"),
            expect_reset,
            "case {case:?}: expected ResetAt present={expect_reset}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_reset_of_a_channel_that_does_not_resolve_is_refused() {
    let emulator = Emulator::new();
    emulator.create_group("present", json!({})).await;
    emulator
        .create_channel("present", "present-channel", json!({}))
        .await;
    // A channel under a DIFFERENT group, to pin that a reset resolves the whole chain
    // rather than the channel name alone.
    emulator.create_group("other", json!({})).await;
    emulator.create_channel("other", "elsewhere", json!({})).await;

    for (case, group, channel) in [
        ("no such group", "absent", "present-channel"),
        ("no such channel in an existing group", "present", "absent"),
        ("a channel that lives in another group", "present", "elsewhere"),
    ] {
        let (status, body) = emulator
            .call(
                "POST",
                &format!("/channelGroup/{group}/channel/{channel}/reset"),
                None,
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // Nothing was recorded by any refused reset.
    for (group, channel) in [("present", "present-channel"), ("other", "elsewhere")] {
        let (_, body) = emulator.get_channel(group, channel).await;
        assert!(
            absent(&body, "ResetAt"),
            "case {group:?}/{channel:?}: a refused reset must record nothing, got {body}"
        );
    }
}

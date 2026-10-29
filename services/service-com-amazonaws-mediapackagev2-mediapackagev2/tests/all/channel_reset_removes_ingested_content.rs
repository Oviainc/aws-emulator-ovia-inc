// SPDX-License-Identifier: CC0-1.0

//! What a channel reset does to the content the channel's ingest data plane already
//! accepted — asserted over a SET of channels, object sizes and object counts rather than
//! one recorded request.
//!
//! The rule under test: an operation the service defines as REMOVING a resource's content
//! discards that content from the store when it succeeds, so the content is retained no
//! longer and the retention budget it occupied is free for later writes. For a channel
//! reset (`ResetChannelState`, whose doc says "A reset refreshes the ingest stream and
//! removes previous content") that budget is the per-channel object count
//! (`state::MAX_INGESTED_OBJECTS_PER_CHANNEL`) and the store-wide retained bytes
//! (`state::MAX_INGESTED_BYTES`), which the store sums off the very maps the reset
//! clears — so both are observable through what the ingest data plane accepts next:
//!
//! 1. A channel pushed to its object ceiling answers every further push `507` until it is
//!    reset, and then takes a whole ceiling's worth of objects again.
//! 2. Only the addressed channel's content goes: a sibling filled the same way is still
//!    full after its neighbour is reset, and frees up only when itself reset.
//! 3. The store-wide retained bytes follow the removal across channels: with the store's
//!    byte budget spent between two channels, resetting one is what lets the OTHER take a
//!    further object.
//! 4. A reset that is REFUSED removes nothing, so a filled channel it did not address is
//!    still full.
//!
//! Content removal is not a configuration change, so each test also pins that the reset
//! leaves the channel's configuration, entity tag and `ModifiedAt` as they were — the axis
//! `channel_reset.rs` owns for the reset instant.
//!
//! The ceilings come from the service crate's own constants rather than being written out
//! here, and the client, the URL splitter and the raw-body push are the ones
//! `rejection_rule_and_negative_paths` already owns.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
    MAX_INGESTED_BYTES, MAX_INGESTED_OBJECTS_PER_CHANNEL, MAX_INGESTED_OBJECT_BYTES,
};

use crate::rejection_rule_and_negative_paths::{split_url, Answer, Emulator};

/// The channel group every channel in this suite hangs under. No part of the rule depends
/// on it.
const GROUP: &str = "reset-content";

/// How many objects at the per-object ceiling the store-wide byte budget holds.
fn store_wide_object_budget() -> usize {
    MAX_INGESTED_BYTES / MAX_INGESTED_OBJECT_BYTES
}

/// One row of the table: a channel to create and the size of the objects pushed to it,
/// with the index of the ingest endpoint within the channel's redundant pair that the
/// pushes address.
struct Row {
    label: &'static str,
    channel: &'static str,
    body: fn() -> Value,
    /// The body size of each pushed object, in bytes. The rule generalises over it: what
    /// the count ceiling counts is objects, whatever they weigh.
    object_bytes: usize,
    /// Which published ingest URL of the channel's redundant pair the pushes go to.
    endpoint_index: usize,
}

/// The rows span the `InputType` values whose channels ingest a stream of their own (CMAF,
/// HLS, and the absent case the member's doc defaults to HLS), the object body sizes from
/// empty upwards, and both indices of the published redundant pair.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "input type absent, empty objects, first ingest endpoint",
            channel: "plain",
            body: || json!({}),
            object_bytes: 0,
            endpoint_index: 0,
        },
        Row {
            label: "HLS input, single-byte objects, second ingest endpoint",
            channel: "hls-channel",
            body: || json!({"InputType": "HLS", "Description": "ingests HLS"}),
            object_bytes: 1,
            endpoint_index: 1,
        },
        Row {
            label: "CMAF input, kibibyte objects, tagged",
            channel: "cmaf-channel",
            body: || json!({"InputType": "CMAF", "tags": {"k": "v"}}),
            object_bytes: 1024,
            endpoint_index: 0,
        },
    ]
}

/// Create the group every row hangs under.
async fn create_group(emulator: &Emulator) {
    let group = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        group.status, 200,
        "setup: creating group {GROUP} must answer 200, got {} with body {}",
        group.status, group.body
    );
}

/// Create one channel and report the host and path of the `index`-th ingest URL it
/// publishes, so every push in this suite goes to a URL the service itself handed out.
async fn ingest_target(
    emulator: &Emulator,
    case: &str,
    channel: &str,
    body: Value,
    index: usize,
) -> (String, String) {
    let created = emulator.create_channel(GROUP, channel, body).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: creating channel {channel} must answer 200, got {} with body {}",
        created.status, created.body
    );
    let url = created
        .json
        .get("IngestEndpoints")
        .and_then(Value::as_array)
        .and_then(|list| list.get(index))
        .and_then(|endpoint| endpoint.get("Url"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: channel {channel} must publish ingest URL {index}, got {}",
                created.body
            )
        });
    split_url(url)
}

/// Push one object of `bytes` bytes to `object` under a published ingest path.
async fn push(
    emulator: &Emulator,
    host: &str,
    path: &str,
    object: &str,
    bytes: usize,
) -> Answer {
    emulator
        .send_raw(
            "PUT",
            &format!("{path}/{object}"),
            "x".repeat(bytes),
            host,
            true,
            &[],
        )
        .await
}

/// Push objects named `<prefix>-<n>` until the data plane refuses with `507`, reporting
/// how many it accepted. Every other answer fails the case.
async fn fill(
    emulator: &Emulator,
    case: &str,
    host: &str,
    path: &str,
    prefix: &str,
    bytes: usize,
    attempts: usize,
) -> usize {
    let mut accepted = 0;
    for attempt in 0..attempts {
        let answer = push(emulator, host, path, &format!("{prefix}-{attempt}"), bytes).await;
        match answer.status {
            200 => accepted += 1,
            507 => break,
            other => panic!(
                "case {case:?}: a push must answer 200 or 507, got {other} with body {}",
                answer.body
            ),
        }
    }
    accepted
}

/// Read one channel back.
async fn get_channel(emulator: &Emulator, case: &str, channel: &str) -> Value {
    let read = emulator
        .call(
            "GET",
            &format!("/channelGroup/{GROUP}/channel/{channel}/"),
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: reading channel {channel} must answer 200, got {} with body {}",
        read.status, read.body
    );
    read.json
}

/// Reset one channel, asserting it succeeded and reported its instant.
async fn reset(emulator: &Emulator, case: &str, channel: &str) -> Value {
    let answer = emulator
        .call(
            "POST",
            &format!("/channelGroup/{GROUP}/channel/{channel}/reset"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: resetting {channel} must answer 200, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.json.get("ResetAt").is_some(),
        "case {case:?}: a reset must report its instant, got {}",
        answer.body
    );
    answer.json
}

/// The configuration members a content removal must not touch.
fn assert_configuration_survived(case: &str, before: &Value, after: &Value) {
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
            "case {case:?}: removing ingested content must leave {member} alone: was {:?}, \
             now {:?}",
            before.get(member),
            after.get(member)
        );
    }
}

#[tokio::test]
async fn a_reset_frees_the_object_count_its_removed_content_occupied() {
    let emulator = Emulator::new();
    create_group(&emulator).await;

    for row in rows() {
        let case = row.label;
        let (host, path) = ingest_target(
            &emulator,
            case,
            row.channel,
            (row.body)(),
            row.endpoint_index,
        )
        .await;

        // Filled to the per-channel ceiling: the count the store publishes is exactly how
        // many objects the channel takes, and the next distinct object is refused.
        let accepted = fill(
            &emulator,
            case,
            &host,
            &path,
            "before",
            row.object_bytes,
            MAX_INGESTED_OBJECTS_PER_CHANNEL + 1,
        )
        .await;
        assert_eq!(
            accepted, MAX_INGESTED_OBJECTS_PER_CHANNEL,
            "case {case:?}: a channel must take exactly {MAX_INGESTED_OBJECTS_PER_CHANNEL} \
             objects before it is full, got {accepted}"
        );
        let full = push(&emulator, &host, &path, "overflow", row.object_bytes).await;
        assert_eq!(
            full.status, 507,
            "case {case:?}: a push to a filled channel must answer 507, got {} with body {}",
            full.status, full.body
        );

        let before = get_channel(&emulator, case, row.channel).await;
        reset(&emulator, case, row.channel).await;
        let after = get_channel(&emulator, case, row.channel).await;
        assert_configuration_survived(case, &before, &after);

        // The content the reset removed is retained no longer, so the whole ceiling it
        // occupied is free: a fresh fill takes another full ceiling's worth of objects.
        let accepted = fill(
            &emulator,
            case,
            &host,
            &path,
            "after",
            row.object_bytes,
            MAX_INGESTED_OBJECTS_PER_CHANNEL + 1,
        )
        .await;
        assert_eq!(
            accepted, MAX_INGESTED_OBJECTS_PER_CHANNEL,
            "case {case:?}: after a reset a channel must take {MAX_INGESTED_OBJECTS_PER_CHANNEL} \
             objects again, got {accepted}"
        );
    }
}

#[tokio::test]
async fn a_reset_removes_the_content_of_the_channel_it_addresses_only() {
    let emulator = Emulator::new();
    create_group(&emulator).await;

    // Two channels filled the same way, so the only thing telling the reset target apart
    // from its sibling is the address.
    let mut targets = Vec::new();
    for channel in ["left", "right"] {
        let (host, path) = ingest_target(&emulator, channel, channel, json!({}), 0).await;
        let accepted = fill(
            &emulator,
            channel,
            &host,
            &path,
            "fill",
            16,
            MAX_INGESTED_OBJECTS_PER_CHANNEL + 1,
        )
        .await;
        assert_eq!(
            accepted, MAX_INGESTED_OBJECTS_PER_CHANNEL,
            "case {channel:?}: the channel must fill to its ceiling, got {accepted}"
        );
        targets.push((channel, host, path));
    }

    reset(&emulator, "left reset only", "left").await;

    for (channel, host, path) in &targets {
        let case = format!("{channel} after resetting left");
        let answer = push(&emulator, host, path, "next", 16).await;
        let expected = if *channel == "left" { 200 } else { 507 };
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected} for a further push, got {} with body {}",
            answer.status, answer.body
        );
    }

    // And the sibling frees up when IT is reset, so nothing but the reset removed it.
    reset(&emulator, "right reset too", "right").await;
    let (_, host, path) = &targets[1];
    let answer = push(&emulator, host, path, "next", 16).await;
    assert_eq!(
        answer.status, 200,
        "case \"right after resetting right\": expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn a_reset_frees_the_store_wide_retained_bytes_its_content_occupied() {
    let emulator = Emulator::new();
    create_group(&emulator).await;

    // The store's whole byte budget, spent between two channels at the per-object ceiling:
    // half the budget's objects each, so neither channel is anywhere near its own object
    // COUNT ceiling and the only thing refusing a further push is the store-wide total.
    let budget = store_wide_object_budget();
    assert!(
        (2..MAX_INGESTED_OBJECTS_PER_CHANNEL).contains(&budget),
        "setup: the store-wide byte budget must span several objects and stay under the \
         per-channel object ceiling, got {budget}"
    );
    let mut targets = Vec::new();
    for channel in ["heavy-one", "heavy-two"] {
        let (host, path) = ingest_target(&emulator, channel, channel, json!({}), 0).await;
        for object in 0..budget / 2 {
            let answer = push(
                &emulator,
                &host,
                &path,
                &format!("segment-{object}"),
                MAX_INGESTED_OBJECT_BYTES,
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {channel:?}: object {object} must fit the store's byte budget, got {} \
                 with body {}",
                answer.status, answer.body
            );
        }
        targets.push((channel, host, path));
    }

    // The budget is spent, so a further object is refused on EITHER channel.
    for (channel, host, path) in &targets {
        let answer = push(&emulator, host, path, "overflow", MAX_INGESTED_OBJECT_BYTES).await;
        assert_eq!(
            answer.status, 507,
            "case {channel:?}: a push past the store's byte budget must answer 507, got {} \
             with body {}",
            answer.status, answer.body
        );
    }

    // Resetting ONE channel gives the bytes its content occupied back to the store, which
    // is what lets the OTHER channel take a further object.
    let before = get_channel(&emulator, "heavy-one", "heavy-one").await;
    reset(&emulator, "heavy-one reset", "heavy-one").await;
    assert_configuration_survived(
        "heavy-one reset",
        &before,
        &get_channel(&emulator, "heavy-one", "heavy-one").await,
    );
    let (_, host, path) = &targets[1];
    let answer = push(&emulator, host, path, "after", MAX_INGESTED_OBJECT_BYTES).await;
    assert_eq!(
        answer.status, 200,
        "case \"heavy-two after resetting heavy-one\": the freed bytes must be spendable by \
         another channel, got {} with body {}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn a_refused_reset_removes_no_content() {
    let emulator = Emulator::new();
    create_group(&emulator).await;
    let (host, path) = ingest_target(&emulator, "filled", "filled", json!({}), 0).await;
    let accepted = fill(
        &emulator,
        "filled",
        &host,
        &path,
        "fill",
        32,
        MAX_INGESTED_OBJECTS_PER_CHANNEL + 1,
    )
    .await;
    assert_eq!(
        accepted, MAX_INGESTED_OBJECTS_PER_CHANNEL,
        "setup: the channel must fill to its ceiling, got {accepted}"
    );

    // Resets that resolve nothing: a missing channel in the group, and a channel of that
    // name under a group that does not exist.
    for (case, group, channel) in [
        ("no such channel in an existing group", GROUP, "absent"),
        ("no such group", "absent-group", "filled"),
    ] {
        let answer = emulator
            .call(
                "POST",
                &format!("/channelGroup/{group}/channel/{channel}/reset"),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            answer.status, answer.body
        );
        let push_answer = push(&emulator, &host, &path, &format!("after-{channel}"), 32).await;
        assert_eq!(
            push_answer.status, 507,
            "case {case:?}: a refused reset must remove nothing, so the filled channel stays \
             full: expected HTTP 507, got {} with body {}",
            push_answer.status, push_answer.body
        );
    }

    // The channel that WAS addressed successfully is the only one whose content goes.
    reset(&emulator, "filled reset", "filled").await;
    let answer = push(&emulator, &host, &path, "after-reset", 32).await;
    assert_eq!(
        answer.status, 200,
        "case \"filled after its own reset\": expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
}

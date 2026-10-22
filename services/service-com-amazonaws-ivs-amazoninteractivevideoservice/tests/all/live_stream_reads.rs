// SPDX-License-Identifier: CC0-1.0

//! `GetStream` end to end: what the emulator reports for the active (live) stream
//! on a channel.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws ivs get-stream` call arrives
//! — so what it asserts is the emulator's HTTP behaviour, not a handler called
//! directly.
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//! `GetStream` REPORTS the stream the store holds for a channel. It answers the
//! persisted stream while that stream is in the `LIVE` state, the operation's own
//! `ChannelNotBroadcasting` (404) for a channel that exists and holds no live
//! stream, and `ResourceNotFoundException` (404) for a channel ARN the store does
//! not hold at all; a request whose ARN is outside the modelled `ChannelArn`
//! constraint is refused with `ValidationException` (400) before any of that. Being
//! a read, repeating it leaves every one of those answers unchanged.
//!
//! Nothing in the IVS control plane starts a stream — an ingest session does, and
//! the RTMPS ingest listener that accepts one is filed separately (cause 9 of the
//! `ivs` behavioural bundle). So these cases bring a channel live through
//! `state::start_stream`, the one place a stream is minted and the same entry point
//! that listener will use, reaching the store through the shared `Ivs` harness's
//! engine handle.

use std::collections::BTreeSet;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state;

// The one harness for this crate's suites, declared in `channel_lifecycle.rs` and
// shared by every suite here rather than copied: it owns the router, the request
// envelope, the reply shape and the store access the seeding below writes through.
use super::channel_lifecycle::{text, Ivs, Reply, CHANNEL_ARN_PREFIX};

/// The modelled upper bound on `ChannelArn` (`@length 1..128`).
const CHANNEL_ARN_MAX: usize = 128;

/// Call `GetStream` for `channel_arn`.
async fn get_stream(ivs: &Ivs, channel_arn: &str) -> Reply {
    ivs.call("GetStream", &json!({ "channelArn": channel_arn }))
        .await
}

/// Delete a channel, requiring the modelled 204.
async fn delete_channel(ivs: &Ivs, channel_arn: &str) {
    let reply = ivs.call("DeleteChannel", &json!({ "arn": channel_arn })).await;
    assert_eq!(
        reply.status, 204,
        "DeleteChannel {channel_arn}: expected HTTP 204, got {} {}",
        reply.status, reply.body
    );
}

/// Bring a channel live the way an ingest session does, returning the id of the
/// stream the store holds for it afterwards.
fn start_ingest(ivs: &Ivs, channel_arn: &str) -> String {
    ivs.with_store_mut(|store| {
        state::start_stream(store, channel_arn)
            .stream_id
            .expect("a minted stream carries its id")
    })
}

/// End a broadcast the way a stopped ingest does: the entry stays in the store and
/// leaves the `LIVE` state (`OFFLINE` is the other value the modelled `StreamState`
/// enum carries).
fn end_ingest(ivs: &Ivs, channel_arn: &str) {
    ivs.with_store_mut(|store| {
        let stream = store
            .streams
            .get_mut(channel_arn)
            .expect("the channel is broadcasting");
        stream.state = Some("OFFLINE".to_string());
    });
}

/// Whether `id` satisfies the modelled `StreamId`: exactly 26 characters matching
/// `^st-[a-zA-Z0-9]+$`.
fn stream_id_is_well_formed(id: &str) -> bool {
    id.len() == 26
        && id
            .strip_prefix("st-")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Whether any string the error body carries names `arn`.
///
/// Which wire key the modelled `exceptionMessage` member renders under is a
/// separately-filed defect of this service's error serializer, so this looks at the
/// whole error object rather than pinning one key.
fn error_mentions(body: &Value, arn: &str) -> bool {
    body.as_object().is_some_and(|members| {
        members
            .values()
            .any(|value| value.as_str().is_some_and(|text| text.contains(arn)))
    })
}

/// `startTime` as seconds since the epoch.
///
/// The modelled `StreamStartTime` carries `@timestampFormat("date-time")`, which a
/// client reads as an RFC3339 string; this emulator's restJson1 serializer renders
/// the protocol's default epoch-seconds number for it. Both are the same instant, and
/// which one is on the wire is not what this suite is about, so either is accepted —
/// what is asserted is that the value IS the instant the ingest started.
fn start_time_secs(case: &str, stream: &Value) -> f64 {
    match stream.get("startTime") {
        Some(Value::String(rendered)) => DateTime::from_str(rendered, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: startTime {rendered:?} must be an RFC3339 instant, got {error}"
                )
            })
            .as_secs_f64(),
        Some(Value::Number(epoch)) => epoch.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: startTime {epoch} must be a finite instant")
        }),
        other => panic!("case {case:?}: a live stream must carry startTime, got {other:?}"),
    }
}

/// Now, in the same units as [`start_time_secs`].
fn now_secs() -> f64 {
    DateTime::from(std::time::SystemTime::now()).as_secs_f64()
}

/// What the store holds for the channel a case reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Broadcast {
    /// The channel exists and nothing has ever ingested on it.
    Never,
    /// An ingest session is live on the channel.
    Live,
    /// An ingest session ran and ended, so the entry has left `LIVE`.
    Ended,
}

/// Assert the whole live-stream representation `GetStream` answered for a channel.
fn assert_live_stream(case: &str, reply: &Reply, channel: &Value, expected_id: &str) {
    let arn = text(channel, "arn");
    assert_eq!(
        reply.status, 200,
        "case {case:?}: a broadcasting channel must answer HTTP 200, got {} {}",
        reply.status, reply.body
    );
    let stream = reply
        .body
        .get("stream")
        .unwrap_or_else(|| panic!("case {case:?}: expected a stream member, got {}", reply.body));
    assert_eq!(
        text(stream, "channelArn"),
        arn,
        "case {case:?}: the stream must name the channel it is ingested on, got {stream}"
    );
    assert_eq!(
        text(stream, "streamId"),
        expected_id,
        "case {case:?}: the stream the store holds must be the one reported, got {stream}"
    );
    assert!(
        stream_id_is_well_formed(expected_id),
        "case {case:?}: streamId must be 26 characters matching ^st-[a-zA-Z0-9]+$, got {expected_id:?}"
    );
    assert_eq!(
        text(stream, "playbackUrl"),
        text(channel, "playbackUrl"),
        "case {case:?}: the stream is played back from its channel's own URL, got {stream}"
    );
    assert_eq!(
        text(stream, "state"),
        "LIVE",
        "case {case:?}: a reported stream is in the LIVE state, got {stream}"
    );
    assert_eq!(
        text(stream, "health"),
        "HEALTHY",
        "case {case:?}: a connected ingest reports HEALTHY, got {stream}"
    );
    assert_eq!(
        stream.get("viewerCount").and_then(Value::as_i64),
        Some(0),
        "case {case:?}: no playback session is attached, so viewerCount is 0, got {stream}"
    );
    let started = start_time_secs(case, stream);
    let elapsed = now_secs() - started;
    assert!(
        (-5.0..300.0).contains(&elapsed),
        "case {case:?}: startTime must be the instant the ingest connected, {elapsed} seconds off, got {stream}"
    );
}

/// Assert `GetStream` refused a channel that exists but is not broadcasting.
fn assert_not_broadcasting(case: &str, reply: &Reply, arn: &str) {
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (404, "ChannelNotBroadcasting"),
        "case {case:?}: expected 404 ChannelNotBroadcasting, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert!(
        error_mentions(&reply.body, arn),
        "case {case:?}: the refusal must name the channel asked about, got {}",
        reply.body
    );
    assert!(
        reply.body.get("stream").is_none(),
        "case {case:?}: a refusal carries no stream, got {}",
        reply.body
    );
}

/// The rule over a table spanning the channel surface and all three things the
/// store can hold for a channel: a live stream is reported member for member, and a
/// channel with no live stream is `ChannelNotBroadcasting` whether it never
/// broadcast or has stopped.
#[tokio::test]
async fn a_channel_answers_only_the_live_stream_the_store_holds_for_it() {
    // Rows differ along what the rule generalises over: the shape of the channel
    // (which members the create sent, its type, its latency mode, its tag count)
    // and which of the three broadcast conditions the store is in for it.
    let cases: Vec<(&str, Value, Broadcast)> = vec![
        ("every member defaulted, never ingested", json!({}), Broadcast::Never),
        (
            "named standard channel, live",
            json!({ "name": "live-standard" }),
            Broadcast::Live,
        ),
        (
            "basic channel, normal latency, live",
            json!({ "name": "live_basic", "type": "BASIC", "latencyMode": "NORMAL" }),
            Broadcast::Live,
        ),
        (
            "advanced-hd channel with several tags, live",
            json!({ "name": "live-advanced-hd", "type": "ADVANCED_HD",
                    "tags": { "env": "test", "team": "video" } }),
            Broadcast::Live,
        ),
        (
            "advanced-sd fragmented-mp4 channel whose ingest ended",
            json!({ "name": "ended-advanced-sd", "type": "ADVANCED_SD",
                    "containerFormat": "FRAGMENTED_MP4" }),
            Broadcast::Ended,
        ),
        (
            "authorized channel, never ingested",
            json!({ "name": "private", "authorized": true }),
            Broadcast::Never,
        ),
    ];

    let ivs = Ivs::new();
    // Every stream the store mints is a distinct resource, so no two channels may
    // ever be reported under one stream id.
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, request, broadcast) in cases {
        let created = ivs.create(request).await;
        let channel = created
            .get("channel")
            .unwrap_or_else(|| panic!("case {case:?}: CreateChannel returns a channel"))
            .clone();
        let arn = text(&channel, "arn");
        assert!(
            arn.starts_with(CHANNEL_ARN_PREFIX),
            "case {case:?}: a minted channel ARN carries the modelled prefix, got {arn:?}"
        );

        let started = match broadcast {
            Broadcast::Never => None,
            Broadcast::Live => Some(start_ingest(&ivs, &arn)),
            Broadcast::Ended => {
                let id = start_ingest(&ivs, &arn);
                end_ingest(&ivs, &arn);
                Some(id)
            }
        };
        if let Some(id) = &started {
            assert!(
                minted.insert(id.clone()),
                "case {case:?}: stream id {id:?} was already minted for another channel"
            );
        }

        let reply = get_stream(&ivs, &arn).await;
        match broadcast {
            Broadcast::Live => assert_live_stream(
                case,
                &reply,
                &channel,
                started.as_deref().expect("a live case started an ingest"),
            ),
            Broadcast::Never | Broadcast::Ended => assert_not_broadcasting(case, &reply, &arn),
        }
    }
}

/// A channel ARN the store does not hold is `ResourceNotFoundException`, not
/// `ChannelNotBroadcasting`: the channel is resolved before its stream is, so a
/// well-formed ARN that was never minted, and one whose channel was deleted, answer
/// the same way.
#[tokio::test]
async fn a_channel_arn_the_store_does_not_hold_is_not_found() {
    let ivs = Ivs::new();
    // A channel that IS broadcasting, so no case below can be answered out of some
    // other channel's stream by accident.
    let bystander = text(
        ivs.create(json!({ "name": "bystander" }))
            .await
            .get("channel")
            .expect("CreateChannel returns a channel"),
        "arn",
    );
    start_ingest(&ivs, &bystander);

    // A deleted channel, and a deleted channel that was broadcasting when it went:
    // both are gone, stream and all.
    let deleted = text(
        ivs.create(json!({ "name": "deleted" }))
            .await
            .get("channel")
            .expect("CreateChannel returns a channel"),
        "arn",
    );
    delete_channel(&ivs, &deleted).await;
    let deleted_live = text(
        ivs.create(json!({ "name": "deleted-live" }))
            .await
            .get("channel")
            .expect("CreateChannel returns a channel"),
        "arn",
    );
    start_ingest(&ivs, &deleted_live);
    delete_channel(&ivs, &deleted_live).await;

    // Rows differ along what the rule generalises over: how the ARN came to name no
    // channel (never minted, deleted, deleted while live) and its form within the
    // modelled pattern — region, account, id length and id alphabet, including an
    // ARN exactly at the modelled 128-character bound.
    let at_the_bound = format!(
        "{CHANNEL_ARN_PREFIX}{}",
        "a".repeat(CHANNEL_ARN_MAX - CHANNEL_ARN_PREFIX.len())
    );
    let cases: Vec<(&str, String)> = vec![
        ("never minted, all-zero id", format!("{CHANNEL_ARN_PREFIX}000000000000")),
        (
            "never minted, another region",
            "arn:aws:ivs:eu-west-1:000000000000:channel/abcdef123456".to_string(),
        ),
        (
            "never minted, another account",
            "arn:aws:ivs:us-east-1:123456789012:channel/abcdef123456".to_string(),
        ),
        (
            "never minted, single-character id",
            format!("{CHANNEL_ARN_PREFIX}a"),
        ),
        ("never minted, at the 128-character bound", at_the_bound),
        (
            "never minted, dashed mixed-case id",
            format!("{CHANNEL_ARN_PREFIX}Live-Channel-01"),
        ),
        ("deleted after creation", deleted),
        ("deleted while broadcasting", deleted_live),
    ];

    for (case, arn) in cases {
        let reply = get_stream(&ivs, &arn).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException for {arn}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert!(
            error_mentions(&reply.body, &arn),
            "case {case:?}: the refusal must name the ARN asked for, got {}",
            reply.body
        );
    }

    // And the channel that is live is untouched by every refusal above.
    let reply = get_stream(&ivs, &bystander).await;
    assert_eq!(
        reply.status, 200,
        "case bystander: the broadcasting channel still reports its stream, got {} {}",
        reply.status, reply.body
    );
}

/// An ARN outside the modelled `ChannelArn` constraint (`@length 1..128`, pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:channel/[a-zA-Z0-9-]+$`) is a bad REQUEST, so it
/// is refused with 400 `ValidationException` rather than reported as a missing
/// resource.
#[tokio::test]
async fn a_channel_arn_outside_the_modelled_constraint_is_refused() {
    let ivs = Ivs::new();
    // Rows: each bound of the modelled length one step past it, and each part of the
    // pattern violated in turn — resource type, alphabet, and the ARN form itself.
    let past_the_bound = format!(
        "{CHANNEL_ARN_PREFIX}{}",
        "a".repeat(CHANNEL_ARN_MAX + 1 - CHANNEL_ARN_PREFIX.len())
    );
    let cases: Vec<(&str, String)> = vec![
        ("empty, below the 1-character minimum", String::new()),
        ("one past the 128-character maximum", past_the_bound),
        (
            "a stream-key ARN, not a channel ARN",
            "arn:aws:ivs:us-east-1:000000000000:stream-key/abcdef123456".to_string(),
        ),
        (
            "an id carrying a character outside [a-zA-Z0-9-]",
            format!("{CHANNEL_ARN_PREFIX}chan_01"),
        ),
        ("not an ARN at all", "live-standard".to_string()),
        (
            "an account segment that is not digits",
            "arn:aws:ivs:us-east-1:not-an-account:channel/abcdef123456".to_string(),
        ),
    ];

    for (case, arn) in cases {
        let reply = get_stream(&ivs, &arn).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException for {arn:?}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// `GetStream` is a REPORT: polling it answers the same stream every time and moves
/// nothing — not the stream id, not the start time, not the viewer count — and a
/// channel that is not broadcasting never becomes live by being read. Each channel
/// reports only its own stream.
#[tokio::test]
async fn repeated_reads_report_the_same_stream_and_change_nothing() {
    let ivs = Ivs::new();
    // Rows differ in whether the channel is broadcasting and in how many channels
    // sit beside it, which is what the "only its own stream" half of the rule
    // generalises over.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("first live channel", json!({ "name": "live-a" }), true),
        ("quiet channel between two live ones", json!({ "name": "quiet-b" }), false),
        (
            "second live channel",
            json!({ "name": "live-c", "type": "BASIC" }),
            true,
        ),
        ("quiet advanced channel", json!({ "name": "quiet-d", "type": "ADVANCED_SD" }), false),
    ];

    let mut seeded: Vec<(&str, Value, Option<String>)> = Vec::new();
    for (case, request, live) in cases {
        let created = ivs.create(request).await;
        let channel = created
            .get("channel")
            .unwrap_or_else(|| panic!("case {case:?}: CreateChannel returns a channel"))
            .clone();
        let arn = text(&channel, "arn");
        let id = live.then(|| start_ingest(&ivs, &arn));
        seeded.push((case, channel, id));
    }

    // Four rounds of polling, the way a client waits for a channel to go live.
    let mut first: Vec<Option<Value>> = Vec::new();
    for round in 0..4 {
        for (index, (case, channel, id)) in seeded.iter().enumerate() {
            let arn = text(channel, "arn");
            let reply = get_stream(&ivs, &arn).await;
            let label = format!("{case} round {round}");
            match id {
                Some(id) => {
                    assert_live_stream(&label, &reply, channel, id);
                    let stream = reply.body.get("stream").cloned();
                    if round == 0 {
                        first.push(stream);
                    } else {
                        assert_eq!(
                            first.get(index).cloned().flatten(),
                            stream,
                            "case {label:?}: a read must report the same stream it reported before, got {}",
                            reply.body
                        );
                    }
                }
                None => {
                    assert_not_broadcasting(&label, &reply, &arn);
                    if round == 0 {
                        first.push(None);
                    }
                }
            }
        }
    }
}

/// A channel holds at most one stream: a second ingest on a channel that is already
/// live keeps the stream it has rather than minting a second id, so what `GetStream`
/// reports does not change under it.
#[tokio::test]
async fn a_second_ingest_on_a_live_channel_keeps_the_stream_it_has() {
    let ivs = Ivs::new();
    let created = ivs
        .create(json!({ "name": "re-ingested", "type": "STANDARD" }))
        .await;
    let channel = created
        .get("channel")
        .expect("CreateChannel returns a channel")
        .clone();
    let arn = text(&channel, "arn");

    let first = start_ingest(&ivs, &arn);
    let before = get_stream(&ivs, &arn).await;
    assert_live_stream("first ingest", &before, &channel, &first);

    // Rows: two further ingest attempts on the same live channel.
    for attempt in 1..=2 {
        let again = start_ingest(&ivs, &arn);
        assert_eq!(
            again, first,
            "case ingest attempt {attempt}: a live channel keeps its stream id, got {again:?}"
        );
        let after = get_stream(&ivs, &arn).await;
        assert_live_stream(&format!("ingest attempt {attempt}"), &after, &channel, &first);
        assert_eq!(
            after.body.get("stream"),
            before.body.get("stream"),
            "case ingest attempt {attempt}: the reported stream must be unchanged, got {}",
            after.body
        );
    }
}

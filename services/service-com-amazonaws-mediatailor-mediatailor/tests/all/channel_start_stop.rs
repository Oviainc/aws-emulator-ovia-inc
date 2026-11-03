// SPDX-License-Identifier: CC0-1.0

//! Starting and stopping a channel, end to end over HTTP.
//!
//! The harness is the one `channel_lifecycle.rs` owns, so every assertion here
//! goes through the router: a request is built, dispatched to the mounted
//! service, and the JSON response read back.
//!
//! The rule under test is the one both calls implement: each names the state
//! the ONE channel its `ChannelName` addresses is to be IN — the playing state
//! for a start, the not-playing state for a stop — so the state every later
//! read of that channel reports is the one the most recent of those calls
//! named, whatever order they came in and however often each was repeated;
//! being idempotent, a call naming the state the channel is already in leaves
//! the stored record byte for byte as it was (its last-modified instant
//! included, which says when the channel itself last changed); no other member
//! of that channel and no other channel is touched; and a name the account
//! holds no channel under addresses nothing to start or stop, so it is refused
//! and stores nothing.
//!
//! The tables span the class the rule generalises over: every channel the
//! lifecycle table spans (both playback modes, every tier, one and several
//! outputs in each playlist form, audiences present and absent, time-shift at
//! both published bounds, tag counts from none to several, several forms of
//! name), crossed with sequences that differ in which call comes first, how the
//! two alternate and how many times each is repeated.

use serde_json::{json, Value};

use crate::channel_lifecycle::{encode, table, Emulator, Reply};

/// The two calls under test. `ChannelState`'s modelled values are `RUNNING |
/// STOPPED` (`shared_types::CHANNEL_STATE_VALUES`), and the operations' own
/// routes are `PUT /channel/{ChannelName}/start` and `.../stop`
/// (`smithy_query(query="operation",operation="StartChannel,StopChannel")`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    Start,
    Stop,
}

impl Call {
    /// The last segment of this call's route.
    fn segment(self) -> &'static str {
        match self {
            Call::Start => "start",
            Call::Stop => "stop",
        }
    }

    /// The `ChannelState` a channel this call succeeded on is in.
    fn state(self) -> &'static str {
        match self {
            Call::Start => "RUNNING",
            Call::Stop => "STOPPED",
        }
    }
}

/// `StartChannel` / `StopChannel`: `ChannelName` is the `@httpLabel` and
/// neither operation carries a request body.
async fn call(emulator: &Emulator, name: &str, which: Call) -> Reply {
    emulator
        .request(
            "PUT",
            &format!("/channel/{}/{}", encode(name), which.segment()),
            None,
        )
        .await
}

/// The members a start or a stop is allowed to move: the state it names, and
/// the instant that records when the channel last changed. Every other member
/// of the stored channel is left as it stood.
const MUTABLE_MEMBERS: &[&str] = &["ChannelState", "LastModifiedTime"];

/// Creates one channel from a lifecycle-table row and reports its read.
async fn create(emulator: &Emulator, case: &str, name: &str, request: &Value) -> Reply {
    let created = emulator.create(name, request).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the channel under test to be created, got {} {:?}",
        created.status, created.body
    );
    let described = emulator.describe(name).await;
    assert_eq!(
        described.string("/ChannelState"),
        Call::Stop.state(),
        "case {case:?}: expected a created channel to be in the not-playing state, got {:?}",
        described.get("/ChannelState")
    );
    described
}

/// The call sequences the table crosses every channel with: which call comes
/// first, how the two alternate, and how often each is repeated.
fn sequences() -> Vec<(&'static str, Vec<Call>)> {
    use Call::{Start, Stop};
    vec![
        ("one start", vec![Start]),
        ("a stop on a channel that was never started", vec![Stop]),
        ("start then stop", vec![Start, Stop]),
        (
            "three starts, then two stops",
            vec![Start, Start, Start, Stop, Stop],
        ),
        (
            "the two alternating, beginning with a stop",
            vec![Stop, Start, Stop, Start],
        ),
    ]
}

#[tokio::test]
async fn the_state_a_channel_reports_is_the_one_the_last_call_named() {
    for (channel_case, name, request) in table() {
        for (sequence_case, sequence) in sequences() {
            let case = format!("{channel_case} / {sequence_case}");
            let emulator = Emulator::new();
            let created = create(&emulator, &case, name, &request).await;
            let creation = created.get("/CreationTime").as_f64().expect("a creation instant");

            let mut previous = created.body.clone();
            let mut current_state = Call::Stop.state().to_string();

            for (index, which) in sequence.iter().copied().enumerate() {
                let idempotent = which.state() == current_state;
                let reply = call(&emulator, name, which).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected call {index} ({which:?}) to answer 200, got {} {:?}",
                    reply.status, reply.body
                );
                assert!(
                    reply.body == Value::Null || reply.body == json!({}),
                    "case {case:?}: expected call {index} ({which:?}) to answer an empty \
                     response, got {:?}",
                    reply.body
                );

                let described = emulator.describe(name).await;
                assert_eq!(
                    described.status, 200,
                    "case {case:?}: expected the channel to still be readable after call \
                     {index} ({which:?}), got {} {:?}",
                    described.status, described.body
                );
                assert_eq!(
                    described.string("/ChannelState"),
                    which.state(),
                    "case {case:?}: expected call {index} ({which:?}) to leave the channel in \
                     {:?}, got {:?}",
                    which.state(),
                    described.get("/ChannelState")
                );

                // Only the state and the last-modified instant may move.
                for (member, value) in previous.as_object().expect("a channel read object") {
                    if MUTABLE_MEMBERS.contains(&member.as_str()) {
                        continue;
                    }
                    assert_eq!(
                        described.body.get(member),
                        Some(value),
                        "case {case:?}: expected call {index} ({which:?}) to leave {member} as \
                         it was ({value:?}), got {:?}",
                        described.body.get(member)
                    );
                }

                let modified = described
                    .get("/LastModifiedTime")
                    .as_f64()
                    .expect("a last-modified instant");
                let previously_modified = previous
                    .pointer("/LastModifiedTime")
                    .and_then(Value::as_f64)
                    .expect("the previous read's last-modified instant");
                if idempotent {
                    // The call named the state the channel was already in, so
                    // the channel did not change — nothing about the stored
                    // record did either.
                    assert_eq!(
                        described.body, previous,
                        "case {case:?}: expected call {index} ({which:?}) on a channel already \
                         in {:?} to change nothing, got {:?}",
                        which.state(),
                        described.body
                    );
                } else {
                    // The channel DID change, so the instant that records when
                    // it last changed is this call's, never an earlier one: it
                    // is at or after both the creation instant and the one the
                    // previous read reported.
                    assert!(
                        modified >= creation && modified >= previously_modified,
                        "case {case:?}: expected call {index} ({which:?}) to record a \
                         last-modified instant at or after the creation instant {creation:?} \
                         and the previous one {previously_modified:?}, got {modified:?}"
                    );
                }

                previous = described.body.clone();
                current_state = which.state().to_string();
            }
        }
    }
}

#[tokio::test]
async fn a_call_moves_only_the_channel_it_names() {
    let emulator = Emulator::new();

    // Every channel the lifecycle table spans, all stored side by side.
    for (case, name, request) in table() {
        create(&emulator, case, name, &request).await;
    }

    // Each row is addressed in turn; every other channel's read has to stand
    // exactly as it did before that call.
    for (case, name, _) in table() {
        for which in [Call::Start, Call::Stop] {
            let before: Vec<(String, Value)> = {
                let mut snapshots = Vec::new();
                for (_, other, _) in table() {
                    snapshots.push((
                        other.to_string(),
                        emulator.describe(other).await.body.clone(),
                    ));
                }
                snapshots
            };

            let reply = call(&emulator, name, which).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected {which:?} to answer 200, got {} {:?}",
                reply.status, reply.body
            );

            for (other, snapshot) in before {
                let described = emulator.describe(&other).await;
                if other == name {
                    assert_eq!(
                        described.string("/ChannelState"),
                        which.state(),
                        "case {case:?}: expected {which:?} to leave {other:?} in {:?}, got {:?}",
                        which.state(),
                        described.get("/ChannelState")
                    );
                } else {
                    assert_eq!(
                        described.body, snapshot,
                        "case {case:?}: expected {which:?} on {name:?} to leave {other:?} \
                         untouched, got {:?}",
                        described.body
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_name_the_account_holds_no_channel_under_is_refused() {
    let emulator = Emulator::new();

    // Stored, so the refusals below are about the name and not about an empty
    // store.
    let (_, stored, request) = table().remove(0);
    create(&emulator, "the stored channel", stored, &request).await;

    for name in ["absent", "stored", "loop-MINIMAL", "loop-minimal-2", "a.b_c"] {
        if name == stored {
            continue;
        }
        for which in [Call::Start, Call::Stop] {
            let refused = call(&emulator, name, which).await;
            assert_eq!(
                refused.status, 400,
                "case {name:?}/{which:?}: expected a channel the account does not hold to be \
                 refused with 400, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code, "BadRequestException",
                "case {name:?}/{which:?}: expected BadRequestException, got {:?}",
                refused.error_code
            );
            assert!(
                refused.message().contains(name),
                "case {name:?}/{which:?}: expected the refusal to name the channel, got {:?}",
                refused.message()
            );

            // The refused call stored nothing: the name is still free.
            let missing = emulator.describe(name).await;
            assert_eq!(
                missing.status, 400,
                "case {name:?}/{which:?}: expected the refused call to store no channel, got \
                 {} {:?}",
                missing.status, missing.body
            );
        }
    }
}

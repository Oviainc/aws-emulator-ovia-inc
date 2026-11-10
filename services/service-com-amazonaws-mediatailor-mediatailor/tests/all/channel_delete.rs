// SPDX-License-Identifier: CC0-1.0

//! Deleting a channel, end to end over HTTP.
//!
//! The harness is the one `channel_lifecycle.rs` owns, so every assertion here
//! goes through the router: a request is built, dispatched to the mounted
//! service, and the JSON response read back.
//!
//! The rule under test is the one the call implements: `DeleteChannel` names
//! the ONE channel its `ChannelName` addresses and leaves the account holding
//! no channel under that name, so the resource and everything keyed to it —
//! its tags, which live under its ARN — are gone, and the call is `@idempotent`
//! (a name the account holds no channel under is nothing left to delete, so the
//! call succeeds and stores nothing, however often it is repeated). From then
//! on every operation that addresses that channel by name, and every tagging
//! operation that addresses it by ARN, is refused as NOT FOUND — HTTP 404 under
//! the error code `NotFoundException`, which is the refusal this service answers
//! a name or an ARN addressing no resource with, as opposed to the one client
//! error its model declares (`BadRequestException`, HTTP 400, "A request
//! contains unexpected data") — and the name is free for a new channel that
//! carries none of the deleted one's state.
//!
//! The tables span the class the rule generalises over: every channel the
//! lifecycle table spans (both playback modes, every tier, one and several
//! outputs in each playlist form, audiences present and absent, time-shift at
//! both published bounds, tag counts from none to several, several forms of
//! name), crossed with how often the delete is repeated and with the state the
//! channel is in when it is deleted, and every dependent operation that
//! addresses a channel by name or by ARN.

use serde_json::{json, Value};

use crate::channel_lifecycle::{
    encode, expected_arn, table, Emulator, Reply, CHANNEL_NOT_FOUND_MESSAGE, NOT_FOUND_CODE,
    NOT_FOUND_STATUS, RESOURCE_NOT_FOUND_MESSAGE,
};
use crate::channel_response_projection::configure_logs;

/// `DeleteChannel`'s success status: `DELETE /channel/{ChannelName}` with
/// `@http` code 200 and an empty response shape
/// (`smithy_query(query="operation",operation="DeleteChannel")`).
const DELETE_STATUS: u16 = 200;

/// `LogType`'s modelled values, so the configure-logs call below names types the
/// model declares (`ops::types::LOG_TYPE_VALUES`).
const LOG_TYPES: &[&str] =
    service_com_amazonaws_mediatailor_mediatailor::ops::types::LOG_TYPE_VALUES;

/// Every operation that addresses a channel by its NAME, as a call the suite can
/// make against a name the account holds no channel under. Each is a route this
/// service serves for a stored channel, so each has to answer the same miss.
async fn channel_name_calls(emulator: &Emulator, name: &str) -> Vec<(&'static str, Reply)> {
    vec![
        ("DescribeChannel", emulator.describe(name).await),
        (
            "StartChannel",
            emulator
                .request("PUT", &format!("/channel/{}/start", encode(name)), None)
                .await,
        ),
        (
            "StopChannel",
            emulator
                .request("PUT", &format!("/channel/{}/stop", encode(name)), None)
                .await,
        ),
        (
            "ConfigureLogsForChannel",
            configure_logs(emulator, name, &json!(LOG_TYPES)).await,
        ),
    ]
}

/// Every tagging operation, as a call against an ARN the account holds no
/// resource under. They address the resource by ARN rather than by name, so
/// their miss is the tagging routes' own sentence.
async fn tagging_calls(emulator: &Emulator, arn: &str) -> Vec<(&'static str, Reply)> {
    vec![
        ("ListTagsForResource", emulator.list_tags(arn).await),
        (
            "TagResource",
            emulator.tag(arn, json!({ "Env": "dev" })).await,
        ),
        ("UntagResource", emulator.untag(arn, &["Env"]).await),
    ]
}

/// Asserts that `reply` is this service's not-found refusal carrying `message`.
fn assert_not_found(case: &str, call: &str, reply: &Reply, message: &str) {
    assert_eq!(
        reply.status, NOT_FOUND_STATUS,
        "case {case:?}: expected {call} to be refused as not found, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code, NOT_FOUND_CODE,
        "case {case:?}: expected {call} to answer {NOT_FOUND_CODE}, got {:?}",
        reply.error_code
    );
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected {call} to answer {message:?}, got {:?}",
        reply.message()
    );
}

/// How often the delete is repeated, and the state the channel is left in before
/// it runs: a delete answers the same way whichever of these it meets.
fn delete_sequences() -> Vec<(&'static str, usize, Option<&'static str>)> {
    vec![
        ("one delete of a stopped channel", 1, None),
        ("one delete of a running channel", 1, Some("start")),
        ("two deletes of a stopped channel", 2, None),
        ("three deletes of a started-then-stopped channel", 3, Some("stop")),
    ]
}

#[tokio::test]
async fn delete_removes_the_channel_and_is_idempotent() {
    for (channel_case, name, request) in table() {
        for (sequence_case, repeats, before) in delete_sequences() {
            let case = format!("{channel_case} / {sequence_case}");
            let emulator = Emulator::new();

            let created = emulator.create(name, &request).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the channel under test to be created, got {} {:?}",
                created.status, created.body
            );
            let arn = created.string("/Arn");
            assert_eq!(
                arn,
                expected_arn(name),
                "case {case:?}: expected the channel's own ARN, got {arn:?}"
            );

            if let Some(segment) = before {
                let moved = emulator
                    .request("PUT", &format!("/channel/{}/{segment}", encode(name)), None)
                    .await;
                assert_eq!(
                    moved.status, 200,
                    "case {case:?}: expected the channel to be {segment}ed first, got {} {:?}",
                    moved.status, moved.body
                );
            }

            // Every delete of the name answers the same way: the first one
            // removes the channel, and a repeat has nothing left to remove.
            for round in 0..repeats {
                let deleted = emulator.delete(name).await;
                assert_eq!(
                    deleted.status, DELETE_STATUS,
                    "case {case:?}: expected delete {round} to answer {DELETE_STATUS}, got {} {:?}",
                    deleted.status, deleted.body
                );
                assert!(
                    deleted.body == Value::Null || deleted.body == json!({}),
                    "case {case:?}: expected delete {round} to answer an empty response, got {:?}",
                    deleted.body
                );
            }

            // The channel is gone: every route that addresses it by name
            // answers the channel miss.
            for (call, reply) in channel_name_calls(&emulator, name).await {
                assert_not_found(&case, call, &reply, CHANNEL_NOT_FOUND_MESSAGE);
            }

            // And its ARN addresses no resource, so the tags that were keyed to
            // it went with it rather than waiting for the next channel of the
            // same name.
            for (call, reply) in tagging_calls(&emulator, &arn).await {
                assert_not_found(&case, call, &reply, RESOURCE_NOT_FOUND_MESSAGE);
            }
        }
    }
}

#[tokio::test]
async fn a_deleted_name_is_free_and_the_new_channel_carries_none_of_the_old_state() {
    let emulator = Emulator::new();

    for (case, name, request) in table() {
        // The first channel, tagged through the tagging surface so there is
        // state keyed to its ARN beyond whatever the request carried.
        let created = emulator.create(name, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the first channel to be created, got {} {:?}",
            created.status, created.body
        );
        let arn = created.string("/Arn");
        assert_eq!(
            emulator.tag(&arn, json!({ "Round": "first" })).await.status,
            204,
            "case {case:?}: expected the first channel to be taggable"
        );
        assert_eq!(
            configure_logs(&emulator, name, &json!(LOG_TYPES)).await.status,
            200,
            "case {case:?}: expected the first channel to accept log types"
        );

        assert_eq!(
            emulator.delete(name).await.status,
            DELETE_STATUS,
            "case {case:?}: expected the first channel to be deleted"
        );

        // The name is free: a create under it succeeds rather than colliding
        // with the deleted channel.
        let recreated = emulator.create(name, &json!({
            "PlaybackMode": "LOOP",
            "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
        }))
        .await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: expected the deleted name to be free, got {} {:?}",
            recreated.status, recreated.body
        );

        // And the new channel carries none of the deleted one's state: not its
        // tags, not its log types, not its members.
        let described = emulator.describe(name).await;
        assert_eq!(
            described.get("/tags"),
            &json!({}),
            "case {case:?}: expected the new channel to carry no tag of the deleted one, got {:?}",
            described.get("/tags")
        );
        assert_eq!(
            described.get("/LogConfiguration/LogTypes"),
            &json!([]),
            "case {case:?}: expected the new channel to carry no log type of the deleted one, \
             got {:?}",
            described.get("/LogConfiguration/LogTypes")
        );
        assert_eq!(
            described.get("/Outputs/0/ManifestName"),
            &json!("index"),
            "case {case:?}: expected the new channel's own outputs, got {:?}",
            described.get("/Outputs")
        );
        assert_eq!(
            described.string("/ChannelState"),
            "STOPPED",
            "case {case:?}: expected the new channel to be created stopped, got {:?}",
            described.get("/ChannelState")
        );
    }
}

#[tokio::test]
async fn a_delete_removes_only_the_channel_it_names() {
    let emulator = Emulator::new();

    // Every channel the lifecycle table spans, all stored side by side, each
    // carrying a tag written through the tagging surface.
    for (case, name, request) in table() {
        let created = emulator.create(name, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the channel to be created, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            emulator
                .tag(&created.string("/Arn"), json!({ "Keep": name }))
                .await
                .status,
            204,
            "case {case:?}: expected the channel to be taggable"
        );
    }

    // Each row is deleted in turn; every channel not yet deleted has to read
    // back exactly as it did before that call, tags included.
    let mut remaining: Vec<&str> = table().iter().map(|(_, name, _)| *name).collect();
    for (case, name, _) in table() {
        let before: Vec<(String, Value)> = {
            let mut snapshots = Vec::new();
            for other in &remaining {
                snapshots.push((
                    (*other).to_string(),
                    emulator.describe(other).await.body.clone(),
                ));
            }
            snapshots
        };

        let deleted = emulator.delete(name).await;
        assert_eq!(
            deleted.status, DELETE_STATUS,
            "case {case:?}: expected the delete to answer {DELETE_STATUS}, got {} {:?}",
            deleted.status, deleted.body
        );
        remaining.retain(|other| *other != name);

        for (other, snapshot) in before {
            let described = emulator.describe(&other).await;
            if other == name {
                assert_not_found(case, "DescribeChannel", &described, CHANNEL_NOT_FOUND_MESSAGE);
            } else {
                assert_eq!(
                    described.body, snapshot,
                    "case {case:?}: expected the delete of {name:?} to leave {other:?} \
                     untouched, got {:?}",
                    described.body
                );
                assert_eq!(
                    emulator
                        .list_tags(&expected_arn(&other))
                        .await
                        .string("/tags/Keep"),
                    other,
                    "case {case:?}: expected {other:?} to keep the tag written against its own ARN"
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_a_name_the_account_holds_no_channel_under_succeeds_and_stores_nothing() {
    let emulator = Emulator::new();

    // Stored, so the deletes below are about the name and not about an empty
    // store.
    let (_, stored, request) = table().remove(0);
    assert_eq!(
        emulator.create(stored, &request).await.status,
        200,
        "the channel the deletes are compared against must be created"
    );

    for name in [
        "absent",
        "LOOP-MINIMAL",
        "loop-minimal-2",
        "loop",
        "a.b_c",
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    ] {
        assert_ne!(name, stored, "the rows must not name the stored channel");
        for round in 0..2 {
            let deleted = emulator.delete(name).await;
            assert_eq!(
                deleted.status, DELETE_STATUS,
                "case {name:?}: expected delete {round} of a name holding no channel to answer \
                 {DELETE_STATUS}, got {} {:?}",
                deleted.status, deleted.body
            );
        }

        // It stored nothing: the name still holds no channel.
        assert_not_found(
            name,
            "DescribeChannel",
            &emulator.describe(name).await,
            CHANNEL_NOT_FOUND_MESSAGE,
        );
    }

    // And the channel that does exist is untouched by all of them.
    let described = emulator.describe(stored).await;
    assert_eq!(
        described.status, 200,
        "expected the stored channel to survive every delete of another name, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.string("/ChannelName"),
        stored,
        "expected the stored channel to report its own name, got {:?}",
        described.get("/ChannelName")
    );
}

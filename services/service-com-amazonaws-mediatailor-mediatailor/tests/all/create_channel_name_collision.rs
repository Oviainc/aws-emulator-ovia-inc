// SPDX-License-Identifier: CC0-1.0

//! `CreateChannel` under a name the account already holds, end to end over HTTP.
//!
//! The rule under test: this service refuses a create whose name is already in
//! use with HTTP 400 `BadRequestException` whose message is its ONE fixed
//! sentence for that collision — the same bytes for every request, carrying no
//! value from the request or from the stored channel — and the refusal changes
//! nothing, so the channel already stored keeps every member it was created
//! with, including its tag set.
//!
//! The table spans the class the rule generalises over: the form and length of
//! the requested name, and how far the second request differs from the stored
//! channel (playback mode, tier, output count and playlist form, audiences,
//! time-shift, tags). The recorded case — a LOOP/BASIC channel re-created with
//! different outputs — is one row.

use serde_json::{json, Value};

use crate::channel_lifecycle::{encode, Emulator, CHANNEL_ALREADY_EXISTS_MESSAGE};

/// Each row: the case, the name both creates carry, the channel that is stored
/// first, and the different channel the second create asks for under that same
/// name.
fn collisions() -> Vec<(&'static str, &'static str, Value, Value)> {
    vec![
        (
            "the same name with different outputs",
            "dup-demo",
            json!({
                "PlaybackMode": "LOOP",
                "Tier": "BASIC",
                "Outputs": [{
                    "ManifestName": "default",
                    "SourceGroup": "default",
                    "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
                }],
            }),
            json!({
                "PlaybackMode": "LOOP",
                "Tier": "BASIC",
                "Outputs": [{
                    "ManifestName": "other",
                    "SourceGroup": "other",
                    "HlsPlaylistSettings": { "ManifestWindowSeconds": 60 },
                }],
            }),
        ),
        (
            "a dotted, underscored name re-created in another tier with tags",
            "channel.one_2",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
            }),
            json!({
                "PlaybackMode": "LOOP",
                "Tier": "STANDARD",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "tags": { "Replaced": "yes" },
            }),
        ),
        (
            "a single-character name re-created in the other playback mode",
            "z",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "tags": { "Env": "prod" },
            }),
            json!({
                "PlaybackMode": "LINEAR",
                "FillerSlate": {
                    "SourceLocationName": "slate-location",
                    "VodSourceName": "slate-source",
                },
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
            }),
        ),
        (
            "a 64-character name re-created with three outputs and audiences",
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{
                    "ManifestName": "manifest",
                    "SourceGroup": "group-b",
                    "DashPlaylistSettings": { "ManifestWindowSeconds": 60 },
                }],
                "TimeShiftConfiguration": { "MaxTimeDelaySeconds": 21600 },
            }),
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [
                    {
                        "ManifestName": "hls",
                        "SourceGroup": "group-a",
                        "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
                    },
                    {
                        "ManifestName": "dash",
                        "SourceGroup": "group-a",
                        "DashPlaylistSettings": { "ManifestWindowSeconds": 30 },
                    },
                    {
                        "ManifestName": "both",
                        "SourceGroup": "group-c",
                        "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
                        "DashPlaylistSettings": { "ManifestWindowSeconds": 30 },
                    },
                ],
                "Audiences": ["en", "fr"],
            }),
        ),
        (
            "the stored channel re-created with exactly the same request",
            "identical-resubmit",
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "tags": { "k": "v" },
            }),
            json!({
                "PlaybackMode": "LOOP",
                "Outputs": [{ "ManifestName": "index", "SourceGroup": "default" }],
                "tags": { "k": "v" },
            }),
        ),
    ]
}

#[tokio::test]
async fn a_name_collision_is_refused_with_one_fixed_sentence() {
    let emulator = Emulator::new();
    let mut sentences: Vec<String> = Vec::new();

    for (case, name, stored_request, conflicting) in collisions() {
        let created = emulator.create(name, &stored_request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first create must succeed, got {} {:?}",
            created.status, created.body
        );
        let stored = emulator.describe(name).await.body;
        let arn = created.string("/Arn");
        let stored_tags = emulator
            .request("GET", &format!("/tags/{}", encode(&arn)), None)
            .await
            .body;

        let refused = emulator.create(name, &conflicting).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the collision to be refused with 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            CHANNEL_ALREADY_EXISTS_MESSAGE,
            "case {case:?}: expected the fixed name-collision sentence, got {:?}",
            refused.message()
        );
        // Fixed text means no part of the request or of the stored record is
        // interpolated into it: not the name, not the ARN.
        assert!(
            !refused.message().contains(name) && !refused.message().contains(&arn),
            "case {case:?}: expected a refusal carrying neither {name:?} nor {arn:?}, got {:?}",
            refused.message()
        );
        sentences.push(refused.message());

        // The refusal returns before anything is written, so the stored channel
        // and its tag set are exactly what the first create left.
        let after = emulator.describe(name).await;
        assert_eq!(
            after.body, stored,
            "case {case:?}: expected the refused create to leave the stored channel unchanged, \
             got {:?}",
            after.body
        );
        let tags_after = emulator
            .request("GET", &format!("/tags/{}", encode(&arn)), None)
            .await
            .body;
        assert_eq!(
            tags_after, stored_tags,
            "case {case:?}: expected the refused create to leave the tag set unchanged, got {:?}",
            tags_after
        );
    }

    // One sentence for the whole class: every row answered with the same bytes.
    for (index, sentence) in sentences.iter().enumerate() {
        assert_eq!(
            sentence,
            &sentences[0],
            "case {index}: expected every collision to be refused with the same sentence as the \
             first ({:?}), got {sentence:?}",
            sentences[0]
        );
    }
}

#[tokio::test]
async fn a_collision_is_refused_however_many_times_it_is_retried() {
    let emulator = Emulator::new();
    let name = "retried-collision";
    let request = json!({
        "PlaybackMode": "LOOP",
        "Tier": "BASIC",
        "Outputs": [{
            "ManifestName": "default",
            "SourceGroup": "default",
            "HlsPlaylistSettings": { "ManifestWindowSeconds": 30 },
        }],
    });
    assert_eq!(
        emulator.create(name, &request).await.status,
        200,
        "the first create must succeed"
    );
    let stored = emulator.describe(name).await.body;

    // A caller that retries the create sees the same refusal every time, and the
    // stored channel never moves.
    for attempt in 1..=3 {
        let refused = emulator.create(name, &request).await;
        assert_eq!(
            refused.status, 400,
            "attempt {attempt}: expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.message(),
            CHANNEL_ALREADY_EXISTS_MESSAGE,
            "attempt {attempt}: expected the fixed name-collision sentence, got {:?}",
            refused.message()
        );
        assert_eq!(
            emulator.describe(name).await.body,
            stored,
            "attempt {attempt}: expected the stored channel to be unchanged"
        );
    }
}

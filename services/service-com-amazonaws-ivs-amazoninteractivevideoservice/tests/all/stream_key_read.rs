// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetStreamKey`, driven over the wire through
//! `MultiServiceRouter::dispatch` on the harness `channel_lifecycle` already owns.
//!
//! The rule under test, exercised over a SET of keys rather than one:
//!
//! * `GetStreamKey` answers, for a `StreamKeyArn` the service persisted a key under,
//!   exactly that persisted key — `arn`, `value`, `channelArn` and `tags`, member for
//!   member, whichever create minted it — and a `StreamKeyArn` no key is persisted
//!   under is a 404 `ResourceNotFoundException`.
//! * It is a reporting operation, so repeating it answers identically and mints
//!   nothing.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::channel_lifecycle::{
    arn_is_well_formed, text, Ivs, CHANNEL_ARN_PREFIX, STREAM_KEY_ARN_PREFIX,
};

/// Every key the service persists is readable by its own ARN, and what comes back is
/// member-for-member what the create returned — including `value`, which only the
/// single-key read and the create carry.
#[tokio::test]
async fn a_persisted_stream_key_is_read_back_member_for_member() {
    let ivs = Ivs::new();

    // Rows differ along what the rule generalises over: which create minted the key
    // (CreateChannel's first key vs. CreateStreamKey's extra ones), the number of
    // tags carried, and which of two channels owns it.
    let first = ivs.create(json!({ "name": "reader-one" })).await;
    let first_arn = text(&first["channel"], "arn");
    let second = ivs.create(json!({ "name": "reader-two", "type": "BASIC" })).await;
    let second_arn = text(&second["channel"], "arn");

    let mut cases: Vec<(String, Value)> = vec![
        (
            "the key CreateChannel minted with its channel".to_string(),
            first["streamKey"].clone(),
        ),
        (
            "the key CreateChannel minted for a second channel".to_string(),
            second["streamKey"].clone(),
        ),
    ];
    for (label, request) in [
        ("an extra key, untagged", json!({})),
        ("an extra key, one tag", json!({ "tags": { "env": "test" } })),
        (
            "an extra key, several tags",
            json!({ "tags": { "env": "prod", "team": "video", "cost.centre": "42" } }),
        ),
        (
            "an extra key with an empty tag map",
            json!({ "tags": {} }),
        ),
        ("an extra key on a channel of another type", json!({})),
    ] {
        // A channel's stream-key quota is one key, which `CreateChannel` has already
        // filled, so a key from `CreateStreamKey` needs the slot freed first — the
        // delete-then-create a client rotates a key with. Each row gets its own
        // channel, so the keys the rows before it minted stay readable.
        let channel = ivs.create(json!({ "name": "extraKeyOwner" })).await;
        let channel_arn = text(&channel["channel"], "arn");
        let freed = ivs
            .call(
                "DeleteStreamKey",
                &json!({ "arn": text(&channel["streamKey"], "arn") }),
            )
            .await;
        assert_eq!(
            freed.status, 204,
            "case {label:?}: deleting the channel's key must free its quota slot, got \
             {} {}",
            freed.status, freed.body
        );
        let mut body = request.clone();
        body["channelArn"] = json!(channel_arn);
        let created = ivs.ok("CreateStreamKey", body).await;
        cases.push((label.to_string(), created["streamKey"].clone()));
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (case, minted) in &cases {
        let key_arn = text(minted, "arn");
        assert!(
            arn_is_well_formed(&key_arn, STREAM_KEY_ARN_PREFIX),
            "case {case:?}: the minted arn must satisfy the modelled StreamKeyArn \
             pattern, got {key_arn:?}"
        );
        assert!(
            seen.insert(key_arn.clone()),
            "case {case:?}: two creates must not mint the same arn, got {key_arn:?}"
        );

        let fetched = ivs.ok("GetStreamKey", json!({ "arn": key_arn })).await;
        let got = &fetched["streamKey"];
        assert_eq!(
            got, minted,
            "case {case:?}: GetStreamKey must answer the persisted key member for \
             member, got {got}"
        );
        // The members the rule names, called out one by one so a failure says which
        // one drifted.
        assert_eq!(
            text(got, "arn"),
            key_arn,
            "case {case:?}: the answered arn must be the one asked for, got {got}"
        );
        assert_eq!(
            text(got, "value"),
            text(minted, "value"),
            "case {case:?}: the secret value must survive the round trip, got {got}"
        );
        assert!(
            !text(got, "value").is_empty(),
            "case {case:?}: GetStreamKey must answer the minted value, got {got}"
        );
        assert_eq!(
            text(got, "channelArn"),
            text(minted, "channelArn"),
            "case {case:?}: the answered key must name the channel that owns it, got {got}"
        );
        assert_eq!(
            got.get("tags").cloned().unwrap_or_else(|| json!({})),
            minted.get("tags").cloned().unwrap_or_else(|| json!({})),
            "case {case:?}: tags must survive the round trip, got {got}"
        );
    }

    // Every key the service holds for a channel is readable by ARN, so the set
    // `ListStreamKeys` reports and the set `GetStreamKey` answers agree.
    for owner in [&first_arn, &second_arn] {
        let listed = ivs
            .ok("ListStreamKeys", json!({ "channelArn": owner }))
            .await;
        for summary in listed["streamKeys"].as_array().expect("a list") {
            let key_arn = text(summary, "arn");
            let fetched = ivs
                .ok("GetStreamKey", json!({ "arn": key_arn.clone() }))
                .await;
            assert_eq!(
                text(&fetched["streamKey"], "channelArn"),
                *owner,
                "case listed key {key_arn:?}: GetStreamKey must agree with \
                 ListStreamKeys about the owning channel, got {fetched}"
            );
        }
    }
}

/// A `StreamKeyArn` no key is persisted under is a 404, whether it never existed or
/// its channel (and with it its keys) was deleted. A malformed ARN is refused by the
/// modelled `StreamKeyArn` constraint as a 400 before the handler runs.
#[tokio::test]
async fn an_arn_with_no_persisted_key_is_a_404() {
    let ivs = Ivs::new();
    let doomed = ivs.create(json!({ "name": "doomed" })).await;
    let doomed_arn = text(&doomed["channel"], "arn");
    let doomed_key = text(&doomed["streamKey"], "arn");
    let survivor = ivs.create(json!({ "name": "survivor" })).await;
    let survivor_key = text(&survivor["streamKey"], "arn");

    // Readable while it is persisted.
    ivs.ok("GetStreamKey", json!({ "arn": doomed_key.clone() }))
        .await;
    // Deleting the channel drops its keys, so the same ARN is then a 404.
    // `DeleteChannel` models an empty output, which restJson1 answers as 204.
    let deleted = ivs.call("DeleteChannel", &json!({ "arn": doomed_arn })).await;
    assert_eq!(
        deleted.status, 204,
        "case delete the owning channel: expected HTTP 204, got {} {}",
        deleted.status, deleted.body
    );

    let absent = [
        (
            "an arn the service never minted",
            format!("{STREAM_KEY_ARN_PREFIX}doesnotexist"),
        ),
        (
            "a well-formed arn in another region",
            "arn:aws:ivs:eu-west-1:000000000000:stream-key/abcdef123456".to_string(),
        ),
        (
            "a well-formed arn under another account",
            "arn:aws:ivs:us-east-1:123456789012:stream-key/abcdef123456".to_string(),
        ),
        (
            "the key of a channel that was deleted",
            doomed_key.clone(),
        ),
    ];
    for (case, arn) in absent {
        let reply = ivs.call("GetStreamKey", &json!({ "arn": arn })).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // The surviving channel's key is untouched by any of those refusals.
    let fetched = ivs
        .ok("GetStreamKey", json!({ "arn": survivor_key.clone() }))
        .await;
    assert_eq!(
        text(&fetched["streamKey"], "arn"),
        survivor_key,
        "case survivor: a refused read must not disturb another key, got {fetched}"
    );

    // Values outside the modelled `StreamKeyArn` pattern
    // `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stream-key/[a-zA-Z0-9-]+$` (and its 1..128
    // length) are a 400, one for each way of leaving it.
    let malformed = [
        ("not an arn at all", "not-an-arn".to_string()),
        ("empty, below the modelled minimum length", String::new()),
        (
            "a channel arn, the wrong resource type",
            format!("{CHANNEL_ARN_PREFIX}abcdef123456"),
        ),
        (
            "an uppercase region",
            "arn:aws:ivs:US-EAST-1:000000000000:stream-key/abcdef123456".to_string(),
        ),
        (
            "an id outside [a-zA-Z0-9-]",
            format!("{STREAM_KEY_ARN_PREFIX}has_underscore"),
        ),
        (
            "an id with no characters at all",
            STREAM_KEY_ARN_PREFIX.to_string(),
        ),
        (
            "129 characters, one past the modelled maximum length",
            format!(
                "{STREAM_KEY_ARN_PREFIX}{}",
                "a".repeat(129 - STREAM_KEY_ARN_PREFIX.len())
            ),
        ),
    ];
    for (case, arn) in malformed {
        let reply = ivs.call("GetStreamKey", &json!({ "arn": arn })).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
    // The modelled upper bound itself is accepted as input (and answers 404, since no
    // key is persisted under it) rather than refused.
    let at_bound = format!(
        "{STREAM_KEY_ARN_PREFIX}{}",
        "a".repeat(128 - STREAM_KEY_ARN_PREFIX.len())
    );
    let reply = ivs.call("GetStreamKey", &json!({ "arn": at_bound })).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case 128 characters, the modelled maximum length: expected 404 \
         ResourceNotFoundException, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
}

/// `GetStreamKey` is a read: repeating it answers identically and changes nothing a
/// later caller sees.
#[tokio::test]
async fn reading_a_stream_key_is_repeatable() {
    let ivs = Ivs::new();
    let created = ivs
        .create(json!({ "name": "stable", "tags": { "env": "test" } }))
        .await;
    let owner = text(&created["channel"], "arn");
    let key_arn = text(&created["streamKey"], "arn");

    for round in 0..3 {
        let fetched = ivs
            .ok("GetStreamKey", json!({ "arn": key_arn.clone() }))
            .await;
        assert_eq!(
            fetched["streamKey"], created["streamKey"],
            "case round {round}: GetStreamKey must answer the same key every time"
        );
        let listed = ivs
            .ok("ListStreamKeys", json!({ "channelArn": owner.clone() }))
            .await;
        assert_eq!(
            listed["streamKeys"].as_array().map(Vec::len),
            Some(1),
            "case round {round}: reading a key must not mint or drop one, got {listed}"
        );
    }
}

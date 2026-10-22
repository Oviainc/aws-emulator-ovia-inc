// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteStreamKey`, driven over the wire.
//!
//! Every case goes through `MultiServiceRouter::dispatch` with a restJson1 request,
//! exactly as a signed `aws ivs` call arrives, reusing the harness
//! `channel_lifecycle` already owns rather than growing a second one. The read side's
//! own table — what `GetStreamKey` answers for a persisted ARN — lives in
//! `stream_key_read.rs`; here the read is the observation that the delete happened.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `DeleteStreamKey` removes exactly the key its ARN names and answers its
//!    modelled empty 204: afterwards that ARN is gone from every reader, and no other
//!    key — of the same channel or of another channel — changes.
//! 2. An ARN no persisted key carries is the modelled `ResourceNotFoundException`
//!    (HTTP 404), so a delete of one ARN succeeds exactly once.
//! 3. Deleting a key and creating another one for the channel (the rotation AWS
//!    documents on `CreateStreamKey`) yields a key with a different ARN and a
//!    different ingest value; the retired value never comes back.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::channel_lifecycle::{arn_is_well_formed, text, Ivs, STREAM_KEY_ARN_PREFIX};

/// `DeleteStreamKey` is modelled with success status 204 and an empty body (Smithy
/// `output: Unit`, `http code 204`), so a successful delete asserts both.
async fn delete_key(ivs: &Ivs, arn: &str, case: &str) {
    let reply = ivs.call("DeleteStreamKey", &json!({ "arn": arn })).await;
    assert_eq!(
        reply.status, 204,
        "case {case:?}: DeleteStreamKey must answer its modelled 204, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.body,
        Value::Null,
        "case {case:?}: DeleteStreamKey models no output members, so the body must be \
         empty, got {}",
        reply.body
    );
}

/// Require `operation` on `arn` to be the modelled 404.
async fn expect_not_found(ivs: &Ivs, operation: &str, arn: &str, case: &str) {
    let reply = ivs.call(operation, &json!({ "arn": arn })).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: {operation} on an ARN no persisted key carries must be 404 \
         ResourceNotFoundException, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
}

/// The ARNs `ListStreamKeys` reports for `channel_arn`.
async fn listed_keys(ivs: &Ivs, channel_arn: &str) -> BTreeSet<String> {
    let listed = ivs
        .ok(
            "ListStreamKeys",
            json!({ "channelArn": channel_arn, "maxResults": 50 }),
        )
        .await;
    listed["streamKeys"]
        .as_array()
        .expect("streamKeys is a list")
        .iter()
        .map(|summary| text(summary, "arn"))
        .collect()
}

/// Rule 1, over a table whose rows differ in how many keys the channel holds, which
/// key of that channel is deleted (the one `CreateChannel` minted, the first added,
/// the last added), and whether that key carries tags.
#[tokio::test]
async fn deleting_a_stream_key_removes_exactly_that_key() {
    let tagged = json!({ "tags": { "env": "prod", "team": "video" } });
    let cases: Vec<(&str, Vec<Value>, usize)> = vec![
        ("the only key, the one CreateChannel minted", vec![], 0),
        ("the minted key, one added key surviving", vec![json!({})], 0),
        ("the added key, the minted one surviving", vec![json!({})], 1),
        (
            "the first added of three",
            vec![json!({}), tagged.clone(), json!({})],
            1,
        ),
        (
            "the last added of three",
            vec![json!({}), tagged.clone(), json!({})],
            3,
        ),
        ("a key carrying tags", vec![tagged.clone()], 1),
    ];

    for (case, extras, victim_index) in cases {
        let ivs = Ivs::new();
        let created = ivs.create(json!({ "name": "owner" })).await;
        let owner = text(&created["channel"], "arn");
        // A second channel with its own key: whatever the delete does, it must not
        // reach a key that names another channel.
        let bystander_created = ivs.create(json!({ "name": "bystander" })).await;
        let bystander = text(&bystander_created["channel"], "arn");
        let bystander_key = bystander_created["streamKey"].clone();

        // The channel's keys, in creation order: the one CreateChannel minted first.
        let mut keys = vec![created["streamKey"].clone()];
        for extra in &extras {
            let mut request = extra.clone();
            request["channelArn"] = json!(owner);
            keys.push(ivs.ok("CreateStreamKey", request).await["streamKey"].clone());
        }

        // The key each ARN names before the delete, so what changed afterwards is
        // attributable to the delete alone.
        for key in &keys {
            let arn = text(key, "arn");
            let fetched = ivs.ok("GetStreamKey", json!({ "arn": arn })).await;
            assert_eq!(
                fetched["streamKey"], *key,
                "case {case:?}: GetStreamKey must answer the persisted key for {arn:?}, \
                 got {fetched}"
            );
            assert_eq!(
                text(key, "channelArn"),
                owner,
                "case {case:?}: every key of this channel must name it, got {key}"
            );
        }

        let victim = keys[victim_index].clone();
        let victim_arn = text(&victim, "arn");
        let survivors: BTreeSet<String> = keys
            .iter()
            .map(|key| text(key, "arn"))
            .filter(|arn| *arn != victim_arn)
            .collect();

        delete_key(&ivs, &victim_arn, case).await;

        // Rule 1 + rule 2: the deleted ARN is gone from every reader, and a repeat of
        // either call on it is the modelled 404.
        for operation in ["GetStreamKey", "DeleteStreamKey"] {
            expect_not_found(&ivs, operation, &victim_arn, case).await;
        }
        assert_eq!(
            listed_keys(&ivs, &owner).await,
            survivors,
            "case {case:?}: ListStreamKeys must answer exactly the keys that survived \
             the delete of {victim_arn:?}"
        );

        // Rule 1: nothing else moved — every surviving key still reads back whole.
        for key in keys.iter().filter(|key| text(key, "arn") != victim_arn) {
            let arn = text(key, "arn");
            let fetched = ivs.ok("GetStreamKey", json!({ "arn": arn })).await;
            assert_eq!(
                fetched["streamKey"], *key,
                "case {case:?}: deleting {victim_arn:?} must leave {arn:?} untouched, \
                 got {fetched}"
            );
        }

        // ... and neither did the other channel's key.
        let bystander_arn = text(&bystander_key, "arn");
        let fetched = ivs
            .ok("GetStreamKey", json!({ "arn": bystander_arn }))
            .await;
        assert_eq!(
            fetched["streamKey"], bystander_key,
            "case {case:?}: a delete on one channel's key must not touch another \
             channel's key, got {fetched}"
        );
        assert_eq!(
            listed_keys(&ivs, &bystander).await,
            BTreeSet::from([bystander_arn]),
            "case {case:?}: the other channel must still hold exactly its own key"
        );
    }
}

/// Rule 2, over a table of ARNs that satisfy the modelled `StreamKeyArn` pattern (so
/// the constraint walk admits them) but name no persisted key: never minted, already
/// deleted, orphaned by `DeleteChannel`, or in another region or account.
#[tokio::test]
async fn deleting_an_arn_no_persisted_key_carries_is_a_404() {
    let ivs = Ivs::new();

    // A key that existed and was deleted.
    let created = ivs.create(json!({ "name": "deleted-key" })).await;
    let deleted_arn = text(&created["streamKey"], "arn");
    delete_key(&ivs, &deleted_arn, "set-up: delete the key").await;

    // A key whose channel was deleted: DeleteChannel drops the channel's keys, so
    // the key's ARN names nothing afterwards.
    let orphaned = ivs.create(json!({ "name": "doomed" })).await;
    let orphan_arn = text(&orphaned["streamKey"], "arn");
    let doomed_channel = text(&orphaned["channel"], "arn");
    let reply = ivs
        .call("DeleteChannel", &json!({ "arn": doomed_channel }))
        .await;
    assert_eq!(
        reply.status, 204,
        "set-up: DeleteChannel must succeed, got {} {}",
        reply.status, reply.body
    );

    let cases = [
        (
            "never minted",
            format!("{STREAM_KEY_ARN_PREFIX}doesnotexist"),
        ),
        ("already deleted", deleted_arn),
        ("orphaned by DeleteChannel", orphan_arn),
        (
            "well formed in another region",
            "arn:aws:ivs:eu-west-1:000000000000:stream-key/abc123".to_string(),
        ),
        (
            "well formed in another account",
            "arn:aws:ivs:us-east-1:123456789012:stream-key/abc123".to_string(),
        ),
        (
            "a single-character id, the shortest the pattern allows",
            format!("{STREAM_KEY_ARN_PREFIX}a"),
        ),
    ];

    for (case, arn) in cases {
        expect_not_found(&ivs, "DeleteStreamKey", &arn, case).await;
        // And the key really is unreadable, not merely undeletable.
        expect_not_found(&ivs, "GetStreamKey", &arn, case).await;
    }
}

/// Rule 3: the delete-then-create rotation the model documents on `CreateStreamKey`
/// ("To reset the stream key on a channel, use DeleteStreamKey and then
/// CreateStreamKey") yields a fresh ARN and a fresh ingest value every round, and the
/// retired value is never answered again.
#[tokio::test]
async fn rotating_a_stream_key_changes_its_arn_and_value() {
    let ivs = Ivs::new();
    let created = ivs.create(json!({ "name": "rotating" })).await;
    let owner = text(&created["channel"], "arn");

    let mut current = created["streamKey"].clone();
    let mut seen_arns = BTreeSet::from([text(&current, "arn")]);
    let mut seen_values = BTreeSet::from([text(&current, "value")]);

    // Rows differ in the round and in whether the replacement carries tags.
    for (round, request) in [
        json!({}),
        json!({ "tags": { "round": "two" } }),
        json!({}),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("rotation round {round}");
        let retired_arn = text(&current, "arn");
        let retired_value = text(&current, "value");

        delete_key(&ivs, &retired_arn, &case).await;
        expect_not_found(&ivs, "GetStreamKey", &retired_arn, &case).await;

        let mut body = request.clone();
        body["channelArn"] = json!(owner);
        let replacement = ivs.ok("CreateStreamKey", body).await["streamKey"].clone();
        let arn = text(&replacement, "arn");
        let value = text(&replacement, "value");

        assert!(
            arn_is_well_formed(&arn, STREAM_KEY_ARN_PREFIX),
            "case {case:?}: a rotated key's arn must satisfy the modelled StreamKeyArn \
             pattern, got {arn:?}"
        );
        assert!(
            seen_arns.insert(arn.clone()),
            "case {case:?}: a rotation must mint an arn no earlier key used, got {arn:?}"
        );
        assert!(
            seen_values.insert(value.clone()),
            "case {case:?}: a rotation must mint a value no earlier key used, got {value:?}"
        );
        assert_ne!(
            value, retired_value,
            "case {case:?}: the replacement key's value must differ from the retired one"
        );
        assert_eq!(
            text(&replacement, "channelArn"),
            owner,
            "case {case:?}: the replacement must name the same channel, got {replacement}"
        );

        let fetched = ivs.ok("GetStreamKey", json!({ "arn": arn })).await;
        assert_eq!(
            fetched["streamKey"], replacement,
            "case {case:?}: GetStreamKey must answer the replacement key, got {fetched}"
        );
        assert_eq!(
            listed_keys(&ivs, &owner).await,
            BTreeSet::from([arn.clone()]),
            "case {case:?}: the channel must hold exactly the key the rotation left"
        );

        current = replacement;
    }
}

// SPDX-License-Identifier: CC0-1.0

//! How `RemoveNotificationChannel` refuses a `channelId` the named profiling group holds
//! no channel for, end to end over HTTP through the router this service mounts in
//! production.
//!
//! The rule every case here asserts: a notification request addresses the profiling
//! GROUP, and the channel id is a parameter of that request, so a well-formed id the
//! EXISTING group's stored configuration cannot resolve is an invalid parameter —
//! `ValidationException`, HTTP 400, stating the lookup that found nothing (the channel
//! id, the group and the account) — while `ResourceNotFoundException` (404) is reserved
//! for the profiling group itself not existing.
//!
//! The rows differ along what that rule generalises over: how the id came to be unknown
//! (never added, added to another group, minted by the service and then removed, removed
//! twice), how much the group does hold at the time (nothing, one channel, the two the
//! model's `Channels` list allows), and the shape of the id inside the model's
//! `ChannelId` pattern (all-zero, all-f, mixed) — no row is privileged, and the recorded
//! case is only one of them.
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds (one shared
//! engine behind one mounted service); only the notification calls and the tables are
//! this suite's own.

use crate::profiling_group_lifecycle::{Answer, Harness};

/// The only value the model's `EventPublisher` enum carries
/// (`smithy_query(query="shape", shape="EventPublisher")`), and `EventPublishers` is
/// `@length(min: 1, max: 1)`, so this one-element list is the only publisher list a
/// channel can carry.
const ANOMALY_DETECTION: &str = "AnomalyDetection";

/// SNS topic ARNs used as channel uris: `ChannelUri` is an `@arnReference` documented as
/// "Currently we only support SNS channels and thus required to be an ARN".
const TOPIC_A: &str = "arn:aws:sns:us-east-1:000000000000:anomalies-a";
const TOPIC_B: &str = "arn:aws:sns:us-east-1:000000000000:anomalies-b";

/// Well-formed `ChannelId`s — each matching the model's `ChannelId` pattern
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`
/// (`smithy_query(query="shape", shape="ChannelId")`), so the boundary validator admits
/// them and the handler is what decides what they mean. The three shapes span the
/// pattern's alphabet: all-digit, all-letter and mixed.
const ALL_ZERO_ID: &str = "00000000-0000-0000-0000-000000000000";
const ALL_F_ID: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";
const MIXED_ID: &str = "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";
const SECOND_MIXED_ID: &str = "9f8e7d6c-5b4a-3029-1807-6f5e4d3c2b1a";

/// `CreateProfilingGroup`: the group record is what holds the notification
/// configuration, so every case starts here.
async fn create_group(harness: &Harness, name: &str) {
    let answer = harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=token-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await;
    assert_eq!(
        answer.status,
        201,
        "case {name}: CreateProfilingGroup must succeed, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// One channel as a request body member; `id: None` leaves the id to the service, which
/// `Channel$id` documents as "A random UUID for channelId is used when adding a channel
/// to the notification configuration if not specified in the request".
fn channel_body(id: Option<&str>, uri: &str) -> serde_json::Value {
    let mut channel = serde_json::Map::new();
    if let Some(id) = id {
        channel.insert("id".to_string(), serde_json::Value::from(id));
    }
    channel.insert("uri".to_string(), serde_json::Value::from(uri));
    channel.insert(
        "eventPublishers".to_string(),
        serde_json::json!([ANOMALY_DETECTION]),
    );
    serde_json::Value::Object(channel)
}

/// `AddNotificationChannels`: `POST
/// /profilingGroups/{profilingGroupName}/notificationConfiguration`.
async fn add(harness: &Harness, group: &str, channels: &[serde_json::Value]) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups/{group}/notificationConfiguration"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "channels": channels }))
                .expect("a serializable body"),
        )
        .await
}

/// `GetNotificationConfiguration`: `GET
/// /profilingGroups/{profilingGroupName}/notificationConfiguration`.
async fn get(harness: &Harness, group: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!("/profilingGroups/{group}/notificationConfiguration"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// `RemoveNotificationChannel`: `DELETE
/// /profilingGroups/{profilingGroupName}/notificationConfiguration/{channelId}`.
async fn remove(harness: &Harness, group: &str, channel_id: &str) -> Answer {
    harness
        .send(
            "DELETE",
            &format!("/profilingGroups/{group}/notificationConfiguration/{channel_id}"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// The channel ids a response's `notificationConfiguration` reports, in order.
fn reported_ids(case: &str, answer: &Answer) -> Vec<String> {
    let body = answer.json();
    let channels = body
        .get("notificationConfiguration")
        .and_then(|configuration| configuration.get("channels"))
        .and_then(|channels| channels.as_array())
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case}: expected notificationConfiguration.channels, got {body}")
        });
    channels
        .iter()
        .map(|channel| {
            channel
                .get("id")
                .and_then(|id| id.as_str())
                .unwrap_or_else(|| {
                    panic!("case {case}: a stored channel must report an id, got {channel}")
                })
                .to_string()
        })
        .collect()
}

/// The sentence the service states for a channel lookup that found nothing, built from
/// the request's own channel id and group name and the account every resource this
/// emulator mints lives in. Read from the runtime rather than written out, so this suite
/// cannot drift from the account the ARNs carry.
fn expected_message(channel_id: &str, group: &str) -> String {
    format!(
        "Channel not found with channelId: {channel_id}, profilingGroupName: {group} and \
         AccountId: {}",
        engine::emulator_account_id()
    )
}

/// Assert the whole refusal of one call: the modelled status, code and message.
fn assert_unknown_channel(case: &str, group: &str, channel_id: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        400,
        "case {case}: an unresolvable channelId must answer 400, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case}: expected ValidationException, got {}",
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_message(),
        expected_message(channel_id, group),
        "case {case}: expected the channel-lookup sentence, got {:?}",
        answer.error_message()
    );
}

/// What the group holds when the refused call arrives, and which unknown id that call
/// names.
struct UnknownCase {
    label: &'static str,
    group: &'static str,
    /// The channels stored before the refused call: each `Some(id)` is caller-named,
    /// `None` leaves the id to the service.
    stored: Vec<Option<&'static str>>,
    /// The id the refused call names: one nobody added, or (with `remove_first`) one the
    /// group held until the removal just before it.
    unknown: &'static str,
    /// Remove this stored channel (by index into `stored`) before the refused call, so
    /// the refused id is one the group held a moment ago.
    remove_first: Option<usize>,
}

fn unknown_cases() -> Vec<UnknownCase> {
    vec![
        // Nothing stored at all: the configuration is empty and the id was never added.
        UnknownCase {
            label: "empty configuration, id never added",
            group: "unknown-empty",
            stored: Vec::new(),
            unknown: ALL_ZERO_ID,
            remove_first: None,
        },
        // One stored channel, and an id of a different shape that nobody added.
        UnknownCase {
            label: "one stored channel, id never added",
            group: "unknown-one",
            stored: vec![Some(MIXED_ID)],
            unknown: ALL_F_ID,
            remove_first: None,
        },
        // The two channels the model's `Channels` list allows, one of them minted.
        UnknownCase {
            label: "full configuration, id never added",
            group: "unknown-full",
            stored: vec![Some(MIXED_ID), None],
            unknown: SECOND_MIXED_ID,
            remove_first: None,
        },
        // The id was there and was removed: the second removal is the same refusal.
        UnknownCase {
            label: "caller-named id already removed",
            group: "unknown-removed",
            stored: vec![Some(ALL_F_ID), Some(MIXED_ID)],
            unknown: ALL_F_ID,
            remove_first: Some(0),
        },
    ]
}

#[tokio::test]
async fn an_unknown_channel_id_on_an_existing_group_is_a_validation_refusal() {
    let harness = Harness::new();

    for case in unknown_cases() {
        let label = case.label;
        create_group(&harness, case.group).await;

        if !case.stored.is_empty() {
            let channels: Vec<serde_json::Value> = case
                .stored
                .iter()
                .enumerate()
                .map(|(index, id)| channel_body(*id, if index == 0 { TOPIC_A } else { TOPIC_B }))
                .collect();
            let added = add(&harness, case.group, &channels).await;
            assert_eq!(
                added.status,
                200,
                "case {label}: the stored channels must be accepted, got {} / {}",
                added.status,
                String::from_utf8_lossy(&added.body)
            );
        }

        if let Some(index) = case.remove_first {
            let id = case.stored[index].expect("the removed-first row names its own id");
            let removed = remove(&harness, case.group, id).await;
            assert_eq!(
                removed.status,
                200,
                "case {label}: removing a stored channel must succeed, got {} / {}",
                removed.status,
                String::from_utf8_lossy(&removed.body)
            );
        }

        let before = reported_ids(label, &get(&harness, case.group).await);

        let refused = remove(&harness, case.group, case.unknown).await;
        assert_unknown_channel(label, case.group, case.unknown, &refused);

        // A refusal is not a write: the configuration is exactly what it was, and the
        // same call refused again says the same thing.
        assert_eq!(
            reported_ids(label, &get(&harness, case.group).await),
            before,
            "case {label}: a refused remove must leave the configuration unchanged"
        );
        let again = remove(&harness, case.group, case.unknown).await;
        assert_unknown_channel(label, case.group, case.unknown, &again);
    }
}

#[tokio::test]
async fn an_id_another_group_holds_is_unknown_in_the_group_the_request_names() {
    let harness = Harness::new();
    // Two groups, each holding a channel under an id of its own: an id is resolvable only
    // inside the configuration of the group the request addresses.
    let rows: [(&str, &str, &str, &str); 2] = [
        ("named ids", "cross-named-a", "cross-named-b", MIXED_ID),
        ("the other shape", "cross-shape-a", "cross-shape-b", ALL_F_ID),
    ];

    for (case, first, second, id) in rows {
        create_group(&harness, first).await;
        create_group(&harness, second).await;
        let added = add(&harness, first, &[channel_body(Some(id), TOPIC_A)]).await;
        assert_eq!(
            added.status,
            200,
            "case {case}: the add to {first} must succeed, got {} / {}",
            added.status,
            String::from_utf8_lossy(&added.body)
        );

        let refused = remove(&harness, second, id).await;
        assert_unknown_channel(case, second, id, &refused);

        // And the group that DOES hold it still removes it.
        let removed = remove(&harness, first, id).await;
        assert_eq!(
            removed.status,
            200,
            "case {case}: {first} must still remove its own channel, got {} / {}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        assert!(
            reported_ids(case, &removed).is_empty(),
            "case {case}: {first} must be left with no channels, got {:?}",
            reported_ids(case, &removed)
        );
    }
}

#[tokio::test]
async fn a_group_that_does_not_exist_stays_the_modelled_not_found() {
    let harness = Harness::new();
    // The other side of the rule: 404 ResourceNotFoundException is about the GROUP, so it
    // is what a request against a name no group has answers, whatever the channel id —
    // including a group that existed and was deleted.
    let deleted = "gone-group";
    create_group(&harness, deleted).await;
    let added = add(&harness, deleted, &[channel_body(Some(MIXED_ID), TOPIC_A)]).await;
    assert_eq!(
        added.status,
        200,
        "case deleted group: the add must succeed, got {} / {}",
        added.status,
        String::from_utf8_lossy(&added.body)
    );
    let dropped = harness
        .send(
            "DELETE",
            &format!("/profilingGroups/{deleted}"),
            "application/json",
            Vec::new(),
        )
        .await;
    assert_eq!(
        dropped.status,
        204,
        "case deleted group: DeleteProfilingGroup must answer 204, got {} / {}",
        dropped.status,
        String::from_utf8_lossy(&dropped.body)
    );

    for (case, group, id) in [
        ("never created, unknown id", "no-such-group", ALL_ZERO_ID),
        ("deleted group, id it held", deleted, MIXED_ID),
        ("deleted group, id it never held", deleted, SECOND_MIXED_ID),
    ] {
        let answer = remove(&harness, group, id).await;
        assert_eq!(
            answer.status,
            404,
            "case {case}: a group that does not exist must answer 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_message(),
            service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::group_not_found_message(
                group
            ),
            "case {case}: a missing group must be worded as the service's group refusal, got {:?}",
            answer.error_message()
        );
        assert_ne!(
            answer.error_message(),
            expected_message(id, group),
            "case {case}: a missing group must not be worded as a missing channel, got {:?}",
            answer.error_message()
        );
    }
}

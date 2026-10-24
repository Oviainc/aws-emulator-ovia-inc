// SPDX-License-Identifier: CC0-1.0

//! Which profiling group a notification channel belongs to, end to end over HTTP
//! through the router this service mounts in production.
//!
//! The rule every case here asserts: a channel's `channelId` identifies it inside the
//! notification configuration of ONE profiling group — `Channel$id` is the "Unique
//! identifier for each `Channel` in the notification configuration of a Profiling Group"
//! — so the configuration is part of that group's record: the same id is usable
//! independently in two groups, an id one group holds names nothing in another, each
//! minted id is distinct from every other this store hands out, and deleting a group
//! takes its configuration with it.
//!
//! This is the half of the notification rule the sibling suite
//! [`crate::notification_configuration`] does not reach: that one builds, reads, refuses
//! and empties the configuration of a SINGLE group (so the add shapes, the grow/shrink
//! orders, the quota, the conflict and the uri/publisher catalogues live there), and
//! never crosses two groups or outlives one. The rows here differ along what this rule
//! generalises over: how many channels a group holds, whether the caller names the id or
//! the service mints it, which of two groups a call is addressed to, and whether the
//! group the id was added to still exists.

use crate::profiling_group_lifecycle::{Answer, Harness};

/// The only value the model's `EventPublisher` enum carries
/// (`smithy_query(query="shape", shape="EventPublisher")`), and — `EventPublishers`
/// being `@length(min: 1, max: 1)` — the only publisher list a channel can carry.
const ANOMALY_DETECTION: &str = "AnomalyDetection";

/// SNS topic ARNs used as channel uris: `ChannelUri` is an `@arnReference` documented as
/// "Currently we only support SNS channels and thus required to be an ARN".
const TOPIC_A: &str = "arn:aws:sns:us-east-1:000000000000:anomalies-a";
const TOPIC_B: &str = "arn:aws:sns:us-east-1:000000000000:anomalies-b";

/// Caller-chosen `channelId`s, each matching the model's `ChannelId` pattern
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`, so the boundary
/// validator admits them and the handler decides what they mean.
const SHARED_ID: &str = "aaaaaaaa-1111-2222-3333-444444444444";
const SECOND_ID: &str = "bbbbbbbb-1111-2222-3333-444444444444";

/// A well-formed `ChannelId` no call below ever adds.
const UNUSED_ID: &str = "cccccccc-1111-2222-3333-444444444444";

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

/// `DeleteProfilingGroup`: `DELETE /profilingGroups/{profilingGroupName}`, answered 204.
async fn delete_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "DELETE",
            &format!("/profilingGroups/{name}"),
            "application/json",
            Vec::new(),
        )
        .await
}

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

/// The channel ids a response's `notificationConfiguration` reports, in order. Panics
/// (naming the case) when the response carries no channel list at all: a configured or
/// unconfigured group reports a list either way, an empty one included.
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

/// Whether `value` has the shape the model's `ChannelId` pattern describes,
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}` — what a MINTED id
/// must look like.
fn is_channel_id(value: &str) -> bool {
    let widths = [8usize, 4, 4, 4, 12];
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == widths.len()
        && parts.iter().zip(widths).all(|(part, width)| {
            part.len() == width && part.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        })
}

/// Two groups that both hold a channel under [`SHARED_ID`], and what else the first one
/// holds: nothing, a channel whose id the service mints, or a second caller-named one.
struct ScopeCase {
    label: &'static str,
    first: &'static str,
    second: &'static str,
    /// `None`: the first group holds only the shared id. `Some(id)`: it also holds a
    /// channel on [`TOPIC_B`] whose id is the one named here, or a minted one for
    /// `Some(None)`.
    extra: Option<Option<&'static str>>,
}

fn scope_cases() -> Vec<ScopeCase> {
    vec![
        ScopeCase {
            label: "one caller-named channel in each group",
            first: "scope-plain-first",
            second: "scope-plain-second",
            extra: None,
        },
        ScopeCase {
            label: "the first group also holds a channel the service named",
            first: "scope-minted-first",
            second: "scope-minted-second",
            extra: Some(None),
        },
        ScopeCase {
            label: "the first group also holds a second caller-named channel",
            first: "scope-named-first",
            second: "scope-named-second",
            extra: Some(Some(SECOND_ID)),
        },
    ]
}

#[tokio::test]
async fn a_channel_id_names_a_channel_only_inside_its_own_group() {
    let harness = Harness::new();

    for case in scope_cases() {
        let label = case.label;
        create_group(&harness, case.first).await;
        create_group(&harness, case.second).await;

        // The SAME caller-named id in two groups: each group's configuration is its own,
        // so this is not a conflict with anything already stored.
        for group in [case.first, case.second] {
            let answer = add(&harness, group, &[channel_body(Some(SHARED_ID), TOPIC_A)]).await;
            assert_eq!(
                answer.status,
                200,
                "case {label}: adding {SHARED_ID} to {group} must succeed, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                reported_ids(label, &answer),
                vec![SHARED_ID.to_string()],
                "case {label}: {group} must report the channel it was just given"
            );
        }

        // What else the first group holds, if the row says so.
        let extra_id = match case.extra {
            None => None,
            Some(named) => {
                let answer = add(&harness, case.first, &[channel_body(named, TOPIC_B)]).await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {label}: the second channel of {} must be accepted, got {} / {}",
                    case.first,
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                let ids = reported_ids(label, &answer);
                assert_eq!(
                    ids.len(),
                    2,
                    "case {label}: {} must now hold two channels, got {ids:?}",
                    case.first
                );
                let minted = ids[1].clone();
                match named {
                    Some(id) => assert_eq!(
                        minted, id,
                        "case {label}: the second channel must keep the id the request named, \
                         got {minted}"
                    ),
                    None => assert!(
                        is_channel_id(&minted),
                        "case {label}: the second channel must carry a minted uuid channelId, \
                         got {minted}"
                    ),
                }
                Some(minted)
            }
        };

        // An id the SECOND group does not hold names nothing there, whether it is one the
        // first group holds or one nobody ever added.
        let mut foreign: Vec<(String, String)> = vec![(
            "never added".to_string(),
            UNUSED_ID.to_string(),
        )];
        if let Some(id) = extra_id.clone() {
            foreign.push(("held by the other group".to_string(), id));
        }
        for (why, id) in &foreign {
            let answer = remove(&harness, case.second, id).await;
            assert_eq!(
                answer.status,
                404,
                "case {label}: removing an id {why} from {} must answer 404, got {} / {}",
                case.second,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {label}: removing an id {why} must be ResourceNotFoundException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }

        // Removing the shared id from ONE group empties that group's configuration and
        // leaves the other group's channel of the same id exactly where it was.
        let removed = remove(&harness, case.second, SHARED_ID).await;
        assert_eq!(
            removed.status,
            200,
            "case {label}: removing {SHARED_ID} from {} must succeed, got {} / {}",
            case.second,
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        assert!(
            reported_ids(label, &removed).is_empty(),
            "case {label}: {} must be left with no channels, got {:?}",
            case.second,
            reported_ids(label, &removed)
        );

        let mut expected = vec![SHARED_ID.to_string()];
        expected.extend(extra_id.clone());
        assert_eq!(
            reported_ids(label, &get(&harness, case.first).await),
            expected,
            "case {label}: {} must still hold its own channels",
            case.first
        );

        // And the id just removed is not found in the group it was removed from, while
        // the other group still answers for it.
        let again = remove(&harness, case.second, SHARED_ID).await;
        assert_eq!(
            again.status,
            404,
            "case {label}: removing {SHARED_ID} from {} twice must answer 404, got {} / {}",
            case.second,
            again.status,
            String::from_utf8_lossy(&again.body)
        );
        let still_there = remove(&harness, case.first, SHARED_ID).await;
        assert_eq!(
            still_there.status,
            200,
            "case {label}: {} must still be able to remove its own {SHARED_ID}, got {} / {}",
            case.first,
            still_there.status,
            String::from_utf8_lossy(&still_there.body)
        );
        assert_eq!(
            reported_ids(label, &still_there),
            extra_id.clone().into_iter().collect::<Vec<String>>(),
            "case {label}: {} must be left with only its other channel",
            case.first
        );
    }
}

#[tokio::test]
async fn deleting_a_profiling_group_takes_its_notification_configuration_with_it() {
    let harness = Harness::new();
    // Each row is a different configuration to lose: one caller-named channel, or the
    // two channels the model's `Channels` list allows with one id minted.
    let rows: [(&str, &str, Vec<Option<&str>>); 2] = [
        ("one named channel", "lost-one", vec![Some(SHARED_ID)]),
        (
            "two channels, one id minted",
            "lost-two",
            vec![Some(SHARED_ID), None],
        ),
    ];

    for (case, group, ids) in rows {
        create_group(&harness, group).await;
        let channels: Vec<serde_json::Value> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                channel_body(*id, if index == 0 { TOPIC_A } else { TOPIC_B })
            })
            .collect();
        let added = add(&harness, group, &channels).await;
        assert_eq!(
            added.status,
            200,
            "case {case}: the add must succeed, got {} / {}",
            added.status,
            String::from_utf8_lossy(&added.body)
        );
        let stored = reported_ids(case, &added);
        assert_eq!(
            stored.len(),
            ids.len(),
            "case {case}: every requested channel must be stored, got {stored:?}"
        );

        let deleted = delete_group(&harness, group).await;
        assert_eq!(
            deleted.status,
            204,
            "case {case}: DeleteProfilingGroup must answer 204, got {} / {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );

        // The configuration went with the group: every notification operation on the
        // name is the modelled 404 until the group exists again.
        for (operation, answer) in [
            ("GetNotificationConfiguration", get(&harness, group).await),
            (
                "RemoveNotificationChannel",
                remove(&harness, group, &stored[0]).await,
            ),
            (
                "AddNotificationChannels",
                add(&harness, group, &[channel_body(None, TOPIC_A)]).await,
            ),
        ] {
            assert_eq!(
                answer.status,
                404,
                "case {case}: {operation} on a deleted group must answer 404, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case}: {operation} on a deleted group must be ResourceNotFoundException, \
                 got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }

        // Re-created under the same name, the group starts with an EMPTY configuration:
        // nothing the deleted group was notifying survived it.
        create_group(&harness, group).await;
        assert!(
            reported_ids(case, &get(&harness, group).await).is_empty(),
            "case {case}: a re-created group must report no channels, got {:?}",
            reported_ids(case, &get(&harness, group).await)
        );
        let orphan = remove(&harness, group, &stored[0]).await;
        assert_eq!(
            orphan.status,
            404,
            "case {case}: a channel of the deleted group must not be found in the new one, \
             got {} / {}",
            orphan.status,
            String::from_utf8_lossy(&orphan.body)
        );
        // And the name is configurable again, from scratch.
        let readded = add(&harness, group, &[channel_body(None, TOPIC_A)]).await;
        assert_eq!(
            readded.status,
            200,
            "case {case}: the re-created group must accept a channel, got {} / {}",
            readded.status,
            String::from_utf8_lossy(&readded.body)
        );
        assert_eq!(
            reported_ids(case, &readded).len(),
            1,
            "case {case}: the re-created group must hold exactly the channel just added"
        );
    }
}

#[tokio::test]
async fn every_minted_channel_id_is_distinct_across_calls_and_groups() {
    let harness = Harness::new();
    // Three groups, each given two channels the service names: one group in a single
    // two-channel call, the others one call at a time — so the ids compared below come
    // from different calls, different groups and the same call alike.
    let groups = ["minted-one", "minted-two", "minted-three"];
    let mut minted: Vec<String> = Vec::new();

    for (index, group) in groups.iter().enumerate() {
        create_group(&harness, group).await;
        let answers = if index == 0 {
            vec![
                add(
                    &harness,
                    group,
                    &[channel_body(None, TOPIC_A), channel_body(None, TOPIC_B)],
                )
                .await,
            ]
        } else {
            vec![
                add(&harness, group, &[channel_body(None, TOPIC_A)]).await,
                add(&harness, group, &[channel_body(None, TOPIC_B)]).await,
            ]
        };
        for answer in &answers {
            assert_eq!(
                answer.status,
                200,
                "case {group}: AddNotificationChannels must succeed, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
        let ids = reported_ids(group, &get(&harness, group).await);
        assert_eq!(
            ids.len(),
            2,
            "case {group}: the group must hold the two channels it was given, got {ids:?}"
        );
        for id in &ids {
            assert!(
                is_channel_id(id),
                "case {group}: a minted channelId must match the ChannelId pattern, got {id}"
            );
        }
        minted.extend(ids);
    }

    let total = minted.len();
    minted.sort_unstable();
    minted.dedup();
    assert_eq!(
        minted.len(),
        total,
        "case minted-ids: every minted channelId must be distinct, got {minted:?}"
    );
}

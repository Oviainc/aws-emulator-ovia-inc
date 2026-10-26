// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the identity of a notification channel: the SNS topic in
//! `Config.Sns.TopicArn` IS the channel, and a topic backs at most one channel.
//!
//! RULE pinned by every test here: because the target the caller names is the
//! registration's identity, registering it is IDEMPOTENT — a repeat registration of a
//! target already registered states what already holds, so it SUCCEEDS with the
//! operation's success status and reports the EXISTING resource's identifier, minting
//! nothing, storing nothing new and changing nothing, rather than being refused as a
//! creation conflict. It holds however many callers repeat it, sequentially or at the
//! same time, so N callers that each ensure one topic is registered all come away with
//! the one id. Freeing the identity (here, removing the channel) makes the next
//! registration mint a fresh one, and a registration for a free identity is unaffected.
//!
//! Every test drives the service the way a client does — a buffered HTTP request
//! through the same `MultiServiceRouter` the emulator boots — and exercises the rule
//! over a SET of inputs; the re-add from the issue report is one row. The harness
//! ([`crate::configuration_lifecycle::Emulator`]) and the channel helpers
//! ([`crate::mutation_idempotency_and_tenancy`]) are the ones the sibling suites
//! already use.

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::configuration_lifecycle::{is_modeled_channel_id, Emulator};
use crate::mutation_idempotency_and_tenancy::{channel_config, listed_channel_ids, minted_id};

/// The channels `ListNotificationChannels` reports, as (Id, Config) pairs, walking its
/// pages — the whole store, so a test can assert a repeat add changed NOTHING.
async fn listed_channels(emulator: &Emulator) -> Vec<(String, Value)> {
    let mut channels = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let reply = emulator.list_notification_channels(token.as_deref()).await;
        assert_eq!(
            reply.status, 200,
            "listing the channels must succeed, got {}",
            reply.raw
        );
        if let Some(page) = reply.at("/Channels").and_then(Value::as_array) {
            channels.extend(page.iter().map(|channel| {
                (
                    channel
                        .get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    channel.get("Config").cloned().unwrap_or(Value::Null),
                )
            }));
        }
        token = reply
            .at("/NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return channels;
        }
    }
}

/// One repeated registration: what was registered first and what the repeat carries.
struct RepeatCase<'a> {
    name: &'a str,
    topic_arn: &'a str,
    /// `Config.Filters` of the registration already in place (`null` for none).
    registered_filters: Value,
    /// `Config.Filters` the repeated add carries (`null` for none).
    repeated_filters: Value,
    /// How many times the add is repeated — each must answer alike.
    attempts: usize,
    /// Another topic registered first, which the repeat must leave alone.
    bystander_topic: Option<&'a str>,
}

/// RULE: a topic may back at most one notification channel and is that channel's
/// identity, so an `AddNotificationChannel` whose `Config.Sns.TopicArn` already backs a
/// channel succeeds with 200 reporting THAT channel's `Id`, and mints nothing, stores
/// nothing new, changes nothing and removes nothing.
///
/// The rows differ in the form of the topic ARN (region, account, name length), in the
/// form of `Config.Filters` on both the registered and the repeated request (absent,
/// one list, both lists, empty lists, changed), in how many times the add is repeated,
/// and in whether another channel exists alongside; the issue's topic added twice is
/// the first row.
#[tokio::test]
async fn re_adding_a_registered_topic_reports_the_same_channel() {
    let both_lists = json!({ "Severities": ["HIGH", "MEDIUM"], "MessageTypes": ["NEW_INSIGHT"] });
    let cases = [
        RepeatCase {
            name: "the reported topic, added twice with the same config",
            topic_arn: "arn:aws:sns:us-east-1:123456789012:guru-alerts",
            registered_filters: json!(null),
            repeated_filters: json!(null),
            attempts: 1,
            bystander_topic: None,
        },
        RepeatCase {
            name: "a repeat that adds filters, three times",
            topic_arn: "arn:aws:sns:us-west-2:000000000000:sim2real-devopsguru-topic-388962c1",
            registered_filters: json!(null),
            repeated_filters: both_lists.clone(),
            attempts: 3,
            bystander_topic: None,
        },
        RepeatCase {
            name: "a repeat that drops the filters, beside another channel",
            topic_arn: "arn:aws:sns:eu-west-1:210987654321:guru-eu-alerts",
            registered_filters: both_lists.clone(),
            repeated_filters: json!(null),
            attempts: 2,
            bystander_topic: Some("arn:aws:sns:eu-west-1:210987654321:guru-eu-bystander"),
        },
        RepeatCase {
            name: "a repeat that changes the filters",
            topic_arn: "arn:aws:sns:ap-southeast-2:000000000000:guru-filtered-topic",
            registered_filters: both_lists.clone(),
            repeated_filters: json!({ "Severities": ["LOW"] }),
            attempts: 1,
            bystander_topic: Some("arn:aws:sns:us-east-1:000000000000:guru-other-region"),
        },
        RepeatCase {
            name: "a one-character topic name, repeated with empty filter lists",
            topic_arn: "arn:aws:sns:us-east-2:000000000000:g",
            registered_filters: json!({ "MessageTypes": ["CLOSED_INSIGHT"] }),
            repeated_filters: json!({ "Severities": [], "MessageTypes": [] }),
            attempts: 4,
            bystander_topic: None,
        },
    ];

    for case in cases {
        let emulator = Emulator::new();

        let mut expected: Vec<(String, Value)> = Vec::new();
        if let Some(bystander) = case.bystander_topic {
            let config = channel_config(bystander, &json!(null));
            let reply = emulator.add_notification_channel(config.clone()).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: the bystander add must succeed, got status {} body {}",
                case.name, reply.status, reply.raw
            );
            expected.push((minted_id(&reply), config));
        }

        let registered_config = channel_config(case.topic_arn, &case.registered_filters);
        let first = emulator
            .add_notification_channel(registered_config.clone())
            .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first add must succeed, got status {} body {}",
            case.name, first.status, first.raw
        );
        let id = minted_id(&first);
        assert!(
            is_modeled_channel_id(&id),
            "case {:?}: expected a 36-char lowercase-hex channel id, got {id:?}",
            case.name
        );
        expected.push((id.clone(), registered_config));
        expected.sort_by(|left, right| left.0.cmp(&right.0));

        let repeat_config = channel_config(case.topic_arn, &case.repeated_filters);
        for attempt in 0..case.attempts {
            let repeat = emulator
                .add_notification_channel(repeat_config.clone())
                .await;
            assert_eq!(
                repeat.status, 200,
                "case {:?}: re-add {attempt} must succeed, got status {} body {}",
                case.name, repeat.status, repeat.raw
            );
            assert_eq!(
                repeat.at("/Id").and_then(Value::as_str),
                Some(id.as_str()),
                "case {:?}: re-add {attempt} must report the registered channel {id:?}, got {}",
                case.name,
                repeat.raw
            );
            // The repeat is a success, so it names no error at all — the conflict the
            // model still declares for the operation is not what a repeat answers.
            assert!(
                !repeat.names_error("ConflictException"),
                "case {:?}: re-add {attempt} must not report a ConflictException, got {}",
                case.name,
                repeat.raw
            );
        }

        // The repeats changed nothing: the same channels, one per topic, each still
        // carrying the config the registration that created it submitted.
        let mut after = listed_channels(&emulator).await;
        after.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            after, expected,
            "case {:?}: expected the store unchanged as {expected:?}, got {after:?}",
            case.name
        );

        // The rule is keyed on the TOPIC, not on the operation: a free topic still
        // mints a channel of its own.
        let free = emulator
            .add_notification_channel(channel_config(
                "arn:aws:sns:us-east-1:000000000000:guru-free-topic",
                &case.repeated_filters,
            ))
            .await;
        assert_eq!(
            free.status, 200,
            "case {:?}: adding an unregistered topic must succeed, got status {} body {}",
            case.name, free.status, free.raw
        );
        let free_id = minted_id(&free);
        assert!(
            !expected.iter().any(|(known, _)| *known == free_id),
            "case {:?}: a different topic must mint its own id, got the known {free_id:?}",
            case.name
        );
    }
}

/// RULE (the same identity, released): the registration a repeat resolves to lasts
/// exactly as long as the channel does — once `RemoveNotificationChannel` frees the
/// topic, adding it again mints a FRESH id, and repeats then resolve to that one.
///
/// The rows differ in how many register/free cycles the topic goes through and in the
/// form of `Config.Filters` each cycle carries.
#[tokio::test]
async fn freeing_the_topic_mints_a_fresh_channel_for_it() {
    let cases: [(&str, usize, Value); 3] = [
        ("one cycle, no filters", 1, json!(null)),
        (
            "two cycles, severities",
            2,
            json!({ "Severities": ["HIGH"] }),
        ),
        (
            "three cycles, message types",
            3,
            json!({ "MessageTypes": ["SEVERITY_UPGRADED"] }),
        ),
    ];

    for (name, cycles, filters) in cases {
        let emulator = Emulator::new();
        let topic_arn = "arn:aws:sns:us-east-1:000000000000:guru-recycled";
        let config = channel_config(topic_arn, &filters);
        let mut seen: BTreeSet<String> = BTreeSet::new();

        for cycle in 0..cycles {
            let added = emulator.add_notification_channel(config.clone()).await;
            assert_eq!(
                added.status, 200,
                "case {name:?}: cycle {cycle}'s add must succeed, got status {} body {}",
                added.status, added.raw
            );
            let id = minted_id(&added);
            assert!(
                seen.insert(id.clone()),
                "case {name:?}: cycle {cycle} reused the id {id:?} of a removed channel"
            );

            let repeat = emulator.add_notification_channel(config.clone()).await;
            assert_eq!(
                repeat.status, 200,
                "case {name:?}: cycle {cycle}'s re-add must succeed, got status {} body {}",
                repeat.status, repeat.raw
            );
            assert_eq!(
                repeat.at("/Id").and_then(Value::as_str),
                Some(id.as_str()),
                "case {name:?}: cycle {cycle} must report the live channel {id:?}, got {}",
                repeat.raw
            );

            let removed = emulator.remove_notification_channel(&id).await;
            assert_eq!(
                removed.status, 200,
                "case {name:?}: cycle {cycle}'s removal must succeed, got {}",
                removed.raw
            );
            let remaining = listed_channel_ids(&emulator).await;
            assert!(
                remaining.is_empty(),
                "case {name:?}: cycle {cycle} left {remaining:?} behind"
            );
        }
    }
}

/// How many callers race to register one topic. More than two, so the test is about ANY
/// number of concurrent registrations rather than a single pair.
const RACERS: usize = 6;

/// RULE (the same identity, reached concurrently): N callers registering one topic at
/// the same time ALL succeed and ALL report the same `Id`, and exactly one channel
/// exists afterwards — the lookup and the insert happen under one lock, so the race
/// collapses onto a single registration instead of one caller winning it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_concurrent_registration_of_one_topic_reports_the_same_channel() {
    let emulator = Arc::new(Emulator::new());
    let topic_arn = "arn:aws:sns:us-west-2:000000000000:sim2real-devopsguru-topic-race";

    let mut racers = Vec::new();
    for racer in 0..RACERS {
        let emulator = Arc::clone(&emulator);
        let config = channel_config(topic_arn, &json!(null));
        racers.push(tokio::spawn(async move {
            (racer, emulator.add_notification_channel(config).await)
        }));
    }

    let mut reported: Vec<String> = Vec::new();
    for racer in racers {
        let (index, reply) = racer.await.expect("the racing add task ran");
        assert_eq!(
            reply.status, 200,
            "racer {index}: every concurrent add must succeed, got status {} body {}",
            reply.status, reply.raw
        );
        let id = minted_id(&reply);
        assert!(
            is_modeled_channel_id(&id),
            "racer {index}: expected a modelled channel id, got {id:?}"
        );
        reported.push(id);
    }

    let distinct: BTreeSet<String> = reported.iter().cloned().collect();
    assert_eq!(
        distinct.len(),
        1,
        "expected all {RACERS} racers to report one id, got {reported:?}"
    );

    let listed = listed_channel_ids(&emulator).await;
    let expected: Vec<String> = distinct.into_iter().collect();
    assert_eq!(
        listed, expected,
        "expected exactly the one registered channel {expected:?} after the race, got {listed:?}"
    );
}

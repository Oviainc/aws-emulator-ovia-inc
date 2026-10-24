// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the identity of a notification channel: the SNS topic in
//! `Config.Sns.TopicArn` IS the channel, and a topic backs at most one channel.
//!
//! RULE pinned by every test here: a create whose target identity is already taken
//! cannot be carried out, so it is REFUSED with the modelled conflict error for the
//! operation (`ConflictException`, HTTP 409) and the store is left exactly as it was —
//! never answered as if the create had happened. Freeing the identity (here, removing
//! the channel) makes the create succeed again, and a create for a free identity is
//! unaffected.
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
/// pages — the whole store, so a test can assert a refusal changed NOTHING.
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

/// One conflicting registration: what was registered first and what the repeat carries.
struct ConflictCase<'a> {
    name: &'a str,
    topic_arn: &'a str,
    /// `Config.Filters` of the registration already in place (`null` for none).
    registered_filters: Value,
    /// `Config.Filters` the conflicting add carries (`null` for none).
    repeated_filters: Value,
    /// How many times the conflicting add is repeated — each must be refused alike.
    attempts: usize,
    /// Another topic registered first, which the conflict must leave alone.
    bystander_topic: Option<&'a str>,
}

/// RULE: a topic may back at most one notification channel, so an
/// `AddNotificationChannel` whose `Config.Sns.TopicArn` already backs a channel is
/// refused with `ConflictException` (HTTP 409) — naming the channel the conflict is in
/// — and mints nothing, changes nothing and removes nothing.
///
/// The rows differ in the form of the topic ARN (region, account, name length), in the
/// form of `Config.Filters` on both the registered and the conflicting request
/// (absent, one list, both lists, empty lists, changed), in how many times the
/// conflicting add is repeated, and in whether another channel exists alongside; the
/// issue's `us-east-1` topic added twice is the first row.
#[tokio::test]
async fn re_adding_a_registered_topic_is_a_conflict() {
    let both_lists = json!({ "Severities": ["HIGH", "MEDIUM"], "MessageTypes": ["NEW_INSIGHT"] });
    let cases = [
        ConflictCase {
            name: "the reported topic, added twice with the same config",
            topic_arn: "arn:aws:sns:us-east-1:123456789012:guru-alerts",
            registered_filters: json!(null),
            repeated_filters: json!(null),
            attempts: 1,
            bystander_topic: None,
        },
        ConflictCase {
            name: "a repeat that adds filters, three times",
            topic_arn: "arn:aws:sns:us-west-2:000000000000:sim2real-devopsguru-topic-388962c1",
            registered_filters: json!(null),
            repeated_filters: both_lists.clone(),
            attempts: 3,
            bystander_topic: None,
        },
        ConflictCase {
            name: "a repeat that drops the filters, beside another channel",
            topic_arn: "arn:aws:sns:eu-west-1:210987654321:guru-eu-alerts",
            registered_filters: both_lists.clone(),
            repeated_filters: json!(null),
            attempts: 2,
            bystander_topic: Some("arn:aws:sns:eu-west-1:210987654321:guru-eu-bystander"),
        },
        ConflictCase {
            name: "a repeat that changes the filters",
            topic_arn: "arn:aws:sns:ap-southeast-2:000000000000:guru-filtered-topic",
            registered_filters: both_lists.clone(),
            repeated_filters: json!({ "Severities": ["LOW"] }),
            attempts: 1,
            bystander_topic: Some("arn:aws:sns:us-east-1:000000000000:guru-other-region"),
        },
        ConflictCase {
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
                repeat.status, 409,
                "case {:?}: re-add {attempt} must be refused with 409, got status {} body {}",
                case.name, repeat.status, repeat.raw
            );
            assert!(
                repeat.names_error("ConflictException"),
                "case {:?}: re-add {attempt} must name a ConflictException, got {}",
                case.name,
                repeat.raw
            );
            assert!(
                repeat.at("/Id").is_none(),
                "case {:?}: a refused re-add must report no Id, got {}",
                case.name,
                repeat.raw
            );
            // `ConflictException` requires Message, ResourceId and ResourceType
            // (smithy_query(query="shape",shape="ConflictException")); the conflict is
            // in the channel that already backs the topic, so it is that channel the
            // error identifies.
            assert_eq!(
                repeat.at("/ResourceId").and_then(Value::as_str),
                Some(id.as_str()),
                "case {:?}: expected the conflicting channel {id:?} in ResourceId, got {}",
                case.name,
                repeat.raw
            );
            for member in ["/Message", "/ResourceType"] {
                let value = repeat.at(member).and_then(Value::as_str).unwrap_or_default();
                assert!(
                    !value.is_empty(),
                    "case {:?}: expected a non-empty {member} on the conflict, got {}",
                    case.name,
                    repeat.raw
                );
            }
        }

        // The refusal changed nothing: the same channels, each still carrying the
        // config it was registered with.
        let mut after = listed_channels(&emulator).await;
        after.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            after, expected,
            "case {:?}: expected the store unchanged as {expected:?}, got {after:?}",
            case.name
        );

        // The rule is keyed on the TOPIC, not on the operation: a free topic still
        // mints its own channel.
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

/// RULE (the same identity, released): the conflict lasts exactly as long as the
/// registration does — once `RemoveNotificationChannel` frees the topic, adding it
/// again succeeds and mints a fresh id, and THAT registration conflicts in its turn.
///
/// The rows differ in how many register/free cycles the topic goes through and in the
/// form of `Config.Filters` each cycle carries.
#[tokio::test]
async fn freeing_the_topic_lets_it_be_registered_again() {
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

            let conflict = emulator.add_notification_channel(config.clone()).await;
            assert_eq!(
                conflict.status, 409,
                "case {name:?}: cycle {cycle}'s re-add must be refused with 409, got status {} body {}",
                conflict.status, conflict.raw
            );
            assert_eq!(
                conflict.at("/ResourceId").and_then(Value::as_str),
                Some(id.as_str()),
                "case {name:?}: cycle {cycle} must name the live channel {id:?}, got {}",
                conflict.raw
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

/// RULE (the same identity, reached concurrently): exactly ONE of N callers registering
/// one topic at the same time creates the channel; every other is refused with the
/// conflict, and exactly one channel exists afterwards — the id the winner reported.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_one_of_many_concurrent_registrations_creates_the_channel() {
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

    let mut created: Vec<String> = Vec::new();
    let mut refused = 0usize;
    for racer in racers {
        let (index, reply) = racer.await.expect("the racing add task ran");
        match reply.status {
            200 => created.push(minted_id(&reply)),
            409 => {
                assert!(
                    reply.names_error("ConflictException"),
                    "racer {index}: a refused concurrent add must name a ConflictException, got {}",
                    reply.raw
                );
                refused += 1;
            }
            status => panic!(
                "racer {index}: expected 200 or 409 for a concurrent add, got status {status} body {}",
                reply.raw
            ),
        }
    }

    assert_eq!(
        created.len(),
        1,
        "expected exactly one of {RACERS} racers to create the channel, got {created:?}"
    );
    assert_eq!(
        refused,
        RACERS - 1,
        "expected the other {} racers to be refused, got {refused}",
        RACERS - 1
    );

    let listed = listed_channel_ids(&emulator).await;
    assert_eq!(
        listed, created,
        "expected exactly the created channel {created:?} after the race, got {listed:?}"
    );
}

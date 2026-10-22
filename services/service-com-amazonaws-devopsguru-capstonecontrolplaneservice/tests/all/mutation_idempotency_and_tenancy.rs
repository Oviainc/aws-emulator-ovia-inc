// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rules a MUTATION of this service's state follows:
//!
//! * a mutation keyed by a caller-named target is IDEMPOTENT — registering a target
//!   already registered reports the existing resource's id, and removing one already
//!   absent succeeds;
//! * a write is REFUSED with the modelled client error, and persists nothing, when the
//!   configuration it would leave stored is one the service cannot represent — a second
//!   app-boundary key for an account that has exactly one, or customer-managed-key
//!   encryption opted in with no key designated.
//!
//! Every test drives the service the way a client does — a buffered HTTP request
//! through the same `MultiServiceRouter` the emulator boots — and states the rule it
//! pins, then exercises it over a SET of inputs; the request from the issue report is
//! one row. The harness ([`crate::configuration_lifecycle::Emulator`]) is the one the
//! configuration-lifecycle suite already uses.
//!
//! The third rule this file is named for — a read scoped to an `AccountId` the caller
//! cannot reach is refused with `AccessDeniedException` before any lookup — is pinned by
//! [`crate::insight_reads`], the suite that came with `DescribeInsight`'s
//! implementation; it is not repeated here.

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::configuration_lifecycle::{
    cloud_formation, is_modeled_channel_id, reported_tag_collections, tags, Emulator, Reply,
};

/// `AddNotificationChannel` input for one SNS topic, with `Config.Filters` only when
/// `filters` is not null.
fn channel_config(topic_arn: &str, filters: &Value) -> Value {
    let mut config = json!({ "Sns": { "TopicArn": topic_arn } });
    if !filters.is_null() {
        config["Filters"] = filters.clone();
    }
    config
}

/// The ids `ListNotificationChannels` reports, walking its pages.
async fn listed_channel_ids(emulator: &Emulator) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let reply = emulator.list_notification_channels(token.as_deref()).await;
        assert_eq!(
            reply.status, 200,
            "listing the channels must succeed, got {}",
            reply.raw
        );
        ids.extend(
            reply
                .at("/Channels")
                .and_then(Value::as_array)
                .map(|channels| {
                    channels
                        .iter()
                        .map(|channel| {
                            channel
                                .get("Id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default(),
        );
        token = reply
            .at("/NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return ids;
        }
    }
}

/// The `Id` an add reported.
fn minted_id(reply: &Reply) -> String {
    reply
        .at("/Id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: a registration keyed by its target is idempotent — the SNS topic IS the
/// channel, so an `AddNotificationChannel` for a topic that already backs a channel
/// reports that channel's id, mints no second channel and changes nothing, however many
/// times it is repeated; a DIFFERENT topic still mints its own channel.
///
/// The rows differ in how many times the registration is repeated (1, 2, 3 and 6), in
/// the form of `Config.Filters` (absent, one list, both lists) and in the form of the
/// topic ARN (region, account, name); the issue's `us-west-2` topic added twice is the
/// first row.
#[tokio::test]
async fn registering_an_already_registered_topic_reports_the_one_channel() {
    let cases: [(&str, &str, Value, usize); 5] = [
        (
            "the reported topic, added twice",
            "arn:aws:sns:us-west-2:000000000000:sim2real-devopsguru-topic-388962c1a961",
            json!(null),
            2,
        ),
        (
            "a single add mints one channel",
            "arn:aws:sns:us-east-1:123456789012:guru-alerts",
            json!(null),
            1,
        ),
        (
            "a topic in another region, three adds",
            "arn:aws:sns:eu-west-1:210987654321:guru-eu-alerts",
            json!({ "Severities": ["HIGH"] }),
            3,
        ),
        (
            "a filtered topic, six adds",
            "arn:aws:sns:ap-southeast-2:000000000000:guru-filtered-topic",
            json!({ "Severities": ["HIGH", "MEDIUM"], "MessageTypes": ["NEW_INSIGHT"] }),
            6,
        ),
        (
            "a short topic name, two adds",
            "arn:aws:sns:us-east-2:000000000000:g",
            json!({ "MessageTypes": ["CLOSED_INSIGHT"] }),
            2,
        ),
    ];

    for (name, topic_arn, filters, repeats) in cases {
        let emulator = Emulator::new();
        let config = channel_config(topic_arn, &filters);

        let mut ids: Vec<String> = Vec::new();
        for attempt in 0..repeats {
            let reply = emulator.add_notification_channel(config.clone()).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}: add {attempt} must succeed, got status {} body {}",
                reply.status, reply.raw
            );
            let id = minted_id(&reply);
            assert!(
                is_modeled_channel_id(&id),
                "case {name:?}: add {attempt} must report a 36-char lowercase-hex id, got {id:?}"
            );
            ids.push(id);
        }

        let unique: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            1,
            "case {name:?}: expected every add to report one shared id, got {ids:?}"
        );

        // Exactly one channel exists for the topic, carrying the config submitted.
        let listed = emulator.list_notification_channels(None).await;
        assert_eq!(
            listed.at("/Channels").cloned().unwrap_or(Value::Null),
            json!([{ "Id": ids[0], "Config": config }]),
            "case {name:?}: expected exactly one channel for the topic, got {}",
            listed.raw
        );

        // Idempotency is keyed by the TARGET, not by the operation: another topic is
        // another channel, with an id of its own.
        let other = emulator
            .add_notification_channel(channel_config(
                "arn:aws:sns:us-east-1:000000000000:guru-second-topic",
                &filters,
            ))
            .await;
        assert_eq!(
            other.status, 200,
            "case {name:?}: adding a second topic must succeed, got {}",
            other.raw
        );
        let other_id = minted_id(&other);
        assert_ne!(
            other_id, ids[0],
            "case {name:?}: a different topic must mint its own id, got {other_id:?} twice"
        );
        let after = listed_channel_ids(&emulator).await;
        assert_eq!(
            after.len(),
            2,
            "case {name:?}: expected two channels for two topics, got {after:?}"
        );
    }
}

/// How many callers race to register one topic. The reported race is 3; this goes
/// further, because the rule is about ANY number of concurrent registrations.
const RACERS: usize = 6;

/// RULE (the same idempotency, reached concurrently): N callers registering ONE topic at
/// the same time all succeed and all get the same single channel id, and exactly one
/// channel exists afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_registrations_of_one_topic_all_report_one_id() {
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

    let mut ids: Vec<String> = Vec::new();
    for racer in racers {
        let (index, reply) = racer.await.expect("the racing add task ran");
        assert_eq!(
            reply.status, 200,
            "racer {index}: every concurrent add of one topic must succeed, got status {} body {}",
            reply.status, reply.raw
        );
        ids.push(minted_id(&reply));
    }

    let unique: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        1,
        "expected all {RACERS} racers to share one id, got {ids:?}"
    );

    let listed = listed_channel_ids(&emulator).await;
    assert_eq!(
        listed,
        vec![ids[0].clone()],
        "expected exactly one channel after the race, got {listed:?}"
    );
}

/// RULE: a removal keyed by its target is idempotent — `RemoveNotificationChannel`
/// states that the id names no channel, which already holds for an id that names none,
/// so such a call succeeds with the empty output and leaves the other channels alone.
///
/// The rows differ in what the id names (never minted, minted and removed, removed
/// repeatedly), in how many removals are issued, and in whether another channel exists
/// alongside; the issue's "remove the same id twice" is the second row.
#[tokio::test]
async fn removing_a_channel_is_idempotent() {
    // (case name, mint the channel first?, how many removals, keep a second channel?)
    let cases: [(&str, bool, usize, bool); 5] = [
        ("an id that was never minted", false, 1, false),
        ("a minted id removed twice", true, 2, false),
        ("a minted id removed four times", true, 4, false),
        ("an id never minted, removed three times", false, 3, false),
        ("a minted id removed twice beside another channel", true, 2, true),
    ];

    for (name, mint, removals, keep_other) in cases {
        let emulator = Emulator::new();

        let other_id = if keep_other {
            let reply = emulator
                .add_notification_channel(channel_config(
                    "arn:aws:sns:us-east-1:000000000000:guru-keep-me",
                    &json!(null),
                ))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}: the bystander add must succeed, got {}",
                reply.raw
            );
            Some(minted_id(&reply))
        } else {
            None
        };

        let id = if mint {
            let reply = emulator
                .add_notification_channel(channel_config(
                    "arn:aws:sns:us-east-1:000000000000:guru-doomed",
                    &json!(null),
                ))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}: the add must succeed, got {}",
                reply.raw
            );
            minted_id(&reply)
        } else {
            // A well-formed `NotificationChannelId` this service never minted.
            "00000000-0000-4000-8000-000000000000".to_string()
        };

        for attempt in 0..removals {
            let reply = emulator.remove_notification_channel(&id).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}: removal {attempt} must succeed, got status {} body {}",
                reply.status, reply.raw
            );
            assert_eq!(
                reply.body,
                json!({}),
                "case {name:?}: removal {attempt} must report the empty output, got {}",
                reply.raw
            );
        }

        let remaining = listed_channel_ids(&emulator).await;
        let expected: Vec<String> = other_id.iter().cloned().collect();
        assert_eq!(
            remaining, expected,
            "case {name:?}: expected {expected:?} to remain, got {remaining:?}"
        );

        // Removal frees the topic: the same topic may be registered again, and gets a
        // channel of its own, while the removed id still names nothing.
        let reused = emulator
            .add_notification_channel(channel_config(
                "arn:aws:sns:us-east-1:000000000000:guru-doomed",
                &json!(null),
            ))
            .await;
        assert_eq!(
            reused.status, 200,
            "case {name:?}: re-adding the freed topic must succeed, got {}",
            reused.raw
        );
        let reused_id = minted_id(&reused);
        assert!(
            is_modeled_channel_id(&reused_id),
            "case {name:?}: expected a modelled id for the re-added topic, got {reused_id:?}"
        );
        if mint {
            assert_ne!(
                reused_id, id,
                "case {name:?}: a re-added topic must get a fresh id, got the removed {id:?}"
            );
        }
    }
}

/// One tag filter: an `AppBoundaryKey` and its `TagValues`.
type TagEntry<'a> = (&'a str, &'a [&'a str]);

struct BoundaryCase<'a> {
    name: &'a str,
    /// Requests applied first, each of which must succeed: (Action, tag filters).
    given: &'a [(&'a str, &'a [TagEntry<'a>])],
    /// The request under test: (Action, tag filters, CloudFormation stack names).
    when: (&'a str, &'a [TagEntry<'a>], &'a [&'a str]),
    expect_status: u16,
    /// What `GetResourceCollection` must report afterwards.
    expect_tags: &'a [TagEntry<'a>],
    expect_stacks: &'a [&'a str],
}

/// An `UpdateResourceCollectionFilter` carrying the members the row names, composed
/// from the request shapes the configuration-lifecycle suite already defines.
fn collection(tag_entries: &[TagEntry<'_>], stacks: &[&str]) -> Value {
    let mut filter = json!({});
    if !stacks.is_empty() {
        filter["CloudFormation"] = cloud_formation(stacks)["CloudFormation"].clone();
    }
    if !tag_entries.is_empty() {
        filter["Tags"] = tags(tag_entries)["Tags"].clone();
    }
    filter
}

/// RULE: an account's tag coverage has exactly ONE app-boundary key — the key whose
/// tagged resources are its DevOps Guru analysis boundary (`TagCollection.AppBoundaryKey`
/// doc) — so an `ADD` whose tag filters would leave the account covering a second,
/// different key is refused with `ValidationException` (400) and persists NOTHING, while
/// adding values under the configured key stays additive.
///
/// The rows differ in HOW a second key would arrive (a later request, two filters in one
/// request, a key differing only in case), in whether a key is configured at all, in the
/// `Action`, and in what else the refused request carried; the issue's
/// `Devops-guru-a` then `Devops-guru-b` is the first row.
#[tokio::test]
async fn an_account_covers_exactly_one_app_boundary_key() {
    let cases = [
        BoundaryCase {
            name: "a second, different key while one is configured",
            given: &[("ADD", &[("Devops-guru-a", &["v1"])])],
            when: ("ADD", &[("Devops-guru-b", &["v1"])], &[]),
            expect_status: 400,
            expect_tags: &[("Devops-guru-a", &["v1"])],
            expect_stacks: &[],
        },
        BoundaryCase {
            name: "a refused ADD persists nothing it carried, stacks included",
            given: &[("ADD", &[("Devops-guru-a", &["v1"])])],
            when: ("ADD", &[("Devops-guru-b", &["v1"])], &["late-stack"]),
            expect_status: 400,
            expect_tags: &[("Devops-guru-a", &["v1"])],
            expect_stacks: &[],
        },
        BoundaryCase {
            name: "two different keys in one request, none configured",
            given: &[],
            when: (
                "ADD",
                &[("Devops-guru-a", &["v1"]), ("Devops-guru-b", &["v1"])],
                &[],
            ),
            expect_status: 400,
            expect_tags: &[],
            expect_stacks: &[],
        },
        BoundaryCase {
            // The AppBoundaryKey doc: "After you create a key, it is case-sensitive ...
            // DevOps Guru works with a key named devops-guru-rds and a key named
            // DevOps-Guru-RDS, and these act as two different keys."
            name: "a key differing only in case is a different key",
            given: &[("ADD", &[("Devops-guru-app", &["prod"])])],
            when: ("ADD", &[("devops-guru-app", &["prod"])], &[]),
            expect_status: 400,
            expect_tags: &[("Devops-guru-app", &["prod"])],
            expect_stacks: &[],
        },
        BoundaryCase {
            name: "more values under the configured key are additive",
            given: &[("ADD", &[("Devops-guru-app", &["prod"])])],
            when: ("ADD", &[("Devops-guru-app", &["test", "stage"])], &[]),
            expect_status: 200,
            expect_tags: &[("Devops-guru-app", &["prod", "stage", "test"])],
            expect_stacks: &[],
        },
        BoundaryCase {
            name: "the same key repeated in one request",
            given: &[],
            when: (
                "ADD",
                &[("Devops-guru-app", &["prod"]), ("Devops-guru-app", &["dev"])],
                &["my-stack"],
            ),
            expect_status: 200,
            expect_tags: &[("Devops-guru-app", &["dev", "prod"])],
            expect_stacks: &["my-stack"],
        },
        BoundaryCase {
            name: "a different key once the configured one is uncovered",
            given: &[
                ("ADD", &[("Devops-guru-app", &["prod"])]),
                ("REMOVE", &[("Devops-guru-app", &["prod"])]),
            ],
            when: ("ADD", &[("Devops-guru-rds", &["prod"])], &[]),
            expect_status: 200,
            expect_tags: &[("Devops-guru-rds", &["prod"])],
            expect_stacks: &[],
        },
        BoundaryCase {
            name: "a REMOVE naming another key subtracts nothing and is not refused",
            given: &[("ADD", &[("Devops-guru-app", &["prod"])])],
            when: ("REMOVE", &[("Devops-guru-rds", &["prod"])], &[]),
            expect_status: 200,
            expect_tags: &[("Devops-guru-app", &["prod"])],
            expect_stacks: &[],
        },
        BoundaryCase {
            name: "an ADD carrying only stacks leaves the key alone",
            given: &[("ADD", &[("Devops-guru-app", &["prod"])])],
            when: ("ADD", &[], &["my-stack", "other-stack"]),
            expect_status: 200,
            expect_tags: &[("Devops-guru-app", &["prod"])],
            expect_stacks: &["my-stack", "other-stack"],
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        for (action, tag_entries) in case.given {
            let reply = emulator
                .update_resource_collection(action, collection(tag_entries, &[]))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: the setup {action} must succeed, got status {} body {}",
                case.name, reply.status, reply.raw
            );
        }

        let (action, tag_entries, stacks) = case.when;
        let reply = emulator
            .update_resource_collection(action, collection(tag_entries, stacks))
            .await;
        assert_eq!(
            reply.status, case.expect_status,
            "case {:?}: expected status {}, got status {} body {}",
            case.name, case.expect_status, reply.status, reply.raw
        );
        if case.expect_status == 400 {
            assert!(
                reply.names_error("ValidationException"),
                "case {:?}: expected a ValidationException, got {}",
                case.name,
                reply.raw
            );
            // The refusal names the keys it is about: the configured ones and the
            // request's own.
            let message = reply
                .at("/Message")
                .or_else(|| reply.at("/message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            for (key, _) in tag_entries {
                assert!(
                    message.contains(key),
                    "case {:?}: expected the refusal to name the provided key {key:?}, got {message:?}",
                    case.name
                );
            }
        }

        let actual_tags = reported_tag_collections(
            &emulator.get_resource_collection("AWS_TAGS", None).await,
        );
        let expect_tags: Vec<(String, Vec<String>)> = case
            .expect_tags
            .iter()
            .map(|(key, values)| {
                (
                    (*key).to_string(),
                    values.iter().map(|value| (*value).to_string()).collect(),
                )
            })
            .collect();
        assert_eq!(
            actual_tags, expect_tags,
            "case {:?}: expected tag coverage {expect_tags:?}, got {actual_tags:?}",
            case.name
        );

        let actual_stacks = emulator.all_stack_names().await;
        assert_eq!(
            actual_stacks, case.expect_stacks,
            "case {:?}: expected stack coverage {:?}, got {actual_stacks:?}",
            case.name, case.expect_stacks
        );
    }
}

struct KmsCase<'a> {
    name: &'a str,
    /// `UpdateServiceIntegration` requests applied first, each of which must succeed.
    given: Vec<Value>,
    /// The `ServiceIntegration` under test.
    when: Value,
    expect_status: u16,
    /// The whole `ServiceIntegrationConfig` `DescribeServiceIntegration` must report
    /// afterwards, so a row also pins what a refusal left alone.
    expect: Value,
}

/// RULE: a write is refused with the modelled client error, and leaves the stored
/// configuration exactly as it was, when the configuration it would RESULT in is one the
/// service cannot represent — here, KMS encryption that is opted in with
/// `Type=CUSTOMER_MANAGED_KEY` and no `KMSKeyId`, since `KMSKeyId` is what designates a
/// customer-supplied key. Whether the result is representable is decided on the MERGE of
/// the request onto what is stored, because every member of
/// `UpdateServiceIntegrationConfig` is optional.
///
/// The rows span the configuration space the rule is about: both
/// `ServerSideEncryptionType` values and both `OptInStatus` values, a key supplied and
/// absent, the three `KMSKeyId` forms the model documents (key id, key ARN, alias), a
/// key supplied by an EARLIER request, and a refused request that also carried an
/// unrelated integration. The issue's `{ENABLED, CUSTOMER_MANAGED_KEY}` with no key is
/// the first row.
#[tokio::test]
async fn an_opted_in_customer_managed_key_must_designate_a_key() {
    let opted_out = json!({
        "OpsCenter": { "OptInStatus": "DISABLED" },
        "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
        "KMSServerSideEncryption": { "OptInStatus": "DISABLED" },
    });
    // The `KMSKeyId` forms from its model documentation: key id, key ARN, alias name,
    // alias ARN.
    let key_id = "1234abcd-12ab-34cd-56ef-1234567890ab";
    let key_arn = "arn:aws:kms:us-east-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";
    let alias_name = "alias/ExampleAlias";
    let alias_arn = "arn:aws:kms:us-east-2:111122223333:alias/ExampleAlias";

    let cases = vec![
        KmsCase {
            name: "opted in to a customer-managed key with no key id",
            given: vec![],
            when: json!({
                "KMSServerSideEncryption": { "OptInStatus": "ENABLED", "Type": "CUSTOMER_MANAGED_KEY" }
            }),
            expect_status: 400,
            expect: opted_out.clone(),
        },
        KmsCase {
            name: "a refused update applies nothing else it carried",
            given: vec![],
            when: json!({
                "OpsCenter": { "OptInStatus": "ENABLED" },
                "KMSServerSideEncryption": { "OptInStatus": "ENABLED", "Type": "CUSTOMER_MANAGED_KEY" }
            }),
            expect_status: 400,
            expect: opted_out.clone(),
        },
        KmsCase {
            name: "a customer-managed key named by key id",
            given: vec![],
            when: json!({
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": key_id
                }
            }),
            expect_status: 200,
            expect: json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": key_id
                },
            }),
        },
        KmsCase {
            name: "a customer-managed key named by key ARN",
            given: vec![],
            when: json!({
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": key_arn
                }
            }),
            expect_status: 200,
            expect: json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": key_arn
                },
            }),
        },
        KmsCase {
            name: "a customer-managed key named by alias name",
            given: vec![],
            when: json!({
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": alias_name
                }
            }),
            expect_status: 200,
            expect: json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": alias_name
                },
            }),
        },
        KmsCase {
            name: "an opted-in AWS-owned key needs no key id",
            given: vec![],
            when: json!({
                "KMSServerSideEncryption": { "OptInStatus": "ENABLED", "Type": "AWS_OWNED_KMS_KEY" }
            }),
            expect_status: 200,
            expect: json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": { "OptInStatus": "ENABLED", "Type": "AWS_OWNED_KMS_KEY" },
            }),
        },
        KmsCase {
            name: "an opted-out customer-managed key needs no key id",
            given: vec![],
            when: json!({
                "KMSServerSideEncryption": { "OptInStatus": "DISABLED", "Type": "CUSTOMER_MANAGED_KEY" }
            }),
            expect_status: 200,
            expect: json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": { "OptInStatus": "DISABLED", "Type": "CUSTOMER_MANAGED_KEY" },
            }),
        },
        KmsCase {
            name: "opting in without naming a type",
            given: vec![],
            when: json!({ "KMSServerSideEncryption": { "OptInStatus": "ENABLED" } }),
            expect_status: 200,
            expect: json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": { "OptInStatus": "ENABLED" },
            }),
        },
        KmsCase {
            name: "the key an earlier request supplied still designates the key",
            given: vec![json!({
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": alias_arn
                }
            })],
            when: json!({
                "KMSServerSideEncryption": { "OptInStatus": "ENABLED", "Type": "CUSTOMER_MANAGED_KEY" }
            }),
            expect_status: 200,
            expect: json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": alias_arn
                },
            }),
        },
        KmsCase {
            name: "switching an opted-in AWS-owned key to customer-managed with no key",
            given: vec![json!({
                "KMSServerSideEncryption": { "OptInStatus": "ENABLED", "Type": "AWS_OWNED_KMS_KEY" }
            })],
            when: json!({ "KMSServerSideEncryption": { "Type": "CUSTOMER_MANAGED_KEY" } }),
            expect_status: 400,
            expect: json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": { "OptInStatus": "ENABLED", "Type": "AWS_OWNED_KMS_KEY" },
            }),
        },
        KmsCase {
            name: "opting a stored customer-managed key back in",
            given: vec![
                json!({
                    "KMSServerSideEncryption": {
                        "OptInStatus": "ENABLED",
                        "Type": "CUSTOMER_MANAGED_KEY",
                        "KMSKeyId": key_id
                    }
                }),
                json!({ "KMSServerSideEncryption": { "OptInStatus": "DISABLED" } }),
            ],
            when: json!({ "KMSServerSideEncryption": { "OptInStatus": "ENABLED" } }),
            expect_status: 200,
            expect: json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": key_id
                },
            }),
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        for (index, given) in case.given.iter().enumerate() {
            let reply = emulator.update_service_integration(given.clone()).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: the setup update {index} must succeed, got status {} body {}",
                case.name, reply.status, reply.raw
            );
        }

        let reply = emulator.update_service_integration(case.when.clone()).await;
        assert_eq!(
            reply.status, case.expect_status,
            "case {:?}: expected status {}, got status {} body {}",
            case.name, case.expect_status, reply.status, reply.raw
        );
        if case.expect_status == 400 {
            assert!(
                reply.names_error("ValidationException"),
                "case {:?}: expected a ValidationException, got {}",
                case.name,
                reply.raw
            );
        }

        let described = emulator.describe_service_integration().await;
        assert_eq!(
            described.status, 200,
            "case {:?}: the describe must succeed, got {}",
            case.name,
            described.raw
        );
        let actual = described
            .at("/ServiceIntegration")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            actual, case.expect,
            "case {:?}: expected the stored integration {}, got {actual}",
            case.name, case.expect
        );
    }
}

/// RULE: a repeat registration of an already-registered topic reports the existing
/// channel, so the config a later read reports is the one the channel was created with.
///
/// The rows differ in HOW the repeat's `Config` differs from the stored one: identical,
/// filters added, filters dropped, and filters changed. Each asserts the shared id and
/// the config `ListNotificationChannels` reports afterwards, so the semantics of a
/// differing repeat are pinned rather than left to chance.
#[tokio::test]
async fn a_repeat_registration_reports_the_channel_as_registered() {
    let topic_arn = "arn:aws:sns:us-east-1:000000000000:guru-converge";
    let with_filters = json!({ "Severities": ["HIGH"], "MessageTypes": ["NEW_INSIGHT"] });
    let other_filters = json!({ "Severities": ["LOW"] });
    let cases: [(&str, Value, Value); 4] = [
        ("an identical repeat", json!(null), json!(null)),
        (
            "a repeat that adds filters",
            json!(null),
            with_filters.clone(),
        ),
        (
            "a repeat that drops the filters",
            with_filters.clone(),
            json!(null),
        ),
        (
            "a repeat that changes the filters",
            with_filters.clone(),
            other_filters,
        ),
    ];

    for (name, registered, repeated) in cases {
        let emulator = Emulator::new();
        let registered_config = channel_config(topic_arn, &registered);

        let first = emulator
            .add_notification_channel(registered_config.clone())
            .await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the first add must succeed, got {}",
            first.raw
        );
        let id = minted_id(&first);

        let repeat = emulator
            .add_notification_channel(channel_config(topic_arn, &repeated))
            .await;
        assert_eq!(
            repeat.status, 200,
            "case {name:?}: the repeat must succeed, got status {} body {}",
            repeat.status, repeat.raw
        );
        assert_eq!(
            repeat.at("/Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {name:?}: the repeat must report the registered id {id:?}, got {}",
            repeat.raw
        );

        // The channel IS the registration that is already in place: the model has no
        // operation that reconfigures one, so the repeat changes nothing and the read
        // reports the config the channel was registered with.
        let listed = emulator.list_notification_channels(None).await;
        assert_eq!(
            listed.at("/Channels").cloned().unwrap_or(Value::Null),
            json!([{ "Id": id, "Config": registered_config }]),
            "case {name:?}: expected the registered config unchanged, got {}",
            listed.raw
        );
    }
}


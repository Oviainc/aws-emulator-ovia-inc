// SPDX-License-Identifier: CC0-1.0

//! Anomaly-notification channels, end to end over HTTP through the router this service
//! mounts in production.
//!
//! The rule every case here asserts: each notification operation answers from the ONE
//! notification configuration stored on the named profiling group —
//! `AddNotificationChannels` appends channels to it (keeping the `id` the request named,
//! or minting a random UUID when it named none), `RemoveNotificationChannel` deletes one
//! of them by id, and `GetNotificationConfiguration` reports exactly what is stored, an
//! EMPTY channel list included — so no response is an echo of the request that asked for
//! it, and a configuration never leaves the two channels the model's `Channels` list
//! allows.
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds (one
//! shared engine behind one mounted service); only the notification calls and the
//! tables are this suite's own.
//!
//! Each table row varies what the rule generalises over (how many channels one request
//! adds, whether their ids are named or minted, the topic ARN's partition/region/topic,
//! which channel is removed first, how many groups hold channels at once); no row is
//! privileged.

use std::collections::BTreeMap;

use crate::profiling_group_lifecycle::{Answer, Harness};

/// The only value the model's `EventPublisher` enum carries
/// (`smithy_query(query="shape", shape="EventPublisher")`), and `EventPublishers` is
/// `@length(min: 1, max: 1)`, so this one-element list is the only publisher list a
/// channel can carry.
const ANOMALY_DETECTION: &str = "AnomalyDetection";

/// `CreateProfilingGroup`: the group record is what holds the notification
/// configuration, so every case starts here.
async fn create_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=token-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await
}

/// `AddNotificationChannels`: `POST
/// /profilingGroups/{profilingGroupName}/notificationConfiguration`.
async fn add(harness: &Harness, name: &str, channels: &[serde_json::Value]) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups/{name}/notificationConfiguration"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "channels": channels }))
                .expect("a serializable body"),
        )
        .await
}

/// `GetNotificationConfiguration`: `GET
/// /profilingGroups/{profilingGroupName}/notificationConfiguration`.
async fn get(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!("/profilingGroups/{name}/notificationConfiguration"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// `RemoveNotificationChannel`: `DELETE
/// /profilingGroups/{profilingGroupName}/notificationConfiguration/{channelId}`.
async fn remove(harness: &Harness, name: &str, channel_id: &str) -> Answer {
    harness
        .send(
            "DELETE",
            &format!("/profilingGroups/{name}/notificationConfiguration/{channel_id}"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// One channel as a request body member. `id` absent exercises `Channel$id`'s documented
/// default: "A random UUID for channelId is used when adding a channel to the
/// notification configuration if not specified in the request."
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

/// The channels of a `notificationConfiguration` in a response, keyed by `uri` so a case
/// can compare them without depending on the order they come back in. Panics (naming the
/// case) when the response carries no channel list at all — an unconfigured group reports
/// an EMPTY list, never a missing one.
fn channels_by_uri(case: &str, answer: &Answer) -> BTreeMap<String, serde_json::Value> {
    let body = answer.json();
    let configuration = body
        .get("notificationConfiguration")
        .unwrap_or_else(|| panic!("case {case}: expected a notificationConfiguration, got {body}"));
    let channels = configuration
        .get("channels")
        .and_then(|channels| channels.as_array())
        .unwrap_or_else(|| {
            panic!("case {case}: expected a channels list, got {configuration}")
        });
    let mut by_uri = BTreeMap::new();
    for channel in channels {
        let uri = channel
            .get("uri")
            .and_then(|uri| uri.as_str())
            .unwrap_or_else(|| panic!("case {case}: a channel carries no uri: {channel}"))
            .to_string();
        assert!(
            by_uri.insert(uri.clone(), channel.clone()).is_none(),
            "case {case}: uri {uri} is reported twice in {channels:?}"
        );
    }
    by_uri
}

/// Whether `value` matches the model's `ChannelId` pattern,
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}` — what a minted channel
/// id must look like.
fn is_channel_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
}

/// The `id` a response reports for a channel.
fn reported_id(case: &str, channel: &serde_json::Value) -> String {
    channel
        .get("id")
        .and_then(|id| id.as_str())
        .unwrap_or_else(|| panic!("case {case}: a stored channel reports no id: {channel}"))
        .to_string()
}

/// One `AddNotificationChannels` request, as the channels it sends: `(id, uri)` per
/// channel, with `None` leaving the id to the service.
struct AddCase {
    label: &'static str,
    group: &'static str,
    channels: Vec<(Option<&'static str>, &'static str)>,
}

/// The add shapes the rule generalises over: one channel or the two the model's
/// `Channels` list allows, ids named or minted, and topic ARNs differing in partition,
/// region, account and topic name.
fn add_cases() -> Vec<AddCase> {
    vec![
        AddCase {
            label: "one channel, id minted",
            group: "minted-single",
            channels: vec![(None, "arn:aws:sns:us-east-1:000000000000:anomalies")],
        },
        AddCase {
            label: "one channel, id named by the request",
            group: "named-single",
            channels: vec![(
                Some("11111111-2222-3333-4444-555555555555"),
                "arn:aws:sns:eu-west-1:000000000000:profiler-alerts",
            )],
        },
        AddCase {
            label: "two channels in one request, both ids minted",
            group: "minted-pair",
            channels: vec![
                (None, "arn:aws:sns:us-west-2:000000000000:anomalies-primary"),
                (None, "arn:aws:sns:us-west-2:000000000000:anomalies-backup"),
            ],
        },
        AddCase {
            label: "two channels in one request, one id named and one minted",
            group: "mixed-pair",
            channels: vec![
                (
                    Some("abcdef01-2345-6789-abcd-ef0123456789"),
                    "arn:aws-us-gov:sns:us-gov-west-1:000000000000:Profiler_Anomalies-1",
                ),
                (None, "arn:aws-cn:sns:cn-north-1:000000000000:anomalies.fifo"),
            ],
        },
    ]
}

#[tokio::test]
async fn get_reports_the_channels_every_add_shape_stored() {
    let harness = Harness::new();

    for case in add_cases() {
        let label = case.label;
        let created = create_group(&harness, case.group).await;
        assert_eq!(
            created.status, 201,
            "case {label}: creating {} must succeed, got {} {}",
            case.group,
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let request: Vec<serde_json::Value> = case
            .channels
            .iter()
            .map(|(id, uri)| channel_body(*id, uri))
            .collect();
        let added = add(&harness, case.group, &request).await;
        assert_eq!(
            added.status,
            200,
            "case {label}: AddNotificationChannels must succeed, got {} {}",
            added.status,
            String::from_utf8_lossy(&added.body)
        );

        // The add's own answer and a later read answer from the same stored record.
        for (source, answer) in [("add", &added), ("get", &get(&harness, case.group).await)] {
            let reported = channels_by_uri(label, answer);
            assert_eq!(
                reported.len(),
                case.channels.len(),
                "case {label}: {source} must report {} channel(s), got {reported:?}",
                case.channels.len()
            );
            for (id, uri) in &case.channels {
                let channel = reported.get(*uri).unwrap_or_else(|| {
                    panic!("case {label}: {source} does not report channel {uri}: {reported:?}")
                });
                let actual_id = reported_id(label, channel);
                match id {
                    Some(named) => assert_eq!(
                        &actual_id, named,
                        "case {label}: {source} must keep the requested id for {uri}, got \
                         {actual_id}"
                    ),
                    None => assert!(
                        is_channel_id(&actual_id),
                        "case {label}: {source} must mint a UUID channelId for {uri}, got \
                         {actual_id}"
                    ),
                }
                assert_eq!(
                    channel.get("eventPublishers"),
                    Some(&serde_json::json!([ANOMALY_DETECTION])),
                    "case {label}: {source} must report the stored eventPublishers for {uri}, \
                     got {channel}"
                );
            }
        }

        // A read does not change what a later caller sees.
        let first = channels_by_uri(label, &get(&harness, case.group).await);
        let second = channels_by_uri(label, &get(&harness, case.group).await);
        assert_eq!(
            first, second,
            "case {label}: two reads of one configuration must agree"
        );
    }

    // Every group above is still reporting only its own channels: the configuration is
    // per group, not per store.
    for case in add_cases() {
        let reported = channels_by_uri(case.label, &get(&harness, case.group).await);
        let expected: Vec<&str> = case.channels.iter().map(|(_, uri)| *uri).collect();
        let actual: Vec<&str> = reported.keys().map(String::as_str).collect();
        let mut sorted = expected.clone();
        sorted.sort_unstable();
        assert_eq!(
            actual, sorted,
            "case {}: group {} must still report only its own channels",
            case.label, case.group
        );
    }
}

/// How the two channels of a group were added, and which one is removed first.
struct GrowShrinkCase {
    label: &'static str,
    group: &'static str,
    /// `true` adds both channels in ONE request, `false` in two successive requests.
    together: bool,
    /// Index (into the channel list below) of the channel removed first.
    remove_first: usize,
}

fn grow_shrink_cases() -> Vec<GrowShrinkCase> {
    vec![
        GrowShrinkCase {
            label: "two separate adds, oldest removed first",
            group: "shrink-separate-oldest",
            together: false,
            remove_first: 0,
        },
        GrowShrinkCase {
            label: "two separate adds, newest removed first",
            group: "shrink-separate-newest",
            together: false,
            remove_first: 1,
        },
        GrowShrinkCase {
            label: "one add of two channels, second removed first",
            group: "shrink-together",
            together: true,
            remove_first: 1,
        },
    ]
}

#[tokio::test]
async fn a_configuration_grows_with_each_add_and_shrinks_with_each_remove() {
    let harness = Harness::new();

    for case in grow_shrink_cases() {
        let label = case.label;
        let uris = [
            "arn:aws:sns:us-east-1:000000000000:anomalies-one",
            "arn:aws:sns:us-east-1:000000000000:anomalies-two",
        ];
        assert_eq!(
            create_group(&harness, case.group).await.status,
            201,
            "case {label}: creating {} must succeed",
            case.group
        );

        // Empty before anything is added.
        assert!(
            channels_by_uri(label, &get(&harness, case.group).await).is_empty(),
            "case {label}: a group nobody configured must report no channels"
        );

        if case.together {
            let request = [channel_body(None, uris[0]), channel_body(None, uris[1])];
            let added = add(&harness, case.group, &request).await;
            assert_eq!(
                added.status,
                200,
                "case {label}: adding both channels at once must succeed, got {}",
                String::from_utf8_lossy(&added.body)
            );
        } else {
            for (step, uri) in uris.iter().enumerate() {
                let added = add(&harness, case.group, &[channel_body(None, uri)]).await;
                assert_eq!(
                    added.status,
                    200,
                    "case {label}: add #{step} must succeed, got {}",
                    String::from_utf8_lossy(&added.body)
                );
                let reported = channels_by_uri(label, &added);
                assert_eq!(
                    reported.len(),
                    step + 1,
                    "case {label}: after add #{step} the configuration must hold {} \
                     channel(s), got {reported:?}",
                    step + 1
                );
            }
        }

        let grown = channels_by_uri(label, &get(&harness, case.group).await);
        assert_eq!(
            grown.len(),
            2,
            "case {label}: both channels must be stored, got {grown:?}"
        );
        let ids: Vec<String> = uris
            .iter()
            .map(|uri| reported_id(label, &grown[*uri]))
            .collect();
        assert_ne!(
            ids[0], ids[1],
            "case {label}: each channel must get its own id, got {ids:?}"
        );

        // Shrink: each remove takes exactly the named channel out, and the answer and a
        // later read agree on what is left.
        let first = case.remove_first;
        let second = 1 - first;
        let removed = remove(&harness, case.group, &ids[first]).await;
        assert_eq!(
            removed.status,
            200,
            "case {label}: removing {} must succeed, got {}",
            ids[first],
            String::from_utf8_lossy(&removed.body)
        );
        for (source, answer) in [
            ("remove", &removed),
            ("get", &get(&harness, case.group).await),
        ] {
            let reported = channels_by_uri(label, answer);
            assert_eq!(
                reported.keys().map(String::as_str).collect::<Vec<&str>>(),
                vec![uris[second]],
                "case {label}: {source} must report only {} after the first remove",
                uris[second]
            );
        }

        let emptied = remove(&harness, case.group, &ids[second]).await;
        assert_eq!(
            emptied.status,
            200,
            "case {label}: removing the last channel must succeed, got {}",
            String::from_utf8_lossy(&emptied.body)
        );
        assert!(
            channels_by_uri(label, &emptied).is_empty(),
            "case {label}: removing the last channel must leave an empty channel list"
        );
        assert!(
            channels_by_uri(label, &get(&harness, case.group).await).is_empty(),
            "case {label}: the emptied configuration must read back empty"
        );

        // And the group accepts channels again afterwards: nothing about the removes
        // consumed its capacity.
        let readded = add(&harness, case.group, &[channel_body(None, uris[0])]).await;
        assert_eq!(
            readded.status,
            200,
            "case {label}: the group must accept a channel again after being emptied, got {}",
            String::from_utf8_lossy(&readded.body)
        );
    }
}

#[tokio::test]
async fn a_configuration_nobody_stored_is_not_found_and_an_unknown_channel_is_not_found() {
    let harness = Harness::new();
    let group = "known-group";
    let uri = "arn:aws:sns:us-east-1:000000000000:anomalies";
    assert_eq!(create_group(&harness, group).await.status, 201);

    // Every notification operation on a group that does not exist is the modelled 404,
    // whichever verb it is.
    let unknown = "no-such-group";
    let absent: [(&str, Answer); 3] = [
        ("GetNotificationConfiguration", get(&harness, unknown).await),
        (
            "AddNotificationChannels",
            add(&harness, unknown, &[channel_body(None, uri)]).await,
        ),
        (
            "RemoveNotificationChannel",
            remove(&harness, unknown, "11111111-2222-3333-4444-555555555555").await,
        ),
    ];
    for (operation, answer) in &absent {
        assert_eq!(
            answer.status,
            404,
            "case {operation} on an unknown group: expected 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {operation} on an unknown group: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A channel id the stored configuration does not hold is the same 404 — before and
    // after the group holds a channel at all.
    for (case, stored) in [("empty configuration", false), ("one channel", true)] {
        if stored {
            assert_eq!(add(&harness, group, &[channel_body(None, uri)]).await.status, 200);
        }
        let answer = remove(&harness, group, "99999999-8888-7777-6666-555555555555").await;
        assert_eq!(
            answer.status,
            404,
            "case {case}: removing an unknown channel id must be 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case}: removing an unknown channel id must be ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // ...and that failed remove left the stored channel alone.
    let reported = channels_by_uri("one channel", &get(&harness, group).await);
    assert_eq!(
        reported.keys().map(String::as_str).collect::<Vec<&str>>(),
        vec![uri],
        "a refused remove must leave the configuration unchanged"
    );
}

#[tokio::test]
async fn adding_past_the_two_channel_limit_is_refused_and_changes_nothing() {
    use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::MAX_NOTIFICATION_CHANNELS_PER_GROUP;

    let harness = Harness::new();
    // Each row: how many channels are already stored, and how many one more request
    // tries to add. Both rows exceed the limit; the first row's add is itself legal
    // input (the `Channels` list allows two), which is what makes the refusal the
    // service's rather than the boundary validator's.
    let rows: [(usize, usize); 2] = [(1, 2), (2, 1)];
    for (stored, extra) in rows {
        let group = format!("quota-{stored}-{extra}");
        let case = format!("{stored} stored + {extra} more");
        assert_eq!(create_group(&harness, &group).await.status, 201);

        let existing: Vec<serde_json::Value> = (0..stored)
            .map(|index| {
                channel_body(
                    None,
                    // A distinct topic per stored channel: `Channels` is @uniqueItems.
                    match index {
                        0 => "arn:aws:sns:us-east-1:000000000000:stored-0",
                        _ => "arn:aws:sns:us-east-1:000000000000:stored-1",
                    },
                )
            })
            .collect();
        assert_eq!(
            add(&harness, &group, &existing).await.status,
            200,
            "case {case}: the first {stored} channel(s) must be accepted"
        );
        let before = channels_by_uri(&case, &get(&harness, &group).await);
        assert_eq!(
            before.len(),
            stored,
            "case {case}: {stored} channel(s) must be stored, got {before:?}"
        );

        let more: Vec<serde_json::Value> = (0..extra)
            .map(|index| {
                channel_body(
                    None,
                    match index {
                        0 => "arn:aws:sns:us-east-1:000000000000:extra-0",
                        _ => "arn:aws:sns:us-east-1:000000000000:extra-1",
                    },
                )
            })
            .collect();
        let refused = add(&harness, &group, &more).await;
        assert_eq!(
            refused.status,
            402,
            "case {case}: an add past {MAX_NOTIFICATION_CHANNELS_PER_GROUP} channels must be \
             402, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "ServiceQuotaExceededException",
            "case {case}: expected ServiceQuotaExceededException, got {}",
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            channels_by_uri(&case, &get(&harness, &group).await),
            before,
            "case {case}: a refused add must leave the configuration unchanged"
        );

        // Removing a channel frees the capacity the refusal reported.
        let id = reported_id(&case, before.values().next().expect("a stored channel"));
        assert_eq!(
            remove(&harness, &group, &id).await.status,
            200,
            "case {case}: removing a stored channel must succeed"
        );
        let after = add(
            &harness,
            &group,
            &[channel_body(None, "arn:aws:sns:us-east-1:000000000000:extra-0")],
        )
        .await;
        assert_eq!(
            after.status,
            200,
            "case {case}: the freed capacity must accept one more channel, got {} {}",
            after.status,
            String::from_utf8_lossy(&after.body)
        );
    }
}

#[tokio::test]
async fn an_id_the_configuration_already_holds_is_a_conflict() {
    let harness = Harness::new();
    let group = "conflict-group";
    let id = "0123abcd-4567-89ef-0123-456789abcdef";
    assert_eq!(create_group(&harness, group).await.status, 201);
    assert_eq!(
        add(
            &harness,
            group,
            &[channel_body(Some(id), "arn:aws:sns:us-east-1:000000000000:first")]
        )
        .await
        .status,
        200
    );

    // Each row names the same channel id twice: once against the stored configuration,
    // once within a single request.
    let rows: [(&str, Vec<serde_json::Value>); 2] = [
        (
            "an id already stored",
            vec![channel_body(
                Some(id),
                "arn:aws:sns:us-east-1:000000000000:second",
            )],
        ),
        (
            "the same id twice in one request",
            vec![
                channel_body(
                    Some("fedcba98-7654-3210-fedc-ba9876543210"),
                    "arn:aws:sns:us-east-1:000000000000:third",
                ),
                channel_body(
                    Some("fedcba98-7654-3210-fedc-ba9876543210"),
                    "arn:aws:sns:us-east-1:000000000000:fourth",
                ),
            ],
        ),
    ];
    for (case, channels) in rows {
        let refused = add(&harness, group, &channels).await;
        assert_eq!(
            refused.status,
            409,
            "case {case}: expected 409, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case}: expected ConflictException, got {}",
            String::from_utf8_lossy(&refused.body)
        );
        let reported = channels_by_uri(case, &get(&harness, group).await);
        assert_eq!(
            reported.keys().map(String::as_str).collect::<Vec<&str>>(),
            vec!["arn:aws:sns:us-east-1:000000000000:first"],
            "case {case}: a refused add must store nothing"
        );
    }
}

#[tokio::test]
async fn a_channel_must_name_an_sns_topic_arn_and_a_modelled_event_publisher() {
    let harness = Harness::new();
    let group = "validation-group";
    assert_eq!(create_group(&harness, group).await.status, 201);

    // `ChannelUri` is an `@arnReference` whose doc says "Currently we only support SNS
    // channels and thus required to be an ARN"
    // (`smithy_query(query="shape", shape="ChannelUri")`), so each of these is refused.
    let bad_uris: [(&str, &str); 5] = [
        ("empty", ""),
        ("not an arn at all", "https://example.invalid/topic"),
        ("too few arn fields", "arn:aws:sns:us-east-1:000000000000"),
        ("another service", "arn:aws:s3:::some-bucket"),
        (
            "an sns arn with no resource",
            "arn:aws:sns:us-east-1:000000000000:",
        ),
    ];
    for (case, uri) in bad_uris {
        let refused = add(&harness, group, &[channel_body(None, uri)]).await;
        assert_eq!(
            refused.status,
            400,
            "case {case}: uri {uri:?} must be refused with 400, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case}: uri {uri:?} must be refused as ValidationException, got {}",
            String::from_utf8_lossy(&refused.body)
        );
    }

    // `eventPublishers` targets the closed `EventPublisher` enum, so a value outside it
    // is refused too — here by the boundary validator, with the same 400.
    let unmodelled = serde_json::json!({
        "uri": "arn:aws:sns:us-east-1:000000000000:anomalies",
        "eventPublishers": ["LatencyRegression"],
    });
    let refused = add(&harness, group, &[unmodelled]).await;
    assert_eq!(
        refused.status,
        400,
        "case unmodelled event publisher: expected 400, got {} {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );

    // Nothing above reached the store.
    assert!(
        channels_by_uri("after refusals", &get(&harness, group).await).is_empty(),
        "a refused channel must not be stored"
    );
}

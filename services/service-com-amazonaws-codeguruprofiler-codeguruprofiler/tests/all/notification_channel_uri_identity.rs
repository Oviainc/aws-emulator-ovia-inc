// SPDX-License-Identifier: CC0-1.0

//! Which channel a notification `uri` names, end to end over HTTP through the router this
//! service mounts in production.
//!
//! The rule every case here asserts: a channel's `uri` is the destination the group's
//! anomaly notifications are published to, and one destination holds ONE channel — so
//! `AddNotificationChannels` is idempotent per uri: a requested channel whose uri the
//! stored configuration already covers REPLACES that channel (taking the id the request
//! named, or a freshly minted one when it named none) instead of joining it, the channels
//! of the other destinations keep their ids, and such a re-assertion consumes none of the
//! group's channel capacity — it is accepted even while the group is at the maximum the
//! model's `Channels` list allows. Only a destination the configuration does not already
//! cover grows it, and growing past that maximum is the operation's
//! `ServiceQuotaExceededException`, which states the limit it enforces.
//!
//! The rows differ along what the rule generalises over: how many destinations are stored,
//! which stored destination is re-asserted, whether the re-assertion names the channel id
//! or leaves it to the service, whether it names the id the stored channel already has,
//! how many channels one request carries, and whether a batch mixes a re-assertion with a
//! new destination.
//!
//! The harness and the notification senders are the ones
//! [`crate::profiling_group_lifecycle`] and [`crate::notification_channels`] already
//! build; only the tables are this suite's own.

use std::collections::BTreeMap;

use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::MAX_NOTIFICATION_CHANNELS_PER_GROUP;

use crate::notification_channels::{add, channel_body, create_group, get, is_channel_id};
use crate::profiling_group_lifecycle::{Answer, Harness};

/// SNS topic ARNs used as channel uris: `ChannelUri` is an `@arnReference` documented as
/// "Currently we only support SNS channels and thus required to be an ARN"
/// (`smithy_query(query="shape", shape="ChannelUri")`). They differ in region, account and
/// topic name, so no case depends on one ARN's spelling.
const TOPIC_A: &str = "arn:aws:sns:us-east-1:000000000000:anomalies-primary";
const TOPIC_B: &str = "arn:aws:sns:eu-west-1:000000000000:anomalies-backup";
const TOPIC_C: &str = "arn:aws:sns:us-west-2:000000000000:anomalies.fifo";

/// Caller-chosen `channelId`s, each matching the model's `ChannelId` pattern
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`, so the boundary
/// validator admits them and the handler decides what they mean.
const ID_ONE: &str = "11111111-aaaa-bbbb-cccc-dddddddddddd";
const ID_TWO: &str = "22222222-aaaa-bbbb-cccc-dddddddddddd";

/// One channel as a table row carries it: the id the request names (`None` leaves it to
/// the service) and the destination uri it reports to.
type Requested = (Option<&'static str>, &'static str);

/// The channel ids a response's `notificationConfiguration` reports, keyed by `uri`, so a
/// case compares destinations without depending on the order they come back in. Panics
/// (naming the case) when the response carries no channel list, or when two channels
/// report the same uri — which is itself the failure this suite is about.
fn ids_by_uri(case: &str, answer: &Answer) -> BTreeMap<String, String> {
    let body = answer.json();
    let channels = body
        .get("notificationConfiguration")
        .and_then(|configuration| configuration.get("channels"))
        .and_then(|channels| channels.as_array())
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case}: expected notificationConfiguration.channels, got {body}")
        });
    let mut by_uri: BTreeMap<String, String> = BTreeMap::new();
    for channel in &channels {
        let uri = channel
            .get("uri")
            .and_then(|uri| uri.as_str())
            .unwrap_or_else(|| panic!("case {case}: a stored channel reports no uri: {channel}"));
        let id = channel
            .get("id")
            .and_then(|id| id.as_str())
            .unwrap_or_else(|| panic!("case {case}: a stored channel reports no id: {channel}"));
        assert!(
            by_uri.insert(uri.to_string(), id.to_string()).is_none(),
            "case {case}: the configuration must hold one channel per uri, got {channels:?}"
        );
    }
    by_uri
}

/// Store the channels of one request and assert the call succeeded.
async fn stored(
    harness: &Harness,
    case: &str,
    group: &str,
    channels: &[(Option<&str>, &str)],
) -> Answer {
    let bodies: Vec<serde_json::Value> = channels
        .iter()
        .map(|(id, uri)| channel_body(*id, uri))
        .collect();
    let answer = add(harness, group, &bodies).await;
    assert_eq!(
        answer.status,
        200,
        "case {case}: AddNotificationChannels{channels:?} must succeed, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
}

/// One group, the destinations it is given first, and the request that re-asserts one of
/// them.
struct ReAssertCase {
    label: &'static str,
    group: &'static str,
    /// `(id, uri)` per channel stored before the re-assertion.
    first: Vec<Requested>,
    /// `(id, uri)` per channel of the re-assertion.
    again: Vec<Requested>,
    /// Every destination the configuration must report afterwards.
    expected: Vec<&'static str>,
}

fn re_assert_cases() -> Vec<ReAssertCase> {
    vec![
        ReAssertCase {
            label: "one destination re-asserted, id left to the service",
            group: "uri-one-minted",
            first: vec![(None, TOPIC_A)],
            again: vec![(None, TOPIC_A)],
            expected: vec![TOPIC_A],
        },
        ReAssertCase {
            label: "one destination re-asserted under a caller-named id",
            group: "uri-one-named",
            first: vec![(None, TOPIC_A)],
            again: vec![(Some(ID_ONE), TOPIC_A)],
            expected: vec![TOPIC_A],
        },
        ReAssertCase {
            label: "one destination re-asserted under the id it already carries",
            group: "uri-one-same-id",
            first: vec![(Some(ID_ONE), TOPIC_A)],
            again: vec![(Some(ID_ONE), TOPIC_A)],
            expected: vec![TOPIC_A],
        },
        ReAssertCase {
            label: "a full group's first destination re-asserted",
            group: "uri-full-first",
            first: vec![(None, TOPIC_A), (None, TOPIC_B)],
            again: vec![(None, TOPIC_A)],
            expected: vec![TOPIC_A, TOPIC_B],
        },
        ReAssertCase {
            label: "a full group's second destination re-asserted under a named id",
            group: "uri-full-second",
            first: vec![(Some(ID_ONE), TOPIC_A), (None, TOPIC_B)],
            again: vec![(Some(ID_TWO), TOPIC_B)],
            expected: vec![TOPIC_A, TOPIC_B],
        },
        ReAssertCase {
            label: "a batch that re-asserts one destination and adds a new one",
            group: "uri-batch-mixed",
            first: vec![(None, TOPIC_A)],
            again: vec![(None, TOPIC_A), (None, TOPIC_C)],
            expected: vec![TOPIC_A, TOPIC_C],
        },
        ReAssertCase {
            label: "a full group's two destinations re-asserted in one batch, reversed",
            group: "uri-batch-both",
            first: vec![(None, TOPIC_A), (None, TOPIC_B)],
            again: vec![(None, TOPIC_B), (None, TOPIC_A)],
            expected: vec![TOPIC_A, TOPIC_B],
        },
    ]
}

#[tokio::test]
async fn re_asserting_a_stored_destination_replaces_its_channel_and_consumes_no_capacity() {
    let harness = Harness::new();

    for case in re_assert_cases() {
        let label = case.label;
        create_group(&harness, case.group).await;

        let first = stored(&harness, label, case.group, &case.first).await;
        let before = ids_by_uri(label, &first);
        assert_eq!(
            before.keys().map(String::as_str).collect::<Vec<&str>>(),
            {
                let mut uris: Vec<&str> = case.first.iter().map(|(_, uri)| *uri).collect();
                uris.sort_unstable();
                uris
            },
            "case {label}: the first call must store exactly its own destinations"
        );

        // The re-assertion: accepted whatever the group already holds, the full group
        // included — it asks for no destination the group does not already report to.
        let again = stored(&harness, label, case.group, &case.again).await;

        let mut expected = case.expected.clone();
        expected.sort_unstable();
        for (source, answer) in [("the add", &again), ("a later read", &get(&harness, case.group).await)]
        {
            let after = ids_by_uri(label, answer);
            assert_eq!(
                after.keys().map(String::as_str).collect::<Vec<&str>>(),
                expected,
                "case {label}: {source} must report one channel per destination, got {after:?}"
            );
            assert!(
                after.len() <= MAX_NOTIFICATION_CHANNELS_PER_GROUP,
                "case {label}: {source} must stay within {MAX_NOTIFICATION_CHANNELS_PER_GROUP} \
                 channels, got {after:?}"
            );

            for (named, uri) in &case.again {
                let id = after.get(*uri).unwrap_or_else(|| {
                    panic!("case {label}: {source} does not report {uri}, got {after:?}")
                });
                match named {
                    // An id the request named is the caller's, replacement or not.
                    Some(named) => assert_eq!(
                        id, named,
                        "case {label}: {source} must report the requested id for {uri}, got {id}"
                    ),
                    // `Channel$id`: "A random UUID for channelId is used when adding a
                    // channel to the notification configuration if not specified in the
                    // request" — so a replacement the request named no id for is issued a
                    // freshly minted one, not the id of the channel it supersedes.
                    None => {
                        assert!(
                            is_channel_id(id),
                            "case {label}: {source} must report a minted ChannelId for {uri}, \
                             got {id}"
                        );
                        if let Some(previous) = before.get(*uri) {
                            assert_ne!(
                                id, previous,
                                "case {label}: {source} must report a fresh id for the replaced \
                                 channel of {uri}, got {id}"
                            );
                        }
                    }
                }
            }

            // Every destination this request said nothing about kept the channel it had.
            for (uri, id) in &before {
                if case.again.iter().any(|(_, asserted)| asserted == uri) {
                    continue;
                }
                assert_eq!(
                    after.get(uri),
                    Some(id),
                    "case {label}: {source} must leave the channel of {uri} alone, got {after:?}"
                );
            }
        }

        // Idempotence is not one-shot: the same request again leaves the same
        // destinations, each still holding exactly one channel.
        let third = stored(&harness, label, case.group, &case.again).await;
        let after_third = ids_by_uri(label, &third);
        assert_eq!(
            after_third.keys().map(String::as_str).collect::<Vec<&str>>(),
            expected,
            "case {label}: a third identical call must change no destination, got {after_third:?}"
        );
    }
}

/// One group, the destinations it is given first, and a request asking for more
/// destinations than the limit allows.
struct LimitCase {
    label: &'static str,
    group: &'static str,
    first: Vec<Requested>,
    more: Vec<Requested>,
}

#[tokio::test]
async fn growing_past_the_channel_limit_is_refused_with_the_limit_it_enforces() {
    let harness = Harness::new();
    // Each row: the destinations stored first, then the channels of a request that asks
    // for more destinations than the limit allows. The last row mixes a re-assertion with
    // a new destination, so the refusal is about the destination that really is new.
    let rows: [LimitCase; 4] = [
        LimitCase {
            label: "one stored, two new destinations",
            group: "limit-one-two",
            first: vec![(None, TOPIC_A)],
            more: vec![(None, TOPIC_B), (None, TOPIC_C)],
        },
        LimitCase {
            label: "a full group, one new destination",
            group: "limit-full-one",
            first: vec![(None, TOPIC_A), (None, TOPIC_B)],
            more: vec![(None, TOPIC_C)],
        },
        LimitCase {
            label: "a full group, two new destinations",
            group: "limit-full-two",
            first: vec![(Some(ID_ONE), TOPIC_A), (None, TOPIC_B)],
            more: vec![
                (None, TOPIC_C),
                (Some(ID_TWO), "arn:aws:sns:us-east-2:000000000000:spare"),
            ],
        },
        LimitCase {
            label: "a full group, one re-assertion and one new destination",
            group: "limit-full-mixed",
            first: vec![(None, TOPIC_A), (None, TOPIC_B)],
            more: vec![(None, TOPIC_A), (None, TOPIC_C)],
        },
    ];

    // The sentence this error class states; its only variable part is the service limit.
    let expected_message = format!(
        "Channel limit exceeded: Only {MAX_NOTIFICATION_CHANNELS_PER_GROUP} channels allowed per \
         profiling group"
    );

    for LimitCase {
        label: case,
        group,
        first,
        more,
    } in rows
    {
        create_group(&harness, group).await;
        let before = ids_by_uri(case, &stored(&harness, case, group, &first).await);

        let bodies: Vec<serde_json::Value> = more
            .iter()
            .map(|(id, uri)| channel_body(*id, uri))
            .collect();
        let refused = add(&harness, group, &bodies).await;
        assert_eq!(
            refused.status,
            402,
            "case {case}: growing past the limit must answer 402, got {} / {}",
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
            refused.error_message(),
            expected_message,
            "case {case}: expected {expected_message:?}, got {:?}",
            refused.error_message()
        );
        // All or nothing: a refused request stores none of its channels, the one that
        // only re-asserted a stored destination included.
        assert_eq!(
            ids_by_uri(case, &get(&harness, group).await),
            before,
            "case {case}: a refused request must leave the configuration unchanged"
        );
    }
}

#[tokio::test]
async fn an_id_naming_another_destinations_channel_is_a_conflict() {
    let harness = Harness::new();
    let group = "uri-conflict";
    create_group(&harness, group).await;
    // The group holds ID_ONE for TOPIC_A and a minted id for TOPIC_B.
    let before = ids_by_uri(
        "setup",
        &stored(
            &harness,
            "setup",
            group,
            &[(Some(ID_ONE), TOPIC_A), (None, TOPIC_B)],
        )
        .await,
    );

    // Each row names a stored channel's id for a DIFFERENT destination, which no
    // re-assertion of that destination can take: the id belongs to a channel the request
    // does not supersede.
    let rows: [(&str, Vec<Requested>); 2] = [
        (
            "a stored id on another stored destination",
            vec![(Some(ID_ONE), TOPIC_B)],
        ),
        (
            "a stored id on a destination nobody stored",
            vec![(Some(ID_ONE), TOPIC_C)],
        ),
    ];
    for (case, channels) in rows {
        let bodies: Vec<serde_json::Value> = channels
            .iter()
            .map(|(id, uri)| channel_body(*id, uri))
            .collect();
        let refused = add(&harness, group, &bodies).await;
        assert_eq!(
            refused.status,
            409,
            "case {case}: expected 409, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case}: expected ConflictException, got {}",
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            ids_by_uri(case, &get(&harness, group).await),
            before,
            "case {case}: a refused add must change nothing"
        );
    }

    // The control: that same id on its OWN destination is a re-assertion, not a
    // conflict, and leaves the configuration with one channel per destination.
    let accepted = stored(
        &harness,
        "the id on its own destination",
        group,
        &[(Some(ID_ONE), TOPIC_A)],
    )
    .await;
    assert_eq!(
        ids_by_uri("the id on its own destination", &accepted),
        before,
        "case the id on its own destination: re-asserting it must change nothing"
    );
}

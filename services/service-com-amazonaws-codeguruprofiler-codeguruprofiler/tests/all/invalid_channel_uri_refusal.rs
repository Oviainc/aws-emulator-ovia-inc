// SPDX-License-Identifier: CC0-1.0

//! How `AddNotificationChannels` refuses a channel `uri` that is not an Amazon SNS topic
//! ARN, end to end over HTTP through the router this service mounts in production.
//!
//! The rule every case here asserts: a channel's destination must be an SNS topic ARN —
//! `Channel$uri` is documented "Unique arn of the resource to be used for notifications.
//! We support a valid SNS topic arn as a channel uri"
//! (`smithy_query(query="doc", shape="Channel", member="uri")`) — and anything else is the
//! operation's modelled `ValidationException` (HTTP 400) stating that channel validation
//! failed and naming the topic ARN it could not read, exactly as the request sent it and
//! with nothing else in the sentence: no member path, no index, no ARN grammar. A refused
//! add stores nothing.
//!
//! The rows differ along what that rule generalises over: HOW the uri fails to be a topic
//! ARN (empty, a bare word, a URL of another scheme, too few ARN fields, a well-formed ARN
//! of another vendor, an SNS ARN with an empty resource or partition, an SNS ARN whose
//! vendor differs only in case), which POSITION of the `channels` list carries it (first,
//! second, behind a channel that would have been accepted), and whether the group already
//! holds a channel — no row is privileged, and a bare word is only one of them. The
//! accepted rows at the end span the ARN forms a topic is written in, so the refusal is
//! shown to be about the class and not about the operation.
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds (one shared
//! engine behind one mounted service); only the notification calls and the tables are this
//! suite's own.

use crate::profiling_group_lifecycle::{Answer, Harness};

/// The only value the model's `EventPublisher` enum carries
/// (`smithy_query(query="shape", shape="EventPublisher")`), and `EventPublishers` is
/// `@length(min: 1, max: 1)`, so this one-element list is the only publisher list a
/// channel can carry.
const ANOMALY_DETECTION: &str = "AnomalyDetection";

/// SNS topic ARNs used as channel uris, the form `Channel$uri` documents.
const TOPIC_A: &str = "arn:aws:sns:us-east-1:000000000000:anomalies-a";
const TOPIC_B: &str = "arn:aws:sns:us-east-1:000000000000:anomalies-b";

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
        answer.status, 201,
        "case {name}: CreateProfilingGroup must succeed, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// One channel as a request body member, with the id left to the service.
fn channel_body(uri: &str) -> serde_json::Value {
    serde_json::json!({
        "uri": uri,
        "eventPublishers": [ANOMALY_DETECTION],
    })
}

/// `AddNotificationChannels`:
/// `POST /profilingGroups/{profilingGroupName}/notificationConfiguration`.
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

/// `GetNotificationConfiguration`:
/// `GET /profilingGroups/{profilingGroupName}/notificationConfiguration`.
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

/// The channel uris a response's `notificationConfiguration` reports, in order.
fn reported_uris(case: &str, answer: &Answer) -> Vec<String> {
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
                .get("uri")
                .and_then(|uri| uri.as_str())
                .unwrap_or_else(|| {
                    panic!("case {case}: a stored channel must report a uri, got {channel}")
                })
                .to_string()
        })
        .collect()
}

/// The sentence the service states for a channel uri it could not read as a topic ARN:
/// that channel validation failed, and the rejected uri as the request sent it.
fn expected_message(uri: &str) -> String {
    format!("Channel Validation failed: Invalid topic ARN: {uri}")
}

/// Assert the whole refusal of one call: the modelled status, code and message.
fn assert_invalid_uri(case: &str, uri: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case}: a channel uri that is not a topic ARN must answer 400, got {} / {}",
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
        expected_message(uri),
        "case {case}: expected {:?}, got {:?}",
        expected_message(uri),
        answer.error_message()
    );
}

/// Uris that are not SNS topic ARNs, as (case, the uri sent). An ARN is read by the one
/// ARN parse in this tree — six colon-separated fields, a literal `arn`, a non-empty
/// partition, vendor and resource — and the vendor must be exactly `sns`, so each of these
/// fails to be a topic ARN in a different way.
fn invalid_uris() -> Vec<(&'static str, &'static str)> {
    vec![
        ("empty", ""),
        ("a bare word", "not-an-sns-arn"),
        ("a url of another scheme", "https://example.invalid/topic"),
        ("too few arn fields", "arn:aws:sns:us-east-1:000000000000"),
        (
            "an arn of another vendor",
            "arn:aws:sqs:us-east-1:000000000000:anomalies",
        ),
        ("an arn of another vendor, partition-global", "arn:aws:s3:::bucket"),
        (
            "an sns arn with no resource",
            "arn:aws:sns:us-east-1:000000000000:",
        ),
        (
            "an sns arn with no partition",
            "arn::sns:us-east-1:000000000000:anomalies",
        ),
        (
            "a vendor differing only in case",
            "arn:aws:SNS:us-east-1:000000000000:anomalies",
        ),
        (
            "an arn whose first field is not arn",
            "urn:aws:sns:us-east-1:000000000000:anomalies",
        ),
    ]
}

#[tokio::test]
async fn a_channel_uri_that_is_not_a_topic_arn_is_refused_in_the_services_own_wording() {
    let harness = Harness::new();
    let group = "channel-uri-refusal";
    create_group(&harness, group).await;

    for (case, uri) in invalid_uris() {
        let refused = add(&harness, group, &[channel_body(uri)]).await;
        assert_invalid_uri(case, uri, &refused);

        // A refused add stores nothing: the group reports no channel at all.
        let stored = reported_uris(case, &get(&harness, group).await);
        assert!(
            stored.is_empty(),
            "case {case}: a refused add must store nothing, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn the_refusal_names_the_uri_alone_whatever_position_it_arrived_in() {
    let harness = Harness::new();
    let group = "channel-uri-position";
    create_group(&harness, group).await;

    // Each row sends the same bad uri in a different position of the `channels` list, and
    // beside a channel that WOULD have been accepted — the message must be the same
    // sentence about the uri either way, naming neither the member path nor the index.
    for (case, uri) in invalid_uris() {
        let rows: [(&str, Vec<serde_json::Value>); 3] = [
            ("alone", vec![channel_body(uri)]),
            (
                "second, behind an acceptable channel",
                vec![channel_body(TOPIC_A), channel_body(uri)],
            ),
            (
                "first, ahead of an acceptable channel",
                vec![channel_body(uri), channel_body(TOPIC_B)],
            ),
        ];
        for (position, channels) in rows {
            let label = format!("{case}, {position}");
            let refused = add(&harness, group, &channels).await;
            assert_invalid_uri(&label, uri, &refused);
            // The acceptable channel of the same request is not stored either: a refused
            // add is never a partial one.
            let stored = reported_uris(&label, &get(&harness, group).await);
            assert!(
                stored.is_empty(),
                "case {label}: a refused add must store neither channel, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_topic_arn_is_accepted_whatever_form_it_is_written_in() {
    let harness = Harness::new();

    // The other side of the rule: a uri whose vendor IS `sns` is a topic ARN, so it is
    // stored — across partitions, with the region and account fields written and left
    // empty, and with a resource carrying the characters a topic name allows.
    let rows: [(&str, &str); 4] = [
        ("the commercial partition", "arn:aws:sns:us-east-1:000000000000:anomalies"),
        ("another partition", "arn:aws-cn:sns:cn-north-1:000000000000:anomalies.fifo"),
        ("no region or account", "arn:aws:sns:::Profiler_Anomalies-1"),
        ("a resource with further colons", "arn:aws:sns:us-west-2:000000000000:topic:alarm"),
    ];

    for (index, (case, uri)) in rows.iter().enumerate() {
        let group = &format!("channel-uri-accepted-{index}");
        create_group(&harness, group).await;
        let added = add(&harness, group, &[channel_body(uri)]).await;
        assert_eq!(
            added.status, 200,
            "case {case}: uri {uri:?} must be accepted, got {} / {}",
            added.status,
            String::from_utf8_lossy(&added.body)
        );
        let stored = reported_uris(case, &get(&harness, group).await);
        assert_eq!(
            stored,
            vec![(*uri).to_string()],
            "case {case}: the stored channel must report the uri as sent, got {stored:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! A FIFO topic with a message ARCHIVE redelivers its archived messages to a subscription
//! that asks for a REPLAY — end to end over the wire, with SQS mounted on the same engine
//! so what was redelivered is observed where a client observes it: on the queue.
//!
//! The RULES asserted here:
//!
//! * "ArchivePolicy – The policy that sets the retention period for messages stored in
//!   the message archive of an Amazon SNS FIFO topic"
//!   (`GetTopicAttributesResponse$Attributes` doc), so a topic that holds one keeps the
//!   messages published to it whether or not any subscription received them, and keeps
//!   delivering those messages live;
//! * "ReplayPolicy – Adds or updates an inline policy document for a subscription to
//!   replay messages stored in the specified Amazon SNS topic"
//!   (`SubscribeInput$Attributes` doc): the archived messages inside the window the policy
//!   names are redelivered to that subscription, in publish order, and a subscription that
//!   asks for no replay is not backfilled;
//! * "ReplayStatus – Retrieves the status of the subscription message replay, which can be
//!   one of … Completed … Pending" (`SubscribeInput$Attributes` doc), reported beside the
//!   replay policy that asked for it and settling to `Completed`.
//!
//! The rows differ along what the rules generalise over: which call wrote the replay
//! policy, where in the archive the window starts, whether the window names an ending
//! point, how many messages the archive holds before the subscription exists, and whether
//! the subscription asked for a replay at all.

use serde_json::json;
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    REPLAY_SETTLES_AFTER, REPLAY_STATUS_COMPLETED, REPLAY_STATUS_FAILED, REPLAY_STATUS_PENDING,
};

use crate::sqs_subscription_delivery::{
    drain, element, emulator, grant_to_sns, set_queue_policy, sns, sqs, Emulator,
};

/// A FIFO queue called `<name>.fifo` an SNS topic may deliver to, with the queue policy
/// that grants SNS the send: its url and the ARN a subscription names it by.
///
/// A FIFO queue is what a FIFO topic's subscriber is, so the order the queue reports its
/// messages in is the order they were sent in — which is what a replay's publish order is
/// read from. Its deduplication is content-based, so a redelivery of a body the queue has
/// not seen is a message of its own.
async fn given_fifo_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let created = sqs(
        emulator,
        "CreateQueue",
        &json!({
            "QueueName": format!("{name}.fifo"),
            "Attributes": {"FifoQueue": "true"},
        }),
    )
    .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let read = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = read["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {name}: no QueueArn in {read}"))
        .to_string();
    set_queue_policy(emulator, &url, &grant_to_sns(&arn)).await;
    (url, arn)
}

/// An archive policy stating `days` of retention, as the `ArchivePolicy` attribute carries
/// one. The member is `MessageRetentionPeriod`, "the configured message retention period
/// set by the topic's message archiving policy"
/// (`GetTopicAttributesResponse$Attributes` doc), stated in days.
fn archive_policy(days: &str) -> String {
    format!(r#"{{"MessageRetentionPeriod":"{days}"}}"#)
}

/// A replay policy naming the window `from ..` of the topic's archive, with `until` as its
/// ending point when one is given.
fn replay_policy(from: &str, until: Option<&str>) -> String {
    match until {
        Some(until) => format!(
            r#"{{"PointType":"Timestamp","StartingPoint":"{from}","EndingPoint":"{until}"}}"#
        ),
        None => format!(r#"{{"PointType":"Timestamp","StartingPoint":"{from}"}}"#),
    }
}

/// A FIFO topic called `name` that archives its messages for `days`, with its ARN and the
/// `BeginningArchiveTime` it reports — "the earliest starting point at which a message in
/// the topic's archive can be replayed from"
/// (`GetTopicAttributesResponse$Attributes` doc).
async fn given_archiving_topic(emulator: &Emulator, name: &str, days: &str) -> (String, String) {
    let created = sns(
        emulator,
        "CreateTopic",
        &[
            ("Name", &format!("{name}.fifo")),
            ("Attributes.entry.1.key", "FifoTopic"),
            ("Attributes.entry.1.value", "true"),
            ("Attributes.entry.2.key", "ArchivePolicy"),
            ("Attributes.entry.2.value", &archive_policy(days)),
            ("Attributes.entry.3.key", "ContentBasedDeduplication"),
            ("Attributes.entry.3.value", "true"),
        ],
    )
    .await;
    let topic = element(&created, "TopicArn");
    let read = sns(emulator, "GetTopicAttributes", &[("TopicArn", &topic)]).await;
    let beginning = attribute(&read, "BeginningArchiveTime")
        .unwrap_or_else(|| panic!("no BeginningArchiveTime in {read}"));
    (topic, beginning)
}

/// The value of one attribute of an awsQuery attribute map response, if the map carries
/// it: the entries are `<entry><key>…</key><value>…</value></entry>` pairs, which the
/// positional element reader of this suite cannot pair up on its own.
fn attribute(body: &str, name: &str) -> Option<String> {
    let needle = format!("<key>{name}</key>");
    let start = body.find(&needle)? + needle.len();
    let rest = &body[start..];
    let open = rest.find("<value>")? + "<value>".len();
    let end = rest[open..].find("</value>")?;
    Some(xml_unescape(&rest[open..open + end]))
}

/// Decode the XML entities an awsQuery response escapes, so a JSON attribute value reads
/// back as the document it was written as.
fn xml_unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Publish `body` to the FIFO topic `topic` in the message group `group`.
async fn publish(emulator: &Emulator, topic: &str, group: &str, body: &str) {
    sns(
        emulator,
        "Publish",
        &[
            ("TopicArn", topic),
            ("Message", body),
            ("MessageGroupId", group),
        ],
    )
    .await;
}

/// Subscribe the queue `queue_arn` to `topic` raw, with the replay policy `replay` when one
/// is given, and answer the subscription's ARN.
async fn subscribe(
    emulator: &Emulator,
    topic: &str,
    queue_arn: &str,
    replay: Option<&str>,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("TopicArn", topic),
        ("Protocol", "sqs"),
        ("Endpoint", queue_arn),
        ("ReturnSubscriptionArn", "true"),
        ("Attributes.entry.1.key", "RawMessageDelivery"),
        ("Attributes.entry.1.value", "true"),
    ];
    if let Some(replay) = replay {
        params.push(("Attributes.entry.2.key", "ReplayPolicy"));
        params.push(("Attributes.entry.2.value", replay));
    }
    element(&sns(emulator, "Subscribe", &params).await, "SubscriptionArn")
}

/// The `ReplayStatus` one subscription reports, or `None` when it reports none.
async fn replay_status(emulator: &Emulator, subscription: &str) -> Option<String> {
    let read = sns(
        emulator,
        "GetSubscriptionAttributes",
        &[("SubscriptionArn", subscription)],
    )
    .await;
    attribute(&read, "ReplayStatus")
}

/// The archived messages of a window are redelivered to the subscription that asks for a
/// replay, in publish order, while a sibling subscription that asks for none receives only
/// what is published after it exists — whichever call wrote the replay policy.
#[tokio::test]
async fn a_replay_redelivers_the_archived_messages_of_its_window_in_publish_order() {
    // (case, whether `Subscribe` carried the policy — else `SetSubscriptionAttributes`
    //  wrote it afterwards)
    let rows: &[(&str, bool)] = &[
        ("the replay policy carried by Subscribe", true),
        ("the replay policy written by SetSubscriptionAttributes", false),
    ];

    for (index, (case, at_subscribe)) in rows.iter().enumerate() {
        let emulator = emulator();
        let (topic, beginning) =
            given_archiving_topic(&emulator, &format!("replayed-{index:02}"), "1").await;

        // Three messages published BEFORE any subscription exists: nothing received them,
        // and the archive holds them all the same.
        let archived = ["first", "second", "third"];
        for body in archived {
            publish(&emulator, &topic, "orders", body).await;
        }

        let (live_url, live_arn) =
            given_fifo_queue(&emulator, &format!("replay-live-{index:02}")).await;
        let (replay_url, replay_arn) =
            given_fifo_queue(&emulator, &format!("replay-backfill-{index:02}")).await;
        // One subscription asks for no replay…
        let live = subscribe(&emulator, &topic, &live_arn, None).await;
        // …and one asks to replay from where the topic's replayable window starts.
        let policy = replay_policy(&beginning, None);
        let replaying = if *at_subscribe {
            subscribe(&emulator, &topic, &replay_arn, Some(&policy)).await
        } else {
            let arn = subscribe(&emulator, &topic, &replay_arn, None).await;
            sns(
                &emulator,
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &arn),
                    ("AttributeName", "ReplayPolicy"),
                    ("AttributeValue", &policy),
                ],
            )
            .await;
            arn
        };

        // A further message published LIVE, after both subscriptions exist.
        publish(&emulator, &topic, "orders", "fourth").await;

        let replayed: Vec<String> = drain(&emulator, &replay_url)
            .await
            .into_iter()
            .map(|(body, _)| body)
            .collect();
        assert_eq!(
            replayed,
            vec![
                "first".to_string(),
                "second".to_string(),
                "third".to_string(),
                "fourth".to_string()
            ],
            "case {case:?}: the replay must redeliver the archive in publish order, \
             followed by the live message, got {replayed:?}"
        );

        let live_bodies: Vec<String> = drain(&emulator, &live_url)
            .await
            .into_iter()
            .map(|(body, _)| body)
            .collect();
        assert_eq!(
            live_bodies,
            vec!["fourth".to_string()],
            "case {case:?}: a subscription that asked for no replay is not backfilled, \
             got {live_bodies:?}"
        );
        assert_eq!(
            replay_status(&emulator, &live).await,
            None,
            "case {case:?}: a subscription with no replay policy reports no replay status"
        );

        // The replay settles: it reports the state it starts in until the span it takes
        // has passed, and `Completed` afterwards.
        let starting = replay_status(&emulator, &replaying).await;
        assert_eq!(
            starting.as_deref(),
            Some(REPLAY_STATUS_PENDING),
            "case {case:?}: a replay that has just started is still pending, got \
             {starting:?}"
        );
        tokio::time::sleep(REPLAY_SETTLES_AFTER + std::time::Duration::from_millis(50)).await;
        let settled = replay_status(&emulator, &replaying).await;
        assert_eq!(
            settled.as_deref(),
            Some(REPLAY_STATUS_COMPLETED),
            "case {case:?}: a settled replay has redelivered everything, got {settled:?}"
        );
    }
}

/// One window row: the case, which archived message the window starts at, which one it
/// ends after (`None` for a policy that names no ending point), and the bodies the replay
/// must redeliver.
type WindowCase<'a> = (&'a str, usize, Option<usize>, &'a [&'a str]);

/// The window a replay policy names decides WHICH archived messages are redelivered: the
/// starting point selects the messages published at or after it, and an ending point stops
/// the replay there.
#[tokio::test]
async fn a_replay_redelivers_the_messages_inside_the_window_its_policy_names() {
    let rows: &[WindowCase<'_>] = &[
        ("the whole archive", 0, None, &["first", "second", "third"]),
        ("from the second message on", 1, None, &["second", "third"]),
        ("from the last message on", 2, None, &["third"]),
        ("the first message alone", 0, Some(0), &["first"]),
        ("the first two messages", 0, Some(1), &["first", "second"]),
    ];

    let emulator = emulator();
    let (topic, beginning) = given_archiving_topic(&emulator, "windowed", "365").await;
    assert!(
        !beginning.is_empty(),
        "the archiving topic must report where its replayable window starts"
    );

    // Three messages, with the instant before each publish remembered, so a window is
    // named relative to the archive this test built rather than by a value copied from a
    // recording. The publishes are a second apart, because an instant is named in the
    // RFC 3339 form this service reports one in, whose resolution is a second — so
    // `marks[i]` is before message `i` and after message `i - 1`.
    let archived = ["first", "second", "third"];
    let mut marks: Vec<String> = Vec::new();
    for body in archived {
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        marks.push(now_rfc3339());
        publish(&emulator, &topic, "orders", body).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    marks.push(now_rfc3339());

    for (index, (case, from, until, expected)) in rows.iter().enumerate() {
        let (url, arn) = given_fifo_queue(&emulator, &format!("windowed-{index:02}")).await;
        // An ending point AFTER message `until` is the mark before the message that
        // follows it.
        let ending = until.map(|until| marks[until + 1].clone());
        let policy = replay_policy(&marks[*from], ending.as_deref());
        subscribe(&emulator, &topic, &arn, Some(&policy)).await;
        let replayed: Vec<String> = drain(&emulator, &url)
            .await
            .into_iter()
            .map(|(body, _)| body)
            .collect();
        let wanted: Vec<String> = expected.iter().map(|body| (*body).to_string()).collect();
        assert_eq!(
            replayed, wanted,
            "case {case:?}: expected {wanted:?} replayed, got {replayed:?}"
        );
    }
}

/// Now, in the RFC 3339 form this service reports and reads an instant in.
fn now_rfc3339() -> String {
    ::aws_smithy_types::DateTime::from(std::time::SystemTime::now())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the clock renders as an RFC 3339 instant")
}

/// A replay policy that names no window the service can read redelivers nothing and says
/// so: the replay could not complete, which is what its status reports.
#[tokio::test]
async fn a_replay_policy_naming_no_readable_window_reports_a_failed_replay() {
    let emulator = emulator();
    let (topic, _) = given_archiving_topic(&emulator, "unreadable-window", "1").await;
    publish(&emulator, &topic, "orders", "archived").await;

    // (case, the policy document — each a JSON object the attribute accepts, none of them
    //  naming an instant the replay can start from)
    let rows: &[(&str, &str)] = &[
        ("a policy with no starting point", r#"{"PointType":"Timestamp"}"#),
        (
            "a starting point that is not an instant",
            r#"{"PointType":"Timestamp","StartingPoint":"yesterday"}"#,
        ),
        (
            "a starting point that is not a string",
            r#"{"PointType":"Timestamp","StartingPoint":17}"#,
        ),
    ];

    for (index, (case, policy)) in rows.iter().enumerate() {
        let (url, arn) = given_fifo_queue(&emulator, &format!("unreadable-{index:02}")).await;
        let subscription = subscribe(&emulator, &topic, &arn, Some(policy)).await;
        let replayed = drain(&emulator, &url).await;
        assert!(
            replayed.is_empty(),
            "case {case:?}: a window that cannot be read redelivers nothing, got \
             {replayed:?}"
        );
        let status = replay_status(&emulator, &subscription).await;
        assert_eq!(
            status.as_deref(),
            Some(REPLAY_STATUS_FAILED),
            "case {case:?}: a replay that could not read its window has failed, got \
             {status:?}"
        );
    }
}

/// The archive is the TOPIC's, so it holds every message published while the archive
/// policy is in force and nothing published before it — and a topic that stops archiving
/// has nothing left to replay.
#[tokio::test]
async fn only_what_was_published_while_the_topic_archives_is_replayed() {
    let emulator = emulator();
    // A FIFO topic that archives NOTHING to start with.
    let created = sns(
        &emulator,
        "CreateTopic",
        &[
            ("Name", "late-archive.fifo"),
            ("Attributes.entry.1.key", "FifoTopic"),
            ("Attributes.entry.1.value", "true"),
            ("Attributes.entry.2.key", "ContentBasedDeduplication"),
            ("Attributes.entry.2.value", "true"),
        ],
    )
    .await;
    let topic = element(&created, "TopicArn");
    publish(&emulator, &topic, "orders", "before-archiving").await;

    // Archiving comes into force now, so the messages published from here on are the ones
    // a replay can reach.
    sns(
        &emulator,
        "SetTopicAttributes",
        &[
            ("TopicArn", &topic),
            ("AttributeName", "ArchivePolicy"),
            ("AttributeValue", &archive_policy("1")),
        ],
    )
    .await;
    let read = sns(&emulator, "GetTopicAttributes", &[("TopicArn", &topic)]).await;
    let beginning = attribute(&read, "BeginningArchiveTime")
        .unwrap_or_else(|| panic!("no BeginningArchiveTime in {read}"));
    publish(&emulator, &topic, "orders", "while-archiving").await;

    let (url, arn) = given_fifo_queue(&emulator, "late-archive-queue").await;
    let policy = replay_policy(&beginning, None);
    subscribe(&emulator, &topic, &arn, Some(&policy)).await;
    let replayed: Vec<String> = drain(&emulator, &url)
        .await
        .into_iter()
        .map(|(body, _)| body)
        .collect();
    assert_eq!(
        replayed,
        vec!["while-archiving".to_string()],
        "only the message published while the topic archives can be replayed, got \
         {replayed:?}"
    );

    // The archive policy removed, the archive goes with it: a replay of the same window
    // now reaches nothing, and the policy itself is refused because the topic archives
    // nothing.
    sns(
        &emulator,
        "SetTopicAttributes",
        &[
            ("TopicArn", &topic),
            ("AttributeName", "ArchivePolicy"),
            ("AttributeValue", ""),
        ],
    )
    .await;
    let (_, bare_arn) = given_fifo_queue(&emulator, "late-archive-bare").await;
    let answer = sns(
        &emulator,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "sqs"),
            ("Endpoint", &bare_arn),
            ("ReturnSubscriptionArn", "true"),
        ],
    )
    .await;
    assert!(
        !element(&answer, "SubscriptionArn").is_empty(),
        "a subscription of a topic that archives nothing is still created, got {answer}"
    );
}

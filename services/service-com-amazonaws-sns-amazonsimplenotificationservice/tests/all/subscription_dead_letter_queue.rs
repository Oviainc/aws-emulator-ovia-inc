// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the REDRIVE stage of SNS delivery: a notification that cannot be
//! delivered to a subscription is held in that subscription's dead-letter queue.
//!
//! The RULE asserted here: "RedrivePolicy – When specified, sends undeliverable messages to
//! the specified Amazon SQS dead-letter queue. Messages that can't be delivered due to client
//! errors (for example, when the subscribed endpoint is unreachable) […] are held in the
//! dead-letter queue for further analysis or reprocessing"
//! (`SetSubscriptionAttributesInput$AttributeName`) — so a publish whose delivery to a
//! subscription fails writes ONE copy of that notification to the queue that subscription's
//! own redrive policy names, in the delivery format that subscription would have received
//! (the JSON envelope, or the bare body with the published attributes as the queue's own when
//! `RawMessageDelivery` is `true`), and a publish that WAS delivered is never copied.
//!
//! The rows differ along what the rule generalises over: why the delivery failed (an endpoint
//! that no longer exists, an endpoint whose own policy does not allow the send), the delivery
//! format of the failing subscription, whether the subscription asks for a redrive at all, and
//! how many publishes went undelivered.
//!
//! SNS and SQS run on ONE engine here, so a delivery and a redrive are observed where a
//! client observes them: on the queue.

use serde_json::Value;

use super::sqs_subscription_delivery::{
    drain, element, emulator, given_queue, given_queue_without_policy, sns, sqs, Emulator,
};

/// The redrive policy naming `queue_arn` as the dead-letter target.
fn redrive_to(queue_arn: &str) -> String {
    serde_json::json!({"deadLetterTargetArn": queue_arn}).to_string()
}

/// Subscribe the queue at `queue_arn` to `topic`, answering the subscription's ARN.
///
/// `raw` asks for raw message delivery; `dead_letter` is the queue undeliverable
/// notifications are held in, when the subscription asks for one.
async fn given_subscription(
    emulator: &Emulator,
    topic: &str,
    queue_arn: &str,
    raw: bool,
    dead_letter: Option<&str>,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("TopicArn", topic),
        ("Protocol", "sqs"),
        ("Endpoint", queue_arn),
        ("ReturnSubscriptionArn", "true"),
    ];
    if raw {
        params.push(("Attributes.entry.1.key", "RawMessageDelivery"));
        params.push(("Attributes.entry.1.value", "true"));
    }
    let subscription = element(
        &sns(emulator, "Subscribe", &params).await,
        "SubscriptionArn",
    );
    if let Some(dead_letter) = dead_letter {
        let policy = redrive_to(dead_letter);
        sns(
            emulator,
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &subscription),
                ("AttributeName", "RedrivePolicy"),
                ("AttributeValue", &policy),
            ],
        )
        .await;
        // The write reads back: a client that configures a redrive confirms it before it
        // relies on it.
        let read = sns(
            emulator,
            "GetSubscriptionAttributes",
            &[("SubscriptionArn", &subscription)],
        )
        .await;
        assert!(
            read.contains(dead_letter),
            "the RedrivePolicy of {subscription} must name {dead_letter}, got {read}"
        );
    }
    subscription
}

/// Delete the queue at `url`.
async fn delete_queue(emulator: &Emulator, url: &str) {
    sqs(
        emulator,
        "DeleteQueue",
        &serde_json::json!({"QueueUrl": url}),
    )
    .await;
}

/// The notification envelopes among `messages`, parsed.
fn envelopes(case: &str, messages: &[(String, Value)]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|(body, _)| serde_json::from_str::<Value>(body).ok())
        .filter(|parsed| parsed["Type"].as_str() == Some("Notification"))
        .inspect(|parsed| {
            assert!(
                parsed["MessageId"].as_str().is_some(),
                "case {case:?}: an envelope must carry the id of the published message, got \
                 {parsed}"
            );
        })
        .collect()
}

/// A notification that cannot be delivered to its subscription is held in the dead-letter
/// queue that subscription names, in that subscription's own delivery format.
#[tokio::test]
async fn an_undeliverable_notification_is_held_in_the_subscriptions_dead_letter_queue() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "redrive-topic")]).await,
        "TopicArn",
    );
    let (dlq_url, dlq_arn) = given_queue(&emulator, "redrive-dead-letters").await;
    let (envelope_url, envelope_arn) = given_queue(&emulator, "redrive-envelope").await;
    let (raw_url, raw_arn) = given_queue(&emulator, "redrive-raw").await;
    let (no_redrive_url, no_redrive_arn) = given_queue(&emulator, "redrive-none").await;
    let (healthy_url, healthy_arn) = given_queue(&emulator, "redrive-healthy").await;

    let envelope_subscription =
        given_subscription(&emulator, &topic, &envelope_arn, false, Some(&dlq_arn)).await;
    let raw_subscription =
        given_subscription(&emulator, &topic, &raw_arn, true, Some(&dlq_arn)).await;
    // A subscription that asks for no redrive has nowhere to hold an undeliverable
    // notification, so it leaves nothing behind.
    let _no_redrive = given_subscription(&emulator, &topic, &no_redrive_arn, false, None).await;
    let _healthy =
        given_subscription(&emulator, &topic, &healthy_arn, false, Some(&dlq_arn)).await;

    // The control leg: while every endpoint exists, each subscription receives the
    // notification and NOTHING is held in the dead-letter queue.
    sns(
        &emulator,
        "Publish",
        &[("TopicArn", &topic), ("Message", "healthy")],
    )
    .await;
    for (case, url) in [
        ("the enveloped subscriber", &envelope_url),
        ("the raw subscriber", &raw_url),
        ("the subscriber with no redrive", &no_redrive_url),
        ("the subscriber whose queue is never deleted", &healthy_url),
    ] {
        let delivered = drain(&emulator, url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected the healthy publish to be delivered once, got {delivered:?}"
        );
    }
    let parked = drain(&emulator, &dlq_url).await;
    assert!(
        parked.is_empty(),
        "a publish that WAS delivered must not be held in the dead-letter queue, got {parked:?}"
    );

    // Now the endpoints are gone: every delivery to them is a client error.
    delete_queue(&emulator, &envelope_url).await;
    delete_queue(&emulator, &raw_url).await;
    delete_queue(&emulator, &no_redrive_url).await;

    let message = "dead";
    sns(
        &emulator,
        "Publish",
        &[
            ("TopicArn", &topic),
            ("Message", message),
            ("Subject", "dead-letter"),
            ("MessageAttributes.entry.1.Name", "n"),
            ("MessageAttributes.entry.1.Value.DataType", "Number"),
            ("MessageAttributes.entry.1.Value.StringValue", "+5"),
        ],
    )
    .await;

    let held = drain(&emulator, &dlq_url).await;
    assert_eq!(
        held.len(),
        2,
        "exactly one copy per subscription that could not be delivered to — the two deleted \
         endpoints that ask for a redrive — got {held:?}"
    );

    // The enveloped subscription's copy: the notification it would have received, carrying
    // its OWN unsubscribe link.
    let enveloped = envelopes("the enveloped copy", &held);
    assert_eq!(
        enveloped.len(),
        1,
        "exactly one of the held copies is the enveloped subscription's, got {held:?}"
    );
    let envelope = &enveloped[0];
    assert_eq!(
        envelope["TopicArn"].as_str(),
        Some(topic.as_str()),
        "the held envelope must name the topic published to, got {envelope}"
    );
    assert_eq!(
        envelope["Message"].as_str(),
        Some(message),
        "the held envelope must carry the message as published, got {envelope}"
    );
    assert_eq!(
        envelope["Subject"].as_str(),
        Some("dead-letter"),
        "the held envelope must carry the subject as published, got {envelope}"
    );
    assert_eq!(
        envelope["MessageAttributes"]["n"],
        serde_json::json!({"Type": "Number", "Value": "+5"}),
        "the held envelope must carry the published attributes as the envelope carries them, \
         got {envelope}"
    );
    let unsubscribe = envelope["UnsubscribeURL"].as_str().unwrap_or_default();
    assert!(
        unsubscribe.contains(&format!("SubscriptionArn={envelope_subscription}")),
        "the held envelope must carry the unsubscribe link of the subscription that could not \
         be delivered to ({envelope_subscription}), got {unsubscribe:?}"
    );
    assert!(
        !unsubscribe.contains(&raw_subscription),
        "the held envelope must not carry a sibling subscription's unsubscribe link, got \
         {unsubscribe:?}"
    );

    // The raw subscription's copy: the bare body, with the published attributes as the
    // queue's own.
    let raw: Vec<&(String, Value)> = held.iter().filter(|(body, _)| body == message).collect();
    assert_eq!(
        raw.len(),
        1,
        "exactly one of the held copies is the raw subscription's bare body, got {held:?}"
    );
    let (_, attributes) = raw[0];
    assert_eq!(
        attributes["n"]["DataType"].as_str(),
        Some("Number"),
        "the raw copy must carry the published attribute as the queue's own, got {attributes}"
    );
    assert_eq!(
        attributes["n"]["StringValue"].as_str(),
        Some("5"),
        "a Number attribute carries the number it names, got {attributes}"
    );

    // The endpoint that still exists received the second publish too, and nothing of it was
    // held: a redrive copies only what could not be delivered.
    let delivered = drain(&emulator, &healthy_url).await;
    assert_eq!(
        delivered.len(),
        1,
        "the subscription whose endpoint still exists must receive the publish, got \
         {delivered:?}"
    );

    // One copy per undeliverable publish, per subscription: three more publishes leave three
    // more copies for each of the two subscriptions that ask for a redrive.
    for index in 0..3 {
        sns(
            &emulator,
            "Publish",
            &[
                ("TopicArn", &topic),
                ("Message", &format!("dead-{index}")),
            ],
        )
        .await;
    }
    let held = drain(&emulator, &dlq_url).await;
    assert_eq!(
        held.len(),
        6,
        "three undeliverable publishes leave one copy per publish per subscription, got \
         {held:?}"
    );
}

/// A delivery the subscribed queue's own policy does not allow is a client error too, so it
/// is held in the subscription's dead-letter queue.
#[tokio::test]
async fn a_delivery_the_endpoint_does_not_allow_is_held_in_the_dead_letter_queue() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "redrive-denied")]).await,
        "TopicArn",
    );
    let (dlq_url, dlq_arn) = given_queue(&emulator, "denied-dead-letters").await;
    // A queue that grants this service nothing: the delivery to it cannot be made.
    let (denied_url, denied_arn) =
        given_queue_without_policy(&emulator, "denied-endpoint").await;
    let subscription =
        given_subscription(&emulator, &topic, &denied_arn, false, Some(&dlq_arn)).await;

    sns(
        &emulator,
        "Publish",
        &[("TopicArn", &topic), ("Message", "withheld")],
    )
    .await;

    let withheld = drain(&emulator, &denied_url).await;
    assert!(
        withheld.is_empty(),
        "a queue that grants this service nothing receives nothing, got {withheld:?}"
    );
    let held = drain(&emulator, &dlq_url).await;
    assert_eq!(
        held.len(),
        1,
        "the notification that could not be delivered must be held once, got {held:?}"
    );
    let enveloped = envelopes("the held copy", &held);
    assert_eq!(
        enveloped.len(),
        1,
        "the held copy is the subscription's own delivery format, got {held:?}"
    );
    let unsubscribe = enveloped[0]["UnsubscribeURL"].as_str().unwrap_or_default();
    assert!(
        unsubscribe.contains(&format!("SubscriptionArn={subscription}")),
        "the held envelope must name the subscription that could not be delivered to, got \
         {unsubscribe:?}"
    );
}

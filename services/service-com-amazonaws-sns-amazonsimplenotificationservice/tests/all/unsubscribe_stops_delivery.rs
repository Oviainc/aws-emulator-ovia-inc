// SPDX-License-Identifier: CC0-1.0

//! `Unsubscribe` takes the subscription out of the topic's delivery set, so a publish issued
//! BEFORE it reaches that endpoint and a publish issued AFTER it does not — end to end over
//! the wire, with SQS mounted on the same engine so what was delivered is observed where a
//! client observes it: on the queue.
//!
//! The RULE asserted here: "Deletes a subscription" (`Unsubscribe`), and SNS "delivers the
//! message to each endpoint that is subscribed to the topic" (`Publish`) — so the deleted
//! subscription is no longer one of them, from the moment the delete answers, while every
//! OTHER subscription of the same topic keeps receiving. A subscription deleted is deleted
//! rather than paused: subscribing the same endpoint again is a NEW subscription, which
//! receives what is published after it.
//!
//! The rows differ along what the rule generalises over: how many publishes precede the
//! delete, how many follow it, and whether the endpoint is subscribed again afterwards. A
//! SIBLING subscription of the same topic rides along throughout, so a message missing from
//! the deleted subscription's queue is one the publish itself did deliver.

use crate::sqs_subscription_delivery::{
    drain, element, emulator, given_queue, sns, sns_call, Emulator,
};

/// Subscribe the queue `queue_arn` to `topic` with raw delivery, and answer the
/// subscription's ARN.
async fn subscribe(emulator: &Emulator, topic: &str, queue_arn: &str) -> String {
    element(
        &sns(
            emulator,
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", "sqs"),
                ("Endpoint", queue_arn),
                ("ReturnSubscriptionArn", "true"),
                ("Attributes.entry.1.key", "RawMessageDelivery"),
                ("Attributes.entry.1.value", "true"),
            ],
        )
        .await,
        "SubscriptionArn",
    )
}

/// Publish `message` to `topic`.
async fn publish(emulator: &Emulator, topic: &str, message: &str) {
    sns(
        emulator,
        "Publish",
        &[("TopicArn", topic), ("Message", message)],
    )
    .await;
}

/// How many messages the queue at `url` holds, taking them off it.
async fn depth(emulator: &Emulator, url: &str) -> usize {
    drain(emulator, url).await.len()
}

/// One row: the case, how many publishes precede the delete, and how many follow it.
type Round<'a> = (&'a str, usize, usize);

#[tokio::test]
async fn a_publish_before_unsubscribe_is_delivered_and_one_after_it_is_not() {
    let rows: &[Round<'_>] = &[
        ("one publish before, one after", 1, 1),
        ("one publish before, three after", 1, 3),
        ("three publishes before, one after", 3, 1),
        ("no publish before, two after", 0, 2),
        ("two publishes before, none after", 2, 0),
    ];

    for (case, before, after) in rows {
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "teardown")]).await,
            "TopicArn",
        );
        let (url, arn) = given_queue(&emulator, "teardown-queue").await;
        // A sibling subscriber of the same topic, which is never torn down: it is what
        // tells "the publish delivered nothing" from "the publish delivered elsewhere".
        let (sibling_url, sibling_arn) = given_queue(&emulator, "teardown-sibling-queue").await;
        let subscription = subscribe(&emulator, &topic, &arn).await;
        subscribe(&emulator, &topic, &sibling_arn).await;

        for index in 0..*before {
            publish(&emulator, &topic, &format!("before-{index}")).await;
        }
        let delivered = depth(&emulator, &url).await;
        assert_eq!(
            delivered, *before,
            "case {case:?}: a subscribed queue must receive every publish that precedes the \
             teardown, expected {before}, got {delivered}"
        );

        let removed = sns_call(
            &emulator,
            "Unsubscribe",
            &[("SubscriptionArn", &subscription)],
        )
        .await;
        assert_eq!(
            removed.0, 200,
            "case {case:?}: Unsubscribe must succeed, got {} / {}",
            removed.0, removed.1
        );

        for index in 0..*after {
            publish(&emulator, &topic, &format!("after-{index}")).await;
        }
        let leaked = depth(&emulator, &url).await;
        assert_eq!(
            leaked, 0,
            "case {case:?}: an unsubscribed endpoint must receive nothing published after the \
             teardown, got {leaked} message(s)"
        );
        // Every publish of the round did reach the topic's remaining subscriber, so the
        // empty queue above is the teardown's doing and not a publish that delivered
        // nothing at all.
        let sibling = depth(&emulator, &sibling_url).await;
        assert_eq!(
            sibling,
            before + after,
            "case {case:?}: the sibling subscription must receive every publish of the round, \
             expected {}, got {sibling}",
            before + after
        );
    }
}

/// Subscribing the same endpoint again after the teardown is a NEW subscription: it receives
/// what is published after it, and nothing published while the endpoint was unsubscribed.
#[tokio::test]
async fn subscribing_the_same_endpoint_again_resumes_delivery_without_a_backlog() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "resubscribe")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "resubscribe-queue").await;

    let first = subscribe(&emulator, &topic, &arn).await;
    publish(&emulator, &topic, "while-subscribed").await;
    let delivered = drain(&emulator, &url).await;
    assert_eq!(
        delivered.len(),
        1,
        "the first subscription must receive the publish that follows it, got {delivered:?}"
    );

    sns(&emulator, "Unsubscribe", &[("SubscriptionArn", &first)]).await;
    publish(&emulator, &topic, "while-unsubscribed").await;

    let second = subscribe(&emulator, &topic, &arn).await;
    assert_ne!(
        second, first,
        "a subscription created after the teardown is a new one, got {second:?}"
    );
    let backlog = drain(&emulator, &url).await;
    assert!(
        backlog.is_empty(),
        "nothing published while the endpoint was unsubscribed may be handed to the new \
         subscription, got {backlog:?}"
    );

    publish(&emulator, &topic, "after-resubscribe").await;
    let resumed = drain(&emulator, &url).await;
    let bodies: Vec<&str> = resumed.iter().map(|(body, _)| body.as_str()).collect();
    assert_eq!(
        bodies,
        vec!["after-resubscribe"],
        "the new subscription must receive the publishes that follow it, got {resumed:?}"
    );
}

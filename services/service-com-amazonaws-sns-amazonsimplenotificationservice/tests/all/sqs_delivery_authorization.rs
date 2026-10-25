// SPDX-License-Identifier: CC0-1.0

//! A notification reaches a subscribed SQS queue only where that queue's OWN policy allows
//! SNS to send it one, end to end over the wire through the real `MultiServiceRouter` with
//! SQS mounted on the same engine — so a withheld delivery is observed where a client
//! observes it: the queue stays empty.
//!
//! The RULE asserted here: SNS delivers to a subscribed queue as the service principal
//! `sns.amazonaws.com`, not as the queue's owner, so the delivery is an `sqs:SendMessage` the
//! queue's resource policy has to grant ("you must ensure that the Amazon SQS queue policy
//! allows the Amazon SNS topic to send messages to the queue"). A queue whose policy states
//! no such grant receives NOTHING — it is not a delivery that silently succeeds — and a
//! client that repairs the policy sees the next notification arrive, while the ones withheld
//! before the repair never do.
//!
//! The rows differ along what the rule generalises over: whether the queue carries a policy
//! at all, which principal it grants to, which action, which resource, how the grant is
//! scoped by condition (the publishing topic's ARN, the publishing account), and whether a
//! `Deny` of the same send stands beside the grant.

use serde_json::{json, Value};

use crate::sqs_subscription_delivery::{
    drain, element, emulator, given_queue_without_policy, grant_to_sns, set_queue_policy, sns,
    sqs, Emulator,
};

/// The account of every ARN this suite's emulator mints, read off the queue's own ARN rather
/// than written out: the grant rows that name an account have to name the one SNS reports as
/// the publishing topic's.
fn account_of(arn: &str) -> String {
    arn.split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("no account in {arn}"))
        .to_string()
}

/// The Region of an ARN this emulator minted.
fn region_of(arn: &str) -> String {
    arn.split(':')
        .nth(3)
        .unwrap_or_else(|| panic!("no region in {arn}"))
        .to_string()
}

/// One statement document around `statement`.
fn policy(statement: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// How many messages the queue at `url` holds.
async fn depth(emulator: &Emulator, url: &str) -> usize {
    drain(emulator, url).await.len()
}

#[tokio::test]
async fn a_queue_receives_a_notification_only_where_its_policy_grants_the_send() {
    // (case, the queue's `Policy` built from (queue ARN, topic ARN), how many messages the
    //  queue must hold after one publish)
    type PolicyOf = fn(&str, &str) -> Option<String>;
    let rows: &[(&str, PolicyOf, usize)] = &[
        (
            "no policy at all, which grants nobody",
            |_, _| None,
            0,
        ),
        (
            "the grant AWS documents for an SNS subscriber",
            |queue, _| Some(grant_to_sns(queue)),
            1,
        ),
        (
            "that grant scoped to the publishing topic",
            |queue, topic| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                    "Condition": {"ArnEquals": {"aws:SourceArn": topic}},
                })))
            },
            1,
        ),
        (
            "that grant scoped to ANOTHER topic",
            |queue, topic| {
                let elsewhere =
                    format!("arn:aws:sns:{}:{}:other", region_of(topic), account_of(topic));
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                    "Condition": {"ArnEquals": {"aws:SourceArn": elsewhere}},
                })))
            },
            0,
        ),
        (
            "that grant scoped by a wildcard over the account's topics",
            |queue, topic| {
                let any = format!("arn:aws:sns:{}:{}:*", region_of(topic), account_of(topic));
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                    "Condition": {"ArnLike": {"aws:SourceArn": any}},
                })))
            },
            1,
        ),
        (
            "that grant scoped by the publishing account",
            |queue, topic| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                    "Condition": {"StringEquals": {"aws:SourceAccount": account_of(topic)}},
                })))
            },
            1,
        ),
        (
            "a grant over every action of the service",
            |queue, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": "sqs:*",
                    "Resource": queue,
                })))
            },
            1,
        ),
        (
            "a grant to every principal",
            |queue, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                })))
            },
            1,
        ),
        (
            "a grant to the queue's owning account only",
            |queue, _| {
                let owner = format!("arn:aws:iam::{}:root", account_of(queue));
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"AWS": owner},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                })))
            },
            0,
        ),
        (
            "a grant of another action to SNS",
            |queue, _| {
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": "sqs:ReceiveMessage",
                    "Resource": queue,
                })))
            },
            0,
        ),
        (
            "a grant naming another queue as its resource",
            |queue, _| {
                let elsewhere = format!("{queue}-elsewhere");
                Some(policy(json!({
                    "Effect": "Allow",
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": "sqs:SendMessage",
                    "Resource": elsewhere,
                })))
            },
            0,
        ),
        (
            "a grant with an explicit Deny of the same send beside it",
            |queue, _| {
                Some(
                    json!({
                        "Version": "2012-10-17",
                        "Statement": [
                            {
                                "Effect": "Allow",
                                "Principal": {"Service": "sns.amazonaws.com"},
                                "Action": "sqs:SendMessage",
                                "Resource": queue,
                            },
                            {
                                "Effect": "Deny",
                                "Principal": {"Service": "sns.amazonaws.com"},
                                "Action": "sqs:SendMessage",
                                "Resource": queue,
                            },
                        ],
                    })
                    .to_string(),
                )
            },
            0,
        ),
        (
            "a statement that states no effect at all",
            |queue, _| {
                Some(policy(json!({
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": "sqs:SendMessage",
                    "Resource": queue,
                })))
            },
            0,
        ),
    ];

    for (case, policy_of, expected) in rows {
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "authorized-fan-out")]).await,
            "TopicArn",
        );
        let (url, arn) = given_queue_without_policy(&emulator, "authorized-subscriber").await;
        if let Some(document) = policy_of(&arn, &topic) {
            set_queue_policy(&emulator, &url, &document).await;
        }
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &arn),
            ],
        )
        .await;
        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", "an event")],
        )
        .await;

        let held = depth(&emulator, &url).await;
        assert_eq!(
            held, *expected,
            "case {case:?}: expected the queue to hold {expected} message(s), got {held}"
        );
    }
}

#[tokio::test]
async fn a_repaired_queue_policy_admits_the_next_notification_and_not_the_withheld_ones() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "repairable")]).await,
        "TopicArn",
    );
    // Two subscribers of ONE topic: one granted from the start, one with no policy at all.
    // The same publish must reach the first and not the second, so a withheld delivery is
    // told apart from a system that delivers nothing.
    let (granted_url, granted_arn) =
        given_queue_without_policy(&emulator, "granted-subscriber").await;
    set_queue_policy(&emulator, &granted_url, &grant_to_sns(&granted_arn)).await;
    let (withheld_url, withheld_arn) =
        given_queue_without_policy(&emulator, "withheld-subscriber").await;
    for endpoint in [&granted_arn, &withheld_arn] {
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
            ],
        )
        .await;
    }

    sns(
        &emulator,
        "Publish",
        &[("TopicArn", &topic), ("Message", "before the repair")],
    )
    .await;
    let delivered = drain(&emulator, &granted_url).await;
    assert_eq!(
        delivered.len(),
        1,
        "the granted queue must receive the notification, got {delivered:?}"
    );
    let withheld = drain(&emulator, &withheld_url).await;
    assert!(
        withheld.is_empty(),
        "the queue with no grant must hold nothing, got {withheld:?}"
    );

    // The repair: the policy the subscriber was missing, written by its owner.
    set_queue_policy(&emulator, &withheld_url, &grant_to_sns(&withheld_arn)).await;
    sns(
        &emulator,
        "Publish",
        &[("TopicArn", &topic), ("Message", "after the repair")],
    )
    .await;
    let repaired = drain(&emulator, &withheld_url).await;
    assert_eq!(
        repaired.len(),
        1,
        "the repaired queue must receive the next notification and not the withheld one, got \
         {repaired:?}"
    );
    let envelope: Value = serde_json::from_str(&repaired[0].0).unwrap_or_else(|error| {
        panic!("the delivered body is not the SNS envelope ({error}): {:?}", repaired[0].0)
    });
    assert_eq!(
        envelope["Message"].as_str(),
        Some("after the repair"),
        "the repaired queue must hold the message published AFTER the repair, got {envelope}"
    );

    // Nothing was lost on the granted arm either: it receives the second publish too.
    let delivered = drain(&emulator, &granted_url).await;
    assert_eq!(
        delivered.len(),
        1,
        "the granted queue must receive the second notification as well, got {delivered:?}"
    );
}

/// A raw-delivery subscriber is authorized the same way: the rule is about the queue's
/// policy, not about the form of the body.
#[tokio::test]
async fn the_envelope_form_does_not_change_whether_a_delivery_is_authorized() {
    // (case, whether the subscription asks for raw delivery, whether the queue grants SNS,
    //  the messages the queue must hold)
    let rows: &[(&str, bool, bool, usize)] = &[
        ("an enveloped subscriber with the grant", false, true, 1),
        ("an enveloped subscriber without it", false, false, 0),
        ("a raw-delivery subscriber with the grant", true, true, 1),
        ("a raw-delivery subscriber without it", true, false, 0),
    ];

    for (case, raw, granted, expected) in rows {
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "raw-or-not")]).await,
            "TopicArn",
        );
        let (url, arn) = given_queue_without_policy(&emulator, "raw-or-not-queue").await;
        if *granted {
            set_queue_policy(&emulator, &url, &grant_to_sns(&arn)).await;
        }
        let subscription = element(
            &sns(
                &emulator,
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", "sqs"),
                    ("Endpoint", &arn),
                ],
            )
            .await,
            "SubscriptionArn",
        );
        if *raw {
            sns(
                &emulator,
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &subscription),
                    ("AttributeName", "RawMessageDelivery"),
                    ("AttributeValue", "true"),
                ],
            )
            .await;
        }
        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", "a payload")],
        )
        .await;

        let held = depth(&emulator, &url).await;
        assert_eq!(
            held, *expected,
            "case {case:?}: expected the queue to hold {expected} message(s), got {held}"
        );
    }
}

/// The same rule governs a BATCHED publish, which fans out through the same delivery path.
#[tokio::test]
async fn a_batched_publish_is_withheld_from_a_queue_that_grants_nothing() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "batched-authorization")]).await,
        "TopicArn",
    );
    let (granted_url, granted_arn) = given_queue_without_policy(&emulator, "batch-granted").await;
    set_queue_policy(&emulator, &granted_url, &grant_to_sns(&granted_arn)).await;
    let (withheld_url, withheld_arn) =
        given_queue_without_policy(&emulator, "batch-withheld").await;
    for endpoint in [&granted_arn, &withheld_arn] {
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
            ],
        )
        .await;
    }

    sns(
        &emulator,
        "PublishBatch",
        &[
            ("TopicArn", &topic),
            ("PublishBatchRequestEntries.member.1.Id", "e0"),
            ("PublishBatchRequestEntries.member.1.Message", "first"),
            ("PublishBatchRequestEntries.member.2.Id", "e1"),
            ("PublishBatchRequestEntries.member.2.Message", "second"),
        ],
    )
    .await;

    let granted = depth(&emulator, &granted_url).await;
    assert_eq!(
        granted, 2,
        "the granted queue must receive one message per batch entry, got {granted}"
    );
    let withheld = depth(&emulator, &withheld_url).await;
    assert_eq!(
        withheld, 0,
        "the queue with no grant must hold nothing of the batch, got {withheld}"
    );
    // The queue really is reachable; it is the grant that was missing.
    let reported = sqs(
        &emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": withheld_url, "AttributeNames": ["ApproximateNumberOfMessages"]}),
    )
    .await;
    assert_eq!(
        reported["Attributes"]["ApproximateNumberOfMessages"].as_str(),
        Some("0"),
        "the withheld queue must report depth 0, got {reported}"
    );
}

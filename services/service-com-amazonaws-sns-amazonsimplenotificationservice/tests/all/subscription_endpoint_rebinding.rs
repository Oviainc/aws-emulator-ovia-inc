// SPDX-License-Identifier: CC0-1.0

//! A subscription is bound to the ENDPOINT ARN it was created with, and that ARN is resolved
//! at DELIVERY time — so an endpoint that is deleted and created again under the same name is
//! reached again by the same, unchanged subscription.
//!
//! The RULE asserted here: "If you send a message to a topic, Amazon SNS delivers the message
//! to each endpoint that is subscribed to the topic. […] When a messageId is returned, the
//! message is saved and Amazon SNS immediately delivers it to subscribers." (`Publish`) — a
//! subscription names its endpoint by ARN and nothing else, so the lifecycle of the resource
//! that ARN names never rewrites the subscription: its `Endpoint`, `SubscriptionArn` and
//! `PendingConfirmation` keep their values while the endpoint is absent
//! (`GetSubscriptionAttributes`), each publish resolves the ARN afresh, and a publish the
//! absent endpoint cannot take is undeliverable NOW — "RedrivePolicy – When specified, sends
//! undeliverable messages to the specified Amazon SQS dead-letter queue"
//! (`SetSubscriptionAttributesInput$AttributeName`) — so it is held in that subscription's
//! dead-letter queue rather than buffered for a later delivery, and the endpoint that exists
//! again receives the publishes made FROM THEN ON and none of the ones made while it was gone.
//!
//! The rows differ along what the rule generalises over: the delivery format of the
//! subscription (the JSON envelope, the raw body), whether the subscription asks for a redrive
//! at all, how many publishes were made while the endpoint was absent (none, one, several),
//! and the form of the endpoint's own name.
//!
//! SNS and SQS run on ONE engine here, so a delivery, a redrive and a re-create are observed
//! where a client observes them: on the queue.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::fifo_archive_replay::attribute;
use super::sqs_subscription_delivery::{
    drain, element, emulator, given_queue, grant_to_sns, queue_arn, set_queue_policy, sns,
    sqs_call, Emulator,
};
use super::subscription_dead_letter_queue::{delete_queue, given_subscription};

/// The wire error code of an awsJson1_0 refusal, which the body carries as its `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// How long the re-create of a just-deleted name is retried for: "If you delete a queue, you
/// must wait at least 60 seconds before creating a queue with the same name" (`CreateQueue`),
/// and Amazon SQS answers `QueueDeletedRecently` until the name is free again — so the name is
/// polled for, exactly as a client backing off would, rather than assumed free at once.
///
/// The budget is far above the span this emulator actually holds a deleted name back for
/// (`QUEUE_DELETED_RECENTLY_INTERVAL` in `service-com-amazonaws-sqs-amazonsqs`'s `state.rs`,
/// which is a fraction of a second), so the poll stays a backoff rather than a wait for AWS's
/// own figure.
const RECREATE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long one re-create attempt waits before the next.
const RECREATE_INTERVAL: Duration = Duration::from_millis(25);

/// Create the queue called `name` again, retrying while the delete still holds the name back,
/// and answer its url.
async fn recreate_queue(emulator: &Emulator, case: &str, name: &str) -> String {
    let deadline = Instant::now() + RECREATE_TIMEOUT;
    loop {
        let (status, parsed) =
            sqs_call(emulator, "CreateQueue", &json!({"QueueName": name})).await;
        if status == 200 {
            return parsed["QueueUrl"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: CreateQueue reported no QueueUrl, got {parsed}")
                })
                .to_string();
        }
        let code = error_code(&parsed);
        assert_eq!(
            code, "QueueDeletedRecently",
            "case {case:?}: a name whose queue was deleted is refused only while the delete \
             holds it back, got {status} with {parsed}"
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the name {name:?} was still held back after \
             {RECREATE_TIMEOUT:?}, got {status} with {parsed}"
        );
        tokio::time::sleep(RECREATE_INTERVAL).await;
    }
}

/// Publish `message` to `topic` and answer the `MessageId` the publish reported.
async fn publish(emulator: &Emulator, topic: &str, message: &str) -> String {
    element(
        &sns(
            emulator,
            "Publish",
            &[("TopicArn", topic), ("Message", message)],
        )
        .await,
        "MessageId",
    )
}

/// What one delivered body carries, read in the delivery format `raw` names: the message, and
/// the `MessageId` of the notification envelope around it — a RAW delivery is the published
/// message and nothing else, so it reports no id.
fn delivered(case: &str, raw: bool, body: &str) -> (String, Option<String>) {
    if raw {
        return (body.to_string(), None);
    }
    let envelope: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: body is not JSON ({error}): {body}"));
    assert_eq!(
        envelope["Type"].as_str(),
        Some("Notification"),
        "case {case:?}: an enveloped delivery must declare its type, got {envelope}"
    );
    let message = envelope["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the envelope carries no Message: {envelope}"))
        .to_string();
    let id = envelope["MessageId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the envelope carries no MessageId: {envelope}"))
        .to_string();
    (message, Some(id))
}

/// One row of the re-bind table.
struct Case<'a> {
    /// What this row is, named by every assertion it makes.
    case: &'a str,
    /// The name of the subscribed queue: deleted, then created again under the SAME name.
    name: &'a str,
    /// Whether the subscription asks for raw message delivery.
    raw: bool,
    /// Whether the subscription names a dead-letter queue for what it cannot receive.
    redrive: bool,
    /// How many messages are published while the endpoint does not exist.
    gap: usize,
}

#[tokio::test]
async fn a_subscription_re_binds_to_its_endpoint_arn_across_a_delete_and_re_create() {
    let rows: &[Case<'_>] = &[
        Case {
            case: "an enveloped subscriber, one publish while the endpoint is gone",
            name: "rebind-envelope",
            raw: false,
            redrive: true,
            gap: 1,
        },
        Case {
            case: "a raw subscriber, three publishes while the endpoint is gone",
            name: "rebind-raw",
            raw: true,
            redrive: true,
            gap: 3,
        },
        Case {
            // A name holding a hyphen and an underscore: the ARN identity of a re-created
            // queue is its name, whatever the name is spelled with.
            case: "a subscriber that asks for no redrive, two publishes while the endpoint is \
                   gone",
            name: "rebind_no-redrive",
            raw: false,
            redrive: false,
            gap: 2,
        },
        Case {
            case: "an endpoint deleted and created again with nothing published in between",
            name: "rebind-no-gap",
            raw: false,
            redrive: true,
            gap: 0,
        },
    ];

    for row in rows {
        let Case {
            case,
            name,
            raw,
            redrive,
            gap,
        } = *row;
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "rebind-topic")]).await,
            "TopicArn",
        );
        let dead_letter = if redrive {
            Some(given_queue(&emulator, &format!("{name}-dlq")).await)
        } else {
            None
        };
        let (url, arn) = given_queue(&emulator, name).await;
        let subscription = given_subscription(
            &emulator,
            &topic,
            &arn,
            raw,
            dead_letter.as_ref().map(|(_, dlq_arn)| dlq_arn.as_str()),
        )
        .await;

        // The control leg: while the endpoint exists the subscription delivers to it, and the
        // delivery carries the id the publish reported.
        let before = publish(&emulator, &topic, "before-the-delete").await;
        let received = drain(&emulator, &url).await;
        assert_eq!(
            received.len(),
            1,
            "case {case:?}: expected the publish to be delivered once before the delete, got \
             {received:?}"
        );
        let (message, id) = delivered(case, raw, &received[0].0);
        assert_eq!(
            message, "before-the-delete",
            "case {case:?}: expected the endpoint to receive the published message, got \
             {message:?}"
        );
        if let Some(id) = id {
            assert_eq!(
                id, before,
                "case {case:?}: the delivered notification must carry the id the publish \
                 reported ({before}), got {id}"
            );
        }

        // The endpoint is gone. The subscription is NOT: it keeps the ARN it was created
        // with, its own ARN and its confirmation.
        delete_queue(&emulator, &url).await;
        let read = sns(
            &emulator,
            "GetSubscriptionAttributes",
            &[("SubscriptionArn", &subscription)],
        )
        .await;
        for (attribute_name, expected) in [
            ("Endpoint", arn.as_str()),
            ("SubscriptionArn", subscription.as_str()),
            ("PendingConfirmation", "false"),
        ] {
            let actual = attribute(&read, attribute_name);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case:?}: deleting the endpoint must leave {attribute_name} at \
                 {expected:?}, got {actual:?} in {read}"
            );
        }
        let listed = sns(
            &emulator,
            "ListSubscriptionsByTopic",
            &[("TopicArn", &topic)],
        )
        .await;
        assert!(
            listed.contains(&subscription),
            "case {case:?}: the topic must still list {subscription}, got {listed}"
        );

        // Every publish made while the endpoint is absent is undeliverable NOW: it is held in
        // the subscription's dead-letter queue, and nothing is buffered for the endpoint.
        let mut gap_markers: Vec<(String, String)> = Vec::new();
        for index in 0..gap {
            let marker = format!("gap-{index}");
            let id = publish(&emulator, &topic, &marker).await;
            gap_markers.push((marker, id));
        }
        if let Some((dlq_url, _)) = dead_letter.as_ref() {
            let held = drain(&emulator, dlq_url).await;
            assert_eq!(
                held.len(),
                gap,
                "case {case:?}: expected one held copy per undeliverable publish ({gap}), got \
                 {held:?}"
            );
            for (marker, id) in &gap_markers {
                let matching: Vec<&(String, Value)> = held
                    .iter()
                    .filter(|(body, _)| &delivered(case, raw, body).0 == marker)
                    .collect();
                assert_eq!(
                    matching.len(),
                    1,
                    "case {case:?}: expected exactly one held copy of {marker:?}, got {held:?}"
                );
                if let (_, Some(held_id)) = delivered(case, raw, &matching[0].0) {
                    assert_eq!(
                        &held_id, id,
                        "case {case:?}: the held notification must carry the id the publish \
                         reported ({id}), got {held_id}"
                    );
                }
            }
        }

        // The name is created again: a queue created under a name that was used before has
        // the SAME ARN, because a queue's ARN names its account, its Region and its name.
        let recreated_url = recreate_queue(&emulator, case, name).await;
        let recreated_arn = queue_arn(&emulator, &recreated_url).await;
        assert_eq!(
            recreated_arn, arn,
            "case {case:?}: a queue created again under the same name must carry the same \
             ARN, got {recreated_arn}"
        );
        // A new queue grants nothing of its own, so the policy an SNS subscriber needs is
        // written again — which is the endpoint's business, not the subscription's.
        set_queue_policy(&emulator, &recreated_url, &grant_to_sns(&recreated_arn)).await;

        // The subscription was never rewritten, so it still reports the same endpoint.
        let read = sns(
            &emulator,
            "GetSubscriptionAttributes",
            &[("SubscriptionArn", &subscription)],
        )
        .await;
        for (attribute_name, expected) in [
            ("Endpoint", arn.as_str()),
            ("SubscriptionArn", subscription.as_str()),
            ("PendingConfirmation", "false"),
        ] {
            let actual = attribute(&read, attribute_name);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case:?}: re-creating the endpoint must leave {attribute_name} at \
                 {expected:?}, got {actual:?} in {read}"
            );
        }

        // Delivery resumes on the UNCHANGED subscription, and only the publishes made from
        // now on are delivered: the ones made while the endpoint was absent are not replayed
        // into it.
        let marker = "after-the-re-create";
        let published = publish(&emulator, &topic, marker).await;
        let received = drain(&emulator, &recreated_url).await;
        assert_eq!(
            received.len(),
            1,
            "case {case:?}: the re-created endpoint must receive the publish made after it \
             exists again, and nothing else, got {received:?}"
        );
        let (message, id) = delivered(case, raw, &received[0].0);
        assert_eq!(
            message, marker,
            "case {case:?}: expected the re-created endpoint to receive {marker:?}, got \
             {message:?}"
        );
        if let Some(id) = id {
            assert_eq!(
                id, published,
                "case {case:?}: the delivered notification must carry the id the publish \
                 reported ({published}), got {id}"
            );
        }
        for (gap_marker, _) in &gap_markers {
            assert_ne!(
                &message, gap_marker,
                "case {case:?}: a publish made while the endpoint was absent must never be \
                 replayed into the re-created endpoint, got {message:?}"
            );
        }
        if let Some((dlq_url, _)) = dead_letter.as_ref() {
            let held = drain(&emulator, dlq_url).await;
            assert!(
                held.is_empty(),
                "case {case:?}: a publish that WAS delivered must leave nothing in the \
                 dead-letter queue, got {held:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the destination a publish names: exactly one of them.
//!
//! The RULE asserted here: "If you don't specify a value for the TargetArn parameter, you
//! must specify a value for the PhoneNumber or TopicArn parameters" (`PublishInput$TargetArn`)
//! — the destination members are mutually exclusive, so a publish naming a topic through
//! either member is served, and a publish naming more than one destination is refused with
//! `InvalidParameter` (HTTP 400) and the sentence SNS states for that condition, which carries
//! no value from the request.
//!
//! The rows differ along what the rule generalises over: which member carried the topic, and
//! whether a second destination member was named beside it.

use super::topics_subscriptions_publish::{Answer, Sns};

/// The sentence a publish naming both a topic and a target is refused with. Every word of it
/// is fixed for this condition: the member after `Invalid parameter:` is the one the service
/// attributes the fault to, not one read off the request.
const BOTH_DESTINATIONS_REFUSAL: &str =
    "Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn specified. Use only one \
     or the other.";

/// The sentence a publish naming NO destination is refused with.
const NO_DESTINATION_REFUSAL: &str =
    "Invalid parameter: TopicArn or TargetArn Reason: no value for required parameter";

/// Publish with the destination members given, answering the call.
async fn publish(sns: &Sns, destinations: &[(&str, &str)]) -> Answer {
    let mut params: Vec<(&str, &str)> = destinations.to_vec();
    params.push(("Message", "a message"));
    sns.call("Publish", &params).await
}

/// One row of the destination table: the case, the destination members the publish names,
/// and the refusal the call must answer (`None` when it must be served).
type DestinationCase<'a> = (&'a str, &'a [(&'a str, &'a str)], Option<&'a str>);

/// A publish naming ONE destination is served, whichever member carried it; a publish naming
/// two is refused with the service's sentence for that condition.
#[tokio::test]
async fn a_publish_names_exactly_one_destination() {
    let sns = Sns::new();
    let topic = sns.topic("destination-topic").await;
    let other = sns.topic("destination-other-topic").await;

    // (case, the destination members named, the refusal the call must answer — `None` when it
    //  must be served)
    let rows: &[DestinationCase<'_>] = &[
        (
            "a topic named by TopicArn",
            &[("TopicArn", topic.as_str())],
            None,
        ),
        (
            // "a message to a mobile platform endpoint (when you specify the TargetArn)"
            // (`Publish`) — a topic's ARN is a target too, and is served like one.
            "the same topic named by TargetArn",
            &[("TargetArn", topic.as_str())],
            None,
        ),
        (
            "one topic named by both members",
            &[("TopicArn", topic.as_str()), ("TargetArn", topic.as_str())],
            Some(BOTH_DESTINATIONS_REFUSAL),
        ),
        (
            "two different topics, one per member",
            &[("TopicArn", topic.as_str()), ("TargetArn", other.as_str())],
            Some(BOTH_DESTINATIONS_REFUSAL),
        ),
        ("no destination at all", &[], Some(NO_DESTINATION_REFUSAL)),
    ];

    for (case, destinations, refusal) in rows {
        let answer = publish(&sns, destinations).await;
        match refusal {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the publish to be served, got {} / {}",
                    answer.status, answer.body
                );
                let message_id = answer.element("MessageId").unwrap_or_default();
                assert!(
                    !message_id.is_empty(),
                    "case {case:?}: a served publish answers the id of the message it saved, \
                     got {}",
                    answer.body
                );
            }
            Some(expected) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} / {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.code(),
                    "InvalidParameter",
                    "case {case:?}: expected the code InvalidParameter, got {}",
                    answer.body
                );
                let message = answer.element("Message").unwrap_or_default();
                assert_eq!(
                    message, *expected,
                    "case {case:?}: expected the message {expected:?}, got {message:?}"
                );
            }
        }
    }
}

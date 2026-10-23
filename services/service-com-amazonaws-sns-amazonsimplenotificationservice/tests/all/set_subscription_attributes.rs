// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the subscription attribute WRITE path:
//! `SetSubscriptionAttributes`, read back through `GetSubscriptionAttributes`.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.
//!
//! The rule under test: `SetSubscriptionAttributes` stores the new value of ONE
//! documented attribute of the subscription its ARN names — accepting it only when the
//! value satisfies the constraint SNS documents for that attribute AND the
//! subscription's delivery protocol is one SNS documents the attribute for, clearing the
//! attribute when the request carries no value, and refusing anything else with HTTP
//! 400 / `InvalidParameter` (a subscription that does not exist: HTTP 404 / `NotFound`)
//! without writing anything.
//!
//! The rows differ in attribute, value form (flag, enumerated word, policy document,
//! ARN), delivery protocol and confirmation state, which is what the rule generalises
//! over; the recorded `RawMessageDelivery` write is one row of it.

use std::collections::HashSet;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::SUBSCRIPTION_ATTRIBUTE_NAMES;

use super::subscription_attributes::{read, subscribe};
use super::topics_subscriptions_publish::{attribute_entries, attribute_map, Answer, Sns};

/// A filter policy, as the `FilterPolicy` attribute carries one: a JSON object of the
/// message properties to match.
const FILTER_POLICY: &str = r#"{"kind":["alert"]}"#;

/// A dead-letter queue, as the `RedrivePolicy` attribute names one.
const REDRIVE_POLICY: &str = r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:dlq"}"#;

/// A delivery policy that overrides one member of SNS's own.
const DELIVERY_POLICY: &str = r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":7}}}"#;

/// An endpoint of the documented form for `protocol`
/// (`SubscribeInput$Endpoint` doc), so a row names only the protocol it varies.
fn endpoint_for(protocol: &str) -> &'static str {
    match protocol {
        "sqs" => "arn:aws:sqs:us-east-1:000000000000:orders",
        "lambda" => "arn:aws:lambda:us-east-1:000000000000:function:notify",
        "http" => "http://example.test/hook",
        "https" => "https://example.test/hook",
        "email" => "owner@example.test",
        "email-json" => "owner@example.test",
        "sms" => "+44 20 7946 0958",
        "firehose" => "arn:aws:firehose:us-east-1:000000000000:deliverystream/events",
        "application" => "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/app/device-1",
        other => panic!("no documented endpoint form for protocol {other:?}"),
    }
}

/// Write one subscription attribute: `value` of `None` is a request that carries no
/// `AttributeValue` at all, which clears the attribute.
async fn set(sns: &Sns, arn: &str, name: &str, value: Option<&str>) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![("SubscriptionArn", arn), ("AttributeName", name)];
    if let Some(value) = value {
        params.push(("AttributeValue", value));
    }
    sns.call("SetSubscriptionAttributes", &params).await
}

/// One accepted write: the case, the subscription's protocol, the attribute and the
/// value set on it.
type WriteCase<'a> = (&'a str, &'a str, &'a str, &'a str);

/// One refused write: the case, the subscription's protocol, the attribute and the
/// value.
type RefusalCase<'a> = (&'a str, &'a str, &'a str, &'a str);

/// A write of a documented attribute with a documented value persists it, the read
/// reports exactly it, a repeat is idempotent, and a request carrying no value clears
/// the attribute — leaving the subscription reporting what it reported before the write.
///
/// The attribute names are the catalog of
/// `SetSubscriptionAttributesInput$AttributeName` (the crate's
/// `SUBSCRIPTION_ATTRIBUTE_NAMES`, asserted covered below); the values are the forms
/// that doc gives for each.
#[tokio::test]
async fn setting_a_documented_subscription_attribute_persists_and_round_trips() {
    let sns = Sns::new();
    let cases: &[WriteCase<'_>] = &[
        (
            "raw delivery enabled on an sqs subscription",
            "sqs",
            "RawMessageDelivery",
            "true",
        ),
        (
            "raw delivery enabled on an https subscription",
            "https",
            "RawMessageDelivery",
            "true",
        ),
        (
            "raw delivery left disabled on a lambda subscription",
            "lambda",
            "RawMessageDelivery",
            "false",
        ),
        (
            "a filter policy on a lambda subscription",
            "lambda",
            "FilterPolicy",
            FILTER_POLICY,
        ),
        (
            "a filter policy on a nested body property",
            "sqs",
            "FilterPolicy",
            r#"{"detail":{"state":["ok","degraded"]}}"#,
        ),
        (
            "a filter policy on an email subscription awaiting confirmation",
            "email",
            "FilterPolicy",
            FILTER_POLICY,
        ),
        (
            "a filter policy on an application subscription",
            "application",
            "FilterPolicy",
            FILTER_POLICY,
        ),
        (
            "the message-body filtering scope",
            "sqs",
            "FilterPolicyScope",
            "MessageBody",
        ),
        (
            "the default filtering scope stated explicitly",
            "sqs",
            "FilterPolicyScope",
            "MessageAttributes",
        ),
        (
            "a delivery policy on an https subscription",
            "https",
            "DeliveryPolicy",
            DELIVERY_POLICY,
        ),
        (
            "a delivery policy on an http subscription",
            "http",
            "DeliveryPolicy",
            DELIVERY_POLICY,
        ),
        (
            "a dead-letter queue on a lambda subscription",
            "lambda",
            "RedrivePolicy",
            REDRIVE_POLICY,
        ),
        (
            "a dead-letter queue on an sms subscription",
            "sms",
            "RedrivePolicy",
            REDRIVE_POLICY,
        ),
        (
            "a replay policy on an sqs subscription",
            "sqs",
            "ReplayPolicy",
            r#"{"PointType":"Timestamp","StartingPoint":"2026-01-01T00:00:00Z"}"#,
        ),
        (
            "the role a firehose delivery is written under",
            "firehose",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
        ),
    ];

    // Every settable attribute SNS defines is a row of the table above.
    let covered: HashSet<&str> = cases.iter().map(|(_, _, name, _)| *name).collect();
    for name in SUBSCRIPTION_ATTRIBUTE_NAMES {
        assert!(
            covered.contains(name),
            "the table must cover every settable subscription attribute; {name} is missing"
        );
    }

    for (index, (case, protocol, name, value)) in cases.iter().enumerate() {
        let topic = sns.topic(&format!("set-sub-attr-{index:02}")).await;
        let arn = subscribe(
            &sns,
            case,
            &topic,
            protocol,
            endpoint_for(protocol),
            &[],
        )
        .await;
        let before = attribute_entries(&read(&sns, case, &arn).await);

        let answer = set(&sns, &arn, name, Some(value)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: SetSubscriptionAttributes expected 200, got {} / {}",
            answer.status, answer.body
        );

        let written = read(&sns, case, &arn).await;
        let held = attribute_map(&written);
        assert_eq!(
            held.get(*name).map(String::as_str),
            Some(*value),
            "case {case:?}: {name} must report the value just set, got {held:?}"
        );
        // A filter policy is applied under a scope, so a policy set with no explicit
        // scope is reported with the documented default.
        if *name == "FilterPolicy" {
            assert_eq!(
                held.get("FilterPolicyScope").map(String::as_str),
                Some("MessageAttributes"),
                "case {case:?}: a filter policy with no explicit scope is applied under \
                 the documented default scope, got {held:?}"
            );
        }

        // The read is reporting only: a second one sees the same document.
        assert_eq!(
            attribute_entries(&read(&sns, case, &arn).await),
            attribute_entries(&written),
            "case {case:?}: two reads of one subscription must report the same entries"
        );

        // Writing the same value again changes nothing.
        let repeat = set(&sns, &arn, name, Some(value)).await;
        assert_eq!(
            repeat.status, 200,
            "case {case:?}: a repeated write expected 200, got {} / {}",
            repeat.status, repeat.body
        );
        assert_eq!(
            attribute_entries(&read(&sns, case, &arn).await),
            attribute_entries(&written),
            "case {case:?}: a repeated write of the same value must leave the \
             subscription as it was"
        );

        // A request carrying no value clears the attribute, so the subscription reports
        // what it reported before it was ever set — the documented default of a flag,
        // and no entry at all for a policy.
        let cleared = set(&sns, &arn, name, None).await;
        assert_eq!(
            cleared.status, 200,
            "case {case:?}: clearing {name} expected 200, got {} / {}",
            cleared.status, cleared.body
        );
        let after = read(&sns, case, &arn).await;
        assert_eq!(
            attribute_entries(&after),
            before,
            "case {case:?}: clearing {name} must leave the subscription reporting what \
             it reported before the write, got {:?}",
            attribute_map(&after)
        );
    }
}

/// A value SNS does not accept for the attribute, or an attribute SNS does not define
/// for the subscription's delivery protocol, is refused with HTTP 400 /
/// `InvalidParameter` — and nothing is written.
///
/// The protocol rows come from the doc's own scoping: raw message delivery is enabled
/// "to Amazon SQS or HTTP/S endpoints", a delivery policy governs "failed deliveries to
/// HTTP/S endpoints", and `SubscriptionRoleArn` "applies only to Amazon Data Firehose
/// delivery stream subscriptions"
/// (`SetSubscriptionAttributesInput$AttributeName` doc).
#[tokio::test]
async fn setting_a_subscription_attribute_sns_does_not_accept_is_refused() {
    let sns = Sns::new();
    let cases: &[RefusalCase<'_>] = &[
        // Names SNS does not define as a settable subscription attribute, including the
        // properties it COMPUTES and an attribute of the other resource kind.
        ("an attribute name SNS defines for nothing", "sqs", "Nonsense", "x"),
        ("a computed property", "sqs", "Protocol", "lambda"),
        (
            "the subscription's own ARN",
            "sqs",
            "SubscriptionArn",
            "arn:aws:sns:us-east-1:000000000000:t:11111111",
        ),
        ("a topic attribute", "sqs", "DisplayName", "orders"),
        ("the owning account", "sqs", "Owner", "000000000000"),
        // A flag takes the two words and nothing else.
        ("raw delivery in capitals", "sqs", "RawMessageDelivery", "TRUE"),
        ("raw delivery as a word of its own", "sqs", "RawMessageDelivery", "yes"),
        ("raw delivery as a number", "sqs", "RawMessageDelivery", "1"),
        // Raw delivery is enabled for Amazon SQS and HTTP/S endpoints alone — the
        // recorded case is the lambda row.
        (
            "raw delivery on a lambda subscription",
            "lambda",
            "RawMessageDelivery",
            "true",
        ),
        ("raw delivery on an email subscription", "email", "RawMessageDelivery", "true"),
        ("raw delivery on an sms subscription", "sms", "RawMessageDelivery", "true"),
        (
            "raw delivery on a firehose subscription",
            "firehose",
            "RawMessageDelivery",
            "true",
        ),
        (
            "raw delivery on an application subscription",
            "application",
            "RawMessageDelivery",
            "true",
        ),
        // A filter policy is a JSON OBJECT of properties to match.
        ("a filter policy that is a list", "sqs", "FilterPolicy", r#"["alert"]"#),
        ("a filter policy that is a string", "sqs", "FilterPolicy", r#""alert""#),
        ("a filter policy that is not JSON", "sqs", "FilterPolicy", "kind=alert"),
        // The filtering scope is one of two words.
        ("a filtering scope that is not one of the two", "sqs", "FilterPolicyScope", "MessageBodies"),
        ("a filtering scope in the wrong case", "sqs", "FilterPolicyScope", "messagebody"),
        // A delivery policy has to be a document the effective policy can be computed
        // from, and governs HTTP/S endpoints alone.
        ("a delivery policy that is a bare number", "https", "DeliveryPolicy", "7"),
        (
            "a delivery policy whose member is of the wrong type",
            "https",
            "DeliveryPolicy",
            r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":"many"}}}"#,
        ),
        ("a delivery policy on an sqs subscription", "sqs", "DeliveryPolicy", DELIVERY_POLICY),
        (
            "a delivery policy on a lambda subscription",
            "lambda",
            "DeliveryPolicy",
            DELIVERY_POLICY,
        ),
        // A redrive policy names an Amazon SQS dead-letter queue.
        ("a redrive policy naming no queue", "sqs", "RedrivePolicy", "{}"),
        (
            "a redrive policy whose target is not an ARN",
            "sqs",
            "RedrivePolicy",
            r#"{"deadLetterTargetArn":"dlq"}"#,
        ),
        (
            "a redrive policy whose target is not a queue",
            "sqs",
            "RedrivePolicy",
            r#"{"deadLetterTargetArn":"arn:aws:sns:us-east-1:000000000000:dlq"}"#,
        ),
        ("a redrive policy that is a list", "sqs", "RedrivePolicy", "[]"),
        // A replay policy is a document too.
        ("a replay policy that is a string", "sqs", "ReplayPolicy", r#""now""#),
        // A subscription role is an ARN, and applies to Firehose subscriptions alone.
        (
            "a subscription role that is not an ARN",
            "firehose",
            "SubscriptionRoleArn",
            "sns-firehose",
        ),
        (
            "a subscription role on an sqs subscription",
            "sqs",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
        ),
        (
            "a subscription role on a lambda subscription",
            "lambda",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
        ),
    ];

    for (index, (case, protocol, name, value)) in cases.iter().enumerate() {
        let topic = sns.topic(&format!("refused-sub-attr-{index:02}")).await;
        let arn = subscribe(
            &sns,
            case,
            &topic,
            protocol,
            endpoint_for(protocol),
            &[],
        )
        .await;
        let before = attribute_entries(&read(&sns, case, &arn).await);

        let answer = set(&sns, &arn, name, Some(value)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );

        // A refused write persists nothing.
        let after = read(&sns, case, &arn).await;
        assert_eq!(
            attribute_entries(&after),
            before,
            "case {case:?}: a refused write must leave the subscription as it was, got \
             {:?}",
            attribute_map(&after)
        );
    }
}

/// `SetSubscriptionAttributes` answers 404 / `NotFound` for a well-formed subscription
/// ARN that names no subscription this emulator holds, and 400 / `InvalidParameter` for
/// a value that is not a subscription ARN at all — including the sentinels SNS publishes
/// in place of an unconfirmed subscription's ARN.
#[tokio::test]
async fn setting_an_attribute_of_a_subscription_that_is_not_there_is_refused() {
    let sns = Sns::new();
    let topic = sns.topic("set-attrs-absent-subscription").await;
    let unsubscribed = subscribe(
        &sns,
        "unsubscribed",
        &topic,
        "sqs",
        "arn:aws:sqs:us-east-1:000000000000:gone",
        &[],
    )
    .await;
    let gone = sns
        .call("Unsubscribe", &[("SubscriptionArn", &unsubscribed)])
        .await;
    assert_eq!(
        gone.status, 200,
        "Unsubscribe must succeed, got {} / {}",
        gone.status, gone.body
    );

    let absent = [
        ("the subscription was unsubscribed", unsubscribed.clone()),
        (
            "an id no subscription of this topic carries",
            format!("{topic}:00000000-0000-4000-8000-000000000000"),
        ),
        (
            "a topic that does not exist",
            "arn:aws:sns:us-east-1:000000000000:absent-topic:\
             11111111-1111-4111-8111-111111111111"
                .to_string(),
        ),
        (
            "another region and account",
            "arn:aws:sns:eu-west-1:123456789012:other-topic:\
             22222222-2222-4222-8222-222222222222"
                .to_string(),
        ),
    ];
    for (case, arn) in &absent {
        let answer = set(&sns, arn, "RawMessageDelivery", Some("true")).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "NotFound",
            "case {case:?}: expected the NotFound code, got {}",
            answer.body
        );
    }

    let malformed = [
        "not-an-arn-at-all",
        "arn:aws:sns",
        "arn:aws:sns:us-east-1:000000000000:a-topic",
        "arn:aws:sns:us-east-1:000000000000:a-topic:",
        "arn:aws:sqs:us-east-1:000000000000:q:11111111",
        "arn:aws:sns:us-east-1:000000000000:two words:11111111",
        "pending confirmation",
        "PendingConfirmation",
    ];
    for arn in malformed {
        let answer = set(&sns, arn, "RawMessageDelivery", Some("true")).await;
        assert_eq!(
            answer.status, 400,
            "case {arn:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {arn:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
    }
}

/// `Subscribe` holds the attributes it carries to the SAME two rules, so an attribute
/// one call accepts is not one the other refuses.
#[tokio::test]
async fn subscribe_holds_its_attributes_to_the_same_rules() {
    let sns = Sns::new();
    let topic = sns.topic("subscribe-attribute-rules").await;
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        (
            "raw delivery on an sqs subscription",
            "sqs",
            "RawMessageDelivery",
            "true",
            true,
        ),
        (
            "raw delivery on a lambda subscription",
            "lambda",
            "RawMessageDelivery",
            "true",
            false,
        ),
        (
            "raw delivery as a number",
            "sqs",
            "RawMessageDelivery",
            "1",
            false,
        ),
        (
            "a filter policy that is a list",
            "sqs",
            "FilterPolicy",
            r#"["alert"]"#,
            false,
        ),
        (
            "a filter policy that is an object",
            "sqs",
            "FilterPolicy",
            FILTER_POLICY,
            true,
        ),
        (
            "a subscription role on a lambda subscription",
            "lambda",
            "SubscriptionRoleArn",
            "arn:aws:iam::000000000000:role/sns-firehose",
            false,
        ),
    ];

    for (case, protocol, name, value, accepted) in cases {
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", protocol),
                    ("Endpoint", endpoint_for(protocol)),
                    ("ReturnSubscriptionArn", "true"),
                    ("Attributes.entry.1.key", name),
                    ("Attributes.entry.1.value", value),
                ],
            )
            .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
        }
    }
}

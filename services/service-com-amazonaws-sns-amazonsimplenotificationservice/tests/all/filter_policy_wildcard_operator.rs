// SPDX-License-Identifier: CC0-1.0

//! A subscription whose filter policy uses the `wildcard` string operator receives exactly
//! the messages whose value the pattern matches — end to end over the wire, with SQS mounted
//! on the same engine so what was delivered is observed where a client observes it: on the
//! queue.
//!
//! The RULE asserted here: "FilterPolicy – The simple JSON object that lets your subscriber
//! receive only a subset of messages, rather than receiving every message published to the
//! topic" (`SubscribeInput$Attributes` doc) — so EVERY operator of the matching Amazon SNS
//! documents for its filter policies decides delivery, `wildcard` included: its pattern
//! matches a value when the value is the pattern with each `*` standing for any run of
//! characters (the empty run included) and every other character matched literally and
//! case-sensitively. The operator decides the same way wherever it appears — under either
//! `FilterPolicyScope`, and nested inside `anything-but`, where it decides what is excluded.
//!
//! The rows differ along what the rule generalises over: where the wildcards sit (leading,
//! trailing, inside, both ends, several, none at all), how much each one has to swallow
//! (nothing, one character, a run, a run containing the literal that follows it), the case
//! of the value, a pattern character that is NOT a wildcard in this grammar (`?`), a literal
//! `*` in the value, and the type of the matched value (a string, an element of a
//! `String.Array`, a value no string condition can match).

use crate::sqs_subscription_delivery::{drain, element, emulator, given_queue, sns, Emulator};

/// The property every policy below matches on, and the attribute every publish carries.
const PROPERTY: &str = "region";

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

/// Write `policy` as the subscription's `FilterPolicy`.
async fn given_policy(emulator: &Emulator, subscription: &str, policy: &str) {
    sns(
        emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", subscription),
            ("AttributeName", "FilterPolicy"),
            ("AttributeValue", policy),
        ],
    )
    .await;
}

/// Publish one message carrying the attribute `PROPERTY` of type `data_type` and value
/// `value`.
async fn publish_attributed(emulator: &Emulator, topic: &str, data_type: &str, value: &str) {
    sns(
        emulator,
        "Publish",
        &[
            ("TopicArn", topic),
            ("Message", "a filtered message"),
            ("MessageAttributes.entry.1.Name", PROPERTY),
            ("MessageAttributes.entry.1.Value.DataType", data_type),
            ("MessageAttributes.entry.1.Value.StringValue", value),
        ],
    )
    .await;
}

/// One row: the case, the pattern the policy names, the attribute's data type, the value
/// published, and whether the subscriber must receive that message.
type WildcardCase<'a> = (&'a str, &'a str, &'a str, &'a str, bool);

#[tokio::test]
async fn a_wildcard_condition_admits_exactly_the_values_its_pattern_matches() {
    let rows: &[WildcardCase<'_>] = &[
        (
            "a wildcard inside the pattern, swallowing a run",
            "*-west-*",
            "String",
            "us-west-2",
            true,
        ),
        (
            "the same pattern against a value it does not describe",
            "*-west-*",
            "String",
            "eu-north-1",
            false,
        ),
        (
            "a trailing wildcard, swallowing a run",
            "us-*",
            "String",
            "us-east-1",
            true,
        ),
        (
            "a trailing wildcard, swallowing nothing",
            "us-east-1*",
            "String",
            "us-east-1",
            true,
        ),
        (
            "a leading wildcard, swallowing a run",
            "*-1",
            "String",
            "eu-west-1",
            true,
        ),
        (
            "a leading wildcard, swallowing nothing",
            "*eu-west-1",
            "String",
            "eu-west-1",
            true,
        ),
        (
            "wildcards at both ends",
            "*west*",
            "String",
            "eu-west-1",
            true,
        ),
        (
            "three wildcards, each swallowing a different run",
            "*-*-*",
            "String",
            "ap-southeast-2",
            true,
        ),
        (
            "a run that itself contains the character the pattern spells next",
            "*-1",
            "String",
            "eu-west-1-west-1",
            true,
        ),
        (
            "a pattern with no wildcard at all matches only the value it spells",
            "us-west-2",
            "String",
            "us-west-2",
            true,
        ),
        (
            "a pattern with no wildcard against another value",
            "us-west-2",
            "String",
            "us-west-1",
            false,
        ),
        (
            // Only `equals-ignore-case` is case-insensitive; every other string
            // condition compares the characters as they were published.
            "a value differing from the pattern only in case",
            "*-west-*",
            "String",
            "US-WEST-2",
            false,
        ),
        (
            // `?` is not a wildcard of this grammar: the pattern spells the character.
            "a question mark in the pattern is matched literally",
            "us-west-?",
            "String",
            "us-west-2",
            false,
        ),
        (
            "a question mark the value really carries",
            "us-west-?",
            "String",
            "us-west-?",
            true,
        ),
        (
            "a wildcard matching a value that itself carries an asterisk",
            "us-*-2",
            "String",
            "us-*-2",
            true,
        ),
        (
            "a pattern longer than the value matches nothing",
            "us-west-2*",
            "String",
            "us-west",
            false,
        ),
        (
            // "String.Array – Strings, numbers, true, false and null"
            // (`MessageAttributeValue` doc): a condition matching any element matches
            // the attribute.
            "an element of a String.Array the pattern matches",
            "*-west-*",
            "String.Array",
            r#"["eu-north-1","us-west-2"]"#,
            true,
        ),
        (
            "no element of a String.Array the pattern matches",
            "*-west-*",
            "String.Array",
            r#"["eu-north-1","ap-south-1"]"#,
            false,
        ),
        (
            // A `Number` attribute holds a number, and a pattern describes text, so
            // there is no string for the pattern to match.
            "a number attribute against a pattern",
            "1*",
            "Number",
            "10",
            false,
        ),
    ];

    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "wildcard-filtered")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "wildcard-filtered-queue").await;
    let subscription = subscribe(&emulator, &topic, &arn).await;

    for (case, pattern, data_type, value, delivered) in rows {
        let policy = format!(r#"{{"{PROPERTY}":[{{"wildcard":"{pattern}"}}]}}"#);
        given_policy(&emulator, &subscription, &policy).await;
        publish_attributed(&emulator, &topic, data_type, value).await;

        let received = drain(&emulator, &url).await;
        let expected = usize::from(*delivered);
        assert_eq!(
            received.len(),
            expected,
            "case {case:?}: pattern {pattern:?} against {value:?} must deliver {expected} \
             message(s), got {received:?}"
        );
    }
}

/// `anything-but` holding a `wildcard` excludes exactly what that pattern matches, so the
/// same patterns decide the complementary way round.
#[tokio::test]
async fn anything_but_a_wildcard_excludes_exactly_what_the_pattern_matches() {
    // (case, pattern, value, delivered)
    let rows: &[(&str, &str, &str, bool)] = &[
        ("a value the pattern matches is excluded", "*-west-*", "us-west-2", false),
        ("a value the pattern does not match is admitted", "*-west-*", "eu-north-1", true),
        ("a trailing wildcard excludes its whole family", "us-*", "us-east-1", false),
        ("a value outside that family is admitted", "us-*", "ap-south-1", true),
        ("the pattern is case-sensitive here too", "*-west-*", "US-WEST-2", true),
    ];

    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "wildcard-excluded")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "wildcard-excluded-queue").await;
    let subscription = subscribe(&emulator, &topic, &arn).await;

    for (case, pattern, value, delivered) in rows {
        let policy =
            format!(r#"{{"{PROPERTY}":[{{"anything-but":{{"wildcard":"{pattern}"}}}}]}}"#);
        given_policy(&emulator, &subscription, &policy).await;
        publish_attributed(&emulator, &topic, "String", value).await;

        let received = drain(&emulator, &url).await;
        let expected = usize::from(*delivered);
        assert_eq!(
            received.len(),
            expected,
            "case {case:?}: anything-but {pattern:?} against {value:?} must deliver \
             {expected} message(s), got {received:?}"
        );
    }
}

/// The operator decides the same way under the MESSAGE BODY scope: there the pattern is
/// matched against the value the body carries for the property, nested property included.
#[tokio::test]
async fn a_wildcard_condition_decides_a_body_scoped_policy_too() {
    // (case, policy, body, delivered)
    let rows: &[(&str, &str, &str, bool)] = &[
        (
            "a body value the pattern matches",
            r#"{"region":[{"wildcard":"*-west-*"}]}"#,
            r#"{"region":"us-west-2"}"#,
            true,
        ),
        (
            "a body value the pattern does not match",
            r#"{"region":[{"wildcard":"*-west-*"}]}"#,
            r#"{"region":"eu-north-1"}"#,
            false,
        ),
        (
            "a pattern against a value nested a property deeper",
            r#"{"detail":{"region":[{"wildcard":"*-west-*"}]}}"#,
            r#"{"detail":{"region":"us-west-2"}}"#,
            true,
        ),
        (
            "an element of a body array the pattern matches",
            r#"{"region":[{"wildcard":"*-west-*"}]}"#,
            r#"{"region":["eu-north-1","us-west-2"]}"#,
            true,
        ),
        (
            "a body that is not a document at all",
            r#"{"region":[{"wildcard":"*"}]}"#,
            "us-west-2",
            false,
        ),
    ];

    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "wildcard-body-filtered")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "wildcard-body-queue").await;
    let subscription = subscribe(&emulator, &topic, &arn).await;
    sns(
        &emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", &subscription),
            ("AttributeName", "FilterPolicyScope"),
            ("AttributeValue", "MessageBody"),
        ],
    )
    .await;

    for (case, policy, body, delivered) in rows {
        given_policy(&emulator, &subscription, policy).await;
        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", body)],
        )
        .await;

        let received = drain(&emulator, &url).await;
        let expected = usize::from(*delivered);
        assert_eq!(
            received.len(),
            expected,
            "case {case:?}: policy {policy} against body {body} must deliver {expected} \
             message(s), got {received:?}"
        );
    }
}

/// A pattern of MANY wildcards against a long value is decided — the match is linear in the
/// two lengths, so neither the pattern nor the value can make the matching run away.
#[tokio::test]
async fn a_pattern_of_many_wildcards_against_a_long_value_is_still_decided() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "wildcard-bounded")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "wildcard-bounded-queue").await;
    let subscription = subscribe(&emulator, &topic, &arn).await;

    // (case, pattern, value, delivered)
    let long = "a".repeat(4_000);
    let rows: &[(&str, String, String, bool)] = &[
        (
            "a wildcard between every pair of the pattern's characters",
            format!("{}z", "*a".repeat(16)),
            format!("{long}z"),
            true,
        ),
        (
            "the same pattern against a value that never reaches its last character",
            format!("{}z", "*a".repeat(16)),
            long.clone(),
            false,
        ),
    ];

    for (case, pattern, value, delivered) in rows {
        let policy = format!(r#"{{"{PROPERTY}":[{{"wildcard":"{pattern}"}}]}}"#);
        given_policy(&emulator, &subscription, &policy).await;
        publish_attributed(&emulator, &topic, "String", value).await;

        let received = drain(&emulator, &url).await;
        let expected = usize::from(*delivered);
        assert_eq!(
            received.len(),
            expected,
            "case {case:?}: expected {expected} delivered message(s), got {}",
            received.len()
        );
    }
}

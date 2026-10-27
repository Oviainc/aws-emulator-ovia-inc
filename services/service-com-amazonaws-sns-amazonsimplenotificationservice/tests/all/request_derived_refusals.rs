// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the refusals SNS builds FROM THE REQUEST, and for the one
//! member whose accepted values it carries verbatim.
//!
//! Three rules, each driven over the wire through the router the emulator serves in
//! production:
//!
//! * an attribute MAP carrying a name the service does not define is refused with the
//!   input member the map arrived in and the key that was not recognised, and the call
//!   writes nothing;
//! * a batch entry `Id` is refused with the published rule it broke — "This identifier
//!   can have up to 80 characters" or "The following characters are accepted:
//!   alphanumeric characters, hyphens(-), and underscores (_)"
//!   (`PublishBatchRequestEntry$Id` doc) — and the id that broke it;
//! * a `Subject` SNS accepts is published as it was sent: "Subjects must be UTF-8 text
//!   with no line breaks or control characters, and less than 100 characters long"
//!   (`PublishInput$Subject` doc) admits the blank space AROUND the text, and nothing
//!   trims it.

use super::topics_subscriptions_publish::{text_of, Sns};

/// The longest batch entry `Id` the member admits: "This identifier can have up to 80
/// characters" (`PublishBatchRequestEntry$Id` doc).
const MAX_ENTRY_ID_CHARS: usize = 80;

/// The sentence a refused attribute key answers, built from the member and the key.
fn unknown_attribute(member: &str, key: &str) -> String {
    format!("Invalid parameter: {member} Reason: Unknown attribute {key}")
}

/// Subscribe an SQS endpoint to `topic`, answering the subscription's ARN.
async fn subscribe(sns: &Sns, topic: &str, endpoint: &str) -> String {
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "Subscribe to {topic}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("Subscribe to {topic}: no SubscriptionArn in {}", answer.body))
}

/// The subject of every message delivered to `subscription`, in delivery order, read
/// from the subscription's own delivery log — the record the handler writes.
fn delivered_subjects(sns: &Sns, subscription: &str) -> Vec<Option<String>> {
    let stored = sns.state();
    let held = stored.lock();
    held.delivered
        .get(subscription)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|message| message.subject.clone())
        .collect()
}

/// An attribute name the service does not define is refused with the MEMBER the map
/// arrived in and the offending key — on every call that takes an attribute map — and
/// the refused call leaves nothing behind.
///
/// The rows differ in the key (a misspelling of a defined name, a name of the OTHER
/// resource's attribute set, a name only the service computes, one in another case, a
/// key carrying a separator) and in the call the map reached: `Subscribe`, whose set is
/// the subscription attributes, and `CreateTopic`, whose set is the topic attributes.
#[tokio::test]
async fn an_unknown_attribute_is_refused_with_the_member_and_the_key() {
    let sns = Sns::new();
    let topic = sns.topic("unknown-attribute-topic").await;
    let endpoint = "arn:aws:sqs:us-east-1:000000000000:unknown-attribute-queue";

    // Keys no subscription attribute set defines: the recorded misspelling, a topic
    // attribute asked of a subscription, a computed one, a defined name in another
    // case, and a key with a separator in it.
    let subscription_keys = [
        "RawMessgeDelivery",
        "DisplayName",
        "SubscriptionArn ",
        "rawmessagedelivery",
        "Filter.Policy",
    ];
    for key in subscription_keys {
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", "sqs"),
                    ("Endpoint", endpoint),
                    ("ReturnSubscriptionArn", "true"),
                    ("Attributes.entry.1.key", key),
                    ("Attributes.entry.1.value", "true"),
                ],
            )
            .await;
        let expected = unknown_attribute("Attributes", key);
        assert_eq!(
            answer.status, 400,
            "case {key:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {key:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(expected.as_str()),
            "case {key:?}: expected the message {expected:?}, got {}",
            answer.body
        );
    }

    // A rejected key is reported even when the map also carries a DEFINED one, and the
    // defined one is not written: the refused call creates no subscription at all.
    let mixed = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
                ("ReturnSubscriptionArn", "true"),
                ("Attributes.entry.1.key", "FilterPolicy"),
                ("Attributes.entry.1.value", r#"{"tier":["gold"]}"#),
                ("Attributes.entry.2.key", "RawMessgeDelivery"),
                ("Attributes.entry.2.value", "true"),
            ],
        )
        .await;
    let expected = unknown_attribute("Attributes", "RawMessgeDelivery");
    assert_eq!(
        mixed.element("Message").as_deref(),
        Some(expected.as_str()),
        "a map carrying one defined and one unknown key: expected {expected:?}, got {}",
        mixed.body
    );
    let listed = sns
        .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
        .await;
    assert_eq!(
        listed.elements("SubscriptionArn"),
        Vec::<String>::new(),
        "a refused Subscribe must create no subscription, got {}",
        listed.body
    );

    // The same rule on the other call that takes an attribute map, over the topic
    // attribute set: a subscription attribute, a computed one, and a misspelling.
    let topic_keys = ["RawMessageDelivery", "SubscriptionsConfirmed", "DisplyName"];
    for key in topic_keys {
        let answer = sns
            .call(
                "CreateTopic",
                &[
                    ("Name", "unknown-attribute-create"),
                    ("Attributes.entry.1.key", key),
                    ("Attributes.entry.1.value", "true"),
                ],
            )
            .await;
        let expected = unknown_attribute("Attributes", key);
        assert_eq!(
            answer.status, 400,
            "case {key:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(expected.as_str()),
            "case {key:?}: expected the message {expected:?}, got {}",
            answer.body
        );
    }
    let topics = sns.call("ListTopics", &[]).await;
    assert!(
        !topics.body.contains("unknown-attribute-create"),
        "a refused CreateTopic must create no topic, got {}",
        topics.body
    );
    // The DEFINED name the same map could have carried is accepted, so the refusal is
    // the key and not the map.
    let accepted = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "unknown-attribute-create"),
                ("Attributes.entry.1.key", "DisplayName"),
                ("Attributes.entry.1.value", "A display name"),
            ],
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "a map of defined names: expected 200, got {} / {}",
        accepted.status, accepted.body
    );
}

/// A batch entry `Id` is refused with the rule it broke and the id that broke it, and
/// an id both rules admit is accepted and reported back on its own result row.
///
/// The rows differ along what the two rules generalise over: the character count
/// against the published bound (at it, one past it, far past it) and each class of
/// character the id may not carry (a period, a space, a symbol, a non-ASCII letter),
/// including on an id that is ALSO too long, which the length rule reports.
#[tokio::test]
async fn a_refused_batch_entry_id_names_the_rule_and_the_id() {
    let sns = Sns::new();
    let topic = sns.topic("entry-id-topic").await;

    let at_bound = "a".repeat(MAX_ENTRY_ID_CHARS);
    let past_bound = "a".repeat(MAX_ENTRY_ID_CHARS + 1);
    let far_past_bound = "b".repeat(MAX_ENTRY_ID_CHARS * 3);
    let long_and_dotted = format!("{}.", "c".repeat(MAX_ENTRY_ID_CHARS));

    // (case, id, the whole message, or None when the id is accepted)
    let cases: Vec<(&str, &str, Option<String>)> = vec![
        ("alphanumeric", "entry1", None),
        ("a hyphen and an underscore", "valid-sibling_2", None),
        ("at the character bound", &at_bound, None),
        (
            "one character past the bound",
            &past_bound,
            Some(format!(
                "The Id of a batch entry in the batch request is too long: {past_bound}"
            )),
        ),
        (
            "far past the bound",
            &far_past_bound,
            Some(format!(
                "The Id of a batch entry in the batch request is too long: {far_past_bound}"
            )),
        ),
        (
            "past the bound and carrying an excluded character",
            &long_and_dotted,
            Some(format!(
                "The Id of a batch entry in the batch request is too long: {long_and_dotted}"
            )),
        ),
        (
            "a period",
            "has.period",
            Some(
                "The Id of a batch entry in the batch request contains an impermissible \
                 character: has.period"
                    .to_string(),
            ),
        ),
        (
            "a space",
            "not valid",
            Some(
                "The Id of a batch entry in the batch request contains an impermissible \
                 character: not valid"
                    .to_string(),
            ),
        ),
        (
            "a symbol",
            "entry#1",
            Some(
                "The Id of a batch entry in the batch request contains an impermissible \
                 character: entry#1"
                    .to_string(),
            ),
        ),
        (
            "a non-ascii letter",
            "entrée",
            Some(
                "The Id of a batch entry in the batch request contains an impermissible \
                 character: entrée"
                    .to_string(),
            ),
        ),
    ];

    for (case, id, refusal) in &cases {
        // A sibling entry the rules admit rides along, so a refused batch is refused
        // for the entry that broke a rule and not for the request's shape.
        let answer = sns
            .call(
                "PublishBatch",
                &[
                    ("TopicArn", &topic),
                    ("PublishBatchRequestEntries.member.1.Id", "sibling"),
                    ("PublishBatchRequestEntries.member.1.Message", "a"),
                    ("PublishBatchRequestEntries.member.2.Id", id),
                    ("PublishBatchRequestEntries.member.2.Message", "b"),
                ],
            )
            .await;
        match refusal {
            Some(message) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected 400, got {} / {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.code(),
                    "InvalidBatchEntryId",
                    "case {case:?}: expected the InvalidBatchEntryId code, got {}",
                    answer.body
                );
                assert_eq!(
                    answer.element("Message").as_deref(),
                    Some(message.as_str()),
                    "case {case:?}: expected the message {message:?}, got {}",
                    answer.body
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected 200, got {} / {}",
                    answer.status, answer.body
                );
                // Round trip: an accepted id is reported back on its own result row,
                // beside a message id the publish minted for it.
                let reported = answer.elements("Id");
                assert!(
                    reported.iter().any(|row| row == id),
                    "case {case:?}: expected the accepted id {id:?} on a result row, got {}",
                    answer.body
                );
                assert_eq!(
                    answer.elements("MessageId").len(),
                    2,
                    "case {case:?}: expected one MessageId per entry, got {}",
                    answer.body
                );
            }
        }
    }

    // Each refusal names ONE id, so the two rules are told apart by the sentence and
    // not only by the code.
    let sentences: Vec<&String> = cases
        .iter()
        .filter_map(|(_, _, refusal)| refusal.as_ref())
        .collect();
    let distinct: std::collections::HashSet<&String> = sentences.iter().copied().collect();
    assert_eq!(
        sentences.len(),
        distinct.len(),
        "every refused id must answer its own sentence, got {sentences:?}"
    );
}

/// A `Subject` SNS accepts is published and delivered as it was SENT: the blank space
/// around the text and the tab inside it are part of the subject, so nothing trims or
/// rewrites them.
///
/// The rows differ in where the blank space sits (leading, trailing, both), in the
/// character it is (a space, a tab) and in the script of the text itself. The accepted
/// subject is read back through the subscription's own delivery log, and through the
/// `Subject` the published notification carries.
#[tokio::test]
async fn an_accepted_subject_is_published_exactly_as_it_was_sent() {
    let sns = Sns::new();
    let topic = sns.topic("subject-fidelity-topic").await;
    let subscription = subscribe(
        &sns,
        &topic,
        "arn:aws:sqs:us-east-1:000000000000:subject-fidelity-queue",
    )
    .await;

    let subjects = [
        ("leading and trailing spaces", "  padded  "),
        ("a leading space", " leading"),
        ("a trailing space", "trailing "),
        ("a tab inside the text", "col\tumn"),
        ("a tab and spaces", "  col\tumn  "),
        ("a non-ascii subject with padding", " Résumé ☃ "),
    ];

    for (case, subject) in subjects {
        let answer = sns
            .call(
                "Publish",
                &[("TopicArn", &topic), ("Message", "m"), ("Subject", subject)],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
    }

    let delivered = delivered_subjects(&sns, &subscription);
    assert_eq!(
        delivered.len(),
        subjects.len(),
        "expected one delivery per accepted publish, got {}",
        delivered.len()
    );
    for ((case, subject), held) in subjects.iter().zip(&delivered) {
        assert_eq!(
            held.as_deref(),
            Some(*subject),
            "case {case:?}: the delivered subject must be the one published, got {held:?}"
        );
    }

    // The same fidelity on a re-publish: the padded subject is held as it was sent, and
    // the delivery log grows by exactly that one message.
    let padded = "  padded  ";
    let envelope = sns
        .call(
            "Publish",
            &[("TopicArn", &topic), ("Message", "m"), ("Subject", padded)],
        )
        .await;
    assert_eq!(
        envelope.status, 200,
        "the re-published padded subject: expected 200, got {} / {}",
        envelope.status, envelope.body
    );
    let after = delivered_subjects(&sns, &subscription);
    assert_eq!(
        after.last().and_then(Option::as_deref),
        Some(padded),
        "the last delivery must keep the padded subject, got {:?}",
        after.last()
    );

    // And a subject carrying no text at all is refused, whichever blank it is made of.
    for (case, subject) in [
        ("no subject at all", ""),
        ("one space", " "),
        ("spaces alone", "    "),
        ("a tab alone", "\t"),
    ] {
        let answer = sns
            .call(
                "Publish",
                &[("TopicArn", &topic), ("Message", "m"), ("Subject", subject)],
            )
            .await;
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
        assert_eq!(
            answer.element("Message").as_deref(),
            Some("Invalid parameter: Subject"),
            "case {case:?}: expected the Subject sentence, got {}",
            answer.body
        );
    }

    // The same rule holds for a BATCH entry's Subject, which is the same member of the
    // same message: the padded one is published, the blank one refuses the batch.
    let batch = sns
        .call(
            "PublishBatch",
            &[
                ("TopicArn", &topic),
                ("PublishBatchRequestEntries.member.1.Id", "padded"),
                ("PublishBatchRequestEntries.member.1.Message", "m"),
                ("PublishBatchRequestEntries.member.1.Subject", padded),
            ],
        )
        .await;
    assert_eq!(
        batch.status, 200,
        "a batched padded subject: expected 200, got {} / {}",
        batch.status, batch.body
    );
    let batched = delivered_subjects(&sns, &subscription);
    assert_eq!(
        batched.last().and_then(Option::as_deref),
        Some(padded),
        "the batched delivery must keep the padded subject, got {:?}",
        batched.last()
    );

    let blank_batch = sns
        .call(
            "PublishBatch",
            &[
                ("TopicArn", &topic),
                ("PublishBatchRequestEntries.member.1.Id", "blank"),
                ("PublishBatchRequestEntries.member.1.Message", "m"),
                ("PublishBatchRequestEntries.member.1.Subject", "   "),
            ],
        )
        .await;
    assert_eq!(
        blank_batch.status, 400,
        "a batched blank subject: expected 400, got {} / {}",
        blank_batch.status, blank_batch.body
    );
    assert_eq!(
        text_of(&blank_batch.body, "Message"),
        "Invalid parameter: Subject",
        "a batched blank subject: expected the Subject sentence, got {}",
        blank_batch.body
    );
}

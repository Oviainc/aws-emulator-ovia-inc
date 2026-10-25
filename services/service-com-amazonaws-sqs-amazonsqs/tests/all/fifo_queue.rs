// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FIFO message plane, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rules pinned here, each over a TABLE of inputs rather than one example:
//!
//! 1. A send to a FIFO queue carries a `MessageGroupId` and a deduplication token —
//!    supplied, or derived from the body under `ContentBasedDeduplication` — and is
//!    refused when it carries neither; the same members are bounded the way their own
//!    documentation bounds them, and a per-message `DelaySeconds` is refused.
//! 2. A FIFO send assigns a `SequenceNumber` that keeps increasing, and a receive hands
//!    a message group's messages over in strict send order, one group member in flight
//!    at a time, while other groups keep flowing.
//! 3. A resend carrying a deduplication token already accepted is accepted without
//!    delivering a second copy, scoped per queue or per message group as the queue's
//!    `DeduplicationScope` says.
//! 4. A receive reports the three attributes a FIFO send records — `MessageGroupId`,
//!    `MessageDeduplicationId` and `SequenceNumber` — when the request selects them.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{as_map, call, create_ok, emulator, error_code, messages, ok};

/// One row of the deduplication table: the case, the attributes the queue is created
/// with, the sends as `(group, body, token)` and how many copies the queue keeps.
type DeduplicationRow<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str, Option<&'a str>)],
    usize,
);

/// One row of the attribute-selection table: the case, the names the request selects
/// (nothing at all when `None`) and the FIFO attributes that must come back.
type SelectionRow<'a> = (&'a str, Option<Vec<&'a str>>, &'a [&'a str]);

/// The attributes a FIFO queue is created with here; `name` always carries the `.fifo`
/// suffix a FIFO queue name must end with.
fn fifo_attributes(extra: &[(&str, &str)]) -> Value {
    let mut attributes = json!({ "FifoQueue": "true" });
    for (name, value) in extra {
        attributes[*name] = json!(value);
    }
    attributes
}

/// Send one message and return the HTTP status and the answer.
async fn send(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "SendMessage", body).await
}

/// Send one message that must be accepted, and return the answer.
async fn send_ok(router: &MultiServiceRouter, body: Value) -> Value {
    ok(router, "SendMessage", body).await
}

/// Receive up to `max` messages from the queue at `url`.
async fn receive(router: &MultiServiceRouter, url: &str, max: u32) -> Vec<Value> {
    let answer = ok(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": max }),
    )
    .await;
    messages(&answer)
}

/// The `Body` of each message in `received`, in the order they were handed over.
fn bodies(received: &[Value]) -> Vec<String> {
    received
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One message's system `Attributes` map.
fn attributes(message: &Value) -> BTreeMap<String, String> {
    message.get("Attributes").map(as_map).unwrap_or_default()
}

/// The deduplication token `ContentBasedDeduplication` derives for `body`, re-derived
/// here from the rule the member documents — "Amazon SQS uses a SHA-256 hash to
/// generate the MessageDeduplicationId using the body of the message" — rather than
/// from the emulator's own helper.
fn content_token(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes()))
}

/// Which members a send to a FIFO queue must carry, and which it may not.
///
/// The rows span the combinations: the two tokens present or absent, with and without
/// `ContentBasedDeduplication`, a per-message `DelaySeconds`, and both FIFO-only
/// members against a STANDARD queue — where `MessageDeduplicationId` "applies only to
/// FIFO queues" and `MessageGroupId` is admitted ("in standard queues, using
/// MessageGroupId enables fair queues").
#[tokio::test]
async fn the_members_a_send_must_carry_follow_the_queue_type() {
    let router = emulator();
    let plain = create_ok(&router, "tokens.fifo", &fifo_attributes(&[])).await;
    let content_based = create_ok(
        &router,
        "content.fifo",
        &fifo_attributes(&[("ContentBasedDeduplication", "true")]),
    )
    .await;
    let standard = create_ok(&router, "tokens-standard", &json!({})).await;

    // (case, queue, extra members, accepted)
    let rows: &[(&str, &str, Value, bool)] = &[
        (
            "FIFO with both tokens",
            &plain,
            json!({ "MessageGroupId": "g", "MessageDeduplicationId": "d1" }),
            true,
        ),
        (
            "FIFO without a MessageGroupId",
            &plain,
            json!({ "MessageDeduplicationId": "d2" }),
            false,
        ),
        ("FIFO with no FIFO member at all", &plain, json!({}), false),
        (
            "FIFO without a MessageDeduplicationId and no ContentBasedDeduplication",
            &plain,
            json!({ "MessageGroupId": "g" }),
            false,
        ),
        (
            "FIFO without a MessageDeduplicationId, ContentBasedDeduplication enabled",
            &content_based,
            json!({ "MessageGroupId": "g" }),
            true,
        ),
        (
            "FIFO with a MessageDeduplicationId overriding the derived one",
            &content_based,
            json!({ "MessageGroupId": "g", "MessageDeduplicationId": "explicit" }),
            true,
        ),
        (
            "FIFO with a per-message DelaySeconds",
            &content_based,
            json!({ "MessageGroupId": "g", "DelaySeconds": 5 }),
            false,
        ),
        (
            "standard with a MessageGroupId, which enables fair queues",
            &standard,
            json!({ "MessageGroupId": "tenant-7" }),
            true,
        ),
        (
            "standard with a MessageDeduplicationId, which applies only to FIFO",
            &standard,
            json!({ "MessageDeduplicationId": "d" }),
            false,
        ),
        (
            "standard with a per-message DelaySeconds",
            &standard,
            json!({ "DelaySeconds": 0 }),
            true,
        ),
    ];

    for (case, url, extra, accepted) in rows {
        let mut body = json!({ "QueueUrl": url, "MessageBody": format!("body for {case}") });
        for (member, value) in extra.as_object().expect("an object of members") {
            body[member] = value.clone();
        }
        let (status, answer) = send(&router, &body).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {answer}"
            );
            assert!(
                answer["MessageId"].as_str().is_some_and(|id| !id.is_empty()),
                "case {case}: expected a MessageId, got {answer}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {case}: expected a 400 ValidationException, got {status} with {answer}"
            );
        }
    }
}

/// `MessageGroupId` and `MessageDeduplicationId` are bounded exactly as their own
/// documentation bounds them: "the length of MessageGroupId is 128 characters" / "the
/// maximum length of MessageDeduplicationId is 128 characters", with "alphanumeric
/// characters and punctuation (!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~)" as the value set.
/// The rows walk the published bound, one past it, the empty token, and characters
/// inside and outside the set.
#[tokio::test]
async fn a_fifo_token_is_bounded_in_length_and_in_its_characters() {
    use service_com_amazonaws_sqs_amazonsqs::state::MAX_MESSAGE_TOKEN_LENGTH;

    let router = emulator();
    let url = create_ok(&router, "bounds.fifo", &fifo_attributes(&[])).await;

    // (case, token, accepted)
    let rows: &[(&str, String, bool)] = &[
        ("one character", "g".to_string(), true),
        (
            "the published bound, 128 characters",
            "g".repeat(MAX_MESSAGE_TOKEN_LENGTH),
            true,
        ),
        (
            "one character past the bound",
            "g".repeat(MAX_MESSAGE_TOKEN_LENGTH + 1),
            false,
        ),
        ("empty", String::new(), false),
        // Punctuation the value set lists, including the characters a test is least
        // likely to try: the backtick, the backslash and the tilde.
        (
            "every punctuation character the set lists",
            "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~".to_string(),
            true,
        ),
        ("digits and mixed case", "aZ09".to_string(), true),
        ("a space, which is neither", "a b".to_string(), false),
        ("a tab, which is neither", "a\tb".to_string(), false),
        (
            "a non-ASCII letter, which is neither",
            "grüße".to_string(),
            false,
        ),
    ];

    for (index, (case, token, accepted)) in rows.iter().enumerate() {
        for member in ["MessageGroupId", "MessageDeduplicationId"] {
            let mut body = json!({
                "QueueUrl": url,
                "MessageBody": format!("body {index}"),
                "MessageGroupId": "g",
                "MessageDeduplicationId": format!("d-{index}-{member}"),
            });
            body[member] = json!(token);
            let (status, answer) = send(&router, &body).await;
            if *accepted {
                assert_eq!(
                    status, 200,
                    "case {member} {case}: expected HTTP 200, got {status} with {answer}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (400, "ValidationException"),
                    "case {member} {case}: expected a 400 ValidationException, got {status} \
                     with {answer}"
                );
                assert!(
                    answer["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(member)),
                    "case {member} {case}: the refusal must name the member, got {answer}"
                );
            }
        }
    }
}

/// A FIFO send assigns a `SequenceNumber` that keeps increasing, and a receive hands a
/// message group's messages over in strict send order. The table interleaves three
/// groups of different sizes, so the per-group order is asserted over a set of inputs
/// rather than over one stream.
#[tokio::test]
async fn a_fifo_receive_preserves_per_group_send_order() {
    let router = emulator();
    let url = create_ok(&router, "ordered.fifo", &fifo_attributes(&[])).await;

    // (group, body) in send order: interleaved, with groups of three, two and one.
    let sent: &[(&str, &str)] = &[
        ("alpha", "a1"),
        ("beta", "b1"),
        ("alpha", "a2"),
        ("gamma", "c1"),
        ("beta", "b2"),
        ("alpha", "a3"),
    ];

    let mut sequence_numbers: Vec<String> = Vec::new();
    for (group, body) in sent {
        let answer = send_ok(
            &router,
            json!({
                "QueueUrl": url,
                "MessageBody": body,
                "MessageGroupId": group,
                "MessageDeduplicationId": body,
            }),
        )
        .await;
        let sequence_number = answer["SequenceNumber"]
            .as_str()
            .unwrap_or_else(|| panic!("case send {body}: expected a SequenceNumber in {answer}"))
            .to_string();
        assert!(
            sequence_number.parse::<u128>().is_ok(),
            "case send {body}: SequenceNumber must be a decimal number inside 128 bits, got \
             {sequence_number}"
        );
        if let Some(previous) = sequence_numbers.last() {
            assert!(
                sequence_number.as_str() > previous.as_str()
                    && sequence_number.len() == previous.len(),
                "case send {body}: SequenceNumber must keep increasing, got {sequence_number} \
                 after {previous}"
            );
        }
        sequence_numbers.push(sequence_number);
    }
    assert_eq!(
        sequence_numbers.iter().collect::<BTreeSet<_>>().len(),
        sent.len(),
        "case sequence numbers: each send must get its own, got {sequence_numbers:?}"
    );

    let received = receive(&router, &url, 10).await;
    let handed = bodies(&received);
    assert_eq!(
        handed.len(),
        sent.len(),
        "case receive: expected every message, got {handed:?}"
    );
    for group in ["alpha", "beta", "gamma"] {
        let expected: Vec<&str> = sent
            .iter()
            .filter(|(sent_group, _)| *sent_group == group)
            .map(|(_, body)| *body)
            .collect();
        let actual: Vec<&str> = handed
            .iter()
            .map(String::as_str)
            .filter(|body| expected.contains(body))
            .collect();
        assert_eq!(
            actual, expected,
            "case group {group}: expected {expected:?} in send order, got {actual:?}"
        );
    }
}

/// "Messages within the same message group are always processed one at a time, in
/// strict order, ensuring that no two messages from the same group are processed
/// simultaneously": while a group's message is in flight, that group hands over nothing
/// more — but the other groups keep flowing, and deleting the in-flight message releases
/// the group's next one.
#[tokio::test]
async fn a_group_with_a_message_in_flight_hands_over_nothing_more() {
    let router = emulator();
    let url = create_ok(&router, "blocking.fifo", &fifo_attributes(&[])).await;

    for (group, body) in [("alpha", "a1"), ("alpha", "a2"), ("beta", "b1")] {
        send_ok(
            &router,
            json!({
                "QueueUrl": url,
                "MessageBody": body,
                "MessageGroupId": group,
                "MessageDeduplicationId": body,
            }),
        )
        .await;
    }

    let first = receive(&router, &url, 1).await;
    assert_eq!(
        bodies(&first),
        vec!["a1".to_string()],
        "case first receive: expected the oldest message of the first group, got {first:?}"
    );

    let second = receive(&router, &url, 1).await;
    assert_eq!(
        bodies(&second),
        vec!["b1".to_string()],
        "case second receive: group alpha is blocked, so beta's head is next, got {second:?}"
    );

    let third = receive(&router, &url, 1).await;
    assert!(
        third.is_empty(),
        "case third receive: both groups have a message in flight, got {third:?}"
    );

    let handle = first[0]["ReceiptHandle"]
        .as_str()
        .expect("a receipt handle on the delivered message");
    ok(
        &router,
        "DeleteMessage",
        json!({ "QueueUrl": url, "ReceiptHandle": handle }),
    )
    .await;

    let fourth = receive(&router, &url, 1).await;
    assert_eq!(
        bodies(&fourth),
        vec!["a2".to_string()],
        "case after deleting a1: group alpha releases its next message, got {fourth:?}"
    );
}

/// A resend carrying a deduplication token the queue has already accepted is "accepted
/// successfully but aren't delivered during the ... deduplication interval": it answers
/// for the message the token first admitted and the queue still holds one copy. The
/// rows span the ways a token arises (supplied, derived from the body, derived and then
/// supplied as the same value) and the two `DeduplicationScope` levels.
#[tokio::test]
async fn a_resend_inside_the_deduplication_interval_delivers_one_copy() {
    // (case, queue attributes, sends as (group, body, token), how many the queue keeps)
    let rows: &[DeduplicationRow<'_>] = &[
        (
            "the same supplied token twice",
            &[],
            &[
                ("g", "first", Some("token")),
                ("g", "second", Some("token")),
            ],
            1,
        ),
        (
            "two supplied tokens",
            &[],
            &[("g", "first", Some("one")), ("g", "second", Some("two"))],
            2,
        ),
        (
            "identical content under ContentBasedDeduplication",
            &[("ContentBasedDeduplication", "true")],
            &[("g", "same", None), ("g", "same", None)],
            1,
        ),
        (
            "different content under ContentBasedDeduplication",
            &[("ContentBasedDeduplication", "true")],
            &[("g", "one", None), ("g", "other", None)],
            2,
        ),
        (
            "a supplied token equal to the derived one",
            &[("ContentBasedDeduplication", "true")],
            &[("g", "same", None), ("g", "same", Some(""))],
            1,
        ),
        (
            "the same token in two groups, deduplicated per queue",
            &[],
            &[
                ("one", "first", Some("token")),
                ("two", "second", Some("token")),
            ],
            1,
        ),
        (
            "the same token in two groups, deduplicated per message group",
            &[("DeduplicationScope", "messageGroup")],
            &[
                ("one", "first", Some("token")),
                ("two", "second", Some("token")),
            ],
            2,
        ),
        (
            "the same token in one group, deduplicated per message group",
            &[("DeduplicationScope", "messageGroup")],
            &[
                ("one", "first", Some("token")),
                ("one", "second", Some("token")),
            ],
            1,
        ),
    ];

    for (index, (case, queue_attributes, sends, kept)) in rows.iter().enumerate() {
        let router = emulator();
        let url = create_ok(
            &router,
            &format!("dedup-{index}.fifo"),
            &fifo_attributes(queue_attributes),
        )
        .await;

        let mut answers: Vec<Value> = Vec::new();
        for (group, body, token) in *sends {
            let mut request = json!({
                "QueueUrl": url,
                "MessageBody": body,
                "MessageGroupId": group,
            });
            if let Some(token) = token {
                // An empty token stands for "the value ContentBasedDeduplication would
                // derive", which is the one case a caller cannot write by hand.
                let token = if token.is_empty() {
                    content_token(body)
                } else {
                    (*token).to_string()
                };
                request["MessageDeduplicationId"] = json!(token);
            }
            answers.push(send_ok(&router, request).await);
        }

        let received = receive(&router, &url, 10).await;
        assert_eq!(
            received.len(),
            *kept,
            "case {case}: expected {kept} message(s) delivered, got {:?}",
            bodies(&received)
        );

        if *kept == 1 {
            let first = &answers[0];
            let resend = answers.last().expect("at least one send");
            assert_eq!(
                (resend["MessageId"].as_str(), resend["SequenceNumber"].as_str()),
                (first["MessageId"].as_str(), first["SequenceNumber"].as_str()),
                "case {case}: the resend must answer for the message the token admitted, got \
                 {resend} after {first}"
            );
            assert_eq!(
                bodies(&received),
                vec![sends[0].1.to_string()],
                "case {case}: the copy delivered must be the first one, got {:?}",
                bodies(&received)
            );
        }
    }
}

/// A receive reports the three attributes a FIFO send records, and
/// `MessageSystemAttributeNames` selects which of them come back. The rows walk every
/// form a request can select in: nothing, `All`, one name, and a subset.
#[tokio::test]
async fn a_receive_reports_the_attributes_a_fifo_send_records() {
    let router = emulator();
    let url = create_ok(&router, "reported.fifo", &fifo_attributes(&[])).await;
    let answer = send_ok(
        &router,
        json!({
            "QueueUrl": url,
            "MessageBody": "payload",
            "MessageGroupId": "group-9",
            "MessageDeduplicationId": "token-9",
        }),
    )
    .await;
    let sequence_number = answer["SequenceNumber"]
        .as_str()
        .expect("a SequenceNumber on a FIFO send")
        .to_string();

    // (case, requested names, the FIFO attributes that must come back)
    let rows: &[SelectionRow<'_>] = &[
        ("nothing selected", None, &[]),
        (
            "All",
            Some(vec!["All"]),
            &["MessageGroupId", "MessageDeduplicationId", "SequenceNumber"],
        ),
        (
            "one name",
            Some(vec!["MessageGroupId"]),
            &["MessageGroupId"],
        ),
        (
            "a subset",
            Some(vec!["SequenceNumber", "MessageDeduplicationId"]),
            &["MessageDeduplicationId", "SequenceNumber"],
        ),
    ];

    for (case, requested, expected) in rows {
        let mut body = json!({ "QueueUrl": url, "VisibilityTimeout": 0 });
        if let Some(requested) = requested {
            body["MessageSystemAttributeNames"] = json!(requested);
        }
        let received = messages(&ok(&router, "ReceiveMessage", body).await);
        assert_eq!(
            received.len(),
            1,
            "case {case}: expected the one message back, got {received:?}"
        );
        let reported = attributes(&received[0]);
        let fifo_reported: Vec<&str> = ["MessageDeduplicationId", "MessageGroupId", "SequenceNumber"]
            .into_iter()
            .filter(|name| reported.contains_key(*name))
            .collect();
        let mut wanted: Vec<&str> = expected.to_vec();
        wanted.sort_unstable();
        assert_eq!(
            fifo_reported, wanted,
            "case {case}: expected {wanted:?}, got {fifo_reported:?}"
        );
        for name in *expected {
            let value = reported.get(*name).map(String::as_str).unwrap_or_default();
            let want = match *name {
                "MessageGroupId" => "group-9",
                "MessageDeduplicationId" => "token-9",
                _ => sequence_number.as_str(),
            };
            assert_eq!(
                value, want,
                "case {case}: {name} must report what the send recorded, got {value}"
            );
        }
    }
}

/// The messages a FIFO queue holds are the ones `GetQueueAttributes` counts: a send is
/// observable through the reporting path too, not only through a receive.
#[tokio::test]
async fn a_fifo_send_is_observable_through_get_queue_attributes() {
    let router = emulator();
    let url = create_ok(&router, "counted.fifo", &fifo_attributes(&[])).await;

    for (index, group) in ["alpha", "beta", "alpha"].into_iter().enumerate() {
        send_ok(
            &router,
            json!({
                "QueueUrl": url,
                "MessageBody": format!("message {index}"),
                "MessageGroupId": group,
                "MessageDeduplicationId": format!("token-{index}"),
            }),
        )
        .await;
        let reported = ok(
            &router,
            "GetQueueAttributes",
            json!({ "QueueUrl": url, "AttributeNames": ["ApproximateNumberOfMessages"] }),
        )
        .await;
        let counted = as_map(&reported["Attributes"])
            .get("ApproximateNumberOfMessages")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            counted,
            (index + 1).to_string(),
            "case after {} send(s): expected that many messages held, got {counted}",
            index + 1
        );
    }
}

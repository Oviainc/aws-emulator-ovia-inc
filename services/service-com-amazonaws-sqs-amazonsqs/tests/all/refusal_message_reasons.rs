// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT a refusal of this service SAYS, driven over the wire through
//! the shared harness in [`super::support`].
//!
//! The rule under test: a refusal's message is Amazon SQS's own fixed text for the error
//! class it is answered as, instantiated with the parameter, value or attribute taken from
//! the request — so a refusal whose class states a `Reason:` clause always carries that
//! clause naming the rule the request broke, and a refusal states the pre-condition it
//! enforces in its own sentence rather than reusing the documentation sentence of another
//! error class. Where the message is the only part of the answer that says WHICH other
//! value to change, dropping it leaves the caller unable to repair the request.
//!
//! Each table spans what the rule generalises over rather than one recorded request: the
//! OPERATION that refuses (`CreateQueue`, `SetQueueAttributes`, `ReceiveMessage`,
//! `StartMessageMoveTask`), WHICH half of a cross-attribute pair the request supplied,
//! the KIND of modification that makes a receive attempt un-retryable, the FORM of the
//! request value the message interpolates, and the kind of queue a pre-condition excludes
//! — each with the accepted case as its control, so a row never passes merely because
//! everything is refused.

use std::collections::BTreeMap;

use serde_json::json;

use super::support::{
    all_attributes, call, create, create_ok, emulator, error_code, expected_arn,
    fifo_attributes as fifo, messages, ok, redrives_to,
};

/// The rule the FIFO throughput limit and the deduplication scope satisfy together, as
/// the `Reason:` clause of the refusal that rejects the pair.
///
/// Source: the `FifoThroughputLimit` attribute's own documentation, reached with
/// `smithy_query(query="doc", shape="QueueAttributeName",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`
/// — "the perMessageGroupId value is allowed only when the value for DeduplicationScope
/// is messageGroup" — stated as the change the caller has to make.
const PAIR_REASON: &str = "To set FifoThroughputLimit to perMessageGroupId, the \
                           DeduplicationScope must be messageGroup.";

/// The whole message a refusal of the attribute pair must carry.
///
/// The attribute it names is the one the rule is stated ON — the throughput limit, whose
/// per-message-group value is the conditionally allowed one — whichever half of the pair
/// the refused call supplied a value for.
fn pair_message() -> String {
    format!("Invalid value for the parameter FifoThroughputLimit. Reason: {PAIR_REASON}")
}

/// The whole message a refusal of a `ReceiveRequestAttemptId` whose batch has been
/// modified must carry: the token comes from the request, the sentence is fixed.
fn not_retryable_message(attempt_id: &str) -> String {
    format!(
        "Value {attempt_id} for parameter ReceiveRequestAttemptId is invalid. Reason: The \
         receiveRequestAttemptId is not valid anymore since the receive call is not retryable."
    )
}

/// The whole message a message movement task whose source is not a dead-letter queue must
/// be refused with — entirely fixed text, because the parameter whose value is refused is
/// already named by the parameter-refusal class it is answered as.
const NOT_A_DEAD_LETTER_QUEUE: &str = "Source queue must be configured as a Dead Letter Queue.";

/// `pairs` as the `Attributes` map of a request.
fn attributes(pairs: &[(&str, &str)]) -> serde_json::Value {
    let mut supplied = json!({});
    for (name, value) in pairs {
        supplied[*name] = json!(value);
    }
    supplied
}

/// Assert that `status`/`answer` is the 400 `code` refusal carrying exactly `message`.
fn refused(case: &str, status: u16, answer: &serde_json::Value, code: &str, message: &str) {
    assert_eq!(
        (status, error_code(answer).as_str()),
        (400, code),
        "case {case}: expected a 400 {code}, got {status} with {answer}"
    );
    assert_eq!(
        answer["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer["message"]
    );
}

/// One row of the create table: its case, the attributes the create supplies beyond
/// `FifoQueue`, and whether the create is refused.
type CreateRow<'a> = (&'a str, &'a [(&'a str, &'a str)], bool);

/// One row of the update table: its case, the attributes the queue is created with, the
/// attributes the update supplies, and whether the update is refused.
type UpdateRow<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a str)],
    bool,
);

/// A `CreateQueue` refused for the cross-attribute pair states the rule the pair broke,
/// whichever way the request breaks it; the accepted combinations are the control.
#[tokio::test]
async fn a_create_refused_for_the_attribute_pair_states_the_rule() {
    // (case, attributes supplied beyond FifoQueue, refused)
    let rows: &[CreateRow<'_>] = &[
        (
            "the per-message-group limit against the queue scope",
            &[
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "queue"),
            ],
            true,
        ),
        (
            "the per-message-group limit with no scope supplied at all",
            &[("FifoThroughputLimit", "perMessageGroupId")],
            true,
        ),
        (
            "the per-message-group limit beside unrelated attributes",
            &[
                ("VisibilityTimeout", "45"),
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "queue"),
            ],
            true,
        ),
        (
            "the per-message-group limit with the message-group scope",
            &[
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "messageGroup"),
            ],
            false,
        ),
        (
            "the per-queue limit with the queue scope",
            &[
                ("FifoThroughputLimit", "perQueue"),
                ("DeduplicationScope", "queue"),
            ],
            false,
        ),
        ("neither half of the pair", &[], false),
    ];

    let router = emulator();
    for (index, (case, supplied, is_refused)) in rows.iter().enumerate() {
        let name = format!("pair-create-{index}.fifo");
        let (status, answer) = create(&router, &name, &fifo(supplied)).await;
        if *is_refused {
            refused(
                case,
                status,
                &answer,
                "InvalidAttributeValue",
                &pair_message(),
            );
            // Refused at 400 means no queue was created, so the name resolves to nothing.
            let (status, answer) = call(&router, "GetQueueUrl", &json!({"QueueName": name})).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "QueueDoesNotExist"),
                "case {case}: the refused create must have created no queue, got {status} with \
                 {answer}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {answer}"
            );
            let url = answer["QueueUrl"].as_str().unwrap_or_default().to_string();
            let reported: BTreeMap<String, String> = all_attributes(&router, &url).await;
            for (attribute, value) in *supplied {
                assert_eq!(
                    reported.get(*attribute).map(String::as_str),
                    Some(*value),
                    "case {case}: the created queue must report {attribute}={value}, got \
                     {reported:?}"
                );
            }
        }
    }
}

/// The same rule on `SetQueueAttributes`, where either half of the pair may come from the
/// request or from the queue it is applied to.
#[tokio::test]
async fn an_update_refused_for_the_attribute_pair_states_the_rule() {
    // (case, attributes the queue is created with, attributes set, refused)
    let rows: &[UpdateRow<'_>] = &[
        (
            "the scope taken off message-group on a per-message-group queue",
            &[
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "messageGroup"),
            ],
            &[("DeduplicationScope", "queue")],
            true,
        ),
        (
            "the limit raised on a queue carrying no scope",
            &[],
            &[("FifoThroughputLimit", "perMessageGroupId")],
            true,
        ),
        (
            "both halves of the pair wrong in one call",
            &[],
            &[
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "queue"),
            ],
            true,
        ),
        (
            "both halves of the pair set together",
            &[],
            &[
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "messageGroup"),
            ],
            false,
        ),
        (
            "the limit lowered on a per-message-group queue",
            &[
                ("FifoThroughputLimit", "perMessageGroupId"),
                ("DeduplicationScope", "messageGroup"),
            ],
            &[("FifoThroughputLimit", "perQueue")],
            false,
        ),
    ];

    let router = emulator();
    for (index, (case, created_with, set, is_refused)) in rows.iter().enumerate() {
        let name = format!("pair-set-{index}.fifo");
        let url = create_ok(&router, &name, &fifo(created_with)).await;
        // What the queue reports BEFORE the call, so a refusal can be shown to have left
        // every attribute exactly as it was rather than merely not to have applied the
        // value it refused.
        let before: BTreeMap<String, String> = all_attributes(&router, &url).await;
        let (status, answer) = call(
            &router,
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": attributes(set)}),
        )
        .await;
        let reported: BTreeMap<String, String> = all_attributes(&router, &url).await;
        if *is_refused {
            refused(
                case,
                status,
                &answer,
                "InvalidAttributeValue",
                &pair_message(),
            );
            // `LastModifiedTimestamp` moves with the clock whether or not anything was
            // written, so the comparison is over every other attribute.
            let settled = |map: &BTreeMap<String, String>| -> BTreeMap<String, String> {
                map.iter()
                    .filter(|(name, _)| name.as_str() != "LastModifiedTimestamp")
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            };
            assert_eq!(
                settled(&reported),
                settled(&before),
                "case {case}: the refused update must have left the queue as it was, got \
                 {reported:?} against {before:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {answer}"
            );
            for (attribute, value) in *set {
                assert_eq!(
                    reported.get(*attribute).map(String::as_str),
                    Some(*value),
                    "case {case}: the queue must report {attribute}={value}, got {reported:?}"
                );
            }
        }
    }
}

/// How the batch a `ReceiveRequestAttemptId` remembers is modified before the retry.
#[derive(Debug, Clone, Copy)]
enum Modify {
    /// Delete the messages of the batch at these positions.
    Delete(&'static [usize]),
    /// Change the visibility timeout of the message of the batch at this position.
    Visibility(usize),
}

/// A retry under a `ReceiveRequestAttemptId` whose batch has been modified is refused with
/// the token from the request AND the sentence saying the receive is no longer retryable,
/// which is what tells a consumer to re-receive instead of retrying the token.
#[tokio::test]
async fn a_non_retryable_attempt_id_refusal_states_why_the_retry_is_not_retryable() {
    // The token form rule — "the maximum length of ReceiveRequestAttemptId is 128
    // characters. ReceiveRequestAttemptId can contain alphanumeric characters (a-z, A-Z,
    // 0-9) and punctuation" (`ReceiveMessageRequest$ReceiveRequestAttemptId`) — so the
    // rows interpolate a short token, a punctuated one, a digit-only one and one at the
    // published bound.
    let longest = "a".repeat(128);
    let rows: &[(&str, &str, Modify)] = &[
        (
            "the first message of the batch deleted",
            "r1",
            Modify::Delete(&[0]),
        ),
        (
            "the last message of the batch deleted",
            "attempt.id-2",
            Modify::Delete(&[1]),
        ),
        (
            "every message of the batch deleted",
            "0123456789",
            Modify::Delete(&[0, 1]),
        ),
        (
            "a message of the batch given a new visibility timeout",
            "~retry!token~",
            Modify::Visibility(0),
        ),
        (
            "the batch modified under a token at the published length bound",
            longest.as_str(),
            Modify::Delete(&[0]),
        ),
    ];

    for (index, (case, attempt_id, modify)) in rows.iter().enumerate() {
        let router = emulator();
        let url = create_ok(&router, &format!("retry-{index}.fifo"), &fifo(&[])).await;
        // Three distinct message groups, so one receive of two hands over two messages.
        for (group, body) in [("alpha", "a1"), ("beta", "b1"), ("gamma", "c1")] {
            ok(
                &router,
                "SendMessage",
                json!({
                    "QueueUrl": url,
                    "MessageBody": body,
                    "MessageGroupId": group,
                    "MessageDeduplicationId": body,
                }),
            )
            .await;
        }

        let served = ok(
            &router,
            "ReceiveMessage",
            json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 2,
                "ReceiveRequestAttemptId": attempt_id,
            }),
        )
        .await;
        let batch = messages(&served);
        assert_eq!(
            batch.len(),
            2,
            "case {case}: expected a batch of two, got {served}"
        );
        let handle = |position: usize| {
            batch[position]["ReceiptHandle"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };

        match modify {
            Modify::Delete(positions) => {
                for position in *positions {
                    ok(
                        &router,
                        "DeleteMessage",
                        json!({"QueueUrl": url, "ReceiptHandle": handle(*position)}),
                    )
                    .await;
                }
            }
            Modify::Visibility(position) => {
                ok(
                    &router,
                    "ChangeMessageVisibility",
                    json!({
                        "QueueUrl": url,
                        "ReceiptHandle": handle(*position),
                        "VisibilityTimeout": 120,
                    }),
                )
                .await;
            }
        }

        let (status, answer) = call(
            &router,
            "ReceiveMessage",
            &json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 2,
                "ReceiveRequestAttemptId": attempt_id,
            }),
        )
        .await;
        refused(
            case,
            status,
            &answer,
            "InvalidParameterValue",
            &not_retryable_message(attempt_id),
        );
    }
}

/// What makes a queue a source a message movement task may take messages from.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// A standard queue no other queue's `RedrivePolicy` names.
    Plain,
    /// A FIFO queue no other queue's `RedrivePolicy` names.
    Fifo,
    /// A queue whose own `RedrivePolicy` drives ITS messages to a dead-letter queue, so
    /// it is a redrive source rather than a dead-letter queue.
    Redriving,
    /// A queue another queue's `RedrivePolicy` names as its dead-letter target.
    DeadLetter,
}

/// A message movement task whose source is not a dead-letter queue is refused with the
/// pre-condition stated in its own sentence; a real dead-letter queue is the control that
/// shows the refusal is about the source and not about the call.
#[tokio::test]
async fn a_move_task_source_that_is_no_dead_letter_queue_states_the_pre_condition() {
    let rows: &[(&str, Source)] = &[
        ("a standard queue nothing redrives to", Source::Plain),
        ("a FIFO queue nothing redrives to", Source::Fifo),
        ("a queue that is itself a redrive source", Source::Redriving),
        ("a queue another queue redrives to", Source::DeadLetter),
    ];

    let router = emulator();
    for (index, (case, source)) in rows.iter().enumerate() {
        let name = match source {
            Source::Fifo => format!("move-source-{index}.fifo"),
            _ => format!("move-source-{index}"),
        };
        match source {
            Source::Plain => {
                create_ok(&router, &name, &json!({})).await;
            }
            Source::Fifo => {
                create_ok(&router, &name, &fifo(&[])).await;
            }
            Source::Redriving => {
                let target = format!("move-target-{index}");
                create_ok(&router, &target, &json!({})).await;
                redrives_to(&router, &name, &expected_arn(&target)).await;
            }
            Source::DeadLetter => {
                create_ok(&router, &name, &json!({})).await;
                redrives_to(
                    &router,
                    &format!("move-feeder-{index}"),
                    &expected_arn(&name),
                )
                .await;
            }
        }

        let (status, answer) = call(
            &router,
            "StartMessageMoveTask",
            &json!({"SourceArn": expected_arn(&name)}),
        )
        .await;
        if matches!(source, Source::DeadLetter) {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {answer}"
            );
            assert!(
                answer["TaskHandle"].as_str().is_some_and(|handle| !handle.is_empty()),
                "case {case}: expected a TaskHandle, got {answer}"
            );
        } else {
            refused(
                case,
                status,
                &answer,
                "InvalidParameterValue",
                NOT_A_DEAD_LETTER_QUEUE,
            );
        }
    }
}

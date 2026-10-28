// SPDX-License-Identifier: CC0-1.0

//! What a receipt handle an EARLIER receive minted still names, driven over the wire
//! (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: a receipt handle names its message for as long as the queue HOLDS
//! that message, so on a standard queue a call carrying the handle an earlier receive
//! minted is served — the handle is tied to the message, not to the latest delivery of it.
//! A FIFO queue is the exception: its messages are "processed one at a time, in strict
//! order", so the receive that takes a message takes the lease on it with it, and once a
//! later receive has handed the message over the earlier handle acts on nothing. That
//! handle is refused as the `ReceiptHandle` VALUE it is — `InvalidParameterValue` at HTTP
//! 400, "Value {handle} for parameter ReceiptHandle is invalid. Reason: The receipt handle
//! has expired." — rather than as a handle the queue never minted, because a consumer
//! recovers from the two differently: a lease that moved on means receive again, while an
//! unknown handle means the request itself cannot be repaired.
//!
//! The table spans what the rule generalises over: both queue types, every operation that
//! takes a `ReceiptHandle` (the two single-message calls and both batch forms, where the
//! outcome is that ENTRY's alone), and both ways a handle can be superseded — a second
//! receive of a message released with `VisibilityTimeout: 0`, and a THIRD receive, so the
//! rule is not keyed to the first supersession.

use serde_json::{json, Value};

use super::support::{
    call, create_ok, emulator, error_code, fifo_attributes, message_counts, messages, ok,
};
use smithy_server_runtime::MultiServiceRouter;

/// How a refusal of a handle whose lease a later receive took is worded, as AWS words it:
/// fixed text with the handle the request carried filled in.
fn expired_handle_message(receipt_handle: &str) -> String {
    format!(
        "Value {receipt_handle} for parameter ReceiptHandle is invalid. \
         Reason: The receipt handle has expired."
    )
}

/// The error code that refusal carries.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The operations a row drives the superseded handle through.
#[derive(Debug, Clone, Copy)]
enum Through {
    /// `DeleteMessage`, whose answer is the whole call's.
    Delete,
    /// `ChangeMessageVisibility`, likewise.
    ChangeVisibility,
    /// `DeleteMessageBatch`, where the outcome is one entry's.
    DeleteBatch,
    /// `ChangeMessageVisibilityBatch`, likewise.
    ChangeVisibilityBatch,
}

impl Through {
    /// The operation's name on the wire.
    fn operation(self) -> &'static str {
        match self {
            Through::Delete => "DeleteMessage",
            Through::ChangeVisibility => "ChangeMessageVisibility",
            Through::DeleteBatch => "DeleteMessageBatch",
            Through::ChangeVisibilityBatch => "ChangeMessageVisibilityBatch",
        }
    }

    /// The request that drives `receipt_handle` through it against the queue at `url`.
    fn request(self, url: &str, receipt_handle: &str) -> Value {
        match self {
            Through::Delete => json!({"QueueUrl": url, "ReceiptHandle": receipt_handle}),
            Through::ChangeVisibility => json!({
                "QueueUrl": url,
                "ReceiptHandle": receipt_handle,
                "VisibilityTimeout": 0,
            }),
            Through::DeleteBatch => json!({
                "QueueUrl": url,
                "Entries": [{"Id": "only", "ReceiptHandle": receipt_handle}],
            }),
            Through::ChangeVisibilityBatch => json!({
                "QueueUrl": url,
                "Entries": [{
                    "Id": "only",
                    "ReceiptHandle": receipt_handle,
                    "VisibilityTimeout": 0,
                }],
            }),
        }
    }

    /// Whether this operation answers through a batch RESULT rather than through the
    /// call's own status.
    fn batched(self) -> bool {
        matches!(self, Through::DeleteBatch | Through::ChangeVisibilityBatch)
    }

    /// Whether a call this operation serves takes the message out of the queue.
    fn deletes(self) -> bool {
        matches!(self, Through::Delete | Through::DeleteBatch)
    }
}

/// Receive one message from the queue at `url`, releasing it again at once
/// (`VisibilityTimeout: 0`) so the next receive can hand it over, and answer the receipt
/// handle that receive minted.
async fn receive_and_release(router: &MultiServiceRouter, case: &str, url: &str) -> String {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 1, "VisibilityTimeout": 0}),
    )
    .await;
    let delivered = messages(&reply);
    assert_eq!(
        delivered.len(),
        1,
        "case {case}: expected the one message back, got {reply}"
    );
    delivered[0]["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no ReceiptHandle in {reply}"))
        .to_string()
}

/// `(the entry's Code, the entry's Message)` of the one `Failed` entry of a batch answer,
/// or `None` when the answer reports none.
fn failed_entry(answer: &Value) -> Option<(String, String)> {
    answer
        .get("Failed")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .map(|entry| {
            (
                entry["Code"].as_str().unwrap_or_default().to_string(),
                entry["Message"].as_str().unwrap_or_default().to_string(),
            )
        })
}

/// A handle an earlier receive minted still acts on its message on a standard queue, and
/// is refused as an expired lease on a FIFO one — through every operation that takes one.
#[tokio::test]
async fn a_superseded_handle_keeps_its_message_on_a_standard_queue_and_loses_it_on_a_fifo_one() {
    // (the queue type, how many receives come after the handle under test)
    let supersessions: [(&str, bool); 2] = [("standard", false), ("fifo", true)];
    let operations = [
        Through::Delete,
        Through::ChangeVisibility,
        Through::DeleteBatch,
        Through::ChangeVisibilityBatch,
    ];
    // One later receive, then two: the rule is about the handle no longer being the
    // latest, not about which receive replaced it.
    let later_receives = [1_usize, 2];

    for (kind, fifo) in supersessions {
        for operation in operations {
            for later in later_receives {
                let case = format!(
                    "a {kind} queue, {} after {later} later receive(s)",
                    operation.operation()
                );
                let router = emulator();
                let name = format!(
                    "lease-{kind}-{}-{later}{}",
                    operation.operation().to_lowercase(),
                    if fifo { ".fifo" } else { "" }
                );
                let attributes = if fifo {
                    fifo_attributes(&[])
                } else {
                    json!({})
                };
                let url = create_ok(&router, &name, &attributes).await;
                let mut send = json!({"QueueUrl": url, "MessageBody": "payload"});
                if fifo {
                    send["MessageGroupId"] = json!("g");
                    send["MessageDeduplicationId"] = json!("d");
                }
                ok(&router, "SendMessage", send).await;

                // The handle under test, then the receives that supersede it.
                let under_test = receive_and_release(&router, &case, &url).await;
                for round in 0..later {
                    let newer = receive_and_release(&router, &case, &url).await;
                    assert_ne!(
                        newer, under_test,
                        "case {case}, round {round}: every receive mints its own handle"
                    );
                }

                let (status, answer) = call(
                    &router,
                    operation.operation(),
                    &operation.request(&url, &under_test),
                )
                .await;
                let reported = failed_entry(&answer);
                if fifo {
                    // The lease moved on with the later receive, so this call acts on
                    // nothing and says why.
                    let expected = expired_handle_message(&under_test);
                    if operation.batched() {
                        assert_eq!(
                            (status, reported.clone()),
                            (
                                200,
                                Some((INVALID_PARAMETER_VALUE.to_string(), expected.clone()))
                            ),
                            "case {case}: expected the entry to fail as an expired lease, \
                             got {status} with {answer}"
                        );
                    } else {
                        assert_eq!(
                            (status, error_code(&answer).as_str()),
                            (400, INVALID_PARAMETER_VALUE),
                            "case {case}: expected a 400 {INVALID_PARAMETER_VALUE}, got \
                             {status} with {answer}"
                        );
                        assert_eq!(
                            answer
                                .get("message")
                                .or_else(|| answer.get("Message"))
                                .and_then(Value::as_str),
                            Some(expected.as_str()),
                            "case {case}: expected {expected:?}, got {answer}"
                        );
                    }
                    // Nothing happened: the message is still there for the consumer that
                    // holds the live handle.
                    assert_eq!(
                        message_counts(&router, &url).await,
                        (1, 0, 0),
                        "case {case}: a refused call must leave the queue as it was"
                    );
                    continue;
                }

                // A standard queue serves the call: the handle still names its message.
                assert_eq!(
                    status, 200,
                    "case {case}: expected the call to be served, got {status} with {answer}"
                );
                assert_eq!(
                    reported, None,
                    "case {case}: expected no failed entry, got {answer}"
                );
                let expected_left = if operation.deletes() { 0 } else { 1 };
                assert_eq!(
                    message_counts(&router, &url).await,
                    (expected_left, 0, 0),
                    "case {case}: expected {expected_left} message(s) left after the call"
                );
            }
        }
    }
}

/// The lease a FIFO handle carries is the only thing it loses: once the message itself is
/// gone the refusal is about the MESSAGE again, and a handle whose message a delete
/// already took still makes a repeat of that delete succeed.
#[tokio::test]
async fn a_fifo_handle_whose_message_is_gone_is_not_refused_as_an_expired_lease() {
    let router = emulator();
    let url = create_ok(&router, "lease-after-the-message.fifo", &fifo_attributes(&[])).await;
    ok(
        &router,
        "SendMessage",
        json!({
            "QueueUrl": url,
            "MessageBody": "payload",
            "MessageGroupId": "g",
            "MessageDeduplicationId": "d",
        }),
    )
    .await;

    let first = receive_and_release(&router, "the first receive", &url).await;
    let second = receive_and_release(&router, "the second receive", &url).await;

    // The live handle deletes the message, which is what makes the first handle name a
    // message that is gone rather than one another receive holds.
    ok(
        &router,
        "DeleteMessage",
        json!({"QueueUrl": url, "ReceiptHandle": second}),
    )
    .await;
    assert_eq!(
        message_counts(&router, &url).await,
        (0, 0, 0),
        "case after the delete: the message must be gone"
    );

    // Repeating that delete succeeds — "the request succeeds, but the message might not
    // be deleted" — and so does the superseded handle, whose message the same delete took.
    for (case, handle) in [("the live handle again", &second), ("the superseded handle", &first)] {
        let (status, answer) = call(
            &router,
            "DeleteMessage",
            &json!({"QueueUrl": url, "ReceiptHandle": handle}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a delete of a message this queue already deleted succeeds, got \
             {status} with {answer}"
        );
    }
}

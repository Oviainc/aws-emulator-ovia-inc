// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for THE FORM OF THE OPAQUE TOKENS THIS SERVICE HANDS OUT — a
//! message's `ReceiptHandle` and a message movement task's `TaskHandle` — driven over the
//! wire (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: **a handle Amazon SQS hands a caller is ONE opaque token — a single
//! long base64-style string, not an identifier a client could have assembled or taken
//! apart — and the service resolves the call that presents it back from the token itself.**
//! A receipt handle is "associated with a specific instance of receiving a message" and a
//! task handle is "an identifier associated with a message movement task": in both cases
//! the VALUE is the service's to mint and the client's to hand back verbatim, so what a
//! client can rely on is the token's form and nothing about its content. A bare
//! 36-character identifier, or two of them joined by a separator, is not that form: a
//! client that validates the token, embeds it in a URL or splits it on a separator diverges,
//! and a handle whose parts are guessable is forgeable.
//!
//! The rows span what the rule generalises over: the kind of queue a handle comes from
//! (standard, FIFO), how many handles one receive hands over (one, a batch of four), the
//! operation the handle is then presented to, and both handles the service mints. Each
//! token is asserted on its form AND on its round trip — the call that takes it back has to
//! be served, and a handle another queue minted has to be refused as that queue's — so a
//! token that merely looked opaque would fail the table too.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    attribute_count as count, call, create_ok, dead_letter_queue, emulator, error_code,
    expected_arn, fifo_attributes, messages, ok, send_all,
};

/// The shortest token this suite accepts as an opaque handle. A CHOSEN floor, not a
/// published one: Amazon SQS hands out a lengthy base64-style string, and this is well past
/// both the 36 characters of one identifier and the 73 of two of them joined, so a token
/// assembled out of identifiers cannot pass it.
const MIN_OPAQUE_TOKEN_LENGTH: usize = 64;

/// The longest token this suite accepts: a handle travels in a request body and is held by
/// a consumer, so it stays bounded. Also a chosen bound.
const MAX_OPAQUE_TOKEN_LENGTH: usize = 1_024;

/// How long a loop waits for messages a queue is owed before giving up.
const RECEIVE_DEADLINE: Duration = Duration::from_secs(10);

/// The visibility timeout a receive here takes its messages under, so every handle this
/// suite holds stays live for the whole row. Inside the documented
/// `ReceiveMessageRequest$VisibilityTimeout` range ("Values range: 0 to 43200").
const HELD_UNDER: i64 = 600;

/// The refusal a handle another queue minted draws, and its status.
const NOT_THIS_QUEUES: (u16, &str) = (404, "ReceiptHandleIsInvalid");

/// How that refusal names the handle it rejects.
fn not_this_queues_message(receipt_handle: &str) -> String {
    format!("The receipt handle {receipt_handle:?} is not valid for this queue.")
}

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Why `token` is not the form an opaque handle takes, or `None` when it is one.
///
/// The form is: one token of the base64 alphabet (the encoding Amazon SQS hands its handles
/// over in), long enough not to be an identifier, and no part of it an identifier a client
/// could have minted itself.
fn opaque_token_defect(token: &str) -> Option<String> {
    let length = token.chars().count();
    if !(MIN_OPAQUE_TOKEN_LENGTH..=MAX_OPAQUE_TOKEN_LENGTH).contains(&length) {
        return Some(format!(
            "its length {length} is outside {MIN_OPAQUE_TOKEN_LENGTH}..={MAX_OPAQUE_TOKEN_LENGTH}"
        ));
    }
    if let Some(outside) = token
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
    {
        return Some(format!("it holds {outside:?}, outside the base64 alphabet"));
    }
    if looks_like_identifier(token) {
        return Some("it is one bare identifier".to_string());
    }
    if let Some(part) = token
        .split(['/', '-', ':', '.'])
        .find(|part| looks_like_identifier(part))
    {
        return Some(format!("one of its parts, {part:?}, is a bare identifier"));
    }
    None
}

/// Whether `value` is one of the 36-character hyphenated identifiers a client can mint for
/// itself — the form a handle must NOT be, spelled out here rather than taken from a crate
/// so the check states what it rejects.
fn looks_like_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(position, byte)| {
        if matches!(position, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

/// Send `count` messages to the queue at `url`; a FIFO send names its message group and
/// gives each message its own deduplication token, so every message is delivered.
async fn send_messages(router: &MultiServiceRouter, url: &str, fifo: bool, count: usize) {
    if !fifo {
        let bodies: Vec<String> = (0..count).map(|number| format!("payload-{number}")).collect();
        send_all(router, url, &bodies).await;
        return;
    }
    for number in 0..count {
        let _ = ok(
            router,
            "SendMessage",
            json!({
                "QueueUrl": url,
                "MessageBody": format!("payload-{number}"),
                "MessageGroupId": "group",
                "MessageDeduplicationId": format!("token-{number}"),
            }),
        )
        .await;
    }
}

/// Receive from the queue at `url` until `wanted` messages have come over or the deadline
/// passes, and answer the receipt handle each one was handed over with.
async fn receive_handles(router: &MultiServiceRouter, url: &str, wanted: usize) -> Vec<String> {
    let deadline = Instant::now() + RECEIVE_DEADLINE;
    let mut handles: Vec<String> = Vec::with_capacity(wanted);
    while handles.len() < wanted && Instant::now() < deadline {
        let reply = ok(
            router,
            "ReceiveMessage",
            json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                "VisibilityTimeout": HELD_UNDER,
                // Inside the documented `WaitTimeSeconds` range ("an integer from 0 to 20").
                "WaitTimeSeconds": 1,
            }),
        )
        .await;
        for message in messages(&reply) {
            handles.push(
                message["ReceiptHandle"]
                    .as_str()
                    .unwrap_or_else(|| panic!("no ReceiptHandle in {message}"))
                    .to_string(),
            );
        }
    }
    handles
}

/// Every receipt handle a receive hands over is one opaque token, distinct per receive, and
/// the delete that takes it back is served.
#[tokio::test]
async fn a_receipt_handle_is_one_opaque_token_the_delete_resolves() {
    // (case, FIFO queue, how many messages the receive hands over)
    let rows: &[(&str, bool, usize)] = &[
        ("standard queue, one message", false, 1),
        ("standard queue, a batch of four", false, 4),
        ("FIFO queue, one message", true, 1),
        ("FIFO queue, a batch of four", true, 4),
    ];

    for (index, (case, fifo, held)) in rows.iter().enumerate() {
        let router = emulator();
        let suffix = if *fifo { ".fifo" } else { "" };
        let attributes = if *fifo {
            fifo_attributes(&[])
        } else {
            json!({})
        };
        let url = create_ok(&router, &format!("handles-{index}{suffix}"), &attributes).await;
        send_messages(&router, &url, *fifo, *held).await;

        let handles = receive_handles(&router, &url, *held).await;
        assert_eq!(
            handles.len(),
            *held,
            "case {case}: expected {held} handles, got {handles:?}"
        );
        for handle in &handles {
            assert!(
                opaque_token_defect(handle).is_none(),
                "case {case}: expected one opaque token, got {handle:?} ({})",
                opaque_token_defect(handle).unwrap_or_default()
            );
        }
        let mut distinct = handles.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            handles.len(),
            "case {case}: every receive mints its own handle, got {handles:?}"
        );

        // The token is the service's to resolve: handing it back deletes the message it was
        // minted for, and the queue ends up holding nothing.
        for handle in &handles {
            let (status, answer) = call(
                &router,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the handle {handle:?} must delete its message, got {status} \
                 with {answer}"
            );
        }
        assert_eq!(
            count(&router, &url, "ApproximateNumberOfMessages").await,
            0,
            "case {case}: every message must be gone once its handle was handed back"
        );
        assert_eq!(
            count(&router, &url, "ApproximateNumberOfMessagesNotVisible").await,
            0,
            "case {case}: no message may still be in flight"
        );
    }
}

/// The token still carries its own provenance: a handle one queue minted is refused by
/// another as that queue's handle, through every operation that takes one.
#[tokio::test]
async fn an_opaque_handle_of_another_queue_is_still_refused_as_that_queues() {
    let router = emulator();
    let addressed = create_ok(&router, "provenance-addressed", &json!({})).await;
    let theirs = create_ok(&router, "provenance-other", &json!({})).await;
    send_messages(&router, &theirs, false, 1).await;
    let handle = receive_handles(&router, &theirs, 1)
        .await
        .pop()
        .expect("case another queue's handle: a receive hands one over");
    assert!(
        opaque_token_defect(&handle).is_none(),
        "case another queue's handle: expected one opaque token, got {handle:?}"
    );

    let requests: [(&str, Value); 2] = [
        (
            "DeleteMessage",
            json!({"QueueUrl": addressed, "ReceiptHandle": handle}),
        ),
        (
            "ChangeMessageVisibility",
            json!({
                "QueueUrl": addressed,
                "ReceiptHandle": handle,
                "VisibilityTimeout": 30,
            }),
        ),
    ];
    for (operation, request) in &requests {
        let (status, answer) = call(&router, operation, request).await;
        assert_eq!(
            (
                status,
                error_code(&answer).as_str(),
                message_of(&answer).as_str()
            ),
            (
                NOT_THIS_QUEUES.0,
                NOT_THIS_QUEUES.1,
                not_this_queues_message(&handle).as_str()
            ),
            "case {operation} with another queue's handle: got {status} with {answer}"
        );
    }
}

/// A started message movement task answers with one opaque token too, the same token every
/// reader reports for the task, and the cancel that presents it is served.
#[tokio::test]
async fn a_message_move_task_handle_is_one_opaque_token_the_task_resolves_by() {
    // (case, how many messages the dead-letter queue holds, the rate the task runs at) —
    // enough messages at a slow enough rate that the task is still running when the cancel
    // lands, which is the only status a cancel is served for.
    let rows: &[(&str, usize, i64)] = &[
        ("six messages at one per second", 6, 1),
        ("ten messages at two per second", 10, 2),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (index, (case, held, rate)) in rows.iter().enumerate() {
        let router = emulator();
        let prefix = format!("task-token-{index}");
        let (dead_letter_url, dead_letter_arn, _source_url) =
            dead_letter_queue(&router, &prefix).await;
        let destination = format!("{prefix}-destination");
        let _ = create_ok(&router, &destination, &json!({})).await;
        let bodies: Vec<String> = (0..*held).map(|number| format!("{case} #{number}")).collect();
        send_all(&router, &dead_letter_url, &bodies).await;

        let reply = ok(
            &router,
            "StartMessageMoveTask",
            json!({
                "SourceArn": dead_letter_arn,
                "DestinationArn": expected_arn(&destination),
                "MaxNumberOfMessagesPerSecond": rate,
            }),
        )
        .await;
        let handle = reply["TaskHandle"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no TaskHandle in {reply}"))
            .to_string();
        assert!(
            opaque_token_defect(&handle).is_none(),
            "case {case}: expected one opaque token, got {handle:?} ({})",
            opaque_token_defect(&handle).unwrap_or_default()
        );
        assert!(
            !seen.contains(&handle),
            "case {case}: every task gets its own handle, and {handle:?} was already minted"
        );
        seen.push(handle.clone());

        // The reader reports the SAME token for the running task, so the value a caller
        // holds is the one the task is known by.
        let listed = ok(
            &router,
            "ListMessageMoveTasks",
            json!({"SourceArn": dead_letter_arn}),
        )
        .await;
        assert_eq!(
            listed["Results"][0]["TaskHandle"].as_str(),
            Some(handle.as_str()),
            "case {case}: expected the running task to report its handle, got {listed}"
        );

        // And the token resolves the task it was minted for: the cancel presenting it is
        // served and answers the progress the task had made.
        let (status, answer) = call(
            &router,
            "CancelMessageMoveTask",
            &json!({"TaskHandle": handle}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the handle must resolve its task, got {status} with {answer}"
        );
        assert!(
            answer["ApproximateNumberOfMessagesMoved"].is_number(),
            "case {case}: expected the task's progress, got {answer}"
        );
    }
}

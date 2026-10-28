// SPDX-License-Identifier: CC0-1.0

//! How a RECEIPT HANDLE is resolved, and the bound on the span a visibility change may
//! ask for, driven over the wire (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`)
//! through the shared harness in [`super::support`].
//!
//! What a change does once it is accepted — and the range, the `MessageNotInflight` and
//! the `QueueDoesNotExist` refusals around it — is pinned by
//! [`super::message_visibility_changes`]; this suite is about the two rules that one does
//! not cover:
//!
//! 1. The refusal of a handle that names no message says WHICH KIND of value it got. A
//!    value that is not formed as a receipt handle — an opaque token of a handle's length
//!    and alphabet — is refused as an invalid receipt handle at HTTP 404, with the
//!    sentence for that class, which names no value; a handle ANOTHER queue minted is
//!    refused at 404 as that queue's handle, naming it; and a handle of this queue whose
//!    message is gone (deleted or purged) is refused at HTTP 400 as a `ReceiptHandle`
//!    value the call cannot act on, naming the handle the request carried. None of them
//!    changes anything the queue holds. The classification belongs to the HANDLE, so a
//!    batch entry is judged by the same kinds of value and reports the same sentences.
//!    (What a handle a LATER receive superseded names is
//!    [`super::receipt_handle_leases`]'s rule.)
//! 2. A change cannot hide a message for longer than ONE receive may, counted from the
//!    receive its handle came from: what a consumer may still ask for shrinks by the span
//!    it has already held the message.
//!
//! The tables span what those rules generalise over rather than one recorded request: the
//! FORM of the value (shorter than a handle, a character no token holds, past the length
//! a handle has, an opaque handle of another queue service's form, a handle another queue
//! of this store minted, a well-formed handle that names nothing), how a handle stopped
//! naming a message (delete, purge, a later receive), and how long the consumer has
//! already held the message against the span it then asks for, at the bound and on both
//! sides of it. The spans are documented
//! durations measured in seconds at the shortest, so no row sleeps for one: they move the
//! instant a stored message was received into the past through the engine seam
//! [`super::support::emulator_over_engine`] hands back.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    queue_name_from_url, ComAmazonawsSqsAmazonsqsState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{call, create_ok, emulator_over_engine, message_counts, messages, ok, REGION};

/// The shortest visibility timeout the parameter takes: "Values range: 0 to 43200"
/// (`ChangeMessageVisibilityRequest$VisibilityTimeout`).
const MIN_VISIBILITY_TIMEOUT: i64 = 0;

/// The longest one, which is also the span one receive may hide a message for: "Maximum:
/// 12 hours" from the same member.
const MAX_VISIBILITY_TIMEOUT: i64 = 43_200;

/// The visibility timeout the queues below hand a message out under: long enough that no
/// row's own wall-clock time can make the message visible again on its own, and well
/// inside the documented range.
const HELD_UNDER: i64 = 600;

/// A well-formed receipt handle no receive of this emulator has ever minted, for the rows
/// that separate "not a handle at all" from "a handle naming nothing".
const UNKNOWN_HANDLE: &str = "0f9a1cb7-4f63-4a45-9b0e-2b5f7c0f2d11";

/// The longest value this service takes for a receipt handle, for the row that crosses
/// that bound. A handle is an opaque token a consumer holds, so the bound is the
/// emulator's own; the row past it must be refused as not a handle rather than echoed.
const MAX_HANDLE_LENGTH: usize = 1_024;

/// Send one message to the queue at `url`.
async fn send(router: &MultiServiceRouter, url: &str, body: &str) {
    let _ = ok(
        router,
        "SendMessage",
        json!({"QueueUrl": url, "MessageBody": body}),
    )
    .await;
}

/// Receive exactly one message from the queue at `url` under `visibility_timeout`, and
/// answer the receipt handle it was handed over with.
async fn receive_handle(router: &MultiServiceRouter, url: &str, visibility_timeout: i64) -> String {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 1,
            "VisibilityTimeout": visibility_timeout,
            // Inside the documented `WaitTimeSeconds` range ("an integer from 0 to 20"),
            // so a receive does not have to race the send it follows.
            "WaitTimeSeconds": 1,
        }),
    )
    .await;
    let handed = messages(&reply);
    assert_eq!(
        handed.len(),
        1,
        "case a receive of {url} under {visibility_timeout}: expected one message, got {reply}"
    );
    handed[0]["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case a receive of {url}: no ReceiptHandle in {reply}"))
        .to_string()
}

/// Ask for `visibility_timeout` on the handle `receipt_handle` of the queue at `url`.
async fn change(
    router: &MultiServiceRouter,
    url: &str,
    receipt_handle: &str,
    visibility_timeout: i64,
) -> (u16, Value) {
    call(
        router,
        "ChangeMessageVisibility",
        &json!({
            "QueueUrl": url,
            "ReceiptHandle": receipt_handle,
            "VisibilityTimeout": visibility_timeout,
        }),
    )
    .await
}

/// The error code and message a refusal carries, as the awsJson envelope carries them.
fn refusal(body: &Value) -> (String, String) {
    (
        super::support::error_code(body),
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// How AWS states that the value supplied is not a receipt handle at all: the class of
/// fault, naming no value.
fn not_a_handle_message() -> String {
    "The input receipt handle is invalid.".to_string()
}

/// How AWS states that the handle names no message whose visibility there is to change.
fn no_such_message_message(receipt_handle: &str) -> String {
    format!(
        "Value {receipt_handle} for parameter ReceiptHandle is invalid. \
         Reason: Message does not exist or is not available for visibility timeout change."
    )
}

/// How AWS states that a value which IS a receipt handle names no message of the queue it
/// was sent to.
fn no_longer_this_queues_message(receipt_handle: &str) -> String {
    format!("The receipt handle {receipt_handle:?} is not valid for this queue.")
}

/// How AWS states that the span asked for reaches past what one receive may hide a
/// message for.
fn beyond_limit_message(requested: i64) -> String {
    format!(
        "Value {requested} for parameter VisibilityTimeout is invalid. \
         Reason: Total VisibilityTimeout for the message is beyond the limit \
         [{MAX_VISIBILITY_TIMEOUT} seconds]."
    )
}

/// Move the instant every message of the queue at `url` was last received `aged_by` into
/// the past, which is how the span a consumer has already held a message is reached
/// without sleeping for it.
///
/// The extra half second keeps the WHOLE-second count the rule is applied in stable while
/// the request that follows is made, so a row asserts the bound and not the test's own
/// scheduling.
fn held_for(engine: &engine::InMemoryEngine, url: &str, aged_by: Duration) {
    let name = queue_name_from_url(url)
        .unwrap_or_else(|| panic!("case ageing the receive of {url}: the url names no queue"))
        .to_string();
    let base = SystemTime::now()
        .checked_sub(aged_by + Duration::from_millis(500))
        .unwrap_or_else(SystemTime::now);
    let store = engine.state::<ComAmazonawsSqsAmazonsqsState>();
    let mut store = store.lock();
    let queue = store
        .queue_mut(REGION, &name)
        .unwrap_or_else(|| panic!("case ageing the receive of {url}: no queue named {name}"));
    for message in &mut queue.messages {
        message.last_received_at = Some(base);
    }
}

/// How the handle a row changes the visibility through came to name no message.
#[derive(Debug, Clone, Copy)]
enum Retired {
    /// It never named one: the value is not a handle, or names nothing.
    Never(&'static str),
    /// It is longer than any handle a consumer can have been handed.
    PastTheHandleLength,
    /// The message it named was deleted.
    Deleted,
    /// The queue it named a message of was purged.
    Purged,
    /// Another queue minted it, so it is a handle — just not one of this queue's.
    AnotherQueue,
}

/// Which sentence the refusal must carry, for the three kinds of value a handle can be.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// Not a receipt handle at all.
    NotAHandle,
    /// A handle, minted by another queue.
    AnotherQueuesHandle,
    /// This queue's handle, naming no message any more.
    NoSuchMessage,
}

/// One row of the refusal table.
struct RefusalRow {
    case: &'static str,
    retired: Retired,
    status: u16,
    code: &'static str,
    expected: Expected,
}

/// A handle that names no message is refused by WHAT THE VALUE IS — not a handle at all,
/// or a handle naming nothing — in that class's own words, and the queue is left exactly
/// as it was.
#[tokio::test]
async fn a_handle_that_names_no_message_is_refused_by_what_the_value_is() {
    let rows: &[RefusalRow] = &[
        RefusalRow {
            case: "a value shorter than any handle, holding no handle's characters",
            retired: Retired::Never("not-a-receipt-handle"),
            status: 404,
            code: "ReceiptHandleIsInvalid",
            expected: Expected::NotAHandle,
        },
        RefusalRow {
            case: "a handle truncated past the form one has",
            retired: Retired::Never("0f9a1cb7-4f63-4a45-9b0e"),
            status: 404,
            code: "ReceiptHandleIsInvalid",
            expected: Expected::NotAHandle,
        },
        RefusalRow {
            case: "a single character",
            retired: Retired::Never("x"),
            status: 404,
            code: "ReceiptHandleIsInvalid",
            expected: Expected::NotAHandle,
        },
        RefusalRow {
            case: "a value holding a character no opaque token does",
            retired: Retired::Never("AQEBwJnKyrHigUMZj6rYigCgxlaS3SLy0a not a handle"),
            status: 404,
            code: "ReceiptHandleIsInvalid",
            expected: Expected::NotAHandle,
        },
        RefusalRow {
            case: "a value longer than any handle a consumer holds",
            retired: Retired::PastTheHandleLength,
            status: 404,
            code: "ReceiptHandleIsInvalid",
            expected: Expected::NotAHandle,
        },
        RefusalRow {
            case: "a handle another queue minted",
            retired: Retired::AnotherQueue,
            status: 404,
            code: "ReceiptHandleIsInvalid",
            expected: Expected::AnotherQueuesHandle,
        },
        RefusalRow {
            case: "a longer opaque handle, of the form another queue service mints",
            retired: Retired::Never(
                "AQEBwJnKyrHigUMZj6rYigCgxlaS3SLy0a7QMW0uY9ZQnY3Zi1FVgvCPHE+LQWRpMlXDlFLZrGxVHOwZ0hlPHQ==",
            ),
            status: 400,
            code: "InvalidParameterValue",
            expected: Expected::NoSuchMessage,
        },
        RefusalRow {
            case: "a well-formed handle no receive minted",
            retired: Retired::Never(UNKNOWN_HANDLE),
            status: 400,
            code: "InvalidParameterValue",
            expected: Expected::NoSuchMessage,
        },
        RefusalRow {
            case: "the handle of a message that was deleted",
            retired: Retired::Deleted,
            status: 400,
            code: "InvalidParameterValue",
            expected: Expected::NoSuchMessage,
        },
        RefusalRow {
            case: "the handle of a message a purge took",
            retired: Retired::Purged,
            status: 400,
            code: "InvalidParameterValue",
            expected: Expected::NoSuchMessage,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, _engine) = emulator_over_engine();
        let url = create_ok(&router, &format!("refusing-{index}"), &json!({})).await;
        send(&router, &url, row.case).await;

        let handle = match row.retired {
            Retired::Never(value) => value.to_string(),
            // One character past the longest value this service takes for a handle, so
            // the bound itself is what the row crosses.
            Retired::PastTheHandleLength => "A".repeat(MAX_HANDLE_LENGTH + 1),
            Retired::Deleted => {
                let handle = receive_handle(&router, &url, HELD_UNDER).await;
                let _ = ok(
                    &router,
                    "DeleteMessage",
                    json!({"QueueUrl": url, "ReceiptHandle": handle}),
                )
                .await;
                handle
            }
            Retired::Purged => {
                let handle = receive_handle(&router, &url, HELD_UNDER).await;
                let _ = ok(&router, "PurgeQueue", json!({"QueueUrl": url})).await;
                handle
            }
            Retired::AnotherQueue => {
                let elsewhere =
                    create_ok(&router, &format!("refusing-{index}-elsewhere"), &json!({})).await;
                send(&router, &elsewhere, row.case).await;
                receive_handle(&router, &elsewhere, HELD_UNDER).await
            }
        };

        let before = message_counts(&router, &url).await;
        let (status, answer) = change(&router, &url, &handle, 30).await;
        let (code, message) = refusal(&answer);
        assert_eq!(
            (status, code.as_str()),
            (row.status, row.code),
            "case {}: expected a {} {}, got {status} with {answer}",
            row.case,
            row.status,
            row.code
        );
        let expected_message = match row.expected {
            Expected::NotAHandle => not_a_handle_message(),
            Expected::AnotherQueuesHandle => no_longer_this_queues_message(&handle),
            Expected::NoSuchMessage => no_such_message_message(&handle),
        };
        assert_eq!(
            message, expected_message,
            "case {}: expected the message {expected_message:?}, got {message:?}",
            row.case
        );
        assert_eq!(
            message_counts(&router, &url).await,
            before,
            "case {}: a refused change must leave the queue as it was",
            row.case
        );
    }
}

/// The classification is the HANDLE's, not the single-message call's: a batch entry is
/// judged by the same three kinds of value, and says so in the entry's own `Message`,
/// where that text is all a client gets for the entry.
#[tokio::test]
async fn a_batch_entry_is_judged_by_the_same_kinds_of_value() {
    let (router, _engine) = emulator_over_engine();
    let url = create_ok(&router, "batched-handles", &json!({})).await;
    send(&router, &url, "held by this queue").await;
    let held = receive_handle(&router, &url, HELD_UNDER).await;

    let elsewhere = create_ok(&router, "batched-handles-elsewhere", &json!({})).await;
    send(&router, &elsewhere, "held by another queue").await;
    let another_queues = receive_handle(&router, &elsewhere, HELD_UNDER).await;

    let answer = ok(
        &router,
        "ChangeMessageVisibilityBatch",
        json!({"QueueUrl": url, "Entries": [
            {"Id": "live", "ReceiptHandle": held, "VisibilityTimeout": 30},
            {"Id": "not-a-handle", "ReceiptHandle": "not-a-receipt-handle",
             "VisibilityTimeout": 30},
            {"Id": "another-queues", "ReceiptHandle": another_queues,
             "VisibilityTimeout": 30},
        ]}),
    )
    .await;

    let succeeded: Vec<String> = answer
        .get("Successful")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        succeeded,
        vec!["live".to_string()],
        "case a batch of the three kinds: only the live handle's entry can be applied, \
         got {answer}"
    );

    let reported: Vec<(String, String, String)> = answer
        .get("Failed")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Id"].as_str().unwrap_or_default().to_string(),
                entry["Code"].as_str().unwrap_or_default().to_string(),
                entry["Message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let expected = vec![
        (
            "not-a-handle".to_string(),
            "ReceiptHandleIsInvalid".to_string(),
            not_a_handle_message(),
        ),
        (
            "another-queues".to_string(),
            "ReceiptHandleIsInvalid".to_string(),
            no_longer_this_queues_message(&another_queues),
        ),
    ];
    assert_eq!(
        reported, expected,
        "case a batch of the three kinds: expected {expected:?}, got {answer}"
    );
}

/// One row of the "time already held" table: how long the consumer has held the message,
/// the span it then asks for, and whether what one receive may hide the message for still
/// covers it.
struct HeldRow {
    case: &'static str,
    held_for_seconds: i64,
    requested: i64,
    accepted: bool,
}

/// A change cannot hide a message for longer than ONE receive may, counted from the
/// receive its handle came from: what a consumer may still ask for shrinks by the span it
/// has already held the message.
#[tokio::test]
async fn a_change_cannot_reach_past_what_one_receive_may_hide_a_message_for() {
    let rows: &[HeldRow] = &[
        HeldRow {
            case: "the whole span, asked for the moment the message was received",
            held_for_seconds: 0,
            requested: MAX_VISIBILITY_TIMEOUT,
            accepted: true,
        },
        HeldRow {
            case: "the whole span, asked for an hour into the work",
            held_for_seconds: 3_600,
            requested: MAX_VISIBILITY_TIMEOUT,
            accepted: false,
        },
        HeldRow {
            case: "exactly what is left an hour into the work",
            held_for_seconds: 3_600,
            requested: MAX_VISIBILITY_TIMEOUT - 3_600,
            accepted: true,
        },
        HeldRow {
            case: "one second more than is left an hour into the work",
            held_for_seconds: 3_600,
            requested: MAX_VISIBILITY_TIMEOUT - 3_600 + 1,
            accepted: false,
        },
        HeldRow {
            case: "a second, with the whole span already held",
            held_for_seconds: MAX_VISIBILITY_TIMEOUT,
            requested: 1,
            accepted: false,
        },
        HeldRow {
            case: "nothing at all, with the whole span already held",
            held_for_seconds: MAX_VISIBILITY_TIMEOUT,
            requested: MIN_VISIBILITY_TIMEOUT,
            accepted: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, engine) = emulator_over_engine();
        let url = create_ok(&router, &format!("held-{index}"), &json!({})).await;
        send(&router, &url, row.case).await;
        let handle = receive_handle(&router, &url, HELD_UNDER).await;
        held_for(
            &engine,
            &url,
            Duration::from_secs(u64::try_from(row.held_for_seconds).unwrap_or_default()),
        );

        let (status, answer) = change(&router, &url, &handle, row.requested).await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {}: expected HTTP 200, got {status} with {answer}",
                row.case
            );
            continue;
        }
        let (code, message) = refusal(&answer);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidParameterValue"),
            "case {}: expected a 400 InvalidParameterValue, got {status} with {answer}",
            row.case
        );
        let expected = beyond_limit_message(row.requested);
        assert_eq!(
            message, expected,
            "case {}: expected the message {expected:?}, got {message:?}",
            row.case
        );
    }
}

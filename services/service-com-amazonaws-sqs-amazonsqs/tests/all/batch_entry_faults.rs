// SPDX-License-Identifier: CC0-1.0

//! How a batch operation reports a fault in ONE entry, and the one refusal a busy
//! message-move source answers with — driven over the wire (`POST /` with `X-Amz-Target:
//! AmazonSQS.<Operation>`) through the shared harness in [`super::support`].
//!
//! The RULES asserted here, each over a TABLE of inputs rather than one example:
//!
//! * a batch entry whose REQUIRED member is supplied with no value is that entry's own
//!   `EmptyValue` fault, whose message names the member by its path in the request
//!   (`<Operation>RequestEntry.<n>.<Member>`, `<n>` the entry's one-based place in
//!   `Entries`), and its siblings are still acted on;
//! * on a FIFO queue a `SendMessageBatch` entry the queue will not take refuses the WHOLE
//!   request, with nothing enqueued, because enqueuing the siblings would let the caller's
//!   retry of that entry land after them;
//! * the queue's `MaximumMessageSize` bounds EACH entry — an entry past it fails alone
//!   while its siblings are taken — and the sum of a batch's entries is bounded only by
//!   the service's own fixed payload ceiling;
//! * a delete is idempotent: a receipt handle whose message a delete already consumed is a
//!   successful entry, however many entries or calls name it;
//! * a value that is not a receipt handle at all and a receipt handle another queue minted
//!   are two different refusals, worded differently;
//! * a second message movement task on a source that already has one running is refused as
//!   a parameter value, and the source keeps its one task.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    maximum_batch_payload_bytes, MAX_BATCH_ENTRIES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    all_attributes, call, create_ok, dead_letter_queue, emulator, error_code, message_counts,
    messages, ok, send_all,
};

/// The per-entry code for a member supplied with no value.
const EMPTY_VALUE: &str = "EmptyValue";

/// The per-entry code for a value that is not a receipt handle this queue can resolve.
const RECEIPT_HANDLE_IS_INVALID: &str = "ReceiptHandleIsInvalid";

/// The `Successful` entry ids of a batch answer, in the order they were reported.
fn successful_ids(answer: &Value) -> Vec<String> {
    answer["Successful"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `Failed` entries of a batch answer, in the order they were reported.
fn failed(answer: &Value) -> Vec<Value> {
    answer["Failed"].as_array().cloned().unwrap_or_default()
}

/// The `(Code, SenderFault, Message)` of the failed entry called `id`.
fn failure(answer: &Value, id: &str) -> (String, bool, String) {
    let entry = failed(answer)
        .into_iter()
        .find(|entry| entry["Id"] == json!(id))
        .unwrap_or_else(|| panic!("no failed entry {id} in {answer}"));
    (
        entry["Code"].as_str().unwrap_or_default().to_string(),
        entry["SenderFault"].as_bool().unwrap_or_default(),
        entry["Message"].as_str().unwrap_or_default().to_string(),
    )
}

/// Send `count` messages to the queue at `url` and hand back the receipt handle of each
/// one, by receiving them all.
async fn received_handles(router: &MultiServiceRouter, url: &str, count: usize) -> Vec<String> {
    let bodies: Vec<String> = (0..count).map(|index| format!("body-{index}")).collect();
    send_all(router, url, &bodies).await;
    let handed = ok(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": count }),
    )
    .await;
    let handles: Vec<String> = messages(&handed)
        .iter()
        .map(|message| {
            message["ReceiptHandle"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        handles.len(),
        count,
        "case the setup receive of {url}: expected {count} handle(s), got {handles:?}"
    );
    handles
}

/// A member supplied with no value is the ENTRY's fault, named by its place in the
/// request. Rows differ along the operation (and therefore the member), the width of the
/// batch and WHERE in it the empty-valued entry sits.
#[tokio::test]
async fn an_entry_supplying_no_value_for_a_required_member_fails_alone() {
    // (case, operation, the member the entry leaves empty, the batch's width, the
    // zero-based place of the empty-valued entry)
    let rows: &[(&str, &str, &str, usize, usize)] = &[
        ("the first of two", "SendMessageBatch", "MessageBody", 2, 0),
        ("the last of two", "SendMessageBatch", "MessageBody", 2, 1),
        ("the middle of three", "SendMessageBatch", "MessageBody", 3, 1),
        (
            "one of a full-width batch",
            "SendMessageBatch",
            "MessageBody",
            MAX_BATCH_ENTRIES,
            4,
        ),
        (
            "the first of two",
            "DeleteMessageBatch",
            "ReceiptHandle",
            2,
            0,
        ),
        (
            "the last of three",
            "DeleteMessageBatch",
            "ReceiptHandle",
            3,
            2,
        ),
        (
            "one of a full-width batch",
            "DeleteMessageBatch",
            "ReceiptHandle",
            MAX_BATCH_ENTRIES,
            7,
        ),
    ];

    for (index, (case, operation, member, width, empty_at)) in rows.iter().enumerate() {
        let router = emulator();
        let url = support_queue(&router, index).await;
        let sends = *operation == "SendMessageBatch";
        // A delete needs a live handle per entry; a send needs none.
        let handles = if sends {
            Vec::new()
        } else {
            received_handles(&router, &url, *width).await
        };

        let entries: Vec<Value> = (0..*width)
            .map(|position| {
                let value = if position == *empty_at {
                    String::new()
                } else if sends {
                    format!("kept-body-{position}")
                } else {
                    handles[position].clone()
                };
                json!({ "Id": format!("e{position}"), *member: value })
            })
            .collect();

        let answer = ok(
            &router,
            operation,
            json!({ "QueueUrl": url, "Entries": entries }),
        )
        .await;

        let kept: Vec<String> = (0..*width)
            .filter(|position| position != empty_at)
            .map(|position| format!("e{position}"))
            .collect();
        assert_eq!(
            successful_ids(&answer),
            kept,
            "case {operation}, {case}: expected every other entry acted on, got {answer}"
        );
        assert_eq!(
            failed(&answer).len(),
            1,
            "case {operation}, {case}: expected exactly one failed entry, got {answer}"
        );
        assert_eq!(
            failure(&answer, &format!("e{empty_at}")),
            (
                EMPTY_VALUE.to_string(),
                true,
                format!(
                    "No Value Found for {operation}RequestEntry.{}.{member}.",
                    empty_at + 1
                ),
            ),
            "case {operation}, {case}: expected the sender's empty-value fault naming the \
             member by its place in the request, got {answer}"
        );

        // The empty-valued entry changed nothing: a send did not enqueue it, and a delete
        // deleted nothing for it.
        let (available, _delayed, in_flight) = message_counts(&router, &url).await;
        let expected = if sends {
            (i64::try_from(*width).unwrap_or_default() - 1, 0)
        } else {
            (0, 1)
        };
        assert_eq!(
            (available, in_flight),
            expected,
            "case {operation}, {case}: expected (available, in flight) {expected:?}"
        );
    }
}

/// One standard queue per row, named for the row so a failure says which one.
async fn support_queue(router: &MultiServiceRouter, index: usize) -> String {
    create_ok(router, &format!("batch-faults-{index}"), &json!({})).await
}

/// On a FIFO queue a batch is all or nothing: an entry the queue will not take refuses
/// the whole request, nothing is enqueued, and the same batch without that entry is
/// accepted whole. Rows differ along WHY the entry is unacceptable and where it sits.
#[tokio::test]
async fn a_fifo_batch_with_an_unacceptable_entry_is_refused_whole() {
    /// The `MaximumMessageSize` the oversize row is judged against, the smallest the
    /// attribute documents ("an integer from 1,024 bytes (1 KiB) to 1,048,576 bytes").
    const SMALL_MAXIMUM: usize = 1_024;

    struct Row {
        case: &'static str,
        /// What the bad entry carries instead of a well-formed body and group id.
        patch: Value,
        /// Its zero-based place in the ten-entry batch.
        bad_at: usize,
        status: u16,
        code: &'static str,
        message: String,
    }
    let rows = [
        Row {
            case: "an entry whose MessageBody carries no value",
            patch: json!({ "MessageBody": "" }),
            bad_at: 4,
            status: 400,
            code: "InvalidParameterValue",
            message: "No Value Found for SendMessageBatchRequestEntry.5.MessageBody.".to_string(),
        },
        Row {
            case: "the first entry carrying no body",
            patch: json!({ "MessageBody": "" }),
            bad_at: 0,
            status: 400,
            code: "InvalidParameterValue",
            message: "No Value Found for SendMessageBatchRequestEntry.1.MessageBody.".to_string(),
        },
        Row {
            case: "an entry with no MessageGroupId",
            patch: json!({ "MessageGroupId": Value::Null }),
            bad_at: 9,
            status: 400,
            code: "MissingParameter",
            message: "The request must contain the parameter MessageGroupId.".to_string(),
        },
        Row {
            case: "an entry whose MessageDeduplicationId is not a token",
            patch: json!({ "MessageDeduplicationId": "not a token" }),
            bad_at: 2,
            status: 400,
            code: "InvalidParameterValue",
            message: "Value not a token for parameter MessageDeduplicationId is invalid. \
                      Reason: MessageDeduplicationId can only include alphanumeric and \
                      punctuation characters. 1 to 128 in length."
                .to_string(),
        },
        Row {
            case: "an entry whose body holds a character outside the allowed set",
            patch: json!({ "MessageBody": "payload\u{1}" }),
            bad_at: 1,
            status: 400,
            code: "InvalidMessageContents",
            message: "Invalid binary character '#x1' was found in the message body, the set \
                      of allowed characters is #x9 | #xA | #xD | [#x20-#xD7FF] | \
                      [#xE000-#xFFFD] | [#x10000-#x10FFFF]."
                .to_string(),
        },
        Row {
            case: "an entry one byte past the queue's MaximumMessageSize",
            patch: json!({ "MessageBody": "x".repeat(SMALL_MAXIMUM + 1) }),
            bad_at: 6,
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "One or more parameters cannot be validated. Reason: Message must be shorter \
                 than {SMALL_MAXIMUM} bytes."
            ),
        },
    ];


    for (index, row) in rows.iter().enumerate() {
        let router = emulator();
        let url = create_ok(
            &router,
            &format!("fifo-whole-{index}.fifo"),
            &json!({
                "FifoQueue": "true",
                "ContentBasedDeduplication": "false",
                "MaximumMessageSize": SMALL_MAXIMUM.to_string(),
            }),
        )
        .await;

        let entry = |position: usize| -> Value {
            json!({
                "Id": format!("e{position}"),
                "MessageBody": format!("body-{position}"),
                "MessageGroupId": "group",
                "MessageDeduplicationId": format!("token-{index}-{position}"),
            })
        };
        let mut entries: Vec<Value> = (0..MAX_BATCH_ENTRIES).map(entry).collect();
        for (member, value) in row.patch.as_object().expect("the patch is an object") {
            if value.is_null() {
                entries[row.bad_at]
                    .as_object_mut()
                    .expect("an entry is an object")
                    .remove(member);
            } else {
                entries[row.bad_at][member] = value.clone();
            }
        }

        let (status, body) = call(
            &router,
            "SendMessageBatch",
            &json!({ "QueueUrl": url, "Entries": entries.clone() }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (row.status, row.code),
            "case {}: expected HTTP {} {}, got {status} with {body}",
            row.case,
            row.status,
            row.code
        );
        assert_eq!(
            body["message"].as_str(),
            Some(row.message.as_str()),
            "case {}: expected the refusal to say why, got {body}",
            row.case
        );
        assert!(
            body.get("Successful").is_none() && body.get("Failed").is_none(),
            "case {}: a refused request reports no entry outcomes, got {body}",
            row.case
        );
        assert_eq!(
            message_counts(&router, &url).await,
            (0, 0, 0),
            "case {}: a FIFO batch refused whole must enqueue nothing",
            row.case
        );

        // The control: the same batch without the one bad entry is taken whole, so the row
        // refused the entry rather than the shape of the request.
        entries.remove(row.bad_at);
        let answer = ok(
            &router,
            "SendMessageBatch",
            json!({ "QueueUrl": url, "Entries": entries }),
        )
        .await;
        assert_eq!(
            successful_ids(&answer).len(),
            MAX_BATCH_ENTRIES - 1,
            "case {}: the control batch must be accepted whole, got {answer}",
            row.case
        );
    }
}

/// The queue's `MaximumMessageSize` bounds ONE message: an entry past it fails alone
/// while its siblings are taken, and a batch whose entries each fit is accepted however
/// many bytes they come to together. Rows walk the configured bound and, for each, the
/// sizes at it and one byte past it.
#[tokio::test]
async fn the_queues_maximum_message_size_bounds_each_entry_not_the_batch() {
    // The bounds the attribute documents ("an integer from 1,024 bytes (1 KiB) to
    // 1,048,576 bytes (1 MiB)"), its smallest value and two larger ones.
    let maxima: &[usize] = &[1_024, 2_048, 64 * 1_024];

    for maximum in maxima.iter().copied() {
        let router = emulator();
        let url = create_ok(
            &router,
            &format!("entry-size-{maximum}"),
            &json!({ "MaximumMessageSize": maximum.to_string() }),
        )
        .await;

        let answer = ok(
            &router,
            "SendMessageBatch",
            json!({
                "QueueUrl": url,
                "Entries": [
                    { "Id": "under", "MessageBody": "a".repeat(maximum - 1) },
                    { "Id": "at", "MessageBody": "b".repeat(maximum) },
                    { "Id": "over", "MessageBody": "c".repeat(maximum + 1) },
                ],
            }),
        )
        .await;
        assert_eq!(
            successful_ids(&answer),
            vec!["under".to_string(), "at".to_string()],
            "case MaximumMessageSize {maximum}: the entries at or under the bound are \
             taken, got {answer}"
        );
        assert_eq!(
            failure(&answer, "over"),
            (
                "InvalidParameterValue".to_string(),
                true,
                format!(
                    "One or more parameters cannot be validated. Reason: Message must be \
                     shorter than {maximum} bytes."
                ),
            ),
            "case MaximumMessageSize {maximum}: the oversize entry fails alone, naming the \
             queue's own bound, got {answer}"
        );

        // Several entries each inside the bound are accepted whole even though their sum
        // is far past it: the attribute is not a bound on the request's payload.
        let wide: Vec<Value> = (0..MAX_BATCH_ENTRIES)
            .map(|position| {
                json!({
                    "Id": format!("w{position}"),
                    "MessageBody": "d".repeat(maximum),
                })
            })
            .collect();
        let answer = ok(
            &router,
            "SendMessageBatch",
            json!({ "QueueUrl": url, "Entries": wide }),
        )
        .await;
        assert_eq!(
            successful_ids(&answer).len(),
            MAX_BATCH_ENTRIES,
            "case MaximumMessageSize {maximum}: a batch of entries that each fit is \
             accepted whole, got {answer}"
        );
        assert_eq!(
            failed(&answer).len(),
            0,
            "case MaximumMessageSize {maximum}: expected no failed entry, got {answer}"
        );

        // Read back through the queue rather than from the answers: two entries from the
        // first batch and every entry of the second.
        let (available, _delayed, _in_flight) = message_counts(&router, &url).await;
        assert_eq!(
            available,
            i64::try_from(MAX_BATCH_ENTRIES + 2).unwrap_or_default(),
            "case MaximumMessageSize {maximum}: the queue must hold exactly the accepted \
             entries"
        );
    }
}

/// The REQUEST's payload is bounded by the service's own fixed ceiling, which is a
/// BOUNDARY: a batch whose entries come to exactly it is accepted, and one byte more is
/// `BatchRequestTooLong`. The queue here carries the default `MaximumMessageSize`, so
/// every entry is inside the per-message bound either way.
#[tokio::test]
async fn the_requests_payload_is_bounded_by_the_services_own_ceiling() {
    let ceiling = usize::try_from(maximum_batch_payload_bytes()).unwrap_or_default();
    // Eight equal entries, so the sum lands exactly on the ceiling and, in the second
    // row, exactly one byte past it.
    let width = 8usize;
    let share = ceiling / width;
    assert_eq!(
        share * width,
        ceiling,
        "case the ceiling: this row needs it to divide by {width}, got {ceiling}"
    );

    let entries = |extra: usize| -> Value {
        Value::Array(
            (0..width)
                .map(|position| {
                    let size = if position == 0 { share + extra } else { share };
                    json!({ "Id": format!("e{position}"), "MessageBody": "x".repeat(size) })
                })
                .collect(),
        )
    };

    let router = emulator();
    let url = create_ok(&router, "payload-ceiling", &json!({})).await;

    let answer = ok(
        &router,
        "SendMessageBatch",
        json!({ "QueueUrl": url, "Entries": entries(0) }),
    )
    .await;
    assert_eq!(
        successful_ids(&answer).len(),
        width,
        "case exactly at the ceiling: expected every entry accepted, got {}",
        failed(&answer).len()
    );

    let (status, body) = call(
        &router,
        "SendMessageBatch",
        &json!({ "QueueUrl": url, "Entries": entries(1) }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "BatchRequestTooLong"),
        "case one byte past the ceiling: expected the request refused whole, got {status}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(
            format!(
                "Batch requests cannot be longer than {ceiling} bytes. You have sent {} bytes.",
                ceiling + 1
            )
            .as_str()
        ),
        "case one byte past the ceiling: expected the refusal to state the ceiling and what \
         was sent, got {}",
        body["message"]
    );
}

/// A delete is idempotent: once a handle's message is gone, a further delete naming that
/// handle succeeds and changes nothing. Rows differ along how the repeat is presented —
/// twice in one batch, more than twice, across two calls, and in either direction between
/// `DeleteMessage` and `DeleteMessageBatch`.
#[tokio::test]
async fn a_delete_is_idempotent_for_a_handle_whose_message_is_gone() {
    // (case, how many entries of the batch name the one handle, whether an earlier call
    // deleted it first, whether that earlier call was the single DeleteMessage)
    let rows: &[(&str, usize, bool, bool)] = &[
        ("two entries naming one handle", 2, false, false),
        ("three entries naming one handle", 3, false, false),
        (
            "a full-width batch naming one handle",
            MAX_BATCH_ENTRIES,
            false,
            false,
        ),
        (
            "a handle a DeleteMessage already consumed",
            2,
            true,
            true,
        ),
        (
            "a handle an earlier batch already consumed",
            1,
            true,
            false,
        ),
    ];

    for (index, (case, repeats, deleted_first, by_single_delete)) in rows.iter().enumerate() {
        let router = emulator();
        let url = create_ok(&router, &format!("idempotent-{index}"), &json!({})).await;
        let handles = received_handles(&router, &url, 1).await;
        let handle = handles[0].clone();

        if *deleted_first {
            if *by_single_delete {
                let _ = ok(
                    &router,
                    "DeleteMessage",
                    json!({ "QueueUrl": url, "ReceiptHandle": handle }),
                )
                .await;
            } else {
                let answer = ok(
                    &router,
                    "DeleteMessageBatch",
                    json!({
                        "QueueUrl": url,
                        "Entries": [{ "Id": "first", "ReceiptHandle": handle }],
                    }),
                )
                .await;
                assert_eq!(
                    successful_ids(&answer),
                    vec!["first".to_string()],
                    "case {case}: the first delete must succeed, got {answer}"
                );
            }
        }

        let entries: Vec<Value> = (0..*repeats)
            .map(|position| json!({ "Id": format!("d{position}"), "ReceiptHandle": handle }))
            .collect();
        let answer = ok(
            &router,
            "DeleteMessageBatch",
            json!({ "QueueUrl": url, "Entries": entries }),
        )
        .await;

        let expected: Vec<String> = (0..*repeats).map(|position| format!("d{position}")).collect();
        assert_eq!(
            successful_ids(&answer),
            expected,
            "case {case}: every entry naming a handle whose message is gone must succeed, \
             got {answer}"
        );
        assert_eq!(
            failed(&answer).len(),
            0,
            "case {case}: expected no failed entry, got {answer}"
        );
        assert_eq!(
            message_counts(&router, &url).await,
            (0, 0, 0),
            "case {case}: the message must be deleted exactly once and stay deleted"
        );
        // Nothing comes back: the repeated delete neither resurrected the message nor left
        // it in flight.
        let handed = ok(
            &router,
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        assert!(
            messages(&handed).is_empty(),
            "case {case}: a deleted message must never redeliver, got {handed}"
        );
    }
}

/// A value that is not a receipt handle and a receipt handle another queue minted are two
/// different refusals: the first is the service's one fixed sentence, with nothing of the
/// request in it; the second names the handle, because what the caller has to fix is which
/// queue it sent it to. Asserted per entry and on the single `DeleteMessage`.
#[tokio::test]
async fn a_handle_the_queue_cannot_resolve_is_refused_by_where_it_came_from() {
    let router = emulator();
    let url = create_ok(&router, "handle-origin", &json!({})).await;
    let other = create_ok(&router, "handle-origin-other", &json!({})).await;
    let mine = received_handles(&router, &url, 1).await;
    let theirs = received_handles(&router, &other, 1).await;

    // (case, the handle, the message the refusal must carry)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a handle no queue ever minted",
            "AQEBsim2real-not-a-real-receipt-handle-0000000000".to_string(),
            "The input receipt handle is invalid.".to_string(),
        ),
        (
            "a well-formed identifier no queue minted",
            "00000000-0000-4000-8000-000000000000".to_string(),
            "The input receipt handle is invalid.".to_string(),
        ),
        (
            "a handle another queue minted",
            theirs[0].clone(),
            format!("The receipt handle \"{}\" is not valid for this queue.", theirs[0]),
        ),
    ];

    for (case, handle, message) in &rows {
        let answer = ok(
            &router,
            "DeleteMessageBatch",
            json!({
                "QueueUrl": url,
                "Entries": [
                    { "Id": "good", "ReceiptHandle": mine[0] },
                    { "Id": "bad", "ReceiptHandle": handle },
                ],
            }),
        )
        .await;
        assert_eq!(
            failure(&answer, "bad"),
            (
                RECEIPT_HANDLE_IS_INVALID.to_string(),
                true,
                message.clone(),
            ),
            "case {case}: expected the refusal worded for where the handle came from, got \
             {answer}"
        );

        let (status, body) = call(
            &router,
            "DeleteMessage",
            &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, RECEIPT_HANDLE_IS_INVALID),
            "case {case}: a single delete refuses it too, got {status} with {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(message.as_str()),
            "case {case}: expected the same wording as the batch entry, got {body}"
        );
    }

    // Every row named a distinct handle, so the table really spans three values rather
    // than repeating one.
    let distinct: BTreeSet<&str> = rows.iter().map(|(_, handle, _)| handle.as_str()).collect();
    assert_eq!(
        distinct.len(),
        rows.len(),
        "case the table itself: expected one handle per row, got {distinct:?}"
    );

    // The queue that minted the foreign handle still holds its message: a refusal on
    // another queue deleted nothing.
    let (_, _, in_flight) = message_counts(&router, &other).await;
    assert_eq!(
        in_flight, 1,
        "case the other queue: its message must still be in flight"
    );
}

/// How long a row waits for a message movement task to report itself running.
const RUNNING_DEADLINE: Duration = Duration::from_secs(20);

/// A second start on a source whose task is still running is refused as a PARAMETER
/// value, with the service's own sentence for a busy source arn, and the source keeps the
/// one task it has. Rows differ along the forms the second request takes.
#[tokio::test]
async fn a_second_task_on_a_busy_source_is_refused_as_a_parameter_value() {
    // (case, whether the second request names the destination, its rate)
    let rows: &[(&str, bool, Option<i64>)] = &[
        ("the same request again", false, Some(1)),
        ("a request naming the destination", true, Some(1)),
        ("a request leaving the rate to the service", false, None),
        ("a request at the documented maximum rate", false, Some(500)),
    ];

    for (index, (case, name_destination, rate)) in rows.iter().enumerate() {
        let router = emulator();
        let (dead_letter_url, dead_letter_arn, source_url) =
            dead_letter_queue(&router, &format!("busy-source-{index}")).await;
        // Enough messages at the slowest rate the member admits that the task is still
        // owed one for seconds after it starts, so it is RUNNING for the second call.
        let bodies: Vec<String> = (0..10).map(|number| format!("dlq #{number}")).collect();
        send_all(&router, &dead_letter_url, &bodies).await;

        let first = ok(
            &router,
            "StartMessageMoveTask",
            json!({ "SourceArn": dead_letter_arn, "MaxNumberOfMessagesPerSecond": 1 }),
        )
        .await;
        let handle = first["TaskHandle"].as_str().unwrap_or_default().to_string();
        assert!(
            !handle.is_empty(),
            "case {case}: expected the first start to answer a TaskHandle, got {first}"
        );
        assert_eq!(
            task_status(&router, &dead_letter_arn).await,
            "RUNNING",
            "case {case}: expected the first task to be running"
        );

        let mut request = json!({ "SourceArn": dead_letter_arn });
        if *name_destination {
            request["DestinationArn"] = json!(queue_arn(&router, &source_url).await);
        }
        if let Some(rate) = rate {
            request["MaxNumberOfMessagesPerSecond"] = json!(rate);
        }
        let (status, body) = call(&router, "StartMessageMoveTask", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValue"),
            "case {case}: expected the busy source refused as a parameter value, got \
             {status} with {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                "There is already a task running. Only one active task is allowed for a \
                 source queue arn at a given time."
            ),
            "case {case}: expected the service's sentence for a busy source, got {body}"
        );

        let listed = ok(
            &router,
            "ListMessageMoveTasks",
            json!({ "SourceArn": dead_letter_arn }),
        )
        .await;
        let results = listed["Results"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            1,
            "case {case}: the refused start must leave the one task, got {listed}"
        );
        assert_eq!(
            results[0]["TaskHandle"].as_str(),
            Some(handle.as_str()),
            "case {case}: expected the first task still listed, got {listed}"
        );
    }
}

/// The `QueueArn` the queue at `url` reports.
async fn queue_arn(router: &MultiServiceRouter, url: &str) -> String {
    all_attributes(router, url)
        .await
        .get("QueueArn")
        .cloned()
        .unwrap_or_else(|| panic!("case the arn of {url}: no QueueArn reported"))
}

/// The status `ListMessageMoveTasks` reports for the newest task of `source_arn`, waited
/// for until it is known.
async fn task_status(router: &MultiServiceRouter, source_arn: &str) -> String {
    let deadline = Instant::now() + RUNNING_DEADLINE;
    loop {
        let listed = ok(
            router,
            "ListMessageMoveTasks",
            json!({ "SourceArn": source_arn }),
        )
        .await;
        let status = listed["Results"][0]["Status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if !status.is_empty() || Instant::now() >= deadline {
            return status;
        }
    }
}

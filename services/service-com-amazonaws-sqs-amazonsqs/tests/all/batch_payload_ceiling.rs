// SPDX-License-Identifier: CC0-1.0

//! The service's PAYLOAD ceiling on a batch send, and the answer that states it — driven
//! over the wire (`POST /` with `X-Amz-Target: AmazonSQS.SendMessageBatch`) through the
//! shared harness in [`super::support`].
//!
//! The rule under test: "the length of all the messages put together is more than the
//! limit" is a refusal of the SERVICE, not of the transport. A batch whose entries are each
//! acceptable and together carry more than the maximum total payload is answered with the
//! modelled `BatchRequestTooLong` — HTTP 400, naming the limit and the number of bytes the
//! request sent — whatever shape it has, and nothing it carried is enqueued. A client
//! catching that error therefore matches, which a bare transport status carrying no error
//! document never lets it do.
//!
//! The table spans what the rule generalises over: how the over-limit payload is divided
//! (two large entries, four, ten small ones, and ten maximum-size ones — the largest batch
//! a client can spell), and the boundary itself, where a batch filling the ceiling exactly
//! is accepted and one byte more is refused.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    maximum_batch_payload_bytes, MAX_BATCH_ENTRIES,
};

use super::support::{attribute_count, call, create_ok, emulator, error_code, ok};

/// The error code the service answers an over-limit aggregate with, and the status it
/// carries: a client fault, not a transport failure.
const BATCH_REQUEST_TOO_LONG: (u16, &str) = (400, "BatchRequestTooLong");

/// The service's maximum total batch payload, in bytes.
fn ceiling() -> usize {
    usize::try_from(maximum_batch_payload_bytes()).unwrap_or(usize::MAX)
}

/// How a refusal of an over-limit aggregate is worded, as AWS words it: fixed text with
/// the limit and the bytes the request sent filled in.
fn too_long_message(total: usize) -> String {
    format!(
        "Batch requests cannot be longer than {} bytes. You have sent {total} bytes.",
        ceiling()
    )
}

/// `count` entries of `bytes` bytes each, with distinct ids.
fn entries(count: usize, bytes: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({ "Id": format!("e{index}"), "MessageBody": "x".repeat(bytes) })
            })
            .collect(),
    )
}

/// Two entries of `first` and `second` bytes, which is how a row reaches an exact total.
fn two_entries(first: usize, second: usize) -> Value {
    json!([
        { "Id": "e0", "MessageBody": "x".repeat(first) },
        { "Id": "e1", "MessageBody": "y".repeat(second) },
    ])
}

/// The refusal's `Message`, whichever case the envelope spells it in.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An over-limit batch of individually legal entries is answered with the modelled error,
/// however the payload is divided.
#[tokio::test]
async fn an_over_limit_aggregate_is_refused_as_a_modeled_batch_error() {
    // (case, how many entries, how many bytes each) — each entry is well inside the
    // queue's own `MaximumMessageSize`, which is the ceiling for ONE message, so what
    // every row breaks is the AGGREGATE.
    let rows: Vec<(String, usize, usize)> = vec![
        ("two entries of half the ceiling each".to_string(), 2, ceiling() / 2 + 1),
        ("four entries of a third each".to_string(), 4, ceiling() / 3 + 1),
        (
            format!("{MAX_BATCH_ENTRIES} entries one tenth of the ceiling each"),
            MAX_BATCH_ENTRIES,
            ceiling() / MAX_BATCH_ENTRIES + 1,
        ),
        (
            format!("{MAX_BATCH_ENTRIES} maximum-size entries, the largest batch there is"),
            MAX_BATCH_ENTRIES,
            ceiling(),
        ),
    ];

    for (case, count, bytes) in rows {
        let total = count * bytes;
        assert!(
            total > ceiling(),
            "case {case}: the row must be over the ceiling, got {total} bytes"
        );
        let router = emulator();
        let url = create_ok(&router, "aggregate-ceiling", &json!({})).await;
        let (status, body) = call(
            &router,
            "SendMessageBatch",
            &json!({ "QueueUrl": url, "Entries": entries(count, bytes) }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            BATCH_REQUEST_TOO_LONG,
            "case {case}: expected the modeled aggregate refusal, got {status} with {body}"
        );
        let expected = too_long_message(total);
        assert_eq!(
            message_of(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );
        assert_eq!(
            attribute_count(&router, &url, "ApproximateNumberOfMessages").await,
            0,
            "case {case}: a refused batch must enqueue nothing"
        );
    }
}

/// The ceiling is a BOUNDARY: a batch whose bodies fill it exactly is accepted and
/// enqueues every entry, and one byte more is refused.
#[tokio::test]
async fn a_batch_at_the_ceiling_is_accepted_and_one_byte_more_is_not() {
    // Two entries, so the sum is what the boundary is reached with rather than one body.
    let half = ceiling() / 2;
    // (case, the two entries' bytes, accepted)
    let rows: [(&str, usize, usize, bool); 3] = [
        ("one byte under the ceiling", half, half - 1, true),
        ("exactly at the ceiling", half, half, true),
        ("one byte past the ceiling", half, half + 1, false),
    ];

    for (case, first, second, accepted) in rows {
        let router = emulator();
        let url = create_ok(&router, "aggregate-boundary", &json!({})).await;
        let (status, body) = call(
            &router,
            "SendMessageBatch",
            &json!({ "QueueUrl": url, "Entries": two_entries(first, second) }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected the batch to be accepted, got {status} with {body}"
            );
            assert_eq!(
                attribute_count(&router, &url, "ApproximateNumberOfMessages").await,
                2,
                "case {case}: both entries must be enqueued"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            BATCH_REQUEST_TOO_LONG,
            "case {case}: expected the modeled aggregate refusal, got {status} with {body}"
        );
        assert_eq!(
            message_of(&body),
            too_long_message(first + second),
            "case {case}: expected the limit and the bytes sent, got {body}"
        );
        assert_eq!(
            attribute_count(&router, &url, "ApproximateNumberOfMessages").await,
            0,
            "case {case}: a refused batch must enqueue nothing"
        );
    }
}

/// The refusal reaches a client as a SERVICE error whatever the aggregate's size: the
/// answer carries this service's error envelope — a code and a message — rather than a
/// bare transport status with no body, which is what a client catching the modelled error
/// has to see.
#[tokio::test]
async fn the_refusal_carries_the_services_own_error_document() {
    let router = emulator();
    let url = create_ok(&router, "aggregate-envelope", &json!({})).await;
    // Well past the ceiling and still a batch the service judges: four entries, each
    // inside the per-message bound.
    let (status, body) = call(
        &router,
        "SendMessageBatch",
        &json!({ "QueueUrl": url, "Entries": entries(4, ceiling() / 3 + 1) }),
    )
    .await;
    assert_eq!(
        status, BATCH_REQUEST_TOO_LONG.0,
        "case the envelope: expected the service's own status, got {status} with {body}"
    );
    assert!(
        body.is_object() && !message_of(&body).is_empty(),
        "case the envelope: expected a parsable error document naming the fault, got {body}"
    );
    // The queue is untouched and still usable, which is what says the request was READ and
    // refused rather than dropped.
    let accepted = ok(
        &router,
        "SendMessage",
        json!({ "QueueUrl": url, "MessageBody": "after the refusal" }),
    )
    .await;
    assert!(
        accepted.get("MessageId").and_then(Value::as_str).is_some(),
        "case after the refusal: the queue must still take a send, got {accepted}"
    );
}

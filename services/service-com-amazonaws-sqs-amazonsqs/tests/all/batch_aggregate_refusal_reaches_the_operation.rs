// SPDX-License-Identifier: CC0-1.0

//! Who refuses a batch whose entries are each acceptable and together carry too much —
//! driven over the wire (`POST /` with `X-Amz-Target: AmazonSQS.SendMessageBatch`) through
//! the shared harness in [`super::support`].
//!
//! The rule under test: an over-aggregate batch is a refusal of the SERVICE, so however
//! far past the maximum total payload the entries add up — and however the payload is
//! split between them — the answer is the modelled `BatchRequestTooLong`: HTTP 400, an
//! error document naming the limit and the bytes the request sent, and nothing enqueued.
//! A bare transport status carries no error code and no numbers, so a client that catches
//! the modelled error, reads the limit and splits its batch never matches one. The
//! transport's own body ceiling therefore has to leave headroom ABOVE the payload limit:
//! an aggregate that breaks that limit is payload a client spells with individually legal
//! entries, and it is read and measured rather than dropped on the wire
//! (`request_size_transport_bound.rs` pins the far side, where a body carries more than
//! any answer of this service could state).
//!
//! The rows span what the rule generalises over: how far over the limit the aggregate
//! reaches (a couple of bytes, a quarter more, a third more, half again) and how it is
//! divided (two entries, three, four, ten, and one maximum-size message beside a small
//! one). The recorded case — four entries of a third of the limit each — is one row.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::service::max_request_body_bytes;
use service_com_amazonaws_sqs_amazonsqs::state::maximum_batch_payload_bytes;

use super::support::{attribute_count, call, create_ok, emulator, error_code, messages, ok};

/// The status and code an over-aggregate batch is refused with: a client fault in the
/// service's own envelope, not a transport failure.
const BATCH_REQUEST_TOO_LONG: (u16, &str) = (400, "BatchRequestTooLong");

/// The transport status a request the service never read would carry, which no row here
/// may see.
const REQUEST_ENTITY_TOO_LARGE: u16 = 413;

/// The service's maximum total batch payload, in bytes.
fn payload_limit() -> usize {
    usize::try_from(maximum_batch_payload_bytes()).unwrap_or(usize::MAX)
}

/// How AWS words the refusal: fixed text with the limit and the bytes the request sent
/// filled in.
fn too_long_message(total: usize) -> String {
    format!(
        "Batch requests cannot be longer than {} bytes. You have sent {total} bytes.",
        payload_limit()
    )
}

/// `sizes` entries with distinct ids, the n-th carrying a body of `sizes[n]` bytes.
fn entries(sizes: &[usize]) -> Value {
    Value::Array(
        sizes
            .iter()
            .enumerate()
            .map(|(index, bytes)| {
                json!({ "Id": format!("e{index}"), "MessageBody": "x".repeat(*bytes) })
            })
            .collect(),
    )
}

/// The refusal's `Message`, whichever case the envelope spells it in.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The sizes of a batch of `count` entries each carrying `bytes` bytes.
fn even_split(count: usize, bytes: usize) -> Vec<usize> {
    vec![bytes; count]
}

/// Every over-aggregate batch is answered by the OPERATION, with the limit and the
/// measured total, and enqueues nothing — whatever multiple of the limit it reaches and
/// however it is divided.
#[tokio::test]
async fn an_over_aggregate_batch_is_refused_by_the_operation_however_far_over_it_reaches() {
    let limit = payload_limit();
    // (case, the entry sizes) — every entry is at or inside the queue's own
    // `MaximumMessageSize` (the default, which is the documented maximum message size), so
    // what each row breaks is the AGGREGATE and nothing else.
    let rows: Vec<(String, Vec<usize>)> = vec![
        (
            "two entries, two bytes over the limit".to_string(),
            even_split(2, limit / 2 + 1),
        ),
        (
            "one maximum-size entry beside a small one".to_string(),
            vec![limit, 1_024],
        ),
        (
            "ten entries, a quarter more than the limit".to_string(),
            even_split(10, limit / 8),
        ),
        (
            "four entries, a third more than the limit".to_string(),
            even_split(4, limit / 3 + 1),
        ),
        (
            "three entries, half again the limit".to_string(),
            even_split(3, limit / 2),
        ),
    ];

    for (case, sizes) in rows {
        let total: usize = sizes.iter().sum();
        assert!(
            total > limit,
            "case {case:?}: the row must break the aggregate bound, got {total} bytes \
             against {limit}"
        );
        assert!(
            sizes.iter().all(|bytes| *bytes <= limit),
            "case {case:?}: every entry must be individually legal, got {sizes:?} against \
             {limit}"
        );

        let router = emulator();
        let url = create_ok(&router, &format!("aggregate-{total}"), &json!({})).await;
        let request = json!({ "QueueUrl": url, "Entries": entries(&sizes) });
        let body_bytes = serde_json::to_vec(&request)
            .expect("the request body serializes")
            .len();
        assert!(
            body_bytes <= max_request_body_bytes(),
            "case {case:?}: a client spells this batch, so the transport must read it, got \
             {body_bytes} bytes against a ceiling of {}",
            max_request_body_bytes()
        );

        let (status, answer) = call(&router, "SendMessageBatch", &request).await;
        assert_ne!(
            status, REQUEST_ENTITY_TOO_LARGE,
            "case {case:?}: the aggregate is the operation's to refuse, got the transport \
             status with {answer}"
        );
        assert_eq!(
            (status, error_code(&answer).as_str()),
            BATCH_REQUEST_TOO_LONG,
            "case {case:?}: expected the modelled aggregate refusal, got {status} with \
             {answer}"
        );
        let expected = too_long_message(total);
        assert_eq!(
            message_of(&answer),
            expected,
            "case {case:?}: expected {expected:?}, got {answer}"
        );
        assert!(
            answer.get("Successful").is_none() && answer.get("Failed").is_none(),
            "case {case:?}: a request refused whole reports no entry outcomes, got {answer}"
        );

        // All or nothing: not one entry of the batch reached the queue.
        assert_eq!(
            attribute_count(&router, &url, "ApproximateNumberOfMessages").await,
            0,
            "case {case:?}: a refused batch must enqueue nothing"
        );
        let delivered = ok(
            &router,
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        assert!(
            messages(&delivered).is_empty(),
            "case {case:?}: a refused batch must deliver nothing, got {delivered}"
        );
    }
}

/// The recovery the refusal exists to trigger: the SAME entries, split so that each
/// request stays inside the aggregate bound, are accepted and every one of them is
/// delivered back. Rows differ in how the over-limit batch is cut up.
#[tokio::test]
async fn the_same_entries_split_under_the_limit_are_accepted_and_delivered() {
    let limit = payload_limit();
    // (case, the entry sizes of the over-limit batch, how many entries per split request)
    let rows: Vec<(String, Vec<usize>, usize)> = vec![
        (
            "four thirds of the limit, sent two entries at a time".to_string(),
            even_split(4, limit / 3 + 1),
            2,
        ),
        (
            "a quarter more than the limit, sent five entries at a time".to_string(),
            even_split(10, limit / 8),
            5,
        ),
        (
            "two entries over the limit, sent one at a time".to_string(),
            even_split(2, limit / 2 + 1),
            1,
        ),
    ];

    for (case, sizes, per_request) in rows {
        let router = emulator();
        let url = create_ok(&router, &format!("aggregate-split-{}", sizes.len()), &json!({})).await;

        // Whole, the batch is refused.
        let (status, answer) = call(
            &router,
            "SendMessageBatch",
            &json!({ "QueueUrl": url, "Entries": entries(&sizes) }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            BATCH_REQUEST_TOO_LONG,
            "case {case:?}: expected the modelled aggregate refusal, got {status} with \
             {answer}"
        );

        // Split, every entry is taken: each request's own aggregate is inside the bound.
        let mut sent = 0usize;
        for chunk in sizes.chunks(per_request) {
            let chunk_total: usize = chunk.iter().sum();
            assert!(
                chunk_total <= limit,
                "case {case:?}: a split request must stay inside the bound, got \
                 {chunk_total} bytes against {limit}"
            );
            let accepted = ok(
                &router,
                "SendMessageBatch",
                json!({ "QueueUrl": url, "Entries": entries(chunk) }),
            )
            .await;
            let successful = accepted
                .get("Successful")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                successful,
                chunk.len(),
                "case {case:?}: expected every entry of the split request accepted, got \
                 {accepted}"
            );
            sent += chunk.len();
        }

        // Read the queue back: it holds exactly the entries the split requests sent.
        assert_eq!(
            attribute_count(&router, &url, "ApproximateNumberOfMessages").await,
            i64::try_from(sent).unwrap_or(i64::MAX),
            "case {case:?}: the queue must hold every entry the split requests sent"
        );
        let delivered = ok(
            &router,
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        assert_eq!(
            messages(&delivered).len(),
            sent,
            "case {case:?}: expected every split entry delivered, got {} of {sent}",
            messages(&delivered).len()
        );
    }
}

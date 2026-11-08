// SPDX-License-Identifier: CC0-1.0

//! The transport bound on a request body, and the refusal that states it.
//!
//! The rule under test: a request whose HTTP body is larger than the ceiling this
//! service's frontend reads at all is refused by the TRANSPORT — a bare HTTP 413 with no
//! service error document — whatever operation it names, before the body is deserialized
//! and before any handler looks at a field; a body the ceiling admits is read and answered
//! in the service's own envelope, so a field-level refusal and a transport refusal stay
//! two distinguishable answers.
//!
//! The ceiling itself is [`max_request_body_bytes`]: the maximum total payload any Amazon
//! SQS request may legally carry — the same figure for a single message and for a whole
//! batch — plus headroom for an aggregate that breaks that payload bound and plus room for
//! the request wrapped around it. A batch whose entries add up to more than the payload
//! limit but whose body the ceiling still admits is read and refused by the OPERATION with
//! the modelled `BatchRequestTooLong` (`batch_payload_ceiling.rs`,
//! `batch_aggregate_refusal_reaches_the_operation.rs`); a body past the ceiling never gets
//! that far.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::service::max_request_body_bytes;

// These rows need the request's byte length controlled exactly, which means posting the
// body the suite built rather than handing a `Value` over to be serialized: that is
// `post_bytes`, the shared harness's raw request.
use crate::support::{
    attribute_count, create_ok, emulator, messages, ok, post_bytes, REQUEST_ENTITY_TOO_LARGE,
};

/// A `SendMessage` request body of exactly `total` bytes: the queue url and the JSON
/// framing, with `MessageBody` padded out to fill whatever is left.
fn send_message_body_of(url: &str, total: usize) -> Vec<u8> {
    let prefix = format!(r#"{{"QueueUrl":"{url}","MessageBody":""#);
    let suffix = r#""}"#;
    let framing = prefix.len() + suffix.len();
    assert!(
        total >= framing,
        "case {total} bytes: the row is shorter than the {framing} bytes of framing it needs"
    );
    let built = format!("{prefix}{}{suffix}", "x".repeat(total - framing)).into_bytes();
    assert_eq!(
        built.len(),
        total,
        "case {total} bytes: the built body must be exactly that long"
    );
    built
}

/// A body above the ceiling is refused by the transport and a body at or under it is read
/// and answered by the service. The rows walk the boundary from either side and then far
/// past it, so what separates the two answers is the SIZE and nothing else.
#[tokio::test]
async fn a_request_body_above_the_transport_ceiling_is_refused_with_a_bare_413() {
    let ceiling = max_request_body_bytes();
    // `(case, body length, refused by the transport)`
    let rows: [(&str, usize, bool); 5] = [
        ("well under the ceiling", 1_024, false),
        ("one byte under the ceiling", ceiling - 1, false),
        ("exactly at the ceiling", ceiling, false),
        ("one byte past the ceiling", ceiling + 1, true),
        ("far past the ceiling", 2 * ceiling, true),
    ];

    let router = emulator();
    let url = create_ok(&router, "transport-ceiling", &json!({})).await;
    // How many rows the SERVICE accepted, which is exactly what the queue must end up
    // holding: a row the transport refused stored nothing.
    let mut accepted = 0_i64;
    for (case, length, refused) in rows {
        let (status, body) =
            post_bytes(&router, "SendMessage", send_message_body_of(&url, length)).await;
        if refused {
            assert_eq!(
                status, REQUEST_ENTITY_TOO_LARGE,
                "case {case:?} ({length} bytes): expected the transport status, got {status} \
                 with {:?}",
                String::from_utf8_lossy(&body)
            );
            // No service error document at all: a client must see the bare transport
            // status rather than an error code it would classify as the service's own.
            assert!(
                body.is_empty(),
                "case {case:?} ({length} bytes): expected an empty body, got {:?}",
                String::from_utf8_lossy(&body)
            );
        } else {
            assert_ne!(
                status, REQUEST_ENTITY_TOO_LARGE,
                "case {case:?} ({length} bytes): the ceiling admits this body, so the \
                 transport must not refuse it"
            );
            // The service read it: the answer is this service's own envelope — a result,
            // or an error document naming one of its codes.
            let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            assert!(
                parsed.is_object(),
                "case {case:?} ({length} bytes): expected the service's own envelope, got \
                 {status} with {:?}",
                String::from_utf8_lossy(&body)
            );
            if status == 200 {
                accepted += 1;
            } else {
                assert!(
                    parsed.get("__type").and_then(Value::as_str).is_some(),
                    "case {case:?} ({length} bytes): a refusal the service made must name its \
                     code, got {status} with {parsed}"
                );
            }
        }
    }

    // Nothing an over-sized request carried reached the queue: the refused rows named a
    // real queue and a real operation, and the queue holds exactly the rows the service
    // itself accepted.
    assert_eq!(
        attribute_count(&router, &url, "ApproximateNumberOfMessages").await,
        accepted,
        "case nothing enqueued: an over-sized request must store no message"
    );
    let delivered = ok(
        &router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        i64::try_from(messages(&delivered).len()).unwrap_or(i64::MAX),
        accepted,
        "case nothing delivered: only the rows the service accepted may be delivered, got \
         {delivered}"
    );
}

/// The ceiling is the TRANSPORT's, not one operation's: the same over-sized body is
/// refused the same way whichever operation it names, including the ones that only read.
/// Each row pads a different string member of a different operation, so no row depends on
/// the shape of another's request.
#[tokio::test]
async fn the_transport_ceiling_refuses_every_operation_alike() {
    let padding = "x".repeat(max_request_body_bytes() + 1);
    // `(operation, request)` — one row per operation, each padded past the ceiling
    // through a member of its own.
    let rows: Vec<(&str, Value)> = vec![
        ("CreateQueue", json!({ "QueueName": padding })),
        (
            "SendMessage",
            json!({ "QueueUrl": "https://example", "MessageBody": padding }),
        ),
        (
            "SendMessageBatch",
            json!({
                "QueueUrl": "https://example",
                "Entries": [{ "Id": "e0", "MessageBody": padding }],
            }),
        ),
        (
            "DeleteMessage",
            json!({ "QueueUrl": "https://example", "ReceiptHandle": padding }),
        ),
        (
            "DeleteMessageBatch",
            json!({
                "QueueUrl": "https://example",
                "Entries": [{ "Id": "d0", "ReceiptHandle": padding }],
            }),
        ),
        (
            "ChangeMessageVisibilityBatch",
            json!({
                "QueueUrl": "https://example",
                "Entries": [{ "Id": "c0", "ReceiptHandle": padding, "VisibilityTimeout": 1 }],
            }),
        ),
        (
            "ReceiveMessage",
            json!({ "QueueUrl": "https://example", "ReceiveRequestAttemptId": padding }),
        ),
        (
            "GetQueueUrl",
            json!({ "QueueName": padding }),
        ),
    ];

    let router = emulator();
    for (operation, request) in rows {
        let body = serde_json::to_vec(&request).expect("the request body serializes");
        assert!(
            body.len() > max_request_body_bytes(),
            "case {operation}: this row must be past the ceiling, got {} bytes",
            body.len()
        );
        let (status, answer) = post_bytes(&router, operation, body).await;
        assert_eq!(
            status, REQUEST_ENTITY_TOO_LARGE,
            "case {operation}: expected the transport status, got {status} with {:?}",
            String::from_utf8_lossy(&answer)
        );
        assert!(
            answer.is_empty(),
            "case {operation}: expected an empty body, got {:?}",
            String::from_utf8_lossy(&answer)
        );
    }
}

/// The two refusals stay apart at their own boundary: a batch whose entries add up to
/// just over the service's payload limit is READ and refused by the operation, while one
/// so large that its body is past the transport ceiling is the bare 413 — and neither
/// enqueues anything.
#[tokio::test]
async fn a_batch_past_the_transport_ceiling_is_the_bare_413_and_not_the_batch_error() {
    let payload_limit =
        usize::try_from(service_com_amazonaws_sqs_amazonsqs::state::maximum_batch_payload_bytes())
            .unwrap_or(usize::MAX);
    let entries = |count: usize, bytes: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({ "Id": format!("e{index}"), "MessageBody": "x".repeat(bytes) }))
                .collect(),
        )
    };

    // (case, the batch, refused by the transport) — the rows differ in how far past the
    // payload limit the request reaches: just past it, and the largest batch a client can
    // spell, whose bodies are ten maximum-size messages.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "two entries adding up to just past the payload limit",
            entries(2, payload_limit / 2 + 1),
            false,
        ),
        (
            "ten maximum-size entries, far past the body this service reads",
            entries(10, payload_limit),
            true,
        ),
    ];

    let router = emulator();
    for (case, batch, refused_by_transport) in rows {
        let url = create_ok(&router, &format!("batch-vs-transport-{}", case.len()), &json!({})).await;
        let body = serde_json::to_vec(&json!({ "QueueUrl": url, "Entries": batch }))
            .expect("the request body serializes");
        let (status, answer) = post_bytes(&router, "SendMessageBatch", body).await;
        if refused_by_transport {
            assert_eq!(
                status, REQUEST_ENTITY_TOO_LARGE,
                "case {case:?}: expected the transport status, got {status} with {:?}",
                String::from_utf8_lossy(&answer)
            );
            assert!(
                answer.is_empty(),
                "case {case:?}: expected an empty body, got {:?}",
                String::from_utf8_lossy(&answer)
            );
        } else {
            let parsed: Value = serde_json::from_slice(&answer).unwrap_or(Value::Null);
            assert_eq!(
                (status, parsed.get("__type").and_then(Value::as_str)),
                // The class named in full, as this `awsQueryCompatible` service names one:
                // the shape id its own model declares for `BatchRequestTooLong`.
                (400, Some("com.amazonaws.sqs#BatchRequestTooLong")),
                "case {case:?}: expected the operation's own refusal, got {status} with {parsed}"
            );
        }
        assert_eq!(
            attribute_count(&router, &url, "ApproximateNumberOfMessages").await,
            0,
            "case {case:?}: a refused batch must enqueue nothing"
        );
    }
}

/// The positive side of the same bound: a body the ceiling admits is accepted and comes
/// back byte for byte, over sizes spanning from one character to the largest message the
/// default `MaximumMessageSize` carries.
#[tokio::test]
async fn a_message_the_ceiling_admits_round_trips_unchanged() {
    // One character (the documented minimum), a small body, 256 KiB, and 1 MiB — the
    // documented maximum message size, which the default `MaximumMessageSize` is.
    let sizes: [usize; 4] = [1, 1_024, 262_144, 1_048_576];

    let router = emulator();
    for size in sizes {
        let url = create_ok(&router, &format!("round-trip-{size}"), &json!({})).await;
        let sent = "x".repeat(size);
        let reply = ok(
            &router,
            "SendMessage",
            json!({ "QueueUrl": url, "MessageBody": sent }),
        )
        .await;
        assert!(
            reply.get("MessageId").and_then(Value::as_str).is_some(),
            "case {size} bytes: expected an accepted send, got {reply}"
        );
        let delivered = ok(
            &router,
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MaxNumberOfMessages": 1 }),
        )
        .await;
        let held = messages(&delivered);
        assert_eq!(
            held.len(),
            1,
            "case {size} bytes: expected the message delivered, got {delivered}"
        );
        assert_eq!(
            held[0]["Body"].as_str(),
            Some(sent.as_str()),
            "case {size} bytes: expected the body back unchanged, got {} bytes",
            held[0]["Body"].as_str().unwrap_or_default().len()
        );
    }
}

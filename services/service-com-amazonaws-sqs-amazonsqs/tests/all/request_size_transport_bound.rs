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
//! The ceiling itself is [`max_request_body_bytes`]: the largest payload any Amazon SQS
//! request can carry — the largest batch a batch operation can be asked to judge, which is
//! ten maximum-size messages, since a batch over the service's payload ceiling is refused
//! by the OPERATION with the modelled `BatchRequestTooLong` and not by the transport —
//! plus room for the request wrapped around it.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::service::max_request_body_bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{self, attribute_count, create_ok, emulator, messages, ok};

/// The status a transport refusal of an over-sized body carries.
const REQUEST_ENTITY_TOO_LARGE: u16 = 413;

/// Post `body` verbatim as `operation`, bypassing the shared helper's JSON
/// serialization: these rows need the request's byte length controlled exactly, which
/// means building the body rather than handing over a `Value`.
///
/// Answers the status and the raw response body.
async fn post(router: &MultiServiceRouter, operation: &str, body: Vec<u8>) -> (u16, Vec<u8>) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", support::HOST)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", support::authorization_in(support::REGION))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.body().to_vec())
}

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
        let (status, body) = post(&router, "SendMessage", send_message_body_of(&url, length)).await;
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
        let (status, answer) = post(&router, operation, body).await;
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

// SPDX-License-Identifier: CC0-1.0

//! Which of the two size refusals a `SendMessage` gets, and what decides it.
//!
//! The rule under test: a request whose HTTP body is larger than the ceiling this service
//! reads at all ([`max_request_body_bytes`]) is refused by the TRANSPORT — a bare HTTP 413
//! with no error document — and that decision is made on the request's SIZE alone, before
//! the body is deserialized and before any handler runs, so it holds whatever queue the
//! request names, whatever `MaximumMessageSize` that queue carries, and even when no such
//! queue exists. A body the ceiling admits is READ, and only then is the message it
//! carries weighed against the addressed queue's own `MaximumMessageSize` — this
//! service's own refusal, HTTP 400 in its own envelope with the code and the sentence it
//! words from that queue's limit. The two therefore never stand in for one another: a
//! client that keys on 413 to shrink, split or offload a payload sees 413 only for a
//! request that never fit on the wire, and never a parameter code for one.
//!
//! What this suite owns is the CROSSING of the two bounds, which neither of the suites on
//! either side of it can state: `request_size_transport_bound.rs` walks the transport
//! ceiling from both sides on queues carrying the default maximum, and
//! `message_lifecycle.rs` / `refusal_taxonomy.rs` pin the queue's own maximum and the
//! sentence its refusal carries. Here one request — a message twice the addressed queue's
//! maximum — is posted against queue limits spanning what the attribute publishes, so the
//! SAME over-large message earns the service's refusal on the queues whose limit leaves
//! the request inside the ceiling and the transport's on the one it does not, and the
//! ordering the 413 asserts (nothing read, so not even the queue looked up) is checked
//! where a handler would have had something to say.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::service::max_request_body_bytes;
use service_com_amazonaws_sqs_amazonsqs::state::{
    integer_bounds, maximum_batch_payload_bytes, MAXIMUM_MESSAGE_SIZE,
};

use crate::support::{
    attribute_count, call, create_ok, emulator, error_code, post_bytes, REQUEST_ENTITY_TOO_LARGE,
};

/// A `SendMessage` request for the queue at `url` whose `MessageBody` is `body_bytes`
/// characters long, as the bytes it travels as.
fn send_of(url: &str, body_bytes: usize) -> Vec<u8> {
    serde_json::to_vec(&json!({ "QueueUrl": url, "MessageBody": "x".repeat(body_bytes) }))
        .expect("the request body serializes")
}

/// How this service states that a message is longer than the addressed queue will carry:
/// the whole-request sentence, whose reason names that queue's own limit.
fn too_long_message(maximum: i64) -> String {
    format!(
        "One or more parameters are invalid. Reason: Message must be shorter than {maximum} bytes."
    )
}

/// One over-large message, across queues whose `MaximumMessageSize` spans what the
/// attribute publishes: the refusal it earns is the transport's bare 413 exactly where
/// the request is past the ceiling, and this service's own 400 wherever the ceiling
/// admits the request and the handler weighs the message against that queue's limit.
/// Either way the queue holds nothing afterwards.
#[tokio::test]
async fn an_over_large_message_is_refused_by_whichever_bound_the_request_size_reaches() {
    // The bounds `MaximumMessageSize` publishes, read from the one place this crate keeps
    // them rather than written out again; the middling row is a limit between them that
    // no row's expectation is keyed to.
    let (smallest, largest) = integer_bounds(MAXIMUM_MESSAGE_SIZE);
    let middling = 262_144_i64;
    assert!(
        smallest < middling && middling < largest,
        "case setup: the middling row must sit inside the published bounds \
         ({smallest}..={largest})"
    );
    // What the ceiling must BE, independently of the function the gate reads it from: the
    // most message content any request of this service can carry, plus room for the
    // request wrapped around it. Above the payload limit, so a batch whose entries add up
    // to more than it is still READ and refused by the operation; below twice it, so no
    // request carrying more than one maximum payload can reach a handler at all — which
    // is what makes the largest queue's row below the transport's and the others' this
    // service's own.
    let ceiling = max_request_body_bytes();
    let payload = usize::try_from(maximum_batch_payload_bytes()).expect("a limit fits a usize");
    assert!(
        payload < ceiling && ceiling < 2 * payload,
        "case setup: the {ceiling}-byte ceiling must be the {payload}-byte payload limit plus \
         framing room"
    );

    let router = emulator();
    for maximum in [smallest, middling, largest] {
        let limit = usize::try_from(maximum).expect("a published maximum fits a usize");
        let url = create_ok(
            &router,
            &format!("size-refusal-{maximum}"),
            &json!({ "MaximumMessageSize": maximum.to_string() }),
        )
        .await;

        // Twice what this queue will carry, so the message is over-large on every row;
        // which bound refuses it is the REQUEST's length against the ceiling.
        let request = send_of(&url, limit * 2);
        let case = format!(
            "case a {}-byte request carrying twice the {maximum}-byte maximum of the queue it \
             names",
            request.len()
        );
        let past_the_ceiling = request.len() > ceiling;
        let (status, answer) = post_bytes(&router, "SendMessage", request).await;
        if past_the_ceiling {
            assert_eq!(
                status, REQUEST_ENTITY_TOO_LARGE,
                "{case}: expected the transport status, got {status} with {:?}",
                String::from_utf8_lossy(&answer)
            );
            // No error document at all: a client must see the bare transport status, not
            // a code it would read as one of this service's own.
            assert!(
                answer.is_empty(),
                "{case}: expected an empty body, got {:?}",
                String::from_utf8_lossy(&answer)
            );
        } else {
            let parsed: Value = serde_json::from_slice(&answer).unwrap_or(Value::Null);
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidParameterValue"),
                "{case}: the ceiling admits this request, so this service must read it and \
                 refuse the message itself, got {status} with {parsed}"
            );
            assert_eq!(
                parsed["message"].as_str(),
                Some(too_long_message(maximum).as_str()),
                "{case}: expected the sentence naming the queue's own limit, got {parsed}"
            );
        }

        // Neither refusal enqueued anything, and the queue it named is still servable.
        assert_eq!(
            attribute_count(&router, &url, "ApproximateNumberOfMessages").await,
            0,
            "{case}: a refused send must store no message"
        );
    }
}

/// The transport refuses before anything of the request is read, so the refusal does not
/// depend on the request naming a queue that exists — while the same request under the
/// ceiling reaches the handler and is answered by this service for the queue it could not
/// find. That ordering is the whole point of the 413: it says the request never arrived,
/// not that the service disliked something in it.
#[tokio::test]
async fn a_request_past_the_ceiling_is_refused_before_the_queue_is_looked_up() {
    let router = emulator();
    // A url of this service's own form that names no queue anybody created.
    let absent = format!(
        "{}-absent",
        create_ok(&router, "size-refusal-ordering", &json!({})).await
    );
    // Past the ceiling however the ceiling is derived, which the row asserts rather than
    // assumes.
    let oversized = send_of(&absent, 2 * max_request_body_bytes());

    let (status, answer) = post_bytes(&router, "SendMessage", oversized).await;
    assert_eq!(
        status, REQUEST_ENTITY_TOO_LARGE,
        "case an absent queue past the ceiling: expected the transport status, got {status} \
         with {:?}",
        String::from_utf8_lossy(&answer)
    );
    assert!(
        answer.is_empty(),
        "case an absent queue past the ceiling: expected an empty body, got {:?}",
        String::from_utf8_lossy(&answer)
    );

    // The same request, with a body the ceiling admits: now the handler runs, and answers
    // for the queue rather than for the size.
    let (status, parsed) = call(
        &router,
        "SendMessage",
        &json!({ "QueueUrl": absent, "MessageBody": "x" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "QueueDoesNotExist"),
        "case an absent queue under the ceiling: expected the service's own refusal, got \
         {status} with {parsed}"
    );
}

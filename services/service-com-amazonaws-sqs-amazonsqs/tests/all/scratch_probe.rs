// SPDX-License-Identifier: CC0-1.0

//! A probe of the message plane after a batch delete, once the CLOCK has had its say,
//! driven over the wire through the shared harness in [`super::support`].
//!
//! The rule asserted here: deleting a message is permanent and idempotent, so when the
//! visibility timeout the messages were received under lapses, the queue hands back
//! exactly the messages no entry of the batch deleted — a message a delete consumed never
//! comes back, however many entries of the batch named its receipt handle.
//!
//! The span is the one the receive ASKED for (`ReceiveMessageRequest$VisibilityTimeout`,
//! "an integer from 0 to 43,200"), so the rule is observed over a real lapse rather than
//! over the queue's default.

use std::time::Instant;

use serde_json::{json, Value};

use super::support::{create_ok, emulator, message_counts, messages, ok};

/// The visibility timeout the receive asks for, in seconds: long enough that every
/// assertion before the lapse runs while the messages are still in flight, short enough
/// to wait out.
const VISIBILITY_TIMEOUT: i64 = 2;

/// How many one-second polls a row gives the queue to hand the survivor back once its
/// visibility timeout has lapsed.
const POLL_ROUNDS: usize = 10;

#[tokio::test]
async fn a_deleted_message_never_comes_back_when_the_visibility_timeout_lapses() {
    let router = emulator();
    let url = create_ok(&router, "batch-delete-clock", &json!({})).await;

    let bodies = ["m0", "m1", "m2"];
    for body in bodies {
        let _ = ok(
            &router,
            "SendMessage",
            json!({ "QueueUrl": url, "MessageBody": body }),
        )
        .await;
    }
    let handed = ok(
        &router,
        "ReceiveMessage",
        json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": bodies.len(),
            "VisibilityTimeout": VISIBILITY_TIMEOUT,
        }),
    )
    .await;
    let received = messages(&handed);
    assert_eq!(
        received.len(),
        bodies.len(),
        "case the setup receive: expected every message in flight, got {handed}"
    );
    let handle = |index: usize| -> String {
        received[index]["ReceiptHandle"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let body_of = |index: usize| -> String {
        received[index]["Body"].as_str().unwrap_or_default().to_string()
    };

    // The first message's handle is named twice, so the row asserts the idempotent repeat
    // and the permanence of the delete together.
    let entries: Vec<Value> = vec![
        json!({ "Id": "once", "ReceiptHandle": handle(0) }),
        json!({ "Id": "again", "ReceiptHandle": handle(0) }),
        json!({ "Id": "other", "ReceiptHandle": handle(1) }),
    ];
    let answer = ok(
        &router,
        "DeleteMessageBatch",
        json!({ "QueueUrl": url, "Entries": entries }),
    )
    .await;
    let deleted: Vec<&str> = answer["Successful"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["Id"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        deleted,
        vec!["once", "again", "other"],
        "case the batch delete: every entry naming a handle of this queue must succeed, \
         got {answer}"
    );
    assert_eq!(
        message_counts(&router, &url).await,
        (0, 0, 1),
        "case straight after the delete: only the message no entry named is still in flight"
    );

    // Past the span the receive asked for, the queue hands back exactly the message the
    // batch did not delete.
    let start = Instant::now();
    let mut came_back: Vec<String> = Vec::new();
    for round in 0..POLL_ROUNDS {
        let reply = ok(
            &router,
            "ReceiveMessage",
            json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 1 }),
        )
        .await;
        for message in messages(&reply) {
            came_back.push(message["Body"].as_str().unwrap_or_default().to_string());
        }
        if !came_back.is_empty() {
            break;
        }
        assert!(
            round + 1 < POLL_ROUNDS,
            "case the lapse: nothing came back in {:?}",
            start.elapsed()
        );
    }
    assert_eq!(
        came_back,
        vec![body_of(2)],
        "case the lapse: expected only the message no entry deleted, got {came_back:?} \
         after {:?}",
        start.elapsed()
    );
    assert!(
        !came_back.contains(&body_of(0)) && !came_back.contains(&body_of(1)),
        "case the lapse: a deleted message must never come back, got {came_back:?}"
    );
}

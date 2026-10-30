// SPDX-License-Identifier: CC0-1.0

//! The dead-letter move a CONSUMER forces, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in
//! [`super::support`].
//!
//! A consumer that takes a message and does not delete it returns it to the queue by
//! changing its visibility to zero, and it is THAT delivery attempt — not a receive that
//! asked for a zero timeout — which spends the last of the allowance the source queue's
//! `RedrivePolicy` gives the message. The rule this suite pins:
//!
//! 1. Once a message has had every delivery its source queue's `RedrivePolicy` allows, the
//!    next receive on that queue moves it to `deadLetterTargetArn` instead of handing it
//!    over again, where it is receivable, carries the instant it was SENT unchanged and
//!    names the queue it was moved out of under `DeadLetterQueueSourceArn`. The first
//!    delivery from the source is the message's first receive, so it reports
//!    `ApproximateReceiveCount` `1`.
//! 2. The dead-letter queue's `MessageRetentionPeriod` is counted from the instant the
//!    queue holding the message ENQUEUED it. A standard queue takes the message as the one
//!    it already was, so the period runs from the original send and the life the message
//!    spent on the source queue is life it does not get in the dead-letter queue; a FIFO
//!    queue enqueues what it is handed, so the period runs from the move and the age the
//!    message arrives with changes nothing.
//!
//! The table spans what rule 2 generalises over: both queue types, how much of the period
//! the message spent on the source queue before the move, and how much time passes in the
//! dead-letter queue after it — rows either side of the boundary for each type. The spans
//! are documented minutes at the shortest, so this suite reaches them the way
//! [`super::time_driven_queue_semantics`] and [`super::dead_letter_instants`] do: it moves
//! the instants a stored message carries into the past through the engine seam instead of
//! sleeping, and every request still goes over the wire.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    queue_name_from_url, ComAmazonawsSqsAmazonsqsState,
};

use super::support::{
    create_ok, emulator_over_engine, expected_arn, fifo_attributes, message_counts, messages, ok,
    REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// How long a loop waits for something the emulator owes before giving up.
const DEADLINE: Duration = Duration::from_secs(20);

/// The `MessageRetentionPeriod` both dead-letter queues in this suite carry, in seconds:
/// the shortest the attribute admits ("an integer representing seconds, from 60 (1 minute)
/// to 1,209,600 (14 days)"), so the ages a row reaches stay small.
const DEAD_LETTER_RETENTION: u64 = 60;

/// The longest the attribute admits, carried by every SOURCE queue here so that nothing a
/// row observes is the source queue's own retention running out.
const LONGEST_RETENTION: &str = "1209600";

/// The `VisibilityTimeout` the source queues carry, in seconds: long enough that a message
/// handed to a consumer comes back only because the consumer changed its visibility, so the
/// second delivery attempt each row drives is the one the visibility change caused.
const SOURCE_VISIBILITY: &str = "100";

/// Let `by` pass for the queue at `url`: every message it holds becomes that much older,
/// both in the instant it was sent and in the instant the queue holding it enqueued it.
fn pass_time(engine: &engine::InMemoryEngine, url: &str, by: Duration) {
    if by.is_zero() {
        return;
    }
    let name = queue_name_from_url(url)
        .unwrap_or_else(|| panic!("case ageing {url}: the url names no queue"))
        .to_string();
    let store = engine.state::<ComAmazonawsSqsAmazonsqsState>();
    let mut store = store.lock();
    let queue = store
        .queue_mut(REGION, &name)
        .unwrap_or_else(|| panic!("case ageing {url}: no queue named {name}"));
    for message in &mut queue.messages {
        message.sent_at = message.sent_at.checked_sub(by).unwrap_or(message.sent_at);
        message.retention_from = message
            .retention_from
            .checked_sub(by)
            .unwrap_or(message.retention_from);
    }
}

/// Receive at most one message from the queue at `url` with every system attribute
/// selected, under the queue's own visibility timeout.
async fn receive_one(router: &MultiServiceRouter, url: &str) -> Option<Value> {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 1,
            "WaitTimeSeconds": 0,
            "MessageSystemAttributeNames": ["All"],
        }),
    )
    .await;
    messages(&reply).first().cloned()
}

/// Receive from the queue at `url` until a message comes over or the deadline passes.
async fn await_message(router: &MultiServiceRouter, case: &str, url: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        if let Some(message) = receive_one(router, url).await {
            return message;
        }
    }
    panic!("case {case}: no message came over within {DEADLINE:?}")
}

/// Hand the message the handle names back to the queue at `url` at once, which is how a
/// consumer that will not process it releases it.
async fn release(router: &MultiServiceRouter, url: &str, receipt_handle: &str) {
    ok(
        router,
        "ChangeMessageVisibility",
        json!({
            "QueueUrl": url,
            "ReceiptHandle": receipt_handle,
            "VisibilityTimeout": 0,
        }),
    )
    .await;
}

/// One system attribute of a received message.
fn attribute(case: &str, message: &Value, name: &str) -> Option<String> {
    message
        .get("Attributes")
        .and_then(|attributes| attributes.get(name))
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: {name} is not a string in {message}"))
                .to_string()
        })
}

/// One system attribute that must be there.
fn required_attribute(case: &str, message: &Value, name: &str) -> String {
    attribute(case, message, name)
        .unwrap_or_else(|| panic!("case {case}: no {name} in {message}"))
}

/// The receipt handle a receive minted.
fn receipt_handle(case: &str, message: &Value) -> String {
    message["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no ReceiptHandle in {message}"))
        .to_string()
}

/// A consumer that releases a message it will not process drives it to the dead-letter
/// queue, which takes the message as it was and retains it from the instant that queue
/// enqueued it: the original send on a standard queue, the move on a FIFO one.
#[tokio::test]
async fn a_released_message_is_dead_lettered_and_retained_from_its_enqueue() {
    // (case, FIFO, seconds the message spends on the source queue before the move,
    //  seconds that pass in the dead-letter queue after it, still receivable there)
    //
    // A standard dead-letter queue counts the period from the original send, so the two
    // spans add up against it; a FIFO one counts from the move, so only the second span
    // counts. Each type has a row either side of its own boundary.
    let rows: &[(&str, bool, u64, u64, bool)] = &[
        ("a standard DLQ, little spent on the source", false, 5, 20, true),
        ("a standard DLQ, most of the period spent on the source", false, 40, 30, false),
        ("a standard DLQ, straight to the DLQ, inside the period", false, 0, 45, true),
        ("a standard DLQ, straight to the DLQ, past the period", false, 0, 75, false),
        ("a FIFO DLQ, most of the period spent on the source", true, 40, 30, true),
        ("a FIFO DLQ, past the period since the move", true, 40, 75, false),
        ("a FIFO DLQ, straight to the DLQ, inside the period", true, 0, 45, true),
    ];

    for (case, fifo, spent_on_source, passed_in_dead_letter, receivable) in rows.iter().copied() {
        let (router, engine) = emulator_over_engine();
        let suffix = if fifo { ".fifo" } else { "" };
        let label = format!("{spent_on_source}-{passed_in_dead_letter}-{fifo}");
        let dead_letter_name = format!("dlq-release-{label}{suffix}");
        let source_name = format!("dlq-release-{label}-source{suffix}");

        let base = if fifo {
            fifo_attributes(&[("ContentBasedDeduplication", "true")])
        } else {
            json!({})
        };
        let mut dead_letter_attributes = base.clone();
        dead_letter_attributes["MessageRetentionPeriod"] =
            json!(DEAD_LETTER_RETENTION.to_string());
        let dead_letter_url = create_ok(&router, &dead_letter_name, &dead_letter_attributes).await;

        let mut source_attributes = base.clone();
        source_attributes["MessageRetentionPeriod"] = json!(LONGEST_RETENTION);
        source_attributes["VisibilityTimeout"] = json!(SOURCE_VISIBILITY);
        source_attributes["RedrivePolicy"] = json!(json!({
            "deadLetterTargetArn": expected_arn(&dead_letter_name),
            "maxReceiveCount": 1,
        })
        .to_string());
        let source_url = create_ok(&router, &source_name, &source_attributes).await;

        let body = format!("poison-{label}");
        let mut send = json!({"QueueUrl": source_url, "MessageBody": body});
        if fifo {
            send["MessageGroupId"] = json!("g");
        }
        ok(&router, "SendMessage", send).await;

        // The message spends `spent_on_source` of its life on the source queue before a
        // consumer takes it, which is the life a standard dead-letter queue does not give
        // it again.
        pass_time(&engine, &source_url, Duration::from_secs(spent_on_source));

        // The first delivery from the source: the allowance is one, so this is the last
        // one the source queue may make.
        let from_source = await_message(&router, case, &source_url).await;
        assert_eq!(
            required_attribute(case, &from_source, "ApproximateReceiveCount"),
            "1",
            "case {case}: the first delivery from the source queue is the message's first \
             receive, got {from_source}"
        );
        assert_eq!(
            attribute(case, &from_source, "DeadLetterQueueSourceArn"),
            None,
            "case {case}: a message sent straight to a queue was moved out of none, got \
             {from_source}"
        );
        let sent_at_source = required_attribute(case, &from_source, "SentTimestamp");
        assert_eq!(
            sent_at_source.len(),
            13,
            "case {case}: SentTimestamp is epoch milliseconds, got {sent_at_source}"
        );

        // The consumer hands the message back rather than deleting it; the queue's own
        // timeout is far longer, so the delivery attempt that follows is the one this
        // release caused.
        release(&router, &source_url, &receipt_handle(case, &from_source)).await;

        // The source queue has no delivery left to make, so the attempt that follows the
        // release moves the message instead of handing it over again, and the source queue
        // is left holding nothing at all.
        let after_release = receive_one(&router, &source_url).await;
        assert!(
            after_release.is_none(),
            "case {case}: the source queue handed the message over again instead of moving \
             it, got {after_release:?}"
        );
        assert_eq!(
            message_counts(&router, &source_url).await,
            (0, 0, 0),
            "case {case}: the source queue holds nothing once its message was dead-lettered"
        );

        let from_dead_letter = await_message(&router, case, &dead_letter_url).await;
        assert_eq!(
            from_dead_letter["Body"].as_str(),
            from_source["Body"].as_str(),
            "case {case}: the dead-letter queue holds the same message, got \
             {from_dead_letter}"
        );
        assert_eq!(
            from_dead_letter["MessageId"].as_str(),
            from_source["MessageId"].as_str(),
            "case {case}: the dead-letter queue holds the same message id, got \
             {from_dead_letter}"
        );
        assert_eq!(
            required_attribute(case, &from_dead_letter, "SentTimestamp"),
            sent_at_source,
            "case {case}: the move does not re-send the message, so SentTimestamp is the \
             one the source queue reported"
        );
        assert_eq!(
            required_attribute(case, &from_dead_letter, "DeadLetterQueueSourceArn"),
            expected_arn(&source_name),
            "case {case}: the moved message names the queue it was moved out of, got \
             {from_dead_letter}"
        );

        // The consumer of the dead-letter queue releases it too, so what the row observes
        // next is the retention period rather than a visibility timeout.
        release(
            &router,
            &dead_letter_url,
            &receipt_handle(case, &from_dead_letter),
        )
        .await;
        pass_time(
            &engine,
            &dead_letter_url,
            Duration::from_secs(passed_in_dead_letter),
        );

        let still_there = receive_one(&router, &dead_letter_url).await;
        assert_eq!(
            still_there.is_some(),
            receivable,
            "case {case}: {spent_on_source}s on the source and {passed_in_dead_letter}s in \
             a dead-letter queue retaining for {DEAD_LETTER_RETENTION}s, expected \
             receivable={receivable}, got {still_there:?}"
        );
    }
}

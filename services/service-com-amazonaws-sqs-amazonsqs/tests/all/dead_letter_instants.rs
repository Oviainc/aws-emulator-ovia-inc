// SPDX-License-Identifier: CC0-1.0

//! What a message KEEPS when a redrive moves it to a dead-letter queue, driven over the
//! wire (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in
//! [`super::support`].
//!
//! Two rules, both about the instants a moved message carries:
//!
//! 1. A move does not re-SEND the message. `SentTimestamp` is the instant its sender was
//!    answered under, whichever queue holds it now and whatever type that queue is: the
//!    identity of the message is unchanged by being dead-lettered.
//! 2. What the dead-letter queue's `MessageRetentionPeriod` is counted from is the instant
//!    the queue holding the message ENQUEUED it. For a standard dead-letter queue that is
//!    the original enqueue — "the expiration of a message is always based on its original
//!    enqueue timestamp", so a message that spent most of its life in the source queue is
//!    discarded that much sooner in the dead-letter queue. A FIFO queue enqueues what it is
//!    handed, so a message arriving in one is enqueued at the MOVE and keeps the whole
//!    period from there.
//!
//! The spans involved are documented minutes at the shortest, so this suite reaches them
//! the way [`super::time_driven_queue_semantics`] does: it moves the instants a stored
//! message carries into the past through the engine seam rather than sleeping, and every
//! request still goes over the wire.
//!
//! The table spans what rule 2 generalises over: both queue types, and an age on either
//! side of the dead-letter queue's own retention period.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    queue_name_from_url, ComAmazonawsSqsAmazonsqsState,
};

use super::support::{
    create_ok, emulator_over_engine, expected_arn, fifo_attributes, messages, ok, REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// How long a loop waits for the move the emulator owes before giving up.
const DEADLINE: Duration = Duration::from_secs(20);

/// The retention period both dead-letter queues in this suite carry, in seconds — the
/// shortest `MessageRetentionPeriod` the attribute admits ("an integer representing seconds,
/// from 60 (1 minute) to 1,209,600 (14 days)"), so the ages a row ages a message by stay
/// small.
const DEAD_LETTER_RETENTION: u64 = 60;

/// Make every message the queue at `url` holds as old as if it had been SENT `by` ago,
/// leaving the instant the queue holding it enqueued it alone: that is how a message that
/// has spent most of its life in the source queue is reached without waiting for it.
fn age_sends(engine: &engine::InMemoryEngine, url: &str, by: Duration) {
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

/// Receive from the queue at `url` until a message comes over or the deadline passes, and
/// answer it with the system attributes selected.
async fn await_message(router: &MultiServiceRouter, case: &str, url: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        let reply = ok(
            router,
            "ReceiveMessage",
            json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 1,
                "VisibilityTimeout": 0,
                "WaitTimeSeconds": 1,
                "MessageSystemAttributeNames": ["All"],
            }),
        )
        .await;
        if let Some(message) = messages(&reply).first() {
            return message.clone();
        }
    }
    panic!("case {case}: no message came over within {DEADLINE:?}")
}

/// `SentTimestamp` as the epoch milliseconds a receive reports it in.
fn sent_timestamp(case: &str, message: &Value) -> u128 {
    message
        .get("Attributes")
        .and_then(|attributes| attributes.get("SentTimestamp"))
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u128>().ok())
        .unwrap_or_else(|| panic!("case {case}: no SentTimestamp in {message}"))
}

/// Drive the one message of the source queue at `source_url` past its delivery allowance,
/// so the redrive moves it to the dead-letter queue, and answer the message the SOURCE
/// queue reported for it on its last delivery.
async fn drive_to_the_dead_letter_queue(
    router: &MultiServiceRouter,
    case: &str,
    source_url: &str,
) -> Value {
    // `maxReceiveCount` is 1 on the queues this suite builds, so one delivery spends the
    // allowance and the next receive on the source moves the message.
    let delivered = await_message(router, case, source_url).await;
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        let reply = ok(
            router,
            "ReceiveMessage",
            json!({"QueueUrl": source_url, "MaxNumberOfMessages": 1, "WaitTimeSeconds": 1}),
        )
        .await;
        if messages(&reply).is_empty() {
            return delivered;
        }
    }
    panic!("case {case}: the source queue never stopped handing the message over")
}

/// A dead-lettered message keeps the instant it was SENT, and its retention in the
/// dead-letter queue runs from the instant THAT queue enqueued it: the original send for a
/// standard queue, the move for a FIFO one.
#[tokio::test]
async fn a_dead_lettered_message_keeps_its_send_instant_and_is_retained_from_its_enqueue() {
    // (case, FIFO, how much of the retention period the message had already spent in the
    // source queue, whether it is still in the dead-letter queue afterwards)
    //
    // A standard dead-letter queue counts the period from the original send, so a message
    // older than the period is discarded on arrival and a younger one survives. A FIFO one
    // counts from the move, so the age it arrives with changes nothing.
    let rows: &[(&str, bool, u64, bool)] = &[
        ("a standard DLQ, well inside the period", false, 5, true),
        (
            "a standard DLQ, older than the period",
            false,
            DEAD_LETTER_RETENTION + 1,
            false,
        ),
        ("a FIFO DLQ, well inside the period", true, 5, true),
        (
            "a FIFO DLQ, older than the period",
            true,
            DEAD_LETTER_RETENTION + 1,
            true,
        ),
    ];

    for (case, fifo, aged_by_seconds, survives) in rows.iter().copied() {
        let (router, engine) = emulator_over_engine();
        let suffix = if fifo { ".fifo" } else { "" };
        let dead_letter_name = format!("dlq-instants-{aged_by_seconds}-{fifo}{suffix}");
        let source_name = format!("dlq-instants-{aged_by_seconds}-{fifo}-source{suffix}");
        // Both queues carry the shortest retention the attribute admits, so the boundary
        // a row crosses is reached by ageing the message by a minute rather than a day.
        let mut attributes = if fifo {
            fifo_attributes(&[])
        } else {
            json!({})
        };
        attributes["MessageRetentionPeriod"] = json!(DEAD_LETTER_RETENTION.to_string());
        let dead_letter_url = create_ok(&router, &dead_letter_name, &attributes).await;

        let mut source_attributes = attributes.clone();
        source_attributes["RedrivePolicy"] = json!(json!({
            "deadLetterTargetArn": expected_arn(&dead_letter_name),
            "maxReceiveCount": 1,
        })
        .to_string());
        // The source keeps the message for far longer than the dead-letter queue, so
        // nothing a row observes is the SOURCE's retention expiring.
        source_attributes["MessageRetentionPeriod"] = json!("1209600");
        let source_url = create_ok(&router, &source_name, &source_attributes).await;

        let mut send = json!({"QueueUrl": source_url, "MessageBody": "poison"});
        if fifo {
            send["MessageGroupId"] = json!("g");
            send["MessageDeduplicationId"] = json!("d");
        }
        ok(&router, "SendMessage", send).await;

        // The message spends `aged_by_seconds` of its life in the source queue before the
        // redrive takes it.
        age_sends(&engine, &source_url, Duration::from_secs(aged_by_seconds));
        let from_source = drive_to_the_dead_letter_queue(&router, case, &source_url).await;
        let sent_in_source = sent_timestamp(case, &from_source);

        if !survives {
            // Its retention is counted from the original enqueue, which has already run
            // out: the dead-letter queue holds nothing for a consumer.
            let reply = ok(
                &router,
                "ReceiveMessage",
                json!({"QueueUrl": dead_letter_url, "MaxNumberOfMessages": 10}),
            )
            .await;
            assert!(
                messages(&reply).is_empty(),
                "case {case}: a message past the dead-letter queue's retention must not be \
                 handed over, got {reply}"
            );
            continue;
        }

        let from_dead_letter = await_message(&router, case, &dead_letter_url).await;
        assert_eq!(
            sent_timestamp(case, &from_dead_letter),
            sent_in_source,
            "case {case}: the move must not re-send the message, so SentTimestamp is the \
             one the source queue reported"
        );
        assert_eq!(
            from_dead_letter["Body"].as_str(),
            from_source["Body"].as_str(),
            "case {case}: the dead-letter queue holds the same message"
        );
        assert_eq!(
            from_dead_letter["MessageId"].as_str(),
            from_source["MessageId"].as_str(),
            "case {case}: the dead-letter queue holds the same message id"
        );
        // A FIFO queue enqueued it at the move, so a message that arrives already older
        // than the period is still there — which the row above pins by surviving where
        // the standard one does not.
        if fifo && aged_by_seconds > DEAD_LETTER_RETENTION {
            let reported = sent_timestamp(case, &from_dead_letter);
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|since| since.as_millis())
                .unwrap_or_default();
            assert!(
                reported + u128::from(DEAD_LETTER_RETENTION) * 1_000 < now,
                "case {case}: the row only says anything if the message is older than the \
                 period it is retained for, got SentTimestamp {reported} at {now}"
            );
        }
    }
}

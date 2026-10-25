// SPDX-License-Identifier: CC0-1.0

//! The queue semantics the CLOCK drives, each over a TABLE of inputs rather than one
//! example, driven over the wire (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`)
//! through the shared harness in [`super::support`].
//!
//! The rules pinned here:
//!
//! 1. A message older than the queue's `MessageRetentionPeriod` is discarded, whatever
//!    state it is in — visible, in flight or still delayed — and stops being counted by
//!    every `ApproximateNumberOfMessages*` attribute. The period is read from the queue
//!    at the instant the question is asked, so shortening it expires what is already
//!    older than the new value.
//! 2. A message carries the visibility timeout it was RECEIVED under, so a later
//!    `VisibilityTimeout` change spares it and applies only to receives that follow.
//! 3. A `DelaySeconds` change is retroactive for a FIFO queue and not for a standard
//!    one.
//!
//! A retention period is a documented duration (its own range starts at 60 seconds), so
//! no test here sleeps for one: the rows move the instants a stored message carries into
//! the past through the engine seam [`super::support::emulator_over_engine`] hands back,
//! and everything they then assert goes over the wire like any other request.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{queue_name_from_url, ComAmazonawsSqsAmazonsqsState};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{create_ok, emulator_over_engine, message_counts as counts, messages, ok};

/// `at` moved `by` into the past, or `at` itself for an instant that far predates the
/// epoch (unreachable for an instant this emulator minted from the clock).
fn earlier(at: SystemTime, by: Duration) -> SystemTime {
    at.checked_sub(by).unwrap_or(at)
}

/// Make every message the queue at `url` holds as old as if it had been sent `by` ago:
/// the instant it was sent and the instant it became available both move back, so a
/// delayed message stays delayed and an in-flight one stays in flight.
///
/// This is how a span the clock owes is reached without sleeping for it.
fn age_messages(engine: &engine::InMemoryEngine, url: &str, by: Duration) {
    let name = queue_name_from_url(url)
        .unwrap_or_else(|| panic!("case ageing {url}: the url names no queue"))
        .to_string();
    let store = engine.state::<ComAmazonawsSqsAmazonsqsState>();
    let mut store = store.lock();
    let queue = store
        .queues
        .get_mut(&name)
        .unwrap_or_else(|| panic!("case ageing {url}: no queue named {name}"));
    for message in &mut queue.messages {
        message.sent_at = earlier(message.sent_at, by);
        message.available_at = earlier(message.available_at, by);
    }
}

/// Receive up to ten messages without hiding anything the answer carries, so a row can
/// ask twice in a row what the queue is prepared to hand over.
async fn receive(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let answer = ok(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "VisibilityTimeout": 0 }),
    )
    .await;
    messages(&answer)
}

/// One row of the retention table.
struct RetentionRow {
    case: &'static str,
    /// The `MessageRetentionPeriod` the queue is created with. "An integer from 60
    /// seconds (1 minute) to 1,209,600 seconds (14 days)" — the rows span that range
    /// and both sides of each value they use.
    retention: &'static str,
    /// The `DelaySeconds` the send carries, so a row can age a DELAYED message.
    delay: i64,
    /// Whether the row receives the message first under a long visibility timeout,
    /// leaving it IN FLIGHT while its retention lapses.
    receive_first: bool,
    /// How long ago the message was sent, by the time the row asks.
    aged_by: Duration,
    /// The counts the queue must report afterwards, in the order the shared harness's
    /// `message_counts` hands them back: (available, delayed, in flight).
    expected: (i64, i64, i64),
}

/// The retention rule, over the states a message can be in and both sides of the
/// period's own bound.
#[tokio::test]
async fn retention_discards_a_message_older_than_the_period_whatever_state_it_is_in() {
    let rows: &[RetentionRow] = &[
        RetentionRow {
            case: "visible, one second past the shortest retention",
            retention: "60",
            delay: 0,
            receive_first: false,
            aged_by: Duration::from_secs(61),
            expected: (0, 0, 0),
        },
        RetentionRow {
            case: "visible, inside the shortest retention",
            retention: "60",
            delay: 0,
            receive_first: false,
            aged_by: Duration::from_secs(50),
            expected: (1, 0, 0),
        },
        RetentionRow {
            case: "visible, exactly as old as its retention",
            retention: "60",
            delay: 0,
            receive_first: false,
            aged_by: Duration::from_secs(60),
            expected: (0, 0, 0),
        },
        RetentionRow {
            case: "in flight, past its retention",
            retention: "60",
            delay: 0,
            receive_first: true,
            aged_by: Duration::from_secs(61),
            expected: (0, 0, 0),
        },
        RetentionRow {
            case: "in flight, inside its retention",
            retention: "120",
            delay: 0,
            receive_first: true,
            aged_by: Duration::from_secs(60),
            expected: (0, 0, 1),
        },
        RetentionRow {
            case: "delayed, past its retention",
            retention: "120",
            delay: 900,
            receive_first: false,
            aged_by: Duration::from_secs(121),
            expected: (0, 0, 0),
        },
        RetentionRow {
            case: "delayed, inside its retention",
            retention: "120",
            delay: 900,
            receive_first: false,
            aged_by: Duration::from_secs(60),
            expected: (0, 1, 0),
        },
        RetentionRow {
            case: "an hour's retention, an hour and a second old",
            retention: "3600",
            delay: 0,
            receive_first: false,
            aged_by: Duration::from_secs(3_601),
            expected: (0, 0, 0),
        },
        RetentionRow {
            case: "the longest retention, a day old",
            retention: "1209600",
            delay: 0,
            receive_first: false,
            aged_by: Duration::from_secs(86_400),
            expected: (1, 0, 0),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, engine) = emulator_over_engine();
        let url = create_ok(
            &router,
            &format!("retention-{index}"),
            &json!({ "MessageRetentionPeriod": row.retention }),
        )
        .await;
        let mut send = json!({ "QueueUrl": url, "MessageBody": "payload" });
        if row.delay > 0 {
            send["DelaySeconds"] = json!(row.delay);
        }
        ok(&router, "SendMessage", send).await;
        if row.receive_first {
            let delivered = ok(
                &router,
                "ReceiveMessage",
                // "An integer from 0 to 43,200 (12 hours)" (VisibilityTimeout): the
                // longest the message can be held, so nothing but retention can free it.
                json!({ "QueueUrl": url, "VisibilityTimeout": 43_200 }),
            )
            .await;
            assert_eq!(
                messages(&delivered).len(),
                1,
                "case {}: expected the message in flight, got {delivered:?}",
                row.case
            );
        }

        age_messages(&engine, &url, row.aged_by);

        let reported = counts(&router, &url).await;
        assert_eq!(
            reported, row.expected,
            "case {}: expected (available, delayed, in flight) {:?}, got {reported:?}",
            row.case, row.expected
        );
        let delivered = receive(&router, &url).await;
        let expected_handed_over = usize::try_from(row.expected.0).unwrap_or_default();
        assert_eq!(
            delivered.len(),
            expected_handed_over,
            "case {}: expected {expected_handed_over} message(s) handed over, got {delivered:?}",
            row.case
        );
    }
}

/// One row of the "retention shortened" table: the period the queue is created with, the
/// one it is changed to, how old the message is by then, and whether it survives.
type ShortenedRow = (&'static str, &'static str, &'static str, u64, bool);

/// The period is read from the queue when the question is asked, so a `MessageRetentionPeriod`
/// change decides the fate of messages that are already stored.
#[tokio::test]
async fn a_shortened_retention_period_expires_what_is_already_older_than_it() {
    let rows: &[ShortenedRow] = &[
        (
            "shortened below the message's age",
            "345600",
            "60",
            120,
            false,
        ),
        (
            "shortened, but still above the message's age",
            "345600",
            "300",
            120,
            true,
        ),
        ("shortened below a younger message's age", "345600", "60", 30, true),
        ("lengthened past the message's age", "60", "1209600", 120, true),
    ];

    for (index, (case, created_with, changed_to, aged_seconds, survives)) in
        rows.iter().enumerate()
    {
        let (router, engine) = emulator_over_engine();
        let url = create_ok(
            &router,
            &format!("retention-change-{index}"),
            &json!({ "MessageRetentionPeriod": created_with }),
        )
        .await;
        ok(
            &router,
            "SendMessage",
            json!({ "QueueUrl": url, "MessageBody": "payload" }),
        )
        .await;
        age_messages(&engine, &url, Duration::from_secs(*aged_seconds));

        ok(
            &router,
            "SetQueueAttributes",
            json!({ "QueueUrl": url, "Attributes": { "MessageRetentionPeriod": changed_to } }),
        )
        .await;

        let reported = counts(&router, &url).await;
        let expected = if *survives { (1, 0, 0) } else { (0, 0, 0) };
        assert_eq!(
            reported, expected,
            "case {case}: expected (available, delayed, in flight) {expected:?}, got {reported:?}"
        );
        let delivered = receive(&router, &url).await;
        assert_eq!(
            delivered.len(),
            usize::from(*survives),
            "case {case}: expected the message {}, got {delivered:?}",
            if *survives { "still there" } else { "discarded" }
        );
    }
}

/// The short visibility timeout the rows below receive a message under — as the string
/// form the queue attribute takes and the integer form the request member takes, the same
/// value in both — and how long they then wait for it to lapse: comfortably past it, so a
/// loaded machine cannot decide a row by being slow.
const SHORT_TIMEOUT_ATTRIBUTE: &str = "3";
const SHORT_TIMEOUT: i32 = 3;
const PAST_SHORT_TIMEOUT: Duration = Duration::from_millis(3_600);

/// One row of the visibility table: the case, the queue's `VisibilityTimeout` when the
/// message is received, the per-request override the receive carries (if any), the value
/// the queue is changed to afterwards, and whether the message the change SPARED is back
/// once the timeout it was received under has lapsed.
type VisibilityRow = (&'static str, &'static str, Option<i32>, &'static str, bool);

/// A `VisibilityTimeout` change spares the messages already in flight — each keeps the
/// timeout it was received under — and applies to the receives that follow it.
#[tokio::test]
async fn a_visibility_timeout_change_spares_the_messages_already_in_flight() {
    let rows: &[VisibilityRow] = &[
        (
            "received under the queue's short timeout, then raised to the longest",
            SHORT_TIMEOUT_ATTRIBUTE,
            None,
            "43200",
            true,
        ),
        (
            "received under a short override, then lowered to zero",
            "43200",
            Some(SHORT_TIMEOUT),
            "0",
            true,
        ),
        (
            "received under a five-minute queue timeout, then lowered to zero",
            "300",
            None,
            "0",
            false,
        ),
        (
            "received under a five-minute override, then lowered to the short one",
            SHORT_TIMEOUT_ATTRIBUTE,
            Some(300),
            SHORT_TIMEOUT_ATTRIBUTE,
            false,
        ),
    ];

    for (index, (case, queue_timeout, override_timeout, changed_to, back_once_lapsed)) in
        rows.iter().enumerate()
    {
        let (router, _engine) = emulator_over_engine();
        let url = create_ok(
            &router,
            &format!("visibility-{index}"),
            &json!({ "VisibilityTimeout": queue_timeout }),
        )
        .await;
        ok(
            &router,
            "SendMessage",
            json!({ "QueueUrl": url, "MessageBody": "in-flight" }),
        )
        .await;

        let mut request = json!({ "QueueUrl": url });
        if let Some(timeout) = override_timeout {
            request["VisibilityTimeout"] = json!(timeout);
        }
        let delivered = ok(&router, "ReceiveMessage", request).await;
        assert_eq!(
            messages(&delivered).len(),
            1,
            "case {case}: expected the message in flight, got {delivered:?}"
        );

        ok(
            &router,
            "SetQueueAttributes",
            json!({ "QueueUrl": url, "Attributes": { "VisibilityTimeout": changed_to } }),
        )
        .await;

        let reported = counts(&router, &url).await;
        assert_eq!(
            reported,
            (0, 0, 1),
            "case {case}: the change must leave the message in flight, got {reported:?}"
        );

        tokio::time::sleep(PAST_SHORT_TIMEOUT).await;
        let delivered = receive(&router, &url).await;
        assert_eq!(
            delivered.len(),
            usize::from(*back_once_lapsed),
            "case {case}: expected the message {} once the short timeout had lapsed, got \
             {delivered:?}",
            if *back_once_lapsed {
                "back"
            } else {
                "still in flight"
            }
        );
    }
}

/// One row of the delay table: the case, whether the queue is FIFO, the `DelaySeconds`
/// it is created with, how old the message is when the attribute changes, the value it
/// changes to, and whether the message is available right after the change.
type DelayRow = (&'static str, bool, &'static str, u64, &'static str, bool);

/// A `DelaySeconds` change is retroactive for a FIFO queue — the messages it already
/// holds are delayed by the new value, measured from when each was sent — and is not
/// retroactive for a standard queue, where each message keeps the delay it was sent
/// under.
#[tokio::test]
async fn a_delay_seconds_change_is_retroactive_for_fifo_and_not_for_standard() {
    let rows: &[DelayRow] = &[
        ("fifo, delay dropped to zero", true, "900", 0, "0", true),
        ("standard, delay dropped to zero", false, "900", 0, "0", false),
        ("fifo, delay raised from zero", true, "0", 0, "900", false),
        ("standard, delay raised from zero", false, "0", 0, "900", true),
        (
            "fifo, delay shortened but not yet elapsed",
            true,
            "900",
            60,
            "300",
            false,
        ),
        (
            "fifo, delay shortened below the message's age",
            true,
            "900",
            120,
            "60",
            true,
        ),
        (
            "standard, delay shortened below the message's age",
            false,
            "900",
            120,
            "60",
            false,
        ),
    ];

    for (index, (case, fifo, created_with, aged_seconds, changed_to, available)) in
        rows.iter().enumerate()
    {
        let (router, engine) = emulator_over_engine();
        let name = if *fifo {
            format!("delay-{index}.fifo")
        } else {
            format!("delay-{index}")
        };
        let mut attributes = json!({ "DelaySeconds": created_with });
        if *fifo {
            attributes["FifoQueue"] = json!("true");
            attributes["ContentBasedDeduplication"] = json!("true");
        }
        let url = create_ok(&router, &name, &attributes).await;

        let mut send = json!({ "QueueUrl": url, "MessageBody": "payload" });
        if *fifo {
            send["MessageGroupId"] = json!("group");
        }
        ok(&router, "SendMessage", send).await;
        if *aged_seconds > 0 {
            age_messages(&engine, &url, Duration::from_secs(*aged_seconds));
        }

        ok(
            &router,
            "SetQueueAttributes",
            json!({ "QueueUrl": url, "Attributes": { "DelaySeconds": changed_to } }),
        )
        .await;

        let reported = counts(&router, &url).await;
        let expected = if *available { (1, 0, 0) } else { (0, 1, 0) };
        assert_eq!(
            reported, expected,
            "case {case}: expected (available, delayed, in flight) {expected:?}, got {reported:?}"
        );
        let delivered = receive(&router, &url).await;
        assert_eq!(
            delivered.len(),
            usize::from(*available),
            "case {case}: expected the message {}, got {delivered:?}",
            if *available {
                "handed over"
            } else {
                "withheld"
            }
        );
    }
}

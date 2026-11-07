// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the BATCH-WIDE constraints `PutLogEvents` holds one request to, and
//! for the sentences it refuses each with.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: a constraint on the BATCH is answered by refusing the whole request
//! with `InvalidParameterException` and storing none of its events — never by accepting the
//! request and reporting the violation as a per-event rejection — and the size refusals
//! report the size the service COUNTS, which is each event's UTF-8 message plus the
//! documented 26 bytes of per-event overhead, against the documented limit:
//!
//! * one event whose message exceeds the per-event limit is `Log event too large: <size>
//!   bytes exceeds limit of 1048576`, where `<size>` is that event's counted size;
//! * a batch whose counted size exceeds the limit is `Upload too large: <size> bytes exceeds
//!   limit of 1048576`, where `<size>` is the counted size of the whole batch — so a message
//!   between the limit less the overhead and the limit itself, whose MESSAGE is inside the
//!   per-event bound but whose COUNTED size is not, is refused as the batch;
//! * a batch whose events INSIDE the ingestion window span more than 24 hours is refused —
//!   "for valid events (within 14 days in the past to 2 hours in future), the time span in a
//!   single batch cannot exceed 24 hours. Otherwise, the operation fails" — while events the
//!   window itself refuses do not stretch that span and are still reported in
//!   `rejectedLogEventsInfo`.
//!
//! The tables span the classes those rules generalise over (how far over each bound, how many
//! events carry it, which side of the window the out-of-window events fall on); the recorded
//! cases are one row each.

use serde_json::{json, Value};

use crate::harness::{batch, group_with_stream, messages, now_ms, Logs};

/// The error code every batch constraint is refused with: `PutLogEvents` models
/// `InvalidParameterException` for "a parameter is specified incorrectly".
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The documented per-event and per-batch size limit: "the maximum batch size is 1,048,576
/// bytes", "each log event can be no larger than 1 MB".
const MAX_BYTES: usize = 1_048_576;

/// The documented per-event overhead the size accounting charges: "this size is calculated as
/// the sum of all event messages in UTF-8, plus 26 bytes for each log event".
const EVENT_OVERHEAD: usize = 26;

/// Milliseconds in one hour.
const HOUR: i64 = 60 * 60 * 1_000;

/// Milliseconds in one day — the widest span one batch's in-window events may cover.
const DAY: i64 = 24 * HOUR;

/// Put one batch of `(timestamp, message)` pairs, returning the status and body.
async fn put(logs: &Logs, group: &str, stream: &str, events: &[(i64, &str)]) -> (u16, Value) {
    logs.call(
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": stream,
            "logEvents": batch(events),
        }),
    )
    .await
}

/// Every message the stream serves back, oldest first.
async fn stored(logs: &Logs, group: &str, stream: &str) -> Vec<String> {
    let page = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": group, "logStreamName": stream, "startFromHead": true}),
        )
        .await;
    messages(&page)
}

/// One event whose message is over the per-event limit is refused with the size the service
/// counts for it — the message plus the per-event overhead — whatever else the batch carries,
/// and nothing of the batch is stored.
#[tokio::test]
async fn an_event_over_the_per_event_limit_is_refused_with_its_counted_size() {
    // (case, message length in bytes)
    let cases: [(&str, usize); 4] = [
        ("one byte over the limit", MAX_BYTES + 1),
        ("a kilobyte over the limit", MAX_BYTES + 1_000),
        ("the overhead over the limit", MAX_BYTES + EVENT_OVERHEAD),
        ("twice the limit", 2 * MAX_BYTES),
    ];

    for (case, length) in cases {
        let logs = Logs::new();
        group_with_stream(&logs, "oversize", "stream", None).await;
        let clock = now_ms();
        let message = "x".repeat(length);

        let (code, reported) = logs
            .refusal(
                "PutLogEvents",
                &json!({
                    "logGroupName": "oversize",
                    "logStreamName": "stream",
                    "logEvents": batch(&[(clock, message.as_str())]),
                }),
            )
            .await;
        let expected = format!(
            "Log event too large: {} bytes exceeds limit of {MAX_BYTES}",
            length + EVENT_OVERHEAD
        );
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {reported:?}"
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert_eq!(
            stored(&logs, "oversize", "stream").await,
            Vec::<String>::new(),
            "case {case:?}: a refused batch stores nothing"
        );
    }
}

/// A batch whose COUNTED size is over the limit is refused with that counted size, over
/// batches differing in how many events carry it; a batch counted at exactly the limit is
/// accepted.
#[tokio::test]
async fn a_batch_over_the_counted_limit_is_refused_with_its_counted_size() {
    // (case, [message length per event]) — each event inside the per-event limit, so what
    // the batch breaks is the batch bound.
    let over: [(&str, Vec<usize>); 5] = [
        // One event counted at exactly one byte over: the message is the limit less the
        // per-event overhead, plus one.
        (
            "one event, one byte over the counted limit",
            vec![MAX_BYTES - EVENT_OVERHEAD + 1],
        ),
        // The message exactly at the per-event limit: inside that bound, so what it breaks
        // is the batch bound its counted size exceeds.
        (
            "one event whose message is exactly the per-event limit",
            vec![MAX_BYTES],
        ),
        ("two half-limit events", vec![MAX_BYTES / 2; 2]),
        ("three events", vec![400_000; 3]),
        ("ten events", vec![110_000; 10]),
    ];

    for (case, lengths) in &over {
        let logs = Logs::new();
        group_with_stream(&logs, "batch", "stream", None).await;
        let clock = now_ms();
        let messages_sent: Vec<String> = lengths
            .iter()
            .enumerate()
            .map(|(index, length)| {
                // Distinct leading characters, so nothing depends on two events being equal.
                let offset = u8::try_from(index % 26).unwrap_or(0);
                let mut message = String::from(char::from(b'a' + offset));
                message.push_str(&"y".repeat(length - 1));
                message
            })
            .collect();
        let events: Vec<(i64, &str)> = messages_sent
            .iter()
            .enumerate()
            .map(|(index, message)| {
                (clock + i64::try_from(index).unwrap_or(0), message.as_str())
            })
            .collect();

        let (code, reported) = logs
            .refusal(
                "PutLogEvents",
                &json!({
                    "logGroupName": "batch",
                    "logStreamName": "stream",
                    "logEvents": batch(&events),
                }),
            )
            .await;
        let counted: usize = lengths.iter().map(|length| length + EVENT_OVERHEAD).sum();
        let expected = format!("Upload too large: {counted} bytes exceeds limit of {MAX_BYTES}");
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {reported:?}"
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert_eq!(
            stored(&logs, "batch", "stream").await,
            Vec::<String>::new(),
            "case {case:?}: a refused batch stores nothing"
        );
    }

    // The batch counted at exactly the limit is inside it: one event whose message is the
    // limit less the one event's overhead.
    let logs = Logs::new();
    group_with_stream(&logs, "batch", "stream", None).await;
    let clock = now_ms();
    let fitting = "z".repeat(MAX_BYTES - EVENT_OVERHEAD);
    let (status, body) = put(&logs, "batch", "stream", &[(clock, fitting.as_str())]).await;
    assert_eq!(
        status, 200,
        "case the batch counted at exactly the limit: expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        stored(&logs, "batch", "stream").await.len(),
        1,
        "case the batch counted at exactly the limit: the event is stored"
    );
}

/// What one batch's event timestamps make of the 24-hour span bound: the span of the events
/// INSIDE the ingestion window decides, so a batch over it is refused whole and one whose
/// only wide reach is an out-of-window event is accepted with that event reported as
/// rejected.
#[tokio::test]
async fn a_batch_is_refused_when_its_in_window_events_span_more_than_a_day() {
    /// What a row expects of the service.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Expect {
        /// Refused whole: nothing of the batch is stored.
        Refused,
        /// Accepted, storing the named messages and reporting this `rejectedLogEventsInfo`.
        Accepted(Vec<&'static str>, Value),
    }

    /// One row of the table: the batch it puts, and what the service must answer.
    struct Row {
        /// What the row is about, named in every assertion.
        case: &'static str,
        /// The batch, as `(offset from the ingestion clock, message)` pairs in the
        /// chronological order the operation requires of its input.
        events: Vec<(i64, &'static str)>,
        /// What the service must answer.
        expected: Expect,
    }

    let cases: Vec<Row> = vec![
        Row {
            case: "one event",
            events: vec![(-HOUR, "only")],
            expected: Expect::Accepted(vec!["only"], Value::Null),
        },
        Row {
            case: "a span of exactly a day",
            events: vec![(-DAY, "old"), (0, "new")],
            expected: Expect::Accepted(vec!["old", "new"], Value::Null),
        },
        Row {
            case: "a span one millisecond over a day",
            events: vec![(-DAY - 1, "old"), (0, "new")],
            expected: Expect::Refused,
        },
        Row {
            case: "a span an hour over a day",
            events: vec![(-DAY - HOUR, "old"), (0, "new")],
            expected: Expect::Refused,
        },
        Row {
            case: "a span of thirty hours across three events",
            events: vec![(-30 * HOUR, "old"), (-HOUR, "middle"), (0, "new")],
            expected: Expect::Refused,
        },
        Row {
            case: "a span of a day either side of the clock",
            events: vec![(-23 * HOUR, "old"), (HOUR, "ahead")],
            expected: Expect::Accepted(vec!["old", "ahead"], Value::Null),
        },
        Row {
            case: "a span over a day either side of the clock",
            events: vec![(-23 * HOUR, "old"), (HOUR + 60_000, "ahead")],
            expected: Expect::Refused,
        },
        Row {
            // The out-of-window event is not one of the valid events the span is measured
            // over, so the batch is inside the bound and that event is reported rejected.
            case: "a wide reach made only by an event older than the window",
            events: vec![(-20 * DAY, "ancient"), (-HOUR, "kept"), (0, "newer")],
            expected: Expect::Accepted(
                vec!["kept", "newer"],
                json!({"tooOldLogEventEndIndex": 1}),
            ),
        },
        Row {
            case: "a wide reach made only by an event ahead of the window",
            events: vec![(-23 * HOUR, "kept"), (3 * HOUR, "ahead")],
            expected: Expect::Accepted(vec!["kept"], json!({"tooNewLogEventStartIndex": 1})),
        },
    ];

    for row in &cases {
        let case = row.case;
        let logs = Logs::new();
        group_with_stream(&logs, "span", "stream", None).await;
        let clock = now_ms();
        let events: Vec<(i64, &str)> = row
            .events
            .iter()
            .map(|(offset, message)| (clock + offset, *message))
            .collect();

        let (status, body) = put(&logs, "span", "stream", &events).await;
        match &row.expected {
            Expect::Refused => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the request to be refused, got {status} {body}"
                );
                assert_eq!(
                    crate::harness::error_code(&body),
                    INVALID_PARAMETER,
                    "case {case:?}: expected {INVALID_PARAMETER}, got {body}"
                );
                assert_eq!(
                    body.get("message").and_then(Value::as_str),
                    Some(
                        "The batch of log events in a single PutLogEvents request cannot span \
                         more than 24 hours."
                    ),
                    "case {case:?}: expected the span sentence, got {body}"
                );
                assert_eq!(
                    stored(&logs, "span", "stream").await,
                    Vec::<String>::new(),
                    "case {case:?}: a refused batch stores nothing"
                );
            }
            Expect::Accepted(kept, rejected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} {body}"
                );
                assert_eq!(
                    body.get("rejectedLogEventsInfo").unwrap_or(&Value::Null),
                    rejected,
                    "case {case:?}: expected rejectedLogEventsInfo {rejected}, got {body}"
                );
                let expected_messages: Vec<String> =
                    kept.iter().map(|message| (*message).to_string()).collect();
                assert_eq!(
                    stored(&logs, "span", "stream").await,
                    expected_messages,
                    "case {case:?}: exactly the admitted events are readable"
                );
            }
        }
    }
}

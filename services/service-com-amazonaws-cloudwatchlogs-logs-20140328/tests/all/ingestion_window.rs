// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ingestion window `PutLogEvents` admits each log event
//! against, and for everything downstream of it: `rejectedLogEventsInfo`, what
//! `GetLogEvents` serves back, the event-time summary `DescribeLogStreams` reports, and
//! the `storedBytes` `DescribeLogGroups` reports.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: an event whose timestamp falls outside the clock-relative
//! ingestion window — more than 2 hours ahead of the ingestion clock, older than 14 days,
//! or before the log group's retention period — is not stored, and the batch's remaining
//! events are processed as usual. The tables below span that class along the axes the rule
//! generalises over (how far outside the window, which side of it, how many events of the
//! batch, and the retention the group carries); the recorded case is one row.
//!
//! Every row that is not meant to sit exactly on a bound is placed [`MARGIN`] clear of it,
//! so no case is decided by how long a request took.

use serde_json::{json, Value};

use crate::harness::{group_with_stream, now_ms, Logs};

/// Milliseconds in one day.
const DAY: i64 = 24 * 60 * 60 * 1_000;

/// Milliseconds in one hour.
const HOUR: i64 = 60 * 60 * 1_000;

/// The published past bound of the ingestion window, from the `PutLogEvents` doc: "Events
/// older than 14 days or preceding the log group's retention period are rejected while
/// processing remaining valid events."
const MAX_AGE: i64 = 14 * DAY;

/// The published future bound of the ingestion window, from the `PutLogEvents` doc:
/// "Events more than 2 hours in the future are rejected while processing remaining valid
/// events."
const MAX_FUTURE: i64 = 2 * HOUR;

/// How far clear of a bound a row that is not testing the bound itself is placed. The
/// ingestion clock advances between a test computing a timestamp and the handler admitting
/// it, so a row this far clear of a bound stays on its side of it.
const MARGIN: i64 = 5_000;

/// How the service must answer one event of a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// Stored, readable, counted in the group's `storedBytes`.
    Accepted,
    /// Refused as older than the ingestion window — `tooOldLogEventEndIndex`.
    TooOld,
    /// Refused as too far ahead of the clock — `tooNewLogEventStartIndex`.
    TooNew,
}

/// Put one batch of `(timestamp, message)` pairs, returning the response.
async fn put(logs: &Logs, group: &str, stream: &str, events: &[(i64, &str)]) -> Value {
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": stream,
            "logEvents": Value::Array(
                events
                    .iter()
                    .map(|(timestamp, message)| json!({"timestamp": timestamp, "message": message}))
                    .collect(),
            ),
        }),
    )
    .await
}

/// Every `message` the stream serves back, oldest first.
async fn stored_messages(logs: &Logs, group: &str, stream: &str) -> Vec<String> {
    let page = logs
        .ok(
            "GetLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": stream,
                "startFromHead": true,
            }),
        )
        .await;
    page.get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("GetLogEvents must return events, got {page}"))
        .iter()
        .map(|event| {
            event["message"]
                .as_str()
                .unwrap_or_else(|| panic!("every event carries a message, got {event}"))
                .to_string()
        })
        .collect()
}

/// The one stream `DescribeLogStreams` reports for `group`.
async fn described_stream(logs: &Logs, group: &str) -> Value {
    let page = logs
        .ok("DescribeLogStreams", &json!({"logGroupName": group}))
        .await;
    page["logStreams"]
        .as_array()
        .and_then(|streams| streams.first())
        .cloned()
        .unwrap_or_else(|| panic!("DescribeLogStreams must report the stream, got {page}"))
}

/// The `storedBytes` `DescribeLogGroups` reports for `group`.
async fn stored_bytes(logs: &Logs, group: &str) -> i64 {
    let page = logs
        .ok("DescribeLogGroups", &json!({"logGroupIdentifiers": [group]}))
        .await;
    page["logGroups"][0]["storedBytes"]
        .as_i64()
        .unwrap_or_else(|| panic!("a log group reports storedBytes, got {page}"))
}

/// What one event of `message` adds to a log group's `storedBytes`: the UTF-8 message
/// plus the documented 26 bytes of per-event overhead.
fn billed_bytes(message: &str) -> i64 {
    i64::try_from(message.len()).expect("a small message") + 26
}

/// A single-event batch at each published bound of the window, clear of each bound on
/// either side, and well outside on each side: the event is stored and readable exactly
/// when it is inside, and `rejectedLogEventsInfo` names the side it fell outside on.
#[tokio::test]
async fn an_event_is_ingested_exactly_when_its_timestamp_is_inside_the_window() {
    // (case, offset from the ingestion clock, how the service must answer it)
    let cases: Vec<(&str, i64, Expect)> = vec![
        ("a hundred days old", -100 * DAY, Expect::TooOld),
        ("a day past the age bound", -(MAX_AGE + DAY), Expect::TooOld),
        ("just past the age bound", -(MAX_AGE + MARGIN), Expect::TooOld),
        (
            "just inside the age bound",
            -(MAX_AGE - MARGIN),
            Expect::Accepted,
        ),
        ("a day old", -DAY, Expect::Accepted),
        ("an hour old", -HOUR, Expect::Accepted),
        ("the ingestion instant itself", 0, Expect::Accepted),
        (
            "just inside the future bound",
            MAX_FUTURE - MARGIN,
            Expect::Accepted,
        ),
        // The future bound is inclusive: only events MORE than 2 hours ahead are refused.
        ("exactly at the future bound", MAX_FUTURE, Expect::Accepted),
        (
            "just past the future bound",
            MAX_FUTURE + MARGIN,
            Expect::TooNew,
        ),
        (
            "an hour past the future bound",
            MAX_FUTURE + HOUR,
            Expect::TooNew,
        ),
        ("a hundred days in the future", 100 * DAY, Expect::TooNew),
    ];

    for (case, offset, expected) in &cases {
        let logs = Logs::new();
        group_with_stream(&logs, "window", "stream", None).await;
        let timestamp = now_ms() + offset;

        let response = put(&logs, "window", "stream", &[(timestamp, "event")]).await;
        let rejected = response.get("rejectedLogEventsInfo").cloned();
        let expected_rejected = match expected {
            Expect::Accepted => None,
            Expect::TooOld => Some(json!({"tooOldLogEventEndIndex": 1})),
            Expect::TooNew => Some(json!({"tooNewLogEventStartIndex": 0})),
        };
        assert_eq!(
            rejected, expected_rejected,
            "case {case:?}: expected rejectedLogEventsInfo {expected_rejected:?}, got {response}"
        );

        let accepted = *expected == Expect::Accepted;
        let expected_messages: Vec<String> = if accepted {
            vec!["event".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            stored_messages(&logs, "window", "stream").await,
            expected_messages,
            "case {case:?}: only an accepted event is readable"
        );

        // The stream's event-time summary follows what was stored: a refused event moves
        // none of the three members.
        let stream = described_stream(&logs, "window").await;
        for member in ["firstEventTimestamp", "lastEventTimestamp"] {
            assert_eq!(
                stream[member].as_i64(),
                accepted.then_some(timestamp),
                "case {case:?}: {member} follows the stored events, got {stream}"
            );
        }
        assert_eq!(
            stream["lastIngestionTime"].as_i64().is_some(),
            accepted,
            "case {case:?}: lastIngestionTime is set only once an event was ingested, got {stream}"
        );

        assert_eq!(
            stored_bytes(&logs, "window").await,
            if accepted { billed_bytes("event") } else { 0 },
            "case {case:?}: only an accepted event is billed to the group"
        );
    }
}

/// A batch straddling the window keeps its in-window events and reports the boundary
/// indexes of the refused runs, over batches that differ in how many events fall on each
/// side of it.
#[tokio::test]
async fn a_batch_straddling_the_window_keeps_only_its_in_window_events() {
    // (case, [(offset from the ingestion clock, message, expectation)], the
    // rejectedLogEventsInfo the batch must report). Every batch is in chronological order,
    // which the operation requires of its input.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Vec<(i64, &str, Expect)>, Value)> = vec![
        (
            "two leading too-old events",
            vec![
                (-40 * DAY, "old-0", Expect::TooOld),
                (-20 * DAY, "old-1", Expect::TooOld),
                (-HOUR, "kept-0", Expect::Accepted),
                (0, "kept-1", Expect::Accepted),
            ],
            json!({"tooOldLogEventEndIndex": 2}),
        ),
        (
            "one trailing too-new event",
            vec![
                (-HOUR, "kept-0", Expect::Accepted),
                (MAX_FUTURE + DAY, "new-0", Expect::TooNew),
            ],
            json!({"tooNewLogEventStartIndex": 1}),
        ),
        (
            "too old at the head and too new at the tail",
            vec![
                (-30 * DAY, "old-0", Expect::TooOld),
                (-HOUR, "kept-0", Expect::Accepted),
                (MAX_FUTURE + HOUR, "new-0", Expect::TooNew),
                (100 * DAY, "new-1", Expect::TooNew),
            ],
            json!({"tooOldLogEventEndIndex": 1, "tooNewLogEventStartIndex": 2}),
        ),
        (
            "every event outside the window",
            vec![
                (-50 * DAY, "old-0", Expect::TooOld),
                (50 * DAY, "new-0", Expect::TooNew),
            ],
            json!({"tooOldLogEventEndIndex": 1, "tooNewLogEventStartIndex": 1}),
        ),
        (
            "nothing outside the window",
            vec![
                (-2 * HOUR, "kept-0", Expect::Accepted),
                (0, "kept-1", Expect::Accepted),
                (MAX_FUTURE, "kept-2", Expect::Accepted),
            ],
            Value::Null,
        ),
    ];

    for (case, events, expected_rejected) in &cases {
        let logs = Logs::new();
        group_with_stream(&logs, "straddle", "stream", None).await;
        let clock = now_ms();
        let batch: Vec<(i64, &str)> = events
            .iter()
            .map(|(offset, message, _)| (clock + offset, *message))
            .collect();

        let response = put(&logs, "straddle", "stream", &batch).await;
        assert_eq!(
            response.get("rejectedLogEventsInfo").unwrap_or(&Value::Null),
            expected_rejected,
            "case {case:?}: expected exactly those boundary indexes, got {response}"
        );

        let kept: Vec<String> = events
            .iter()
            .filter(|(_, _, expect)| *expect == Expect::Accepted)
            .map(|(_, message, _)| (*message).to_string())
            .collect();
        assert_eq!(
            stored_messages(&logs, "straddle", "stream").await,
            kept,
            "case {case:?}: exactly the in-window events are readable"
        );
        let expected_bytes: i64 = kept.iter().map(|message| billed_bytes(message)).sum();
        assert_eq!(
            stored_bytes(&logs, "straddle").await,
            expected_bytes,
            "case {case:?}: only the in-window events are billed to the group"
        );

        // The stream reports event times exactly when it holds events.
        let stream = described_stream(&logs, "straddle").await;
        assert_eq!(
            stream["firstEventTimestamp"].as_i64().is_some(),
            !kept.is_empty(),
            "case {case:?}: firstEventTimestamp is set only when events were stored, got {stream}"
        );
        assert_eq!(
            stream["lastIngestionTime"].as_i64().is_some(),
            !kept.is_empty(),
            "case {case:?}: lastIngestionTime is set only when events were ingested, got {stream}"
        );
    }
}

/// A log group's retention is the other half of the past bound: an event inside the
/// 14-day age bound but before the group's retention period is refused as expired, while
/// one before the age bound is too old whatever the retention is. Rows span retentions
/// from the model's published catalogue on both sides of the age bound.
#[tokio::test]
async fn an_event_preceding_the_groups_retention_is_expired() {
    // Retentions from the model's `Days` shape: "Possible values are: 1, 3, 5, 7, 14, 30,
    // 60, … and 3653." The first three are shorter than the 14-day age bound, so the
    // retention is the binding boundary; 14 and 30 are not, so the age bound binds and an
    // event before the retention period is too old rather than expired.
    let retentions: [i64; 5] = [1, 3, 7, 14, 30];

    for days in retentions {
        let logs = Logs::new();
        let group = format!("retention-{days}");
        group_with_stream(&logs, &group, "stream", Some(days)).await;
        let clock = now_ms();

        // Half way inside whichever of the two past bounds is the tighter one.
        let inside = clock - (days * DAY).min(MAX_AGE) / 2;
        let response = put(&logs, &group, "stream", &[(inside, "inside")]).await;
        assert_eq!(
            response.get("rejectedLogEventsInfo"),
            None,
            "case retention {days}: an event inside both past bounds is accepted, got {response}"
        );

        // Just before the retention period: expired when the retention is the tighter
        // bound, too old when the 14-day age bound is.
        let before_retention = clock - days * DAY - MARGIN;
        let expected = if days * DAY < MAX_AGE {
            json!({"expiredLogEventEndIndex": 1})
        } else {
            json!({"tooOldLogEventEndIndex": 1})
        };
        let response = put(&logs, &group, "stream", &[(before_retention, "before")]).await;
        assert_eq!(
            response.get("rejectedLogEventsInfo"),
            Some(&expected),
            "case retention {days}: expected {expected} for an event just older than the retention, got {response}"
        );

        // Before the age bound: too old whatever the retention is.
        let ancient = clock - MAX_AGE - DAY;
        let response = put(&logs, &group, "stream", &[(ancient, "ancient")]).await;
        assert_eq!(
            response.get("rejectedLogEventsInfo"),
            Some(&json!({"tooOldLogEventEndIndex": 1})),
            "case retention {days}: an event before the age bound is too old, got {response}"
        );

        assert_eq!(
            stored_messages(&logs, &group, "stream").await,
            vec!["inside".to_string()],
            "case retention {days}: only the admitted event is readable"
        );
        assert_eq!(
            stored_bytes(&logs, &group).await,
            billed_bytes("inside"),
            "case retention {days}: only the admitted event is billed to the group"
        );
    }
}

/// A stream whose every submitted event was refused is indistinguishable from one nothing
/// was ever written to, and a later in-window batch lands on it normally.
#[tokio::test]
async fn a_stream_whose_events_were_all_refused_reports_no_event_times() {
    let logs = Logs::new();
    group_with_stream(&logs, "out-of-window", "stream", None).await;
    let clock = now_ms();

    let response = put(
        &logs,
        "out-of-window",
        "stream",
        &[
            (clock - 20 * 365 * DAY, "out-of-window event: too old"),
            (clock + 20 * 365 * DAY, "too new"),
        ],
    )
    .await;
    assert_eq!(
        response.get("rejectedLogEventsInfo"),
        Some(&json!({"tooOldLogEventEndIndex": 1, "tooNewLogEventStartIndex": 1})),
        "expected both boundary indexes, got {response}"
    );
    assert!(
        response
            .get("nextSequenceToken")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.is_empty()),
        "the call is still accepted and still mints a sequence token, got {response}"
    );

    assert_eq!(
        stored_messages(&logs, "out-of-window", "stream").await,
        Vec::<String>::new(),
        "a refused event is never served back"
    );
    let stream = described_stream(&logs, "out-of-window").await;
    for member in [
        "firstEventTimestamp",
        "lastEventTimestamp",
        "lastIngestionTime",
    ] {
        assert_eq!(
            stream.get(member),
            None,
            "expected {member} to be absent on a stream that stored nothing, got {stream}"
        );
    }
    assert_eq!(
        stored_bytes(&logs, "out-of-window").await,
        0,
        "nothing was stored, so nothing is billed"
    );

    // The stream is still usable: an in-window batch lands and is reported clean.
    let response = put(&logs, "out-of-window", "stream", &[(clock, "in window")]).await;
    assert_eq!(
        response.get("rejectedLogEventsInfo"),
        None,
        "an in-window batch after a fully refused one reports nothing rejected, got {response}"
    );
    assert_eq!(
        stored_messages(&logs, "out-of-window", "stream").await,
        vec!["in window".to_string()],
        "the in-window batch is readable"
    );
    let stream = described_stream(&logs, "out-of-window").await;
    assert_eq!(
        stream["firstEventTimestamp"].as_i64(),
        Some(clock),
        "the summary now follows the stored event, got {stream}"
    );
    assert!(
        stream["lastIngestionTime"].as_i64().is_some(),
        "the summary now carries an ingestion time, got {stream}"
    );
    assert_eq!(
        stored_bytes(&logs, "out-of-window").await,
        billed_bytes("in window"),
        "only the stored event is billed"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! Which stored events a `GetLogEvents` time window reports when an event's own timestamp
//! and the instant it was ingested at fall in different windows.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: a `GetLogEvents` window selects on each event's own stored
//! `timestamp` — the one the writing client stamped it with
//! (`GetLogEventsRequest$startTime`: "Events with a timestamp equal to this time or later
//! than this time are included") — and on nothing else, so neither `ingestionTime` (the
//! clock reading the write took) nor the clock the read happens on moves an event into or
//! out of a window.
//!
//! Every bound below is derived from the timestamps and ingestion times the SERVICE
//! reports, never from a clock reading taken beside the request: a second reading of a
//! wall clock is a different instant, so a bound taken from one sits a few milliseconds
//! from where the case means it and the page is then decided by how long a request took.
//! Only the WRITE reads the clock, and every event it writes is placed an hour or more
//! clear of the ingestion instant.
//!
//! The tables span what the rule generalises over: how old a stored timestamp is, which of
//! an event's two instants each bound is taken from, which side of that pair the window
//! sits on, and whether the event was stamped behind or ahead of its own ingestion.
//!
//! What this suite owns, beyond the window tables of `tests/all/event_window_selection.rs`
//! (which pin the inclusive and exclusive bounds, and that the same window reports the same
//! page again): the INGESTION-INSTANT dimension — that a window is read against an event's
//! own `timestamp` and never against the `ingestionTime` beside it, in both directions, for
//! an event stamped behind its ingestion and for one stamped ahead of it.

use serde_json::{json, Value};

use crate::harness::{batch, group_with_stream, messages, now_ms, Logs};

/// Milliseconds in one hour.
const HOUR_MS: i64 = 60 * 60 * 1_000;

/// Milliseconds in one day.
const DAY_MS: i64 = 24 * HOUR_MS;

/// The one stream every group in this suite holds its events in.
const STREAM: &str = "stream";

/// The `(timestamp, ingestionTime, message)` of every event the stream serves over an
/// unbounded window, oldest first: what the SERVICE says it stored, which every bound in
/// this suite is derived from.
async fn stored_events(logs: &Logs, case: &str, group: &str) -> Vec<(i64, i64, String)> {
    let page = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": group, "logStreamName": STREAM, "startFromHead": true}),
        )
        .await;
    page.get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: GetLogEvents must return events, got {page}"))
        .iter()
        .map(|event| {
            let number = |member: &str| {
                event.get(member).and_then(Value::as_i64).unwrap_or_else(|| {
                    panic!("case {case:?}: every event carries {member}, got {event}")
                })
            };
            let message = event
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every event carries a message, got {event}")
                })
                .to_string();
            (number("timestamp"), number("ingestionTime"), message)
        })
        .collect()
}

/// A `GetLogEvents` request over this suite's stream, with the ABSOLUTE bounds the case
/// names: the sibling suite's builder takes its bounds as offsets from the base it stamped
/// its events with, where every bound here comes from what the service reported.
fn window(group: &str, start: Option<i64>, end: Option<i64>) -> Value {
    let mut request = json!({
        "logGroupName": group,
        "logStreamName": STREAM,
        "startFromHead": true,
    });
    if let Some(start) = start {
        request["startTime"] = json!(start);
    }
    if let Some(end) = end {
        request["endTime"] = json!(end);
    }
    request
}

/// One row of a window table: its name, the bounds the request carries, and the indexes of
/// the written batch the window must report.
type WindowRow<'a> = (&'a str, Option<i64>, Option<i64>, Vec<usize>);

/// Ask `request`, assert the page is exactly `expected`, and — for a window the store
/// holds nothing in — assert that handing the page's own forward cursor back reports that
/// same empty page rather than a refusal or a different selection.
async fn assert_window(logs: &Logs, case: &str, request: &Value, expected: &[String]) {
    let page = logs.ok("GetLogEvents", request).await;
    let reported = messages(&page);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected exactly the events the window holds, got {page}"
    );
    if expected.is_empty() {
        let mut again_request = request.clone();
        again_request["nextToken"] = page["nextForwardToken"].clone();
        let again = logs.ok("GetLogEvents", &again_request).await;
        let reported_again = messages(&again);
        assert_eq!(
            reported_again, expected,
            "case {case:?}: the cursor of an empty window pages to an empty page, got {again}"
        );
    }
}

/// An event stamped in the past is ingested later than its own timestamp, and only its own
/// timestamp decides the window: a window covering every ingestion instant holds nothing,
/// while one covering the stored timestamps and ending before every ingestion instant
/// holds them all.
#[tokio::test]
async fn a_window_selects_on_the_stored_timestamp_and_never_on_the_ingestion_time() {
    const GROUP: &str = "timestamp-not-ingestion";
    let logs = Logs::new();
    group_with_stream(&logs, GROUP, STREAM, None).await;

    // Ages from the past half of `PutLogEvents`' ingestion window ("Events older than 14
    // days or preceding the log group's retention period are rejected"), each an hour or
    // more clear of the ingestion instant, in the chronological order a batch is written
    // in. The clock is read ONCE, for the write, and no bound below comes from it.
    let ages: [(&str, i64); 3] = [
        ("thirteen days old", 13 * DAY_MS),
        ("a day old", DAY_MS),
        ("an hour old", HOUR_MS),
    ];
    let ingested_at = now_ms();
    let written: Vec<(i64, String)> = ages
        .iter()
        .map(|(label, age)| (ingested_at - age, format!("an event {label}")))
        .collect();
    // ONE event per request: the span these ages cover is wider than the 24 hours
    // `PutLogEvents` admits between one batch's oldest and newest in-window event, and a
    // batch that breaks that is refused whole. Writing them separately keeps every timestamp
    // and ingestion instant this suite is about and is what a client stamping events this
    // far apart does.
    for (timestamp, message) in &written {
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": GROUP,
                "logStreamName": STREAM,
                "logEvents": batch(&[(*timestamp, message.as_str())]),
            }),
        )
        .await;
    }

    let stored = stored_events(&logs, "the stored events", GROUP).await;
    let reported: Vec<(i64, String)> = stored
        .iter()
        .map(|(timestamp, _, message)| (*timestamp, message.clone()))
        .collect();
    assert_eq!(
        reported, written,
        "case \"the stored events\": an event keeps the timestamp its batch carried, got {reported:?}"
    );
    for (timestamp, ingestion, message) in &stored {
        assert!(
            ingestion > timestamp,
            "case {message:?}: an event stamped in the past carries an ingestionTime later than its own timestamp, got timestamp {timestamp} and ingestionTime {ingestion}"
        );
    }
    let newest_timestamp = stored
        .iter()
        .map(|(timestamp, _, _)| *timestamp)
        .max()
        .expect("the batch stored events");
    let earliest_ingestion = stored
        .iter()
        .map(|(_, ingestion, _)| *ingestion)
        .min()
        .expect("the batch stored events");
    assert!(
        earliest_ingestion > newest_timestamp,
        "case \"the two instants\": every ingestion instant is later than every stored timestamp, got ingestionTime {earliest_ingestion} and timestamp {newest_timestamp}"
    );

    let oldest_timestamp = stored
        .iter()
        .map(|(timestamp, _, _)| *timestamp)
        .min()
        .expect("the batch stored events");
    let latest_ingestion = stored
        .iter()
        .map(|(_, ingestion, _)| *ingestion)
        .max()
        .expect("the batch stored events");
    let middle_timestamp = stored[1].0;
    let every_message: Vec<usize> = (0..written.len()).collect();

    // (case, startTime, endTime, the indexes of the written batch the window holds)
    let rows: Vec<WindowRow<'_>> = vec![
        (
            "a window covering every ingestion instant and no stored timestamp",
            Some(earliest_ingestion),
            Some(latest_ingestion + 1),
            vec![],
        ),
        (
            "a window from the earliest ingestion instant onwards",
            Some(earliest_ingestion),
            None,
            vec![],
        ),
        (
            "a window covering every stored timestamp",
            Some(oldest_timestamp),
            Some(newest_timestamp + 1),
            every_message.clone(),
        ),
        (
            "a window over the stored timestamps, ending before every ingestion instant",
            Some(oldest_timestamp),
            Some(earliest_ingestion),
            every_message.clone(),
        ),
        (
            "a window ending on the oldest stored timestamp",
            None,
            Some(oldest_timestamp),
            vec![],
        ),
        (
            "a window holding one stored timestamp",
            Some(middle_timestamp),
            Some(middle_timestamp + 1),
            vec![1],
        ),
        (
            "a window from the newest stored timestamp to the latest ingestion instant",
            Some(newest_timestamp),
            Some(latest_ingestion),
            vec![2],
        ),
    ];

    for (case, start, end, indexes) in &rows {
        let expected: Vec<String> = indexes
            .iter()
            .map(|index| written[*index].1.clone())
            .collect();
        assert_window(&logs, case, &window(GROUP, *start, *end), &expected).await;
    }
}

/// The same rule the other way round: an event stamped AHEAD of the clock that ingested it
/// is reported by a window on its own timestamp and by no window on its ingestion instant.
#[tokio::test]
async fn an_event_stamped_ahead_of_its_ingestion_is_selected_by_its_own_timestamp() {
    const GROUP: &str = "stamped-ahead";
    const MESSAGE: &str = "an event stamped an hour ahead";
    let logs = Logs::new();
    group_with_stream(&logs, GROUP, STREAM, None).await;

    // Inside the future half of `PutLogEvents`' ingestion window ("Events more than 2
    // hours in the future are rejected"), an hour clear of that bound.
    let stamped = now_ms() + HOUR_MS;
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": GROUP,
            "logStreamName": STREAM,
            "logEvents": batch(&[(stamped, MESSAGE)]),
        }),
    )
    .await;

    let stored = stored_events(&logs, "the event stamped ahead", GROUP).await;
    assert_eq!(
        stored.len(),
        1,
        "case \"the event stamped ahead\": the batch stored one event, got {stored:?}"
    );
    let (timestamp, ingestion, _) = stored[0];
    assert_eq!(
        timestamp, stamped,
        "case \"the event stamped ahead\": the event keeps the timestamp its batch carried, got {timestamp}"
    );
    assert!(
        ingestion < timestamp,
        "case \"the event stamped ahead\": its ingestion instant precedes the timestamp the client stamped, got ingestionTime {ingestion} and timestamp {timestamp}"
    );

    // (case, startTime, endTime, does the window report the event?)
    let rows: Vec<(&str, Option<i64>, Option<i64>, bool)> = vec![
        (
            "a window on the event's own timestamp",
            Some(timestamp),
            Some(timestamp + 1),
            true,
        ),
        (
            "a window from the event's own timestamp onwards",
            Some(timestamp),
            None,
            true,
        ),
        (
            "a window on the ingestion instant",
            Some(ingestion),
            Some(ingestion + 1),
            false,
        ),
        (
            "a window from the ingestion instant up to the timestamp",
            Some(ingestion),
            Some(timestamp),
            false,
        ),
        (
            "a window ending just after the ingestion instant",
            None,
            Some(ingestion + 1),
            false,
        ),
        (
            "a window one millisecond past the timestamp",
            Some(timestamp + 1),
            None,
            false,
        ),
    ];

    for (case, start, end, holds) in &rows {
        let expected: Vec<String> = if *holds {
            vec![MESSAGE.to_string()]
        } else {
            Vec::new()
        };
        assert_window(&logs, case, &window(GROUP, *start, *end), &expected).await;
    }
}

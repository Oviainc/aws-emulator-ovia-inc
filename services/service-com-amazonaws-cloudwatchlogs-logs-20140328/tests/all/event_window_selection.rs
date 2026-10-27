// SPDX-License-Identifier: CC0-1.0

//! Which stored events a `GetLogEvents` time window selects.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: an event belongs to the window iff its own stored timestamp
//! satisfies `startTime <= timestamp < endTime` (`GetLogEventsRequest$startTime`:
//! "Events with a timestamp equal to this time or later than this time are included";
//! `GetLogEventsRequest$endTime`: "Events with a timestamp equal to or later than this
//! time are not included"), with an omitted bound meaning "unbounded that way". The
//! comparison is against what the store holds, so the answer does not depend on when the
//! read happens: the same window asked again reports the same page, and a bound placed
//! outside every stored timestamp selects nothing however near the edge it sits.
//!
//! The tables span what the rule generalises over: which bounds are present, where each
//! bound falls relative to a timestamp (below, exactly on, above), and how far outside the
//! stored events an excluding bound sits — one millisecond to one day.
//!
//! What this suite owns, beyond the window table of `tests/all/log_core_loop.rs` (which pins
//! the inclusive/exclusive bounds over one regularly spaced stream): that the answer does not
//! depend on WHEN the window is read — the same request served again after further requests
//! reports the identical page — that a bound can separate two adjacent milliseconds, and that
//! an excluding bound holds nothing at any distance outside the stored events, a single
//! millisecond included.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, group_with_stream, Logs};

/// The offsets, in milliseconds from the base, the suite's events are stamped at:
/// irregularly spaced, with two adjacent milliseconds among them, so a bound can fall
/// between two events as well as on one.
const OFFSETS: [i64; 5] = [0, 1_000, 2_500, 2_501, 7_000];

/// A group with one stream holding one event per entry of [`OFFSETS`], named `event-<i>`,
/// returned with the base those offsets are taken from. A window a case means to place
/// relative to those events is derived from THIS base: a second clock reading is a
/// different instant, and the window is decided on the stored timestamps.
async fn stream_at_offsets(logs: &Logs, group: &str) -> i64 {
    group_with_stream(logs, group, "stream", None).await;
    let base = base_ms();
    let messages: Vec<String> = (0..OFFSETS.len())
        .map(|index| format!("event-{index}"))
        .collect();
    let events: Vec<(i64, &str)> = OFFSETS
        .iter()
        .zip(messages.iter())
        .map(|(offset, message)| (base + offset, message.as_str()))
        .collect();
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": "stream",
            "logEvents": batch(&events),
        }),
    )
    .await;
    base
}

/// The `(timestamp, message)` pairs of a `GetLogEvents` page, in the order returned.
fn page_events(case: &str, page: &Value) -> Vec<(i64, String)> {
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
            (number("timestamp"), message)
        })
        .collect()
}

/// The `(timestamp, message)` pairs the events at `indices` of [`OFFSETS`] must report.
fn expected_events(base: i64, indices: &[usize]) -> Vec<(i64, String)> {
    indices
        .iter()
        .map(|index| (base + OFFSETS[*index], format!("event-{index}")))
        .collect()
}

/// A `GetLogEvents` request over the suite's stream, with the bounds the case gives as
/// offsets from the base.
fn window_request(group: &str, base: i64, start: Option<i64>, end: Option<i64>) -> Value {
    let mut request = json!({
        "logGroupName": group,
        "logStreamName": "stream",
        "startFromHead": true,
    });
    if let Some(start) = start {
        request["startTime"] = json!(base + start);
    }
    if let Some(end) = end {
        request["endTime"] = json!(base + end);
    }
    request
}

/// One row of the window table: its name, the bounds as offsets from the base, and the
/// indices of [`OFFSETS`] the window holds.
type WindowCase<'a> = (&'a str, Option<i64>, Option<i64>, Vec<usize>);

/// The window selects exactly the events whose stored timestamp satisfies it — each bound
/// tried below a timestamp, exactly on one, and above one — and asking the same window
/// again, after further requests have moved the clock on, reports the very same page.
#[tokio::test]
async fn a_window_selects_the_events_its_bounds_hold_and_reports_them_again_unchanged() {
    let logs = Logs::new();
    let base = stream_at_offsets(&logs, "window-rows").await;
    let all: Vec<usize> = (0..OFFSETS.len()).collect();

    let rows: Vec<WindowCase<'_>> = vec![
        ("no bound at all", None, None, all.clone()),
        ("a start on the first timestamp", Some(0), None, all.clone()),
        (
            "a start one millisecond past the first timestamp",
            Some(1),
            None,
            vec![1, 2, 3, 4],
        ),
        ("an end on the first timestamp", None, Some(0), vec![]),
        (
            "an end one millisecond past the first timestamp",
            None,
            Some(1),
            vec![0],
        ),
        ("an end between two timestamps", None, Some(2_000), vec![0, 1]),
        (
            "both bounds on a timestamp",
            Some(1_000),
            Some(2_501),
            vec![1, 2],
        ),
        (
            "a window holding one of two adjacent milliseconds",
            Some(2_500),
            Some(2_501),
            vec![2],
        ),
        (
            "a window whose bounds are equal",
            Some(2_501),
            Some(2_501),
            vec![],
        ),
        ("a start on the last timestamp", Some(7_000), None, vec![4]),
        (
            "a start one millisecond past the last timestamp",
            Some(7_001),
            None,
            vec![],
        ),
        (
            "a window a day either side of every event",
            Some(-86_400_000),
            Some(86_400_000),
            all.clone(),
        ),
    ];

    for (case, start, end, indices) in &rows {
        let request = window_request("window-rows", base, *start, *end);
        let page = logs.ok("GetLogEvents", &request).await;
        let reported = page_events(case, &page);
        assert_eq!(
            reported,
            expected_events(base, indices),
            "case {case:?}: expected exactly the events the window holds, got {reported:?}"
        );

        // The same window, asked again once the clock has moved on and other requests
        // have been served: the page is a function of the store and the window only.
        logs.ok("DescribeLogStreams", &json!({"logGroupName": "window-rows"}))
            .await;
        let again = logs.ok("GetLogEvents", &request).await;
        assert_eq!(
            again, page,
            "case {case:?}: the same window reports the same page, got {again}"
        );
    }
}

/// A bound that places the window outside every stored timestamp selects nothing, at every
/// distance from the edge — one millisecond included, which is all a read taken a moment
/// later than the write would be off by. The stream still serves its events over an
/// unbounded window, so the empty pages come from the bounds.
#[tokio::test]
async fn a_bound_outside_every_stored_timestamp_holds_nothing_at_any_distance() {
    let logs = Logs::new();
    let base = stream_at_offsets(&logs, "outside").await;
    let first = OFFSETS[0];
    let last = OFFSETS[OFFSETS.len() - 1];

    // One millisecond, one second, one minute, one hour, one day.
    for distance in [1_i64, 1_000, 60_000, 3_600_000, 86_400_000] {
        let rows: Vec<WindowCase<'_>> = vec![
            (
                "a window ending before the first event",
                Some(first - 10 * distance),
                Some(first - distance),
                vec![],
            ),
            (
                "a window ending before the first event, open at the start",
                None,
                Some(first - distance),
                vec![],
            ),
            (
                "a window starting after the last event",
                Some(last + distance),
                None,
                vec![],
            ),
            (
                "a window starting after the last event, closed at the end",
                Some(last + distance),
                Some(last + 10 * distance),
                vec![],
            ),
        ];
        for (row, start, end, indices) in &rows {
            let case = format!("{row}, {distance} ms outside");
            let page = logs
                .ok(
                    "GetLogEvents",
                    &window_request("outside", base, *start, *end),
                )
                .await;
            let reported = page_events(&case, &page);
            assert_eq!(
                reported,
                expected_events(base, indices),
                "case {case:?}: expected no events, got {reported:?}"
            );
        }
    }

    let whole = logs
        .ok(
            "GetLogEvents",
            &window_request("outside", base, None, None),
        )
        .await;
    let reported = page_events("the unbounded window", &whole);
    assert_eq!(
        reported,
        expected_events(base, &(0..OFFSETS.len()).collect::<Vec<usize>>()),
        "case \"the unbounded window\": expected every stored event, got {reported:?}"
    );
}

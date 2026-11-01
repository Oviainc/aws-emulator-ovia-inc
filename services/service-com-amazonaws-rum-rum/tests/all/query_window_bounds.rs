// SPDX-License-Identifier: CC0-1.0

//! The TIME WINDOW `GetAppMonitorData` answers a query against.
//!
//! The rule under test: a bound the caller SENT selects events at exactly the instants it
//! names, and the "If you omit this, the time range extends to the time that this
//! operation is performed" reading of `TimeRange$Before` belongs to the member's modelled
//! `@default` alone — the value the deserializer fills in for a request that left the
//! member out. `QueryTimestamp` is a `long` with no `@range`
//! (`smithy_query(query="shape", shape="TimeRange,QueryTimestamp")`), so an instant before
//! the epoch is a bound a client may send, and a window that covers no stored event
//! reports none rather than being widened to now.
//!
//! The rows span what that rule generalises over — the value of each bound and whether
//! `Before` was sent at all: windows wholly before the epoch (one of them covering
//! nothing), a window of a single pre-epoch instant, an inverted one, windows spanning the
//! epoch and reaching into the future, a `Before` of the modelled default, and an omitted
//! `Before` from two different starts.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, create, emulator};

/// The monitor every case below queries.
const MONITOR: &str = "window-monitor";

/// `2024-01-01T00:00:00Z` in seconds since the epoch: an ordinary instant in the past.
const PAST: i64 = 1_704_067_200;

/// `9999-12-31T23:59:59Z` in seconds since the epoch: the last instant RFC 3339 can spell,
/// and an instant no clock this emulator reads has reached.
const FUTURE: i64 = 253_402_300_799;

/// One second before `0001-01-01T00:00:00Z`, the first instant RFC 3339 can spell: an
/// instant the stored document therefore has to carry as a number rather than as a
/// rendering, and a bound the window still has to place it by.
const UNRENDERABLE: i64 = -62_135_596_801;

/// The instants the fixture events occupy, in the order they are delivered (which is the
/// order a query reports them in: oldest first).
///
/// Each event's index in this list, 1-based, is what a row's expectation names.
fn instants() -> Vec<i64> {
    vec![UNRENDERABLE, -300, -200, -100, 0, PAST, FUTURE]
}

/// The `id` of the fixture event at `index`: 36 characters matching `RumEvent$id`'s
/// pattern `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`.
fn id_of(index: usize) -> String {
    format!("{index:08x}-1234-4234-8234-123456789012")
}

/// One monitor holding one event at each of [`instants`], ready to query.
async fn seeded() -> MultiServiceRouter {
    let (router, _engine) = emulator();
    let monitor_id = create(
        &router,
        "seed",
        json!({"Name": MONITOR, "Domain": "window.example.com"}),
    )
    .await;
    let events: Vec<Value> = instants()
        .into_iter()
        .enumerate()
        .map(|(position, instant)| {
            json!({
                "id": id_of(position + 1),
                "timestamp": instant,
                "type": "com.amazon.rum.page_view_event",
                "details": "{\"pageId\":\"/home\"}",
            })
        })
        .collect();
    let reply = call(
        &router,
        "POST",
        &format!("/appmonitors/{monitor_id}/"),
        Some(json!({
            "BatchId": "11111111-2222-4333-8444-555555555555",
            "AppMonitorDetails": {"name": MONITOR, "id": monitor_id, "version": "1.0.0"},
            "UserDetails": {
                "userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "sessionId": "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb",
            },
            "RumEvents": events,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"seed\": expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    router
}

/// The `id` of each event `GetAppMonitorData` reports for the window `[after, before]`,
/// in the order it reported them. `before` of `None` leaves the member out, which is what
/// its modelled default stands for.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    after: i64,
    before: Option<i64>,
) -> Vec<String> {
    let mut time_range = json!({"After": after});
    if let Some(before) = before {
        time_range["Before"] = json!(before);
    }
    let reply = call(
        router,
        "POST",
        &format!("/appmonitor/{MONITOR}/data"),
        Some(json!({"TimeRange": time_range})),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetAppMonitorData to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);
    answered["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Events array, got {answered}"))
        .iter()
        .map(|entry| {
            let text = entry.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected each event to be a JSON string, got {entry}")
            });
            let document: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: expected each event to carry a JSON document, got \
                     {text:?} ({error})"
                )
            });
            document["id"].as_str().unwrap_or_default().to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_window_the_request_names_is_the_window_it_is_answered_against() {
    let router = seeded().await;

    // (case, After, Before, the 1-based fixture indices the window covers)
    let table: Vec<(&str, i64, Option<i64>, Vec<usize>)> = vec![
        ("a window wholly before the epoch", -300, Some(-100), vec![2, 3, 4]),
        // The window the issue's request named: it covers no stored event, so the query
        // reports none instead of running on to now.
        ("a negative window covering no event", -250, Some(-210), vec![]),
        ("one pre-epoch instant", -200, Some(-200), vec![3]),
        ("an inverted negative window", -100, Some(-300), vec![]),
        (
            "a negative window up to the second before the epoch",
            -300,
            Some(-1),
            vec![2, 3, 4],
        ),
        (
            "one unrenderable pre-epoch instant",
            UNRENDERABLE,
            Some(UNRENDERABLE),
            vec![1],
        ),
        (
            "the whole pre-epoch span",
            UNRENDERABLE,
            Some(-1),
            vec![1, 2, 3, 4],
        ),
        ("a window spanning the epoch", -100, Some(PAST), vec![4, 5, 6]),
        ("a window reaching into the future", PAST, Some(FUTURE), vec![6, 7]),
        // `Before` at its modelled `@default` is what an omitted member carries, so these
        // two rows are the same request: the window ends NOW, which is after every
        // fixture instant but the future one.
        (
            "a Before of the modelled default",
            -300,
            Some(0),
            vec![2, 3, 4, 5, 6],
        ),
        ("an omitted Before", -300, None, vec![2, 3, 4, 5, 6]),
        ("an omitted Before from the epoch", 0, None, vec![5, 6]),
    ];

    for (case, after, before, expected) in table {
        let ids = reported(&router, case, after, before).await;
        assert_eq!(
            ids,
            expected.iter().copied().map(id_of).collect::<Vec<_>>(),
            "case {case:?}: expected the events the window [{after}, {before:?}] covers, \
             got {ids:?}"
        );
    }
}

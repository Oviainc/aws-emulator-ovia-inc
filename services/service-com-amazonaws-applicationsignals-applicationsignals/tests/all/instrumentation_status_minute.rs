// SPDX-License-Identifier: CC0-1.0

//! The minute a reported instrumentation status event is held at, over HTTP.
//!
//! `InstrumentationStatusEvent.Time` is "The time when the status was reported, rounded to the
//! nearest minute" (`smithy_query(query="shape",shape="InstrumentationStatusEvent")`), so a
//! reported event is held at the whole minute NEAREST the instant its report named. That has
//! three observable consequences, one test each:
//!
//! * the `Time` a read reports is that whole minute, never the instant the report carried;
//! * two reports of ONE status whose instants fall in one minute are one event, while reports
//!   in neighbouring minutes stay two;
//! * a read's window selects an event by the minute it is held at, since that is the only
//!   instant the event has.
//!
//! The harness is the one `instrumentation_configuration_status` drives the same two
//! operations through, so these suites agree about the wire surface.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_status::{
    create_configuration, create_request, event_times, events_of, get_status, nearest_minute,
    now_minute, report, report_entry, router, status_request, Query, BREAKPOINT, PROBE,
};

const SERVICE: &str = "payments";
const ENVIRONMENT: &str = "eks:cluster-prod/payments";

/// Create one configuration on a fresh router and answer both, so each row of a table below
/// starts from a store of its own.
async fn configuration(kind: &str) -> (MultiServiceRouter, String) {
    let router = router();
    let hash =
        create_configuration(&router, &create_request(SERVICE, ENVIRONMENT, kind, "src/Pay.java"))
            .await;
    (router, hash)
}

/// Report `status` for `hash` at `time`, asserting the report was processed.
async fn report_at(
    router: &MultiServiceRouter,
    kind: &str,
    hash: &str,
    status: &str,
    time: f64,
    cause: Option<&str>,
    case: &str,
) {
    let (code, body) = report(
        router,
        &json!({
            "Service": SERVICE,
            "Environment": ENVIRONMENT,
            "Configurations": [report_entry(kind, hash, status, time, cause)],
        }),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected HTTP 200 from the report, got {code} with {body}"
    );
    assert_eq!(
        body["UnprocessedStatusEvents"],
        json!([]),
        "case {case:?}: expected the report of a stored configuration to be processed, got {body}"
    );
}

/// Read the events of `status` for `hash`, over `window` when one is named.
async fn read(
    router: &MultiServiceRouter,
    kind: &str,
    hash: &str,
    status: &str,
    window: Option<(f64, f64)>,
    case: &str,
) -> Value {
    let (code, body) = get_status(
        router,
        &status_request(
            SERVICE,
            ENVIRONMENT,
            kind,
            json!({ "LocationHash": hash }),
            &Query {
                status: Some(status),
                window,
                ..Query::default()
            },
        ),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected HTTP 200 from the read, got {code} with {body}"
    );
    body
}

/// The `Time` a read reports is the whole minute nearest the instant the report named — for
/// every part of a minute, both instrumentation types and a report carrying a subsecond part.
#[tokio::test]
async fn an_event_is_reported_at_the_minute_nearest_its_report() {
    // The base is a minute boundary, so each row's offset is its position INSIDE a minute and
    // the minute it belongs to is the base (offset under half a minute) or the next one. The
    // expected offset is written out rather than computed, so the table states the rounding
    // instead of restating the code.
    let base = now_minute() - 300.0;
    let rows: [(&str, f64, f64); 10] = [
        ("the minute boundary itself", 0.0, 0.0),
        ("one second past the boundary", 1.0, 0.0),
        ("a subsecond part only", 0.25, 0.0),
        ("just under half a minute", 29.0, 0.0),
        ("a subsecond part just under half a minute", 29.75, 0.0),
        ("exactly half a minute, which ties to the later minute", 30.0, 60.0),
        ("just over half a minute", 31.0, 60.0),
        ("well past half a minute", 37.0, 60.0),
        ("late in the minute", 51.0, 60.0),
        ("the last second of the minute", 59.5, 60.0),
    ];

    for kind in [BREAKPOINT, PROBE] {
        for (why, offset, expected_offset) in rows {
            let case = format!("{kind} {why} (+{offset}s)");
            let (router, hash) = configuration(kind).await;
            report_at(&router, kind, &hash, "ACTIVE", base + offset, None, &case).await;

            let body = read(&router, kind, &hash, "ACTIVE", None, &case).await;
            let times = event_times(&body);
            assert_eq!(
                times,
                vec![base + expected_offset],
                "case {case:?}: expected the event at the nearest minute, got {times:?}"
            );
            assert_eq!(
                times.first().map(|time| time % 60.0),
                Some(0.0),
                "case {case:?}: expected a whole minute, got {times:?}"
            );
            // The table and the helper the other suite reads its expectations through must
            // agree about what the nearest minute of this row is.
            assert_eq!(
                nearest_minute(base + offset),
                base + expected_offset,
                "case {case:?}: expected the nearest minute of {} to be {}",
                base + offset,
                base + expected_offset
            );
        }
    }
}

/// Reports of one status whose instants fall in ONE minute are one event, whatever their
/// number, order or batching — and reports in neighbouring minutes stay two events.
#[tokio::test]
async fn reports_inside_one_minute_are_one_event() {
    let base = now_minute() - 600.0;

    // Each row is a set of instants reported for one status, and the minutes the read must
    // report. The rows differ in how many reports there are, which part of the minute they
    // name, whether they arrive in one batch or in several, and whether they share a minute.
    struct Row {
        why: &'static str,
        offsets: Vec<f64>,
        batched: bool,
        expected_offsets: Vec<f64>,
    }
    let rows = vec![
        Row {
            why: "two reports in the second half of one minute",
            offsets: vec![37.0, 51.0],
            batched: false,
            expected_offsets: vec![60.0],
        },
        Row {
            why: "two reports in the first half of one minute",
            offsets: vec![5.0, 20.0],
            batched: false,
            expected_offsets: vec![0.0],
        },
        Row {
            why: "four reports across one minute's first half, in one batch",
            offsets: vec![0.0, 7.0, 13.5, 29.0],
            batched: true,
            expected_offsets: vec![0.0],
        },
        Row {
            why: "reports arriving newest first",
            offsets: vec![51.0, 37.0],
            batched: false,
            expected_offsets: vec![60.0],
        },
        Row {
            why: "two reports either side of half a minute, which are two minutes",
            offsets: vec![20.0, 40.0],
            batched: false,
            expected_offsets: vec![60.0, 0.0],
        },
        Row {
            why: "one report per minute over three minutes",
            offsets: vec![5.0, 65.0, 125.0],
            batched: true,
            expected_offsets: vec![120.0, 60.0, 0.0],
        },
        Row {
            why: "six reports collapsing onto two minutes",
            offsets: vec![1.0, 29.0, 31.0, 45.0, 59.0, 2.0],
            batched: true,
            expected_offsets: vec![60.0, 0.0],
        },
    ];

    for Row {
        why,
        offsets,
        batched,
        expected_offsets,
    } in rows
    {
        let (router, hash) = configuration(PROBE).await;
        if batched {
            let (code, body) = report(
                &router,
                &json!({
                    "Service": SERVICE,
                    "Environment": ENVIRONMENT,
                    "Configurations": offsets
                        .iter()
                        .map(|offset| report_entry(PROBE, &hash, "READY", base + offset, None))
                        .collect::<Vec<Value>>(),
                }),
            )
            .await;
            assert_eq!(
                code, 200,
                "case {why:?}: expected HTTP 200 from the batch, got {code} with {body}"
            );
        } else {
            for offset in &offsets {
                report_at(&router, PROBE, &hash, "READY", base + offset, None, why).await;
            }
        }

        let body = read(&router, PROBE, &hash, "READY", None, why).await;
        let expected: Vec<f64> = expected_offsets
            .iter()
            .map(|offset| base + offset)
            .collect();
        assert_eq!(
            event_times(&body),
            expected,
            "case {why:?}: expected {expected:?}, got {:?}",
            body["Events"]
        );
    }
}

/// The last report of one status inside a minute is the event that minute holds: a later
/// report's `ErrorCause` replaces the one recorded rather than adding an event, while another
/// status reported in the same minute is an event of its own.
#[tokio::test]
async fn the_last_report_of_a_minute_is_the_event_it_holds() {
    let base = now_minute() - 180.0;
    let (router, hash) = configuration(BREAKPOINT).await;

    for (offset, cause) in [(32.0, "FILE_NOT_FOUND"), (44.0, "METHOD_NOT_FOUND")] {
        report_at(
            &router,
            BREAKPOINT,
            &hash,
            "ERROR",
            base + offset,
            Some(cause),
            "an error reported twice inside one minute",
        )
        .await;
    }
    // Another status in that same minute is a separate event: a minute holds one event PER
    // status.
    report_at(
        &router,
        BREAKPOINT,
        &hash,
        "ACTIVE",
        base + 38.0,
        None,
        "another status inside that minute",
    )
    .await;

    let body = read(
        &router,
        BREAKPOINT,
        &hash,
        "ERROR",
        None,
        "the error chronology",
    )
    .await;
    assert_eq!(
        events_of(&body),
        vec![(base + 60.0, Some("METHOD_NOT_FOUND".to_string()))],
        "expected the minute to hold the last cause reported for it, got {:?}",
        body["Events"]
    );

    let body = read(
        &router,
        BREAKPOINT,
        &hash,
        "ACTIVE",
        None,
        "the active chronology",
    )
    .await;
    assert_eq!(
        events_of(&body),
        vec![(base + 60.0, None)],
        "expected the other status to hold its own event at that minute, got {:?}",
        body["Events"]
    );
}

/// A read's window selects an event by the minute the event is held at — the only instant it
/// has — so a window around the report's own instant that misses that minute reports nothing.
#[tokio::test]
async fn a_window_selects_an_event_by_the_minute_it_is_held_at() {
    let base = now_minute() - 900.0;
    let reported_at = base + 37.0;
    let held_at = base + 60.0;
    let (router, hash) = configuration(PROBE).await;
    report_at(
        &router,
        PROBE,
        &hash,
        "DISABLED",
        reported_at,
        None,
        "a report late in a minute",
    )
    .await;

    // Each row is a window and whether the event falls in it once it is held at its minute.
    let rows: [(&str, (f64, f64), Vec<f64>); 6] = [
        ("a single-instant window at the minute held", (held_at, held_at), vec![held_at]),
        (
            "a single-instant window at the instant reported",
            (reported_at, reported_at),
            vec![],
        ),
        (
            "a window ending one second before the minute held",
            (base, held_at - 1.0),
            vec![],
        ),
        (
            "a window from the minute held onwards",
            (held_at, held_at + 600.0),
            vec![held_at],
        ),
        (
            "a window covering the whole of both minutes",
            (base, held_at + 60.0),
            vec![held_at],
        ),
        (
            "a window starting one second after the minute held",
            (held_at + 1.0, held_at + 600.0),
            vec![],
        ),
    ];

    for (why, window, expected) in rows {
        let body = read(&router, PROBE, &hash, "DISABLED", Some(window), why).await;
        assert_eq!(
            event_times(&body),
            expected,
            "case {why:?}: expected {expected:?}, got {:?}",
            body["Events"]
        );
    }
}

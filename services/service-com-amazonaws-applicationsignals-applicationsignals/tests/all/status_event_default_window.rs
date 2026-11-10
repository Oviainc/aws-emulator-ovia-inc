// SPDX-License-Identifier: CC0-1.0

//! The window a status read covers when the request names NEITHER bound, end to end over
//! HTTP through the router production mounts.
//!
//! The rule under test: `GetInstrumentationConfigurationStatus` derives that window from its
//! own clock — "when both are omitted, the time range defaults to the last hour"
//! (`smithy_query(query="shape",shape="GetInstrumentationConfigurationStatusRequest")`) — so
//! it reports every event whose report named an instant inside that hour, and nothing whose
//! report named one before it. An event is held at the whole minute NEAREST the instant its
//! report named (`InstrumentationStatusEvent.Time` is "The time when the status was reported,
//! rounded to the nearest minute"), which for a report past the half-minute is the boundary
//! AHEAD of the call, so a derived window stated as the bare instants would close before the
//! minute a report of its own last minute is held at and answer nothing for it.
//!
//! Two tests, one for each half of the rule:
//!
//! * a report of the minute the read falls in is in that window — driven in the part of a
//!   minute where the event is held AHEAD of the call, which is the only part where the two
//!   can disagree, once per modelled status;
//! * the window spans the hour behind the read: every instant in it is reported, and an
//!   instant before it is not.
//!
//! The rows differ along what the rule generalises over: where in its minute the report fell,
//! how far back in the hour it was reported (and, for the rows the window must not report, how
//! far before it), the status reported, the instrumentation type and the form of the
//! `LocationIdentifier` the read names. The harness is the one
//! `instrumentation_configuration_status` drives the same two operations through, so these
//! suites agree about the wire surface.

use aws_smithy_types::DateTime;
use serde_json::json;

use crate::instrumentation_configuration_status::{
    create_configuration, create_request, events_of, get_status, nearest_minute, report,
    report_entry, router, status_request, Query, BREAKPOINT, PROBE, STATUSES,
};

/// The configuration every row reports for.
const SERVICE: &str = "inventory";
const ENVIRONMENT: &str = "eks:cluster-prod/inventory";
const FILE_PATH: &str = "src/Stock.java";

/// How many seconds make a minute, the granularity an event's `Time` carries.
const SECONDS_PER_MINUTE: f64 = 60.0;

/// The events a read reported: each one's `Time` and its `ErrorCause`, in the order the read
/// reported them (as [`events_of`] projects them).
type Events = Vec<(f64, Option<String>)>;

/// The clock, in the epoch seconds `restJson1` carries a `Timestamp` as — the same scale a
/// report names its instant on.
pub(crate) fn clock() -> f64 {
    DateTime::from(std::time::SystemTime::now()).as_secs_f64()
}

/// How far into its minute `at` falls, in seconds.
pub(crate) fn second_of_the_minute(at: f64) -> f64 {
    at - (at / SECONDS_PER_MINUTE).floor() * SECONDS_PER_MINUTE
}

/// The clock, taken in the SECOND half of a minute: the part where the whole minute nearest an
/// instant is the boundary AHEAD of it, so an event reported now is held after the call that
/// reported it.
///
/// Nothing is waited for once the clock is already there, and the wait is bounded by half a
/// minute. The instant is taken after the wait, so the caller reports for the minute the
/// following read falls in.
pub(crate) fn clock_past_the_half_minute() -> f64 {
    for _ in 0..4 {
        let at = clock();
        let position = second_of_the_minute(at);
        if position >= SECONDS_PER_MINUTE / 2.0 {
            return at;
        }
        std::thread::sleep(std::time::Duration::from_secs_f64(
            SECONDS_PER_MINUTE / 2.0 - position + 0.25,
        ));
    }
    panic!("the clock never reached the second half of a minute");
}

/// Create one configuration, report `status` for it at `time`, and answer the events a read
/// naming NO window reports for that status.
///
/// The read is taken through both forms of `LocationIdentifier`, which must agree: the window
/// a read derives is a property of the read, not of how it names the configuration.
async fn report_then_read_without_a_window(
    kind: &str,
    status: &str,
    time: f64,
    case: &str,
) -> Events {
    let router = router();
    let request = create_request(SERVICE, ENVIRONMENT, kind, FILE_PATH);
    let hash = create_configuration(&router, &request).await;

    let (code, body) = report(
        &router,
        &json!({
            "Service": SERVICE,
            "Environment": ENVIRONMENT,
            "Configurations": [report_entry(kind, &hash, status, time, None)],
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

    let mut answered: Vec<(&str, Events)> = Vec::new();
    for (form, identifier) in [
        ("location hash", json!({ "LocationHash": hash.clone() })),
        (
            "code location",
            json!({ "CodeLocation": request["Location"]["CodeLocation"] }),
        ),
    ] {
        let (code, body) = get_status(
            &router,
            &status_request(
                SERVICE,
                ENVIRONMENT,
                kind,
                identifier,
                &Query {
                    status: Some(status),
                    ..Query::default()
                },
            ),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?} by {form}: expected HTTP 200 from the read, got {code} with {body}"
        );
        answered.push((form, events_of(&body)));
    }
    assert_eq!(
        answered[0].1, answered[1].1,
        "case {case:?}: expected both forms of LocationIdentifier to report the same events, \
         got {:?} by {} and {:?} by {}",
        answered[0].1, answered[0].0, answered[1].1, answered[1].0
    );
    answered.swap_remove(0).1
}

/// A status reported for the minute the read falls in is in the window that read derives for
/// itself — including when the event is held at the minute AHEAD of the call that reported it,
/// for every status the model declares, with the instrumentation type alternating across them
/// (both types are driven over the whole hour by the table below).
///
/// One row per status, so the clock is aligned once in practice: the rows after it run in the
/// same half-minute.
#[tokio::test]
async fn a_report_of_the_current_minute_is_in_the_default_window() {
    for (index, status) in STATUSES.into_iter().enumerate() {
        let kind = if index % 2 == 0 { BREAKPOINT } else { PROBE };
        let reported_at = clock_past_the_half_minute();
        let held_at = nearest_minute(reported_at);
        let case = format!(
            "{kind} {status} reported {:.2}s into its minute",
            second_of_the_minute(reported_at)
        );
        assert!(
            held_at > reported_at,
            "case {case:?}: expected the event of a report past the half-minute to be held \
             at the minute ahead of it, got {held_at} for a report at {reported_at}"
        );

        let events = report_then_read_without_a_window(kind, status, reported_at, &case).await;
        assert_eq!(
            events,
            vec![(held_at, None)],
            "case {case:?}: expected the just-reported event at {held_at}, got {events:?}"
        );
    }
}

/// One row: how long before the read the status was reported, and whether the window the read
/// derives therefore reports it.
struct Row {
    why: &'static str,
    seconds_ago: f64,
    reported: bool,
}

/// The table: instants spanning the hour the window covers — the instant of the call, both
/// halves of the minute it falls in, whole minutes, and the far end of the hour — then
/// instants before it, which the window must not report. The rows either side of the hour stay
/// clear of its boundary by more than half a minute, the most the holding of an event can move
/// an instant.
fn rows() -> Vec<Row> {
    vec![
        Row {
            why: "the instant of the call",
            seconds_ago: 0.0,
            reported: true,
        },
        Row {
            why: "half a second before the call",
            seconds_ago: 0.5,
            reported: true,
        },
        Row {
            why: "seven seconds before the call",
            seconds_ago: 7.0,
            reported: true,
        },
        Row {
            why: "just under half a minute before the call",
            seconds_ago: 29.0,
            reported: true,
        },
        Row {
            why: "just over half a minute before the call",
            seconds_ago: 31.0,
            reported: true,
        },
        Row {
            why: "a second before the call's own minute",
            seconds_ago: 59.5,
            reported: true,
        },
        Row {
            why: "a minute before the call",
            seconds_ago: 61.0,
            reported: true,
        },
        Row {
            why: "two minutes before the call",
            seconds_ago: 125.0,
            reported: true,
        },
        Row {
            why: "a quarter of an hour before the call",
            seconds_ago: 900.0,
            reported: true,
        },
        Row {
            why: "nearly an hour before the call",
            seconds_ago: 3_500.0,
            reported: true,
        },
        Row {
            why: "just over an hour before the call",
            seconds_ago: 3_700.0,
            reported: false,
        },
        Row {
            why: "two hours before the call",
            seconds_ago: 7_200.0,
            reported: false,
        },
        Row {
            why: "a day before the call",
            seconds_ago: 86_400.0,
            reported: false,
        },
    ]
}

/// The window a read derives for itself spans the hour behind it: an event whose report named
/// an instant inside that hour is reported, at the whole minute it is held at, and one whose
/// report named an instant before it is not.
#[tokio::test]
async fn the_default_window_spans_the_hour_behind_the_read() {
    for (index, row) in rows().into_iter().enumerate() {
        // The status and the instrumentation type travel with the row, so the table spans the
        // whole modelled status catalogue and both types as well as the instants.
        let status = STATUSES[index % STATUSES.len()];
        let kind = if index % 2 == 0 { BREAKPOINT } else { PROBE };
        let reported_at = clock() - row.seconds_ago;
        let case = format!("{} ({kind} {status})", row.why);

        let events = report_then_read_without_a_window(kind, status, reported_at, &case).await;
        let expected = if row.reported {
            vec![(nearest_minute(reported_at), None)]
        } else {
            Vec::new()
        };
        assert_eq!(
            events, expected,
            "case {case:?}: expected {expected:?} for a report at {reported_at}, got {events:?}"
        );
    }
}

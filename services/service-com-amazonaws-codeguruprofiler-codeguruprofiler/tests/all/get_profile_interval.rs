// SPDX-License-Identifier: CC0-1.0

//! The window `GetProfile` selects over must be an INTERVAL — however the request names
//! that window — end to end over HTTP through the router this service mounts in
//! production.
//!
//! The rule every case here asserts: a time range names an interval only when its start
//! is STRICTLY before its end, and `GetProfile` decides that about the window it actually
//! selected — the pair of bounds the request gave, or the pair a `period` completed from
//! whichever single bound it was given — so a range that is no interval is refused with
//! HTTP 400 `ValidationException` naming both bounds of that window, before any lookup,
//! rather than answered about a range with the operation's 404 for a window holding no
//! aggregated profile.
//!
//! Rows vary what the rule generalises over: how the window was NAMED (both bounds, a
//! start completed forwards by a period, an end completed backwards by a period, a single
//! open bound, a period alone), the direction and width of the resulting pair (a day, a
//! second, a millisecond, nothing at all), the spelling of a zero-length duration, and
//! where on the time axis the bounds sit (pre-epoch, the epoch itself, a later instant).
//! The sibling [`crate::interval_validation`] table asserts the same rule across every
//! ranged operation of this service for EXPLICIT bounds; this suite is about the window
//! `GetProfile` computes, which no other operation does.
//!
//! The emulator and the HTTP plumbing are the shared [`crate::profiling_group_lifecycle`]
//! harness, not a fork of it.

use crate::interval_validation::{create_group, expected_refusal};
use crate::profiling_group_lifecycle::Harness;

/// The group the served rows run against, created through the API like any other.
const GROUP: &str = "get-profile-window";

/// A name no group is stored under, so a row can show the window is checked ahead of the
/// group lookup (which would be a 404).
const MISSING_GROUP: &str = "get-profile-window-never-created";

/// A window wide enough to hold any instant these cases place a profile at, used to show
/// that the group serves a profile over a range that IS an interval.
const WIDE_WINDOW: &str = "?startTime=2000-01-01T00:00:00Z&endTime=2099-01-01T00:00:00Z";

/// The profile every case here posts, in the representation it is posted under.
const PROFILE: &[u8] = br#"{"callgraph":{"A":{}},"samples":1}"#;
const JSON: &str = "application/json";

/// How a `GetProfile` request names the window it selects over.
///
/// SOURCE: the operation's own doc — "Specify the time range for the requested aggregated
/// profile using 1 or 2 of the following parameters: startTime, endTime, period. … If you
/// specify only period, the latest aggregated profile is returned"
/// (`smithy_query(query="operation",operation="GetProfile")`), so `period` is the LENGTH
/// of the window and completes whichever single bound was given.
#[derive(Debug)]
enum Window {
    /// Both bounds, exactly as the request gives them.
    Bounds {
        start: &'static str,
        end: &'static str,
    },
    /// A start, completed FORWARDS by a period.
    StartAndPeriod {
        start: &'static str,
        period: &'static str,
    },
    /// An end, completed BACKWARDS by a period.
    EndAndPeriod {
        end: &'static str,
        period: &'static str,
    },
    /// A request that names no pair at all: one open bound, or a period on its own.
    Open { query: &'static str },
}

impl Window {
    /// The query string this window is asked for with.
    fn query(&self) -> String {
        match self {
            Window::Bounds { start, end } => format!("?startTime={start}&endTime={end}"),
            Window::StartAndPeriod { start, period } => {
                format!("?startTime={start}&period={period}")
            }
            Window::EndAndPeriod { end, period } => format!("?endTime={end}&period={period}"),
            Window::Open { query } => (*query).to_string(),
        }
    }
}

/// What the interval rule says about the window a row names.
#[derive(Debug)]
enum Expected {
    /// The window is no interval: refused, naming these two bounds — the bounds of the
    /// window the request SELECTED, which for a period-completed row is the given bound
    /// twice.
    Refused {
        start: &'static str,
        end: &'static str,
    },
    /// The window is an interval, or the request names no pair at all: the interval rule
    /// has nothing to refuse.
    Accepted,
}

#[derive(Debug)]
struct Row {
    label: &'static str,
    window: Window,
    expected: Expected,
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "bounds reversed by a day",
            window: Window::Bounds {
                start: "2026-01-02T00:00:00Z",
                end: "2026-01-01T00:00:00Z",
            },
            expected: Expected::Refused {
                start: "2026-01-02T00:00:00Z",
                end: "2026-01-01T00:00:00Z",
            },
        },
        Row {
            label: "bounds reversed by one second",
            window: Window::Bounds {
                start: "2026-01-01T00:00:01Z",
                end: "2026-01-01T00:00:00Z",
            },
            expected: Expected::Refused {
                start: "2026-01-01T00:00:01Z",
                end: "2026-01-01T00:00:00Z",
            },
        },
        Row {
            label: "bounds reversed by one millisecond",
            window: Window::Bounds {
                start: "2026-01-01T00:00:00.002Z",
                end: "2026-01-01T00:00:00.001Z",
            },
            expected: Expected::Refused {
                start: "2026-01-01T00:00:00.002Z",
                end: "2026-01-01T00:00:00.001Z",
            },
        },
        Row {
            label: "bounds reversed across the epoch",
            window: Window::Bounds {
                start: "1970-01-01T00:00:01Z",
                end: "1969-12-31T23:59:59Z",
            },
            expected: Expected::Refused {
                start: "1970-01-01T00:00:01Z",
                end: "1969-12-31T23:59:59Z",
            },
        },
        Row {
            label: "equal whole-second bounds",
            window: Window::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-01T00:00:00Z",
            },
            expected: Expected::Refused {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-01T00:00:00Z",
            },
        },
        Row {
            label: "equal bounds carrying a millisecond fraction",
            window: Window::Bounds {
                start: "2026-01-01T00:00:00.123Z",
                end: "2026-01-01T00:00:00.123Z",
            },
            expected: Expected::Refused {
                start: "2026-01-01T00:00:00.123Z",
                end: "2026-01-01T00:00:00.123Z",
            },
        },
        Row {
            label: "equal bounds at the epoch",
            window: Window::Bounds {
                start: "1970-01-01T00:00:00Z",
                end: "1970-01-01T00:00:00Z",
            },
            expected: Expected::Refused {
                start: "1970-01-01T00:00:00Z",
                end: "1970-01-01T00:00:00Z",
            },
        },
        Row {
            label: "a zero-length period completing forwards from a start",
            window: Window::StartAndPeriod {
                start: "2026-01-01T00:00:00Z",
                period: "PT0S",
            },
            expected: Expected::Refused {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-01T00:00:00Z",
            },
        },
        Row {
            label: "a zero-length period written as a fraction, forwards from a start",
            window: Window::StartAndPeriod {
                start: "2026-06-01T13:15:02.001Z",
                period: "PT0.000S",
            },
            expected: Expected::Refused {
                start: "2026-06-01T13:15:02.001Z",
                end: "2026-06-01T13:15:02.001Z",
            },
        },
        Row {
            label: "a zero-length period written with every field, forwards from a start",
            window: Window::StartAndPeriod {
                start: "1970-01-01T00:00:00Z",
                period: "P0DT0H0M0S",
            },
            expected: Expected::Refused {
                start: "1970-01-01T00:00:00Z",
                end: "1970-01-01T00:00:00Z",
            },
        },
        Row {
            label: "a zero-length period completing backwards from an end",
            window: Window::EndAndPeriod {
                end: "2026-01-01T00:00:00Z",
                period: "PT0S",
            },
            expected: Expected::Refused {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-01T00:00:00Z",
            },
        },
        Row {
            label: "a zero-length period completing backwards from a pre-epoch end",
            window: Window::EndAndPeriod {
                end: "1969-12-31T23:59:59Z",
                period: "PT0S",
            },
            expected: Expected::Refused {
                start: "1969-12-31T23:59:59Z",
                end: "1969-12-31T23:59:59Z",
            },
        },
        Row {
            label: "bounds increasing by a day",
            window: Window::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-02T00:00:00Z",
            },
            expected: Expected::Accepted,
        },
        Row {
            label: "bounds increasing by one millisecond",
            window: Window::Bounds {
                start: "2026-01-01T00:00:00.001Z",
                end: "2026-01-01T00:00:00.002Z",
            },
            expected: Expected::Accepted,
        },
        Row {
            label: "bounds increasing across the epoch",
            window: Window::Bounds {
                start: "1969-12-31T23:59:59Z",
                end: "1970-01-01T00:00:01Z",
            },
            expected: Expected::Accepted,
        },
        Row {
            label: "a one-millisecond period completing forwards from a start",
            window: Window::StartAndPeriod {
                start: "2026-01-01T00:00:00Z",
                period: "PT0.001S",
            },
            expected: Expected::Accepted,
        },
        Row {
            label: "a five-minute period completing backwards from an end",
            window: Window::EndAndPeriod {
                end: "2026-01-01T00:00:00Z",
                period: "PT5M",
            },
            expected: Expected::Accepted,
        },
        Row {
            label: "a start alone, leaving the end open",
            window: Window::Open {
                query: "?startTime=2026-01-01T00:00:00Z",
            },
            expected: Expected::Accepted,
        },
        Row {
            label: "an end alone, leaving the start open",
            window: Window::Open {
                query: "?endTime=2026-01-01T00:00:00Z",
            },
            expected: Expected::Accepted,
        },
        Row {
            label: "a period alone, which names no pair",
            window: Window::Open {
                query: "?period=PT1H",
            },
            expected: Expected::Accepted,
        },
    ]
}

/// A group holding one aggregated profile, so every row reaches an operation that has
/// something to serve rather than only an absence.
async fn group_holding_a_profile(harness: &Harness) {
    create_group(harness, GROUP).await;
    let posted = harness
        .post_agent_profile(GROUP, JSON, None, PROFILE.to_vec())
        .await;
    assert_eq!(
        posted.status,
        204,
        "posting the profile must succeed, got {} {}",
        posted.status,
        String::from_utf8_lossy(&posted.body)
    );
    // The aggregation the posted profile is owed has to have fallen due, or every read
    // below would answer the pending-aggregation 404 for its own reason.
    harness.complete_aggregation(GROUP);
}

#[tokio::test]
async fn a_get_profile_window_that_is_not_an_interval_is_refused_however_it_is_named() {
    let harness = Harness::new();
    group_holding_a_profile(&harness).await;

    for row in rows() {
        let query = row.window.query();
        let case = format!("{} ({query})", row.label);
        let answer = harness.get_profile(GROUP, &query).await;

        match row.expected {
            Expected::Refused { start, end } => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case}: expected HTTP 400, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {case}: expected ValidationException, got {}",
                    String::from_utf8_lossy(&answer.body)
                );
                let expected = expected_refusal(start, end);
                assert_eq!(
                    answer.error_message(),
                    expected,
                    "case {case}: expected {expected:?}, got {:?}",
                    answer.error_message()
                );
            }
            Expected::Accepted => {
                assert_ne!(
                    answer.status,
                    400,
                    "case {case}: a window the interval rule allows must not be refused, \
                     got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                // A served profile's body is the posted document, not a modelled error, so
                // the error code is read only from an answer that carries one.
                if answer.status >= 400 {
                    assert_ne!(
                        answer.error_code(),
                        "ValidationException",
                        "case {case}: a window the interval rule allows must not be refused, \
                         got {}",
                        String::from_utf8_lossy(&answer.body)
                    );
                }
            }
        }
    }

    // The group really does serve its profile over a range that IS an interval, so the
    // non-refusals above are not a store that answers nothing whatever it is asked.
    let served = harness.get_profile(GROUP, WIDE_WINDOW).await;
    assert_eq!(
        served.status,
        200,
        "a wide window around the stored profile must serve it, got {} {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
    assert_eq!(
        served.body,
        PROFILE.to_vec(),
        "a wide window must serve the posted profile, got {:?}",
        String::from_utf8_lossy(&served.body)
    );
}

#[tokio::test]
async fn the_get_profile_window_is_checked_before_the_group_is_looked_up() {
    let harness = Harness::new();

    for row in rows() {
        let Expected::Refused { start, end } = row.expected else {
            continue;
        };
        let query = row.window.query();
        let case = format!("{} ({query})", row.label);
        // No group of this name was ever created, so a lookup would answer 404: the 400
        // shows the window is checked first.
        let answer = harness.get_profile(MISSING_GROUP, &query).await;
        assert_eq!(
            answer.status,
            400,
            "case {case}: a window that is no interval must be refused before the group \
             lookup, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case}: expected ValidationException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        let expected = expected_refusal(start, end);
        assert_eq!(
            answer.error_message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            answer.error_message()
        );
    }
}

#[tokio::test]
async fn a_refused_get_profile_window_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new();
    group_holding_a_profile(&harness).await;

    let before = harness.get_profile(GROUP, WIDE_WINDOW).await;
    assert_eq!(
        before.status,
        200,
        "the stored profile must be servable first, got {} {}",
        before.status,
        String::from_utf8_lossy(&before.body)
    );

    for row in rows() {
        if matches!(row.expected, Expected::Accepted) {
            continue;
        }
        let query = row.window.query();
        let answer = harness.get_profile(GROUP, &query).await;
        assert_eq!(
            answer.status,
            400,
            "case {} ({query}): expected the interval refusal, got {} {}",
            row.label,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    let after = harness.get_profile(GROUP, WIDE_WINDOW).await;
    assert_eq!(
        after.status, before.status,
        "a refused window must not change a later answer's status, got {} {}",
        after.status,
        String::from_utf8_lossy(&after.body)
    );
    assert_eq!(
        after.body,
        before.body,
        "a refused window must not change what a later caller is served, got {}",
        String::from_utf8_lossy(&after.body)
    );
    assert_eq!(
        after.content_type, before.content_type,
        "a refused window must not change the representation served, got {:?}",
        after.content_type
    );
}

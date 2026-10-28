// SPDX-License-Identifier: CC0-1.0

//! How LONG a window `GetProfile` serves over may be, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule every case here asserts: an operation whose doc bounds the time range it
//! answers over refuses a request whose range is LONGER than that maximum, whether the
//! request named the range with a pair of bounds, with one bound and a length, or with a
//! length alone — HTTP 400 `ValidationException` stating the limit and the range the
//! request ended up naming, answered before any lookup — and serves a range at or inside
//! the maximum. For `GetProfile` the limit is the one its own doc states: "Specify the
//! time range for the requested aggregated profile using 1 or 2 of the following
//! parameters: startTime, endTime, period. The maximum time range allowed is 7 days"
//! (`smithy_query(query="doc", shape="GetProfile")`).
//!
//! Rows vary what the rule generalises over: how the range is NAMED (both bounds, a start
//! completed forwards by a period, an end completed backwards by a period, a period alone,
//! nothing at all), its WIDTH (two months, twice the limit, one
//! second over it, one millisecond over it, exactly the limit, a day, a millisecond), how
//! the length is SPELLED (days, hours, minutes, a fraction of a second, every field at
//! once) and where on the time axis the range sits (pre-epoch, across the epoch, a later
//! instant).
//!
//! The sibling rule for the same operation — that the window must be an INTERVAL at all —
//! is [`crate::get_profile_interval`]; the same length rule for the other operation that
//! documents a maximum (`BatchGetFrameMetricData`, at `PT24H`) is
//! [`crate::frame_metric_data`]'s table.
//!
//! The emulator and the HTTP plumbing are the shared [`crate::profiling_group_lifecycle`]
//! harness, not a fork of it.

use crate::interval_validation::{canonical, create_group};
use crate::profiling_group_lifecycle::{Harness, servable_window};

/// The group the rows run against, created through the API like any other.
const GROUP: &str = "get-profile-length";

/// A name no group is stored under, so a row can show the length is checked ahead of the
/// group lookup (which would be a 404).
const MISSING_GROUP: &str = "get-profile-length-never-created";

/// The maximum range, in the ISO 8601 duration form `period` is spelled in. SOURCE: the
/// operation's own doc — "The maximum time range allowed is 7 days".
const MAX_RANGE: &str = "P7D";

/// What the operation's doc calls the range that maximum bounds, as the refusal names it.
const SUBJECT: &str = "the time range for the requested aggregated profile";

/// The profile every case here posts, in the representation it is posted under. It carries
/// the `start` a submitted profile must state, which is what `PostAgentProfile` parses out
/// of it.
const PROFILE: &[u8] = br#"{"start":1600000000000,"callgraph":{"B":{}},"samples":2}"#;
const JSON: &str = "application/json";

/// How a request names the range it asks about.
#[derive(Debug)]
enum Named {
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
    /// A request that names no pair: a period on its own, or nothing at all. A single
    /// BOUND is not one of these — this operation's bound members require a companion,
    /// which is [`crate::get_profile_window_companion`]'s rule.
    Open { query: &'static str },
}

impl Named {
    /// The query string this range is asked for with.
    fn query(&self) -> String {
        match self {
            Named::Bounds { start, end } => format!("?startTime={start}&endTime={end}"),
            Named::StartAndPeriod { start, period } => {
                format!("?startTime={start}&period={period}")
            }
            Named::EndAndPeriod { end, period } => format!("?endTime={end}&period={period}"),
            Named::Open { query } => (*query).to_string(),
        }
    }
}

/// The range a refusal names: the two bounds of the pair the request ended up with, or the
/// length itself where the request named no pair.
#[derive(Debug)]
enum Range {
    Bounds {
        start: &'static str,
        end: &'static str,
    },
    Length(&'static str),
}

/// What the length rule says about the range a row names.
#[derive(Debug)]
enum Expected {
    /// Longer than the maximum: refused, naming this range.
    TooLong(Range),
    /// At or inside the maximum, or naming no length at all: the length rule has nothing
    /// to refuse.
    Allowed,
}

#[derive(Debug)]
struct Row {
    label: &'static str,
    named: Named,
    expected: Expected,
}

/// The sentence a range longer than the maximum is refused with, built from the row's own
/// range rather than written out per row.
fn expected_refusal(range: &Range) -> String {
    let named = match range {
        Range::Bounds { start, end } => format!("{} to {}", canonical(start), canonical(end)),
        Range::Length(period) => (*period).to_string(),
    };
    format!("{SUBJECT} must be at most {MAX_RANGE} long, got {named}")
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        // ---- named with a PAIR OF BOUNDS ----
        Row {
            label: "two months of bounds",
            named: Named::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-03-01T00:00:00Z",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-03-01T00:00:00Z",
            }),
        },
        Row {
            label: "twice the maximum, in bounds",
            named: Named::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-15T00:00:00Z",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-15T00:00:00Z",
            }),
        },
        Row {
            label: "one second over the maximum, in bounds",
            named: Named::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-08T00:00:01Z",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-08T00:00:01Z",
            }),
        },
        Row {
            label: "one millisecond over the maximum, in bounds",
            named: Named::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-08T00:00:00.001Z",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-08T00:00:00.001Z",
            }),
        },
        Row {
            label: "a pre-epoch pair wider than the maximum",
            named: Named::Bounds {
                start: "1960-01-01T00:00:00Z",
                end: "1960-02-01T00:00:00Z",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "1960-01-01T00:00:00Z",
                end: "1960-02-01T00:00:00Z",
            }),
        },
        Row {
            label: "a pair straddling the epoch, wider than the maximum",
            named: Named::Bounds {
                start: "1969-12-25T00:00:00Z",
                end: "1970-01-08T00:00:00Z",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "1969-12-25T00:00:00Z",
                end: "1970-01-08T00:00:00Z",
            }),
        },
        Row {
            label: "exactly the maximum, in bounds",
            named: Named::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-08T00:00:00Z",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "exactly the maximum, in bounds carrying a millisecond fraction",
            named: Named::Bounds {
                start: "2026-06-01T13:15:02.001Z",
                end: "2026-06-08T13:15:02.001Z",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "a millisecond short of the maximum, straddling the epoch",
            named: Named::Bounds {
                start: "1969-12-29T00:00:00.001Z",
                end: "1970-01-05T00:00:00Z",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "a day of bounds",
            named: Named::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-02T00:00:00Z",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "a millisecond of bounds",
            named: Named::Bounds {
                start: "2026-01-01T00:00:00.001Z",
                end: "2026-01-01T00:00:00.002Z",
            },
            expected: Expected::Allowed,
        },
        // ---- named with ONE BOUND and a LENGTH ----
        Row {
            label: "a month-long period forwards from a start",
            named: Named::StartAndPeriod {
                start: "2026-01-01T00:00:00Z",
                period: "P30D",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-31T00:00:00Z",
            }),
        },
        Row {
            label: "a period one second over the maximum, forwards from a start",
            named: Named::StartAndPeriod {
                start: "2026-01-01T00:00:00Z",
                period: "P7DT1S",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-08T00:00:01Z",
            }),
        },
        Row {
            label: "a period one minute over the maximum spelled in minutes, from a start",
            named: Named::StartAndPeriod {
                start: "2026-01-01T00:00:00Z",
                period: "PT10081M",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-08T00:01:00Z",
            }),
        },
        Row {
            label: "a period a millisecond over the maximum, forwards from a start",
            named: Named::StartAndPeriod {
                start: "2026-01-01T00:00:00Z",
                period: "P7DT0.001S",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-08T00:00:00.001Z",
            }),
        },
        Row {
            label: "a month-long period backwards from an end",
            named: Named::EndAndPeriod {
                end: "2026-03-01T00:00:00Z",
                period: "P30D",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "2026-01-30T00:00:00Z",
                end: "2026-03-01T00:00:00Z",
            }),
        },
        Row {
            label: "a two-week period backwards from a pre-epoch end",
            named: Named::EndAndPeriod {
                end: "1970-01-01T00:00:00Z",
                period: "P14D",
            },
            expected: Expected::TooLong(Range::Bounds {
                start: "1969-12-18T00:00:00Z",
                end: "1970-01-01T00:00:00Z",
            }),
        },
        Row {
            label: "exactly the maximum as a period, forwards from a start",
            named: Named::StartAndPeriod {
                start: "2026-01-01T00:00:00Z",
                period: "P7D",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "exactly the maximum spelled in hours, backwards from an end",
            named: Named::EndAndPeriod {
                end: "2026-01-08T00:00:00Z",
                period: "PT168H",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "a millisecond short of the maximum, every field spelled out",
            named: Named::StartAndPeriod {
                start: "2026-01-01T00:00:00Z",
                period: "P6DT23H59M59.999S",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "the member doc's own example period, forwards from a start",
            named: Named::StartAndPeriod {
                start: "2026-01-01T00:00:00Z",
                period: "P1DT1H1M1S",
            },
            expected: Expected::Allowed,
        },
        // ---- named with a LENGTH ALONE, or with no length at all ----
        Row {
            label: "a month-long period alone",
            named: Named::Open {
                query: "?period=P30D",
            },
            expected: Expected::TooLong(Range::Length("P30D")),
        },
        Row {
            label: "a period alone, one second over the maximum, spelled in hours",
            named: Named::Open {
                query: "?period=PT168H1S",
            },
            expected: Expected::TooLong(Range::Length("PT168H1S")),
        },
        Row {
            label: "a period alone, exactly the maximum",
            named: Named::Open {
                query: "?period=P7D",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "a period alone, an hour long",
            named: Named::Open {
                query: "?period=PT1H",
            },
            expected: Expected::Allowed,
        },
        // A single BOUND names no length either, but it is refused before this rule is
        // reached: the operation's bound members must be accompanied by the other bound or
        // by `period` ([`crate::get_profile_window_companion`]), so it is no row here.
        Row {
            label: "no parameters at all",
            named: Named::Open { query: "" },
            expected: Expected::Allowed,
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
async fn a_requested_range_longer_than_the_documented_maximum_is_refused_however_it_is_named() {
    let harness = Harness::new();
    group_holding_a_profile(&harness).await;

    for row in rows() {
        let query = row.named.query();
        let case = format!("{} ({query})", row.label);
        let answer = harness.get_profile(GROUP, &query).await;

        match row.expected {
            Expected::TooLong(range) => {
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
                let expected = expected_refusal(&range);
                assert_eq!(
                    answer.error_message(),
                    expected,
                    "case {case}: expected {expected:?}, got {:?}",
                    answer.error_message()
                );
            }
            Expected::Allowed => {
                assert_ne!(
                    answer.status,
                    400,
                    "case {case}: a range the length rule allows must not be refused, got \
                     {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
            }
        }
    }

    // The group really does serve its profile over a range inside the maximum, so the
    // non-refusals above are not a store that answers nothing whatever it is asked.
    let served = harness.get_profile(GROUP, &servable_window()).await;
    assert_eq!(
        served.status,
        200,
        "a window inside the maximum around the stored profile must serve it, got {} {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
    assert_eq!(
        served.body,
        PROFILE.to_vec(),
        "that window must serve the posted profile, got {:?}",
        String::from_utf8_lossy(&served.body)
    );
}

#[tokio::test]
async fn the_requested_range_is_bounded_before_the_group_is_looked_up() {
    let harness = Harness::new();

    for row in rows() {
        let Expected::TooLong(range) = row.expected else {
            continue;
        };
        let query = row.named.query();
        let case = format!("{} ({query})", row.label);
        // No group of this name was ever created, so a lookup would answer 404: the 400
        // shows the length is checked first.
        let answer = harness.get_profile(MISSING_GROUP, &query).await;
        assert_eq!(
            answer.status,
            400,
            "case {case}: a range longer than the maximum must be refused before the group \
             lookup, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let expected = expected_refusal(&range);
        assert_eq!(
            answer.error_message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            answer.error_message()
        );
    }
}

#[tokio::test]
async fn a_refused_range_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new();
    group_holding_a_profile(&harness).await;

    let window = servable_window();
    let before = harness.get_profile(GROUP, &window).await;
    assert_eq!(
        before.status,
        200,
        "the stored profile must be servable first, got {} {}",
        before.status,
        String::from_utf8_lossy(&before.body)
    );

    for row in rows() {
        if matches!(row.expected, Expected::Allowed) {
            continue;
        }
        let query = row.named.query();
        let answer = harness.get_profile(GROUP, &query).await;
        assert_eq!(
            answer.status,
            400,
            "case {} ({query}): expected the length refusal, got {} {}",
            row.label,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    let after = harness.get_profile(GROUP, &window).await;
    assert_eq!(
        after.status, before.status,
        "a refused range must not change a later answer's status, got {} {}",
        after.status,
        String::from_utf8_lossy(&after.body)
    );
    assert_eq!(
        after.body,
        before.body,
        "a refused range must not change what a later caller is served, got {}",
        String::from_utf8_lossy(&after.body)
    );
    assert_eq!(
        after.content_type, before.content_type,
        "a refused range must not change the representation served, got {:?}",
        after.content_type
    );
}

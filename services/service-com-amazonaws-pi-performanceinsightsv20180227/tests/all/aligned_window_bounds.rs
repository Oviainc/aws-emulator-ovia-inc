// SPDX-License-Identifier: CC0-1.0

//! The ALIGNED WINDOW the time-series operations report, end to end over the
//! wire.
//!
//! RULE under test: the two bounds have different drivers. `AlignedStartTime` is
//! the requested `StartTime` floored to the effective period, as its own
//! documentation states. `AlignedEndTime` is not a function of the requested
//! `EndTime` at all: Performance Insights reports through the latest period
//! boundary it HOLDS, which is the service's current time floored to that period
//! (measured from the Unix epoch) — so it can land either side of the requested
//! end, it is never raised past the server's clock, and no data point is
//! reported for a period the clock has not reached.
//!
//! The rows span what the rule generalises over: every documented
//! `PeriodInSeconds`, and requested ends in the past, inside the current period,
//! on a boundary and in the future — a window whose requested end rounds UP past
//! the clock (the shape of the recorded unit: `PeriodInSeconds=300`,
//! `StartTime=now-37min`, `EndTime=now-2min`) is ONE row among them. Both
//! operations that report the window are checked, because the bound is the same
//! claim in both.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator};

/// The granularities `PeriodInSeconds` documents: 1 (one second), 60 (one
/// minute), 300 (five minutes), 3600 (one hour), 86400 (twenty-four hours).
const DOCUMENTED_PERIODS: &[i64] = &[1, 60, 300, 3600, 86400];

/// A monitored data source, in the form every `Identifier` member documents.
const IDENTIFIER: &str = "db-ABCDEFG1234567";

/// Epoch seconds, as the service reads its own clock.
fn unix_now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is at or after the Unix epoch")
            .as_secs(),
    )
    .expect("the current time fits in an i64")
}

/// `seconds` floored to a `period` boundary measured from the Unix epoch.
fn floor_to(seconds: i64, period: i64) -> i64 {
    seconds - seconds.rem_euclid(period)
}

/// `seconds` raised to a `period` boundary measured from the Unix epoch.
fn ceil_to(seconds: i64, period: i64) -> i64 {
    let remainder = seconds.rem_euclid(period);
    if remainder == 0 {
        seconds
    } else {
        seconds - remainder + period
    }
}

/// The `AlignedEndTime` values a call made between `before` and `after` may
/// report at `period`.
fn end_candidates(before: i64, after: i64, period: i64) -> Vec<i64> {
    let mut candidates = vec![floor_to(before, period), floor_to(after, period)];
    candidates.dedup();
    candidates
}

fn metrics_input(start: i64, end: i64, period: i64) -> Value {
    json!({
        "ServiceType": "RDS",
        "Identifier": IDENTIFIER,
        "MetricQueries": [{"Metric": "db.load.avg"}],
        "StartTime": start,
        "EndTime": end,
        "PeriodInSeconds": period,
    })
}

fn dimension_keys_input(start: i64, end: i64, period: i64) -> Value {
    json!({
        "ServiceType": "RDS",
        "Identifier": IDENTIFIER,
        "StartTime": start,
        "EndTime": end,
        "Metric": "db.load.avg",
        "GroupBy": {"Group": "db.sql"},
        "PeriodInSeconds": period,
    })
}

/// How a row places the requested `EndTime` relative to the service's clock, in
/// seconds (negative is in the past).
struct Placement {
    case: &'static str,
    /// Where the window ends, as an offset from the current time.
    end_offset: i64,
    /// How wide the window is, held to at most a hundred periods by the caller so
    /// the finest granularity stays inside the emulator's data-point budget.
    span: i64,
}

/// The rows: an end two minutes in the past (the recorded unit's shape, whose
/// round-up at a 300 s period lands PAST the clock), an end a whole period and
/// several periods back, an end right at the current second, and an end in the
/// future — which no boundary the service holds may ever reach.
const PLACEMENTS: &[Placement] = &[
    Placement {
        case: "an end two minutes in the past",
        end_offset: -120,
        span: 37 * 60,
    },
    Placement {
        case: "an end an hour in the past",
        end_offset: -3_600,
        span: 7_200,
    },
    Placement {
        case: "an end a day in the past",
        end_offset: -86_400,
        span: 2 * 86_400,
    },
    Placement {
        case: "an end at the current second",
        end_offset: 0,
        span: 1_800,
    },
    Placement {
        case: "an end ten minutes in the future",
        end_offset: 600,
        span: 3_600,
    },
    Placement {
        case: "an end a day in the future",
        end_offset: 86_400,
        span: 2 * 86_400,
    },
];

/// RULE: `AlignedEndTime` is the service's clock floored to the effective
/// period, whatever the request asked to stop at; `AlignedStartTime` is the
/// requested start floored to the same period.
#[tokio::test]
async fn the_end_bound_is_the_clock_floored_to_the_period_not_the_requested_end() {
    let emulator = emulator();

    for placement in PLACEMENTS {
        for &period in DOCUMENTED_PERIODS {
            let before = unix_now();
            let end = before + placement.end_offset;
            // The window is the row's span, held to at most a hundred periods so
            // the finest granularity stays inside the emulator's data-point
            // budget, and at least one period longer than the row's offset so the
            // window STARTS in the past: the bound under test is the end of the
            // window, not its width.
            let span = placement
                .span
                .min(period * 100)
                .max(period)
                .max(placement.end_offset + period);
            let start = end - span;
            let case = format!("{} / period={period}", placement.case);

            let answer = call(&emulator, "GetResourceMetrics", metrics_input(start, end, period)).await;
            let after = unix_now();
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} {:?}",
                answer.status, answer.body
            );

            let aligned_start = answer.member("AlignedStartTime").as_i64().unwrap_or(-1);
            let aligned_end = answer.member("AlignedEndTime").as_i64().unwrap_or(-1);

            assert_eq!(
                aligned_start,
                floor_to(start, period),
                "case {case}: expected AlignedStartTime at the {period}s boundary at or before \
                 StartTime {start}, got {aligned_start}"
            );
            let candidates = end_candidates(before, after, period);
            assert!(
                candidates.contains(&aligned_end),
                "case {case}: expected AlignedEndTime to be the current time floored to the \
                 {period}s boundary ({candidates:?}), got {aligned_end}; the requested EndTime \
                 {end} is not the driver of this value"
            );
            // Never a boundary the clock has not reached, however far ahead the
            // caller asked the window to stop.
            assert!(
                aligned_end <= after,
                "case {case}: expected an end bound at or before the clock {after}, got \
                 {aligned_end}"
            );
            // And never the requested end rounded UP, which is the defect this
            // row of the table is shaped to catch.
            let ceilinged = ceil_to(end, period);
            if ceilinged > after {
                assert_ne!(
                    aligned_end, ceilinged,
                    "case {case}: expected the end bound NOT to be the requested EndTime \
                     ceilinged to {ceilinged}, which is past the clock {after}"
                );
            }
        }
    }
}

/// RULE: the same bound is the same claim in `DescribeDimensionKeys`, which
/// reports the aligned window of the same Active Session History data.
#[tokio::test]
async fn the_dimension_key_window_reports_the_same_end_bound() {
    let emulator = emulator();

    for placement in PLACEMENTS {
        for &period in DOCUMENTED_PERIODS {
            let before = unix_now();
            let end = before + placement.end_offset;
            // The same window as the row above, so the two operations' bounds are
            // comparable.
            let span = placement
                .span
                .min(period * 100)
                .max(period)
                .max(placement.end_offset + period);
            let start = end - span;
            let case = format!("{} / period={period}", placement.case);

            let answer = call(
                &emulator,
                "DescribeDimensionKeys",
                dimension_keys_input(start, end, period),
            )
            .await;
            let after = unix_now();
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} {:?}",
                answer.status, answer.body
            );

            let aligned_start = answer.member("AlignedStartTime").as_i64().unwrap_or(-1);
            let aligned_end = answer.member("AlignedEndTime").as_i64().unwrap_or(-1);
            assert_eq!(
                aligned_start,
                floor_to(start, period),
                "case {case}: expected AlignedStartTime at the {period}s boundary at or before \
                 StartTime {start}, got {aligned_start}"
            );
            let candidates = end_candidates(before, after, period);
            assert!(
                candidates.contains(&aligned_end),
                "case {case}: expected AlignedEndTime to be the current time floored to the \
                 {period}s boundary ({candidates:?}), got {aligned_end}"
            );
        }
    }
}

/// RULE: no data point is reported for a period the clock has not reached, so
/// the series stops at the latest boundary the service holds — a caller that
/// stitches successive windows is never handed buckets the service does not
/// claim to cover.
#[tokio::test]
async fn the_series_stops_at_the_latest_boundary_the_service_holds() {
    let emulator = emulator();

    // The rows differ in how far past the clock the requested window reaches:
    // ending just inside the current period, one period past it, and a day
    // past it.
    for &period in &[60_i64, 300, 3_600] {
        for end_offset in [-period / 2, period, period * 3, 86_400] {
            let before = unix_now();
            let end = before + end_offset;
            let start = end - period * 10;
            let case = format!("period={period} / end_offset={end_offset}");

            let answer =
                call(&emulator, "GetResourceMetrics", metrics_input(start, end, period)).await;
            let after = unix_now();
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} {:?}",
                answer.status, answer.body
            );

            let aligned_start = answer.member("AlignedStartTime").as_i64().unwrap_or(-1);
            let stamps: Vec<i64> = answer
                .member("MetricList")
                .as_array()
                .and_then(|list| list.first())
                .and_then(|entry| entry.get("DataPoints"))
                .and_then(Value::as_array)
                .map(|points| {
                    points
                        .iter()
                        .filter_map(|point| point.get("Timestamp").and_then(Value::as_i64))
                        .collect()
                })
                .unwrap_or_default();

            // Every point is stamped at a period boundary of the window, in
            // order, and none of them past the latest boundary the service
            // holds (the default `END_TIME` alignment stamps a point with its
            // period's END, so the last stamp is that boundary itself).
            for (index, stamp) in stamps.iter().enumerate() {
                assert_eq!(
                    *stamp,
                    aligned_start + (index as i64 + 1) * period,
                    "case {case}: expected point {index} on the grid from {aligned_start}, got \
                     {stamp}"
                );
            }
            if let Some(last) = stamps.last() {
                assert!(
                    *last <= floor_to(after, period),
                    "case {case}: expected the last point at or before the latest boundary the \
                     service holds ({}), got {last}",
                    floor_to(after, period)
                );
            }
            // The series covers the requested window up to that boundary, so
            // nothing inside the held part of the window is dropped either.
            let expected = (ceil_to(end, period).min(floor_to(before, period)) - aligned_start)
                / period;
            assert!(
                (stamps.len() as i64) >= expected,
                "case {case}: expected at least {expected} points across the held part of the \
                 window, got {}",
                stamps.len()
            );
        }
    }
}

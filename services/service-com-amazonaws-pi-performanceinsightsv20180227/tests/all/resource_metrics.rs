// SPDX-License-Identifier: CC0-1.0

//! `GetResourceMetrics`, end to end over the wire.
//!
//! The rules under test are the ones the model, the member documentation and the
//! service's recorded behaviour state: `AlignedStartTime` sits on a period
//! boundary at or before the requested `StartTime`, while `AlignedEndTime` is
//! the latest period boundary the service HOLDS metrics through — its own clock
//! floored to the period, never a boundary computed from the requested
//! `EndTime` (the bound's own suite is `aligned_window_bounds.rs`); the period is
//! one of the documented granularities, chosen from the window when the caller
//! names none; the series a metric key carries is one data point per period from
//! `AlignedStartTime` across the requested window, never past the latest
//! boundary the service holds, and `PeriodAlignment` decides whether a point is
//! stamped with its period's start or its end (`END_TIME` by default); one
//! `MetricList` entry per `MetricQueries` element; and a repeated call answers
//! the same series because nothing is stored.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator, is_modelled_next_token};

/// The granularities `PeriodInSeconds` documents: 1 (one second), 60 (one
/// minute), 300 (five minutes), 3600 (one hour), 86400 (twenty-four hours).
const DOCUMENTED_PERIODS: &[i64] = &[1, 60, 300, 3600, 86400];

/// One day in seconds — a multiple of every documented period, which is what
/// makes a day boundary a boundary of all of them.
const ONE_DAY: i64 = 86_400;

/// How far clear of the service's clock [`past_start`] keeps a window's end.
///
/// Two whole days, which is more than the coarsest documented period: a window
/// that ends this far back is one the service holds metrics through from end to
/// end, so its series covers the whole requested window at every granularity.
const PAST_MARGIN: i64 = 2 * ONE_DAY;

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

/// `seconds` floored to a `period` boundary measured from the Unix epoch — the
/// alignment both of the service's reported bounds use.
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

/// A window start that leaves a `span`-second window entirely in the PAST, clear
/// of the service's clock by [`PAST_MARGIN`], and on a boundary of every
/// documented period.
fn past_start(span: i64) -> i64 {
    floor_to(unix_now() - span - PAST_MARGIN, ONE_DAY)
}

/// The `AlignedEndTime` values a call made between `before` and `after` may
/// report at `period`: the service's clock floored to the period, which is the
/// one driver of that bound.
fn aligned_end_candidates(before: i64, after: i64, period: i64) -> Vec<i64> {
    let mut candidates = vec![floor_to(before, period), floor_to(after, period)];
    candidates.dedup();
    candidates
}

fn metrics_input(
    identifier: &str,
    metrics: &[&str],
    start: i64,
    end: i64,
    period: Option<i64>,
    alignment: Option<&str>,
) -> Value {
    let mut input = json!({
        "ServiceType": "RDS",
        "Identifier": identifier,
        "MetricQueries": metrics
            .iter()
            .map(|metric| json!({"Metric": metric}))
            .collect::<Vec<_>>(),
        "StartTime": start,
        "EndTime": end,
    });
    if let Some(period) = period {
        input["PeriodInSeconds"] = json!(period);
    }
    if let Some(alignment) = alignment {
        input["PeriodAlignment"] = json!(alignment);
    }
    input
}

fn data_points(answer: &Value, index: usize) -> Vec<Value> {
    answer
        .get("MetricList")
        .and_then(Value::as_array)
        .and_then(|list| list.get(index))
        .and_then(|entry| entry.get("DataPoints"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// RULE: `AlignedStartTime` is the requested `StartTime` floored to the
/// effective period, `AlignedEndTime` is the service's own clock floored to the
/// same period, and the series for the requested metric spans the requested
/// window at that granularity with no invented magnitudes.
#[tokio::test]
async fn the_window_is_reported_from_the_request_start_and_the_services_clock() {
    let emulator = emulator();

    // The rows differ along what the rule generalises over: the period (every
    // documented value, including both published bounds), whether the requested
    // start and end already sit on a boundary, and the alignment.
    let mut cases: Vec<(i64, i64, i64, Option<&str>)> = Vec::new();
    for &period in DOCUMENTED_PERIODS {
        let t0 = past_start(period * 4);
        for (start_offset, end_offset) in [
            (0, period),              // both ends already aligned
            (1, period + 1),          // both ends inside a period
            (period - 1, period * 2), // start just before a boundary
            (0, period * 3 + 7),      // an end that needs widening
        ] {
            for alignment in [None, Some("START_TIME"), Some("END_TIME")] {
                cases.push((t0 + start_offset, t0 + end_offset, period, alignment));
            }
        }
    }

    for (start, end, period, alignment) in cases {
        let label = format!("start={start} end={end} period={period} alignment={alignment:?}");
        let before = unix_now();
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &["db.load.avg"],
                start,
                end,
                Some(period),
                alignment,
            ),
        )
        .await;
        let after = unix_now();
        assert_eq!(
            answer.status, 200,
            "case {label}: expected 200, got {} {:?}",
            answer.status, answer.body
        );

        assert_eq!(
            answer.member("Identifier").as_str(),
            Some(IDENTIFIER),
            "case {label}: expected the requested Identifier echoed, got {:?}",
            answer.body
        );

        let aligned_start = answer
            .member("AlignedStartTime")
            .as_i64()
            .unwrap_or_else(|| panic!("case {label}: no AlignedStartTime in {:?}", answer.body));
        let aligned_end = answer
            .member("AlignedEndTime")
            .as_i64()
            .unwrap_or_else(|| panic!("case {label}: no AlignedEndTime in {:?}", answer.body));

        assert_eq!(
            aligned_start,
            floor_to(start, period),
            "case {label}: expected AlignedStartTime at the {period}s boundary at or before \
             StartTime, got {aligned_start}"
        );
        // The end bound is the latest boundary the service holds, i.e. its clock
        // floored to the period — NOT the requested end rounded in either
        // direction.
        let candidates = aligned_end_candidates(before, after, period);
        assert!(
            candidates.contains(&aligned_end),
            "case {label}: expected AlignedEndTime to be the current time floored to the \
             {period}s boundary ({candidates:?}), got {aligned_end}"
        );

        // The series spans the requested window at the effective period, and
        // stops at the latest boundary the service holds.
        let points = data_points(&answer.body, 0);
        let expected = (ceil_to(end, period).min(aligned_end) - aligned_start) / period;
        assert_eq!(
            points.len() as i64,
            expected,
            "case {label}: expected {expected} data points across the requested window, got {}",
            points.len()
        );
    }
}

/// RULE: the series a metric key carries is the GRID the requested window holds:
/// one data point per `PeriodInSeconds` period from `AlignedStartTime` to the
/// requested window's own aligned end, in time order, each stamped at that
/// period's start or end as `PeriodAlignment` asks and each carrying the
/// `@required` `Value` the model declares.
#[tokio::test]
async fn the_series_is_the_grid_the_requested_window_holds() {
    let emulator = emulator();

    // The rows differ along what the rule generalises over: the period (every
    // documented value, including both published bounds), the alignment
    // (absent — the documented END_TIME default — and both enum values), and
    // whether the requested window already sits on period boundaries.
    let mut cases: Vec<(i64, i64, i64, Option<&str>)> = Vec::new();
    for &period in DOCUMENTED_PERIODS {
        let t0 = past_start(period * 3);
        for (start_offset, end_offset) in [(0, period * 3), (1, period * 2 + 1)] {
            for alignment in [None, Some("START_TIME"), Some("END_TIME")] {
                cases.push((t0 + start_offset, t0 + end_offset, period, alignment));
            }
        }
    }

    for (start, end, period, alignment) in cases {
        let label = format!("start={start} end={end} period={period} alignment={alignment:?}");
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &["db.load.avg"],
                start,
                end,
                Some(period),
                alignment,
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label}: expected 200, got {} {:?}",
            answer.status, answer.body
        );

        let aligned_start = answer.member("AlignedStartTime").as_i64().unwrap_or(-1);
        let points = data_points(&answer.body, 0);

        // The window is wholly in the past, so the series covers it completely:
        // one point per period from the aligned start to the aligned end of the
        // REQUESTED window.
        let series_end = ceil_to(end, period);
        let expected = (series_end - aligned_start) / period;
        assert_eq!(
            points.len() as i64,
            expected,
            "case {label}: expected {expected} points across {aligned_start}..{series_end}, \
             got {}",
            points.len()
        );

        // Each point is stamped at its period's start or end, in time order,
        // and carries a Value.
        let stamps_the_period_start = alignment == Some("START_TIME");
        for (index, point) in points.iter().enumerate() {
            let timestamp = point.get("Timestamp").and_then(Value::as_i64);
            let offset = if stamps_the_period_start {
                index as i64
            } else {
                index as i64 + 1
            };
            assert_eq!(
                timestamp,
                Some(aligned_start + offset * period),
                "case {label}: expected point {index} stamped at {}, got {point:?}",
                aligned_start + offset * period
            );
            assert!(
                point.get("Value").and_then(Value::as_f64).is_some(),
                "case {label}: expected point {index} to carry the required Value, got {point:?}"
            );
        }

        // The first and last stamps are the series' own ends, so a client
        // charting it has the x-axis the requested window describes: inclusive
        // of the start under START_TIME, inclusive of the end under END_TIME.
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            let first_stamp = first.get("Timestamp").and_then(Value::as_i64);
            let last_stamp = last.get("Timestamp").and_then(Value::as_i64);
            if stamps_the_period_start {
                assert_eq!(
                    (first_stamp, last_stamp),
                    (Some(aligned_start), Some(series_end - period)),
                    "case {label}: expected the period-start grid {aligned_start}..\
                     {}, got {first_stamp:?}..{last_stamp:?}",
                    series_end - period
                );
            } else {
                assert_eq!(
                    (first_stamp, last_stamp),
                    (Some(aligned_start + period), Some(series_end)),
                    "case {label}: expected the period-end grid {}..{series_end}, got \
                     {first_stamp:?}..{last_stamp:?}",
                    aligned_start + period
                );
            }
        }
    }
}

/// RULE: every metric key of one answer carries the same grid — the series
/// depends on the window and the alignment, never on which metric was asked
/// for.
#[tokio::test]
async fn every_metric_key_carries_the_same_grid() {
    let emulator = emulator();
    let metrics = ["db.load.avg", "db.sampledload.avg", "os.memory.free.min"];

    let answer = call(
        &emulator,
        "GetResourceMetrics",
        metrics_input(IDENTIFIER, &metrics, past_start(1_800), past_start(1_800) + 1_800, Some(60), None),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case three metrics: expected 200, got {} {:?}",
        answer.status, answer.body
    );

    let first = data_points(&answer.body, 0);
    assert_eq!(
        first.len(),
        30,
        "case three metrics: expected the 30-period grid, got {} points",
        first.len()
    );
    for (index, metric) in metrics.iter().enumerate().skip(1) {
        let series = data_points(&answer.body, index);
        assert_eq!(
            series, first,
            "case entry {index} ({metric}): expected the same grid as the first entry, got \
             {series:?}"
        );
    }
}

/// RULE: a cursor this operation did not issue is refused — including one whose
/// scope it would accept but whose offset is at or past the metric-query list,
/// which it never hands out. A refusal is what keeps the answer's SIZE bounded
/// by the budget: an out-of-range offset must not be answered as an empty page
/// over an arbitrarily wide window.
#[tokio::test]
async fn a_cursor_whose_offset_is_past_the_query_list_is_refused() {
    let emulator = emulator();
    let metrics = ["db.load.avg", "db.sampledload.avg"];

    // A legitimately paged request, to obtain a cursor whose scope IS this
    // request's. The cursor is `hex("<offset>|<scope>")`, so the rows below
    // rewrite only the offset and keep the scope the service accepted.
    let t0 = past_start(3_600);
    let mut paged = metrics_input(IDENTIFIER, &metrics, t0, t0 + 3_600, Some(60), None);
    paged["MaxResults"] = json!(1);
    let issued = call(&emulator, "GetResourceMetrics", paged.clone())
        .await
        .member("NextToken")
        .as_str()
        .expect("a truncated page hands back a cursor")
        .to_string();
    let decoded = String::from_utf8(hex::decode(&issued).expect("the cursor is hex"))
        .expect("the cursor is UTF-8");
    let scope = decoded
        .split_once('|')
        .expect("the cursor carries an offset and a scope")
        .1
        .to_string();

    // The rows differ in how far past the two-query list the offset sits.
    for offset in [2_usize, 3, 99, usize::MAX] {
        let crafted = hex::encode(format!("{offset}|{scope}"));
        let mut input = paged.clone();
        input["NextToken"] = json!(crafted);
        let answer = call(&emulator, "GetResourceMetrics", input).await;
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case offset={offset}: expected InvalidArgumentException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.member("MetricList").is_null(),
            "case offset={offset}: expected no MetricList in a refusal, got {:?}",
            answer.body
        );
    }

    // The offset the service DID issue still resumes the page.
    let mut resumed = paged;
    resumed["NextToken"] = json!(issued);
    let answer = call(&emulator, "GetResourceMetrics", resumed).await;
    assert_eq!(
        answer.status, 200,
        "case the issued cursor: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer
            .member("MetricList")
            .as_array()
            .and_then(|list| list.first())
            .and_then(|entry| entry.get("Key"))
            .and_then(|key| key.get("Metric"))
            .and_then(Value::as_str),
        Some(metrics[1]),
        "case the issued cursor: expected the second metric's page, got {:?}",
        answer.body
    );
}

/// RULE: a response the emulator cannot materialise the grid of is refused with
/// the modelled 400, never answered with a truncated series a client could not
/// tell from a complete one. The budget is this emulator's own (one data point
/// per period per metric key has to fit in memory), so what is pinned here is
/// the SHAPE of the answer at the bound: served at it, refused past it, taken
/// over the WHOLE response rather than per series, and paged back inside the
/// bound by `MaxResults`.
#[tokio::test]
async fn a_response_past_the_emulators_data_point_budget_is_refused() {
    let emulator = emulator();

    // The rows differ in the period they reach the bound at, so the rule is
    // about the POINT COUNT rather than about one granularity.
    let budget = service_com_amazonaws_pi_performanceinsightsv20180227::state::MAX_DATA_POINTS_PER_RESPONSE;
    for &period in &[1_i64, 60] {
        // The whole window sits in the past, so the budget is measured against a
        // grid the service holds every period of.
        let t0 = past_start((budget + 1) * period);
        let at_bound = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &["db.load.avg"],
                t0,
                t0 + budget * period,
                Some(period),
                None,
            ),
        )
        .await;
        assert_eq!(
            at_bound.status, 200,
            "case period={period} at the budget: expected 200, got {} {:?}",
            at_bound.status, at_bound.body
        );
        assert_eq!(
            data_points(&at_bound.body, 0).len() as i64,
            budget,
            "case period={period} at the budget: expected the full grid"
        );

        let past_bound = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &["db.load.avg"],
                t0,
                t0 + (budget + 1) * period,
                Some(period),
                None,
            ),
        )
        .await;
        assert_eq!(
            past_bound.status, 400,
            "case period={period} one period past the budget: expected 400, got {} {:?}",
            past_bound.status, past_bound.body
        );
        assert_eq!(
            past_bound.error_code(),
            "InvalidArgumentException",
            "case period={period} one period past the budget: expected \
             InvalidArgumentException, got {:?}",
            past_bound.body
        );
    }

    // The budget is over the whole response: a window half the budget wide is
    // served for one metric key and refused for three, and `MaxResults` pages
    // it back inside.
    let half = budget / 2;
    let half_window_start = past_start(half * 60);
    for (keys, expected_status) in [(1_usize, 200), (2, 200), (3, 400)] {
        let metrics: Vec<String> = (0..keys).map(|index| format!("db.sql.stat{index}.avg")).collect();
        let metrics: Vec<&str> = metrics.iter().map(String::as_str).collect();
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &metrics,
                half_window_start,
                half_window_start + half * 60,
                Some(60),
                None,
            ),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {keys} series of {half} points: expected {expected_status}, got {} {:?}",
            answer.status, answer.body
        );
    }

    let metrics: Vec<String> = (0..3).map(|index| format!("db.sql.stat{index}.avg")).collect();
    let metrics: Vec<&str> = metrics.iter().map(String::as_str).collect();
    let mut paged = metrics_input(
        IDENTIFIER,
        &metrics,
        half_window_start,
        half_window_start + half * 60,
        Some(60),
        None,
    );
    paged["MaxResults"] = json!(1);
    let answer = call(&emulator, "GetResourceMetrics", paged).await;
    assert_eq!(
        answer.status, 200,
        "case the same request paged to one key: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.member("MetricList").as_array().map(Vec::len),
        Some(1),
        "case the same request paged to one key: expected one entry, got {:?}",
        answer.body
    );
}

/// RULE: with no `PeriodInSeconds` the service picks the finest documented
/// granularity that keeps the window inside the documented 100-200 data-point
/// goal — observable in the grid the series carries and in the boundary the
/// start bound is floored to.
#[tokio::test]
async fn the_period_is_chosen_from_the_window_when_the_caller_names_none() {
    let emulator = emulator();

    // Spans from one second to two years, so the chosen period has to move
    // across the whole documented set.
    let spans: &[i64] = &[
        1, 59, 60, 120, 600, 3_600, 7_200, 86_400, 604_800, 2_592_000, 31_536_000, 63_072_000,
    ];

    for &span in spans {
        // The window starts one second past a day boundary — a multiple of every
        // documented period — so the start bound reveals which period was
        // chosen, and the whole window sits in the past.
        let start = past_start(span) + 1;
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(IDENTIFIER, &["db.load.avg"], start, start + span, None, None),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case span={span}: expected 200, got {} {:?}",
            answer.status, answer.body
        );

        let aligned_start = answer.member("AlignedStartTime").as_i64().unwrap_or(-1);

        // The finest documented granularity that keeps the window inside the
        // documented budget, and the coarsest one for a window too long for any
        // of them (two years of daily periods is 730, which no finer period
        // could improve on).
        let expected_period = DOCUMENTED_PERIODS
            .iter()
            .copied()
            .find(|candidate| {
                let periods = span / candidate + i64::from(span % candidate != 0);
                periods <= 200
            })
            .unwrap_or(86_400);
        // The grid the requested window holds at the period the rule above
        // chose: the start floored to it, the end raised to it.
        let end = start + span;
        let expected_start = floor_to(start, expected_period);
        let series_end = ceil_to(end, expected_period);
        let periods_in_window = (series_end - expected_start) / expected_period;

        assert_eq!(
            aligned_start, expected_start,
            "case span={span}: expected the start floored to the {expected_period}s \
             boundary at {expected_start}, got {aligned_start}"
        );
        let points = data_points(&answer.body, 0);
        assert_eq!(
            points.len() as i64,
            periods_in_window,
            "case span={span}: expected {periods_in_window} periods of {expected_period}s \
             across {expected_start}..{series_end}, got {} points",
            points.len()
        );
        // The spacing between points is the chosen period itself.
        if let (Some(first), Some(second)) = (points.first(), points.get(1)) {
            let spacing = second.get("Timestamp").and_then(Value::as_i64).unwrap_or(0)
                - first.get("Timestamp").and_then(Value::as_i64).unwrap_or(0);
            assert_eq!(
                spacing, expected_period,
                "case span={span}: expected the points {expected_period}s apart, got {spacing}"
            );
        }
        if expected_period != 86_400 {
            assert!(
                periods_in_window <= 200,
                "case span={span}: expected roughly 100-200 periods at most, got \
                 {periods_in_window}"
            );
        }
    }
}

/// RULE: `PeriodInSeconds` accepts the documented granularities and nothing
/// else.
#[tokio::test]
async fn period_in_seconds_accepts_exactly_the_documented_granularities() {
    let emulator = emulator();

    for &period in DOCUMENTED_PERIODS {
        // A window of one period, so even the one-second granularity stays
        // small.
        let t0 = past_start(period);
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &["db.load.avg"],
                t0,
                t0 + period,
                Some(period),
                None,
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case period={period} (documented): expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    // Each row is one period off the documented set, including values just
    // below and just above every published one.
    let t0 = past_start(3_600);
    for period in [0, 2, 30, 59, 61, 299, 301, 3_599, 3_601, 86_399, 86_401, 172_800] {
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &["db.load.avg"],
                t0,
                t0 + 3_600,
                Some(period),
                None,
            ),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case period={period} (not documented): expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case period={period} (not documented): expected InvalidArgumentException, got {:?}",
            answer.body
        );
    }
}

/// RULE: one `MetricList` entry per `MetricQueries` element, keyed by that
/// query's metric, in request order.
#[tokio::test]
async fn one_metric_list_entry_per_query_keyed_by_its_metric() {
    let emulator = emulator();

    // The rows differ in count (1, 2, 4 and the modelled maximum of 15), in the
    // metrics named, and in whether a metric repeats.
    let fifteen: Vec<String> = (0..15)
        .map(|index| format!("db.sql.stat{index}.avg"))
        .collect();
    let fifteen: Vec<&str> = fifteen.iter().map(String::as_str).collect();
    let cases: Vec<Vec<&str>> = vec![
        vec!["db.load.avg"],
        vec!["db.load.avg", "db.sampledload.avg"],
        vec![
            "db.SQL.Innodb_rows_read.avg",
            "os.cpuUtilization.total.avg",
            "db.load.avg",
            "db.Transactions.xact_commit.avg",
        ],
        vec!["db.load.avg", "db.load.avg"],
        fifteen,
    ];

    for metrics in cases {
        let t0 = past_start(3_600);
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &metrics,
                t0,
                t0 + 3_600,
                Some(300),
                Some("END_TIME"),
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {metrics:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );

        let keys: Vec<&str> = answer
            .member("MetricList")
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|entry| {
                        entry
                            .get("Key")
                            .and_then(|key| key.get("Metric"))
                            .and_then(Value::as_str)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            keys, metrics,
            "case {metrics:?}: expected one entry per query keyed by its metric, got {keys:?}"
        );
        assert!(
            answer.member("NextToken").is_null(),
            "case {metrics:?}: expected no NextToken without MaxResults, got {:?}",
            answer.body
        );
    }
}

/// RULE: the series is computed from the request and nothing else, so repeating
/// a call answers the same series and the same start bound — and no entry
/// carries an invented magnitude. The END bound is the service's clock floored
/// to the period, so it is the one member a repeat may advance (never a value
/// the request decided).
#[tokio::test]
async fn repeating_the_same_request_answers_the_same_series() {
    let emulator = emulator();

    let t0 = past_start(3_600);
    let input = metrics_input(
        IDENTIFIER,
        &["db.load.avg", "os.cpuUtilization.total.avg"],
        t0 + 17,
        t0 + 3_600,
        Some(60),
        Some("END_TIME"),
    );

    let first = call(&emulator, "GetResourceMetrics", input.clone()).await;
    let second = call(&emulator, "GetResourceMetrics", input.clone()).await;
    assert_eq!(
        first.status, 200,
        "case the first call: expected 200, got {} {:?}",
        first.status, first.body
    );
    for member in ["Identifier", "AlignedStartTime", "MetricList", "NextToken"] {
        assert_eq!(
            first.member(member),
            second.member(member),
            "case repeated call: expected the same {member}, got {:?} then {:?}",
            first.member(member),
            second.member(member)
        );
    }
    let (first_end, second_end) = (
        first.member("AlignedEndTime").as_i64().unwrap_or(-1),
        second.member("AlignedEndTime").as_i64().unwrap_or(-1),
    );
    assert!(
        second_end >= first_end,
        "case repeated call: expected the end bound to track the clock forward, got \
         {first_end} then {second_end}"
    );

    // The window is 60 periods of 60 seconds wide ([t0 + 17, t0 + 3600) aligned
    // to the nearest minute boundaries), and both entries carry that same grid.
    for index in 0..2 {
        let series = data_points(&first.body, index);
        assert_eq!(
            series.len(),
            60,
            "case entry {index}: expected the 60-period grid, got {} points",
            series.len()
        );
    }
}

/// RULE: `MaxResults` pages the metric keys, and the cursor resumes exactly
/// where the page stopped.
#[tokio::test]
async fn max_results_pages_the_metric_keys_exactly_once() {
    let emulator = emulator();
    let metrics = [
        "db.load.avg",
        "db.sampledload.avg",
        "os.cpuUtilization.total.avg",
        "db.Transactions.xact_commit.avg",
        "db.SQL.Innodb_rows_read.avg",
    ];

    for page_size in [1, 2, 3, 5, 6, 25] {
        let t0 = past_start(3_600);
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut input = metrics_input(
                IDENTIFIER,
                &metrics,
                t0,
                t0 + 3_600,
                Some(3_600),
                Some("END_TIME"),
            );
            input["MaxResults"] = json!(page_size);
            if let Some(token) = &next_token {
                input["NextToken"] = json!(token);
            }

            let answer = call(&emulator, "GetResourceMetrics", input).await;
            assert_eq!(
                answer.status, 200,
                "case page_size={page_size} page {pages}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let page = answer
                .member("MetricList")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page_size={page_size} page {pages}: expected at most {page_size} entries, \
                 got {}",
                page.len()
            );
            for entry in &page {
                seen.push(
                    entry
                        .get("Key")
                        .and_then(|key| key.get("Metric"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }

            next_token = answer
                .member("NextToken")
                .as_str()
                .map(ToString::to_string);
            if let Some(token) = &next_token {
                assert!(
                    is_modelled_next_token(token),
                    "case page_size={page_size} page {pages}: expected a NextToken matching \
                     ^[a-zA-Z0-9_=-]+$, got {token:?}"
                );
            }
            pages += 1;
            if next_token.is_none() || pages > 10 {
                break;
            }
        }

        assert_eq!(
            seen,
            metrics.to_vec(),
            "case page_size={page_size}: expected every metric key exactly once in request \
             order, got {seen:?}"
        );
    }

    // A cursor from one window does not resume another: the result set it was
    // issued for is part of the token.
    let t0 = past_start(90_000);
    let mut first = metrics_input(
        IDENTIFIER,
        &metrics,
        t0,
        t0 + 3_600,
        Some(3_600),
        Some("END_TIME"),
    );
    first["MaxResults"] = json!(1);
    let token = call(&emulator, "GetResourceMetrics", first)
        .await
        .member("NextToken")
        .as_str()
        .expect("a truncated page hands back a cursor")
        .to_string();

    let mut elsewhere = metrics_input(
        IDENTIFIER,
        &metrics,
        t0 + 86_400,
        t0 + 90_000,
        Some(3_600),
        Some("END_TIME"),
    );
    elsewhere["MaxResults"] = json!(1);
    elsewhere["NextToken"] = json!(token);
    let reused = call(&emulator, "GetResourceMetrics", elsewhere).await;
    assert_eq!(
        reused.error_code(),
        "InvalidArgumentException",
        "case another window's cursor: expected InvalidArgumentException, got {} {:?}",
        reused.status,
        reused.body
    );
}

/// RULE: documented on `StartTime` — "The value for StartTime must be earlier
/// than the value for EndTime."
#[tokio::test]
async fn a_window_whose_end_does_not_follow_its_start_is_refused() {
    let emulator = emulator();

    let t0 = past_start(86_400);
    for (start, end, label) in [
        (t0, t0, "equal"),
        (t0, t0 - 1, "one second inverted"),
        (t0, t0 - 86_400, "a day inverted"),
    ] {
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(IDENTIFIER, &["db.load.avg"], start, end, Some(60), None),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {label}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case {label}: expected InvalidArgumentException, got {:?}",
            answer.body
        );
    }
}

/// RULE: a caller-chosen `ISOTimestamp` is bounded before any arithmetic reads
/// it. A timestamp shape carries no `@range`, so an extreme value reaches the
/// handler: one the window arithmetic could not hold must come back as the
/// modelled 400, never as a 5xx from an overflow — and one it CAN hold must be
/// answered, however far from today it is.
#[tokio::test]
async fn extreme_timestamps_are_refused_rather_than_overflowing() {
    let emulator = emulator();

    // Beyond what the window arithmetic can hold: far-negative and
    // far-positive ends, and values at the edges of what the wire can carry.
    let refused: Vec<(&str, f64)> = vec![
        ("both ends at opposite extremes", 9.0e18),
        ("an end near i64::MAX", 9.2e18),
        ("an end near i64::MIN", -9.2e18),
        ("an end past a quarter of i64::MAX", 3.0e18),
    ];

    for (label, magnitude) in refused {
        for period in [None, Some(1_i64), Some(60), Some(86_400)] {
            let mut input = json!({
                "ServiceType": "RDS",
                "Identifier": IDENTIFIER,
                "MetricQueries": [{"Metric": "db.load.avg"}],
                "StartTime": -magnitude.abs(),
                "EndTime": magnitude.abs(),
            });
            if let Some(period) = period {
                input["PeriodInSeconds"] = json!(period);
            }

            let answer = call(&emulator, "GetResourceMetrics", input).await;
            assert_eq!(
                answer.status, 400,
                "case {label} period={period:?}: expected the modelled 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgumentException",
                "case {label} period={period:?}: expected InvalidArgumentException, got {:?}",
                answer.body
            );
        }
    }

    // Windows far from today that the arithmetic CAN hold are still answered:
    // the bound refuses nothing the handler could have computed.
    let served: Vec<(&str, i64, i64)> = vec![
        ("a window before the epoch", -86_400, -3_600),
        ("a window spanning the epoch", -3_600, 3_600),
        (
            "a window long past year 9999",
            300_000_000_000,
            300_000_360_000,
        ),
    ];

    for (label, start, end) in served {
        let before = unix_now();
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(IDENTIFIER, &["db.load.avg"], start, end, Some(3_600), None),
        )
        .await;
        let after = unix_now();
        assert_eq!(
            answer.status, 200,
            "case {label}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let aligned_start = answer.member("AlignedStartTime").as_i64().unwrap_or(1);
        let aligned_end = answer.member("AlignedEndTime").as_i64().unwrap_or(1);
        assert_eq!(
            (aligned_start.rem_euclid(3_600), aligned_end.rem_euclid(3_600)),
            (0, 0),
            "case {label}: expected both ends on a 3600s boundary, got \
             {aligned_start}..{aligned_end}"
        );
        assert_eq!(
            aligned_start,
            floor_to(start, 3_600),
            "case {label}: expected the start floored to the 3600s boundary, got {aligned_start}"
        );
        // A window the service's clock has already passed reports the latest
        // boundary it holds; one that has not been reached yet holds NOTHING, so
        // its reported window is empty and carries no point.
        let points = data_points(&answer.body, 0);
        if aligned_start <= floor_to(before, 3_600) {
            let candidates = aligned_end_candidates(before, after, 3_600);
            assert!(
                candidates.contains(&aligned_end),
                "case {label}: expected AlignedEndTime at the current 3600s boundary \
                 ({candidates:?}), got {aligned_end}"
            );
            assert_eq!(
                points.len() as i64,
                (ceil_to(end, 3_600).min(aligned_end) - aligned_start) / 3_600,
                "case {label}: expected the grid of the held part of the window, got {} points",
                points.len()
            );
        } else {
            assert_eq!(
                (aligned_end, points.len()),
                (aligned_start, 0),
                "case {label}: expected an empty window for one the clock has not reached, \
                 got {aligned_start}..{aligned_end} with {} points",
                points.len()
            );
        }
    }

    // The emulator still answers normally afterwards: nothing was wedged.
    let t0 = past_start(3_600);
    let healthy = call(
        &emulator,
        "GetResourceMetrics",
        metrics_input(IDENTIFIER, &["db.load.avg"], t0, t0 + 3_600, Some(60), None),
    )
    .await;
    assert_eq!(
        healthy.status, 200,
        "case a sane window after the extremes: expected 200, got {} {:?}",
        healthy.status, healthy.body
    );
}

/// RULE: what the emulator cannot compute faithfully it refuses honestly.
/// Dimension-grouped and filtered series are computed from individual DB
/// sessions, which this emulator does not hold, so those queries answer 501
/// instead of returning the ungrouped series under a grouped key.
#[tokio::test]
async fn grouped_or_filtered_queries_answer_501_rather_than_fabricating() {
    let emulator = emulator();

    let cases: Vec<(&str, Value)> = vec![
        (
            "GroupBy",
            json!({"Metric": "db.load.avg", "GroupBy": {"Group": "db.sql"}}),
        ),
        (
            "GroupBy with dimensions",
            json!({
                "Metric": "db.load.avg",
                "GroupBy": {"Group": "db.sql", "Dimensions": ["db.sql.id"], "Limit": 5},
            }),
        ),
        (
            "Filter",
            json!({"Metric": "db.load.avg", "Filter": {"db.user.name": "postgres"}}),
        ),
    ];

    let t0 = past_start(3_600);
    for (label, query) in cases {
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            json!({
                "ServiceType": "RDS",
                "Identifier": IDENTIFIER,
                "MetricQueries": [query],
                "StartTime": t0,
                "EndTime": t0 + 3_600,
                "PeriodInSeconds": 300,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 501,
            "case {label}: expected 501 NotImplemented, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotImplemented",
            "case {label}: expected NotImplemented, got {:?}",
            answer.body
        );
    }

    // An empty filter map asks for nothing to be filtered, so it is served.
    let unfiltered = call(
        &emulator,
        "GetResourceMetrics",
        json!({
            "ServiceType": "RDS",
            "Identifier": IDENTIFIER,
            "MetricQueries": [{"Metric": "db.load.avg", "Filter": {}}],
            "StartTime": t0,
            "EndTime": t0 + 3_600,
            "PeriodInSeconds": 300,
        }),
    )
    .await;
    assert_eq!(
        unfiltered.status, 200,
        "case an empty Filter: expected 200, got {} {:?}",
        unfiltered.status, unfiltered.body
    );
}

/// RULE: reading metrics changes nothing a later caller sees — the report store
/// is untouched by a `GetResourceMetrics` call.
#[tokio::test]
async fn reading_metrics_does_not_touch_the_report_store() {
    let emulator = emulator();

    let t0 = past_start(3_600);
    let created = call(
        &emulator,
        "CreatePerformanceAnalysisReport",
        json!({
            "ServiceType": "RDS",
            "Identifier": IDENTIFIER,
            "StartTime": t0,
            "EndTime": t0 + 3_600,
        }),
    )
    .await;
    let before = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({"ServiceType": "RDS", "Identifier": IDENTIFIER}),
    )
    .await;

    for _ in 0..3 {
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(IDENTIFIER, &["db.load.avg"], t0, t0 + 3_600, Some(60), None),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case a metric read: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    let after = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({"ServiceType": "RDS", "Identifier": IDENTIFIER}),
    )
    .await;

    // The ids are what the store holds; the status is the clock's business, so
    // it is deliberately not part of this comparison.
    let ids = |answer: &Value| -> Vec<String> {
        answer
            .get("AnalysisReports")
            .and_then(Value::as_array)
            .map(|reports| {
                reports
                    .iter()
                    .filter_map(|report| {
                        report
                            .get("AnalysisReportId")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        ids(&before.body),
        ids(&after.body),
        "case metric reads around a listing: expected the report store untouched, got {:?} \
         then {:?} (created {:?})",
        before.body,
        after.body,
        created.body
    );
    assert_eq!(
        ids(&after.body).len(),
        1,
        "case metric reads around a listing: expected the one created report to still be \
         there, got {:?}",
        after.body
    );
}

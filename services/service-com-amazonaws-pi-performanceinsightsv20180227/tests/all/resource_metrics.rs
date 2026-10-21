// SPDX-License-Identifier: CC0-1.0

//! `GetResourceMetrics`, end to end over the wire.
//!
//! The rules under test are the ones the model and the member documentation
//! state: `AlignedStartTime` sits on a period boundary at or before the
//! requested `StartTime` and `AlignedEndTime` on one at or after the requested
//! `EndTime`; the period is one of the documented granularities, chosen from the
//! window when the caller names none; `PeriodAlignment` decides whether a point
//! is stamped with its period's start or its end (`END_TIME` by default); one
//! `MetricList` entry per `MetricQueries` element; and a repeated call answers
//! identically because nothing is stored.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator, is_modelled_next_token};

/// 2026-01-01T00:00:00Z. Windows below are stated relative to it, including ones
/// that start and end off a period boundary.
const T0: i64 = 1_767_225_600;

/// The granularities `PeriodInSeconds` documents: 1 (one second), 60 (one
/// minute), 300 (five minutes), 3600 (one hour), 86400 (twenty-four hours).
const DOCUMENTED_PERIODS: &[i64] = &[1, 60, 300, 3600, 86400];

const IDENTIFIER: &str = "db-ABCDEFG1234567";

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

/// RULE: the returned window is the requested one widened to the effective
/// period's NEAREST boundaries, and the series for the requested metric carries
/// no invented magnitudes.
#[tokio::test]
async fn the_window_is_widened_to_the_nearest_period_boundaries() {
    let emulator = emulator();

    // The rows differ along what the rule generalises over: the period (every
    // documented value, including both published bounds), whether the requested
    // start and end already sit on a boundary, and the alignment.
    let mut cases: Vec<(i64, i64, i64, Option<&str>)> = Vec::new();
    for &period in DOCUMENTED_PERIODS {
        for (start_offset, end_offset) in [
            (0, period),              // both ends already aligned
            (1, period + 1),          // both ends inside a period
            (period - 1, period * 2), // start just before a boundary
            (0, period * 3 + 7),      // an end that needs widening
        ] {
            for alignment in [None, Some("START_TIME"), Some("END_TIME")] {
                cases.push((T0 + start_offset, T0 + end_offset, period, alignment));
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
            aligned_start % period,
            0,
            "case {label}: expected AlignedStartTime on a {period}s boundary, got {aligned_start}"
        );
        assert_eq!(
            aligned_end % period,
            0,
            "case {label}: expected AlignedEndTime on a {period}s boundary, got {aligned_end}"
        );
        assert!(
            aligned_start <= start,
            "case {label}: expected AlignedStartTime <= StartTime, got {aligned_start}"
        );
        assert!(
            aligned_end >= end,
            "case {label}: expected AlignedEndTime >= EndTime, got {aligned_end}"
        );
        // Widened by less than one period at each end, i.e. the NEAREST
        // boundary, not an arbitrary earlier or later one.
        assert!(
            start - aligned_start < period && aligned_end - end < period,
            "case {label}: expected the nearest boundaries, got {aligned_start}..{aligned_end}"
        );

        // Performance Insights reports counters measured on a running DB
        // engine, which this emulator does not monitor, so the series is empty
        // rather than carrying a value per period that nothing measured.
        let points = data_points(&answer.body, 0);
        assert!(
            points.is_empty(),
            "case {label}: expected no invented data points, got {points:?}"
        );
    }
}

/// RULE: with no `PeriodInSeconds` the service picks the finest documented
/// granularity that keeps the window inside the documented 100-200 data-point
/// goal — observable in the boundaries the window is widened to.
#[tokio::test]
async fn the_period_is_chosen_from_the_window_when_the_caller_names_none() {
    let emulator = emulator();

    // Spans from one second to two years, so the chosen period has to move
    // across the whole documented set.
    let spans: &[i64] = &[
        1, 59, 60, 120, 600, 3_600, 7_200, 86_400, 604_800, 2_592_000, 31_536_000, 63_072_000,
    ];

    for &span in spans {
        // T0 is a multiple of every documented period, so starting one second
        // past it makes the widened window reveal which period was chosen.
        let start = T0 + 1;
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
        let aligned_end = answer.member("AlignedEndTime").as_i64().unwrap_or(-1);

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
        // The window the documented alignment rule widens this request to, at
        // the period the rule above chose.
        let end = start + span;
        let expected_start = start - start.rem_euclid(expected_period);
        let covered = end - expected_start;
        let periods_in_window =
            covered / expected_period + i64::from(covered % expected_period != 0);
        let expected_end = expected_start + periods_in_window * expected_period;

        assert_eq!(
            aligned_start, expected_start,
            "case span={span}: expected the window widened down to the {expected_period}s \
             boundary at {expected_start}, got {aligned_start}"
        );
        assert_eq!(
            aligned_end, expected_end,
            "case span={span}: expected a window of {periods_in_window} periods of \
             {expected_period}s ending at {expected_end}, got {aligned_start}..{aligned_end}"
        );
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
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &["db.load.avg"],
                T0,
                T0 + period,
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
    for period in [0, 2, 30, 59, 61, 299, 301, 3_599, 3_601, 86_399, 86_401, 172_800] {
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &["db.load.avg"],
                T0,
                T0 + 3_600,
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
    let fifteen: Vec<String> = (0..15).map(|index| format!("db.sql.stat{index}")).collect();
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
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(
                IDENTIFIER,
                &metrics,
                T0,
                T0 + 3_600,
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

/// RULE: the answer is computed from the request and nothing else, so repeating
/// a call answers identically — and no entry carries an invented magnitude.
#[tokio::test]
async fn repeating_the_same_request_answers_identically() {
    let emulator = emulator();

    let input = metrics_input(
        IDENTIFIER,
        &["db.load.avg", "os.cpuUtilization.total.avg"],
        T0 + 17,
        T0 + 3_600,
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
    assert_eq!(
        first.body, second.body,
        "case repeated call: expected an identical answer, got {:?} then {:?}",
        first.body, second.body
    );

    for index in 0..2 {
        let series = data_points(&first.body, index);
        assert!(
            series.is_empty(),
            "case entry {index}: expected no invented data points, got {series:?}"
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
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut input = metrics_input(
                IDENTIFIER,
                &metrics,
                T0,
                T0 + 3_600,
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
    let mut first = metrics_input(
        IDENTIFIER,
        &metrics,
        T0,
        T0 + 3_600,
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
        T0 + 86_400,
        T0 + 90_000,
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

    for (start, end, label) in [
        (T0, T0, "equal"),
        (T0, T0 - 1, "one second inverted"),
        (T0, T0 - 86_400, "a day inverted"),
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
        ("a window long past year 9999", 300_000_000_000, 400_000_000_000),
    ];

    for (label, start, end) in served {
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            metrics_input(IDENTIFIER, &["db.load.avg"], start, end, Some(3_600), None),
        )
        .await;
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
        assert!(
            aligned_start <= start && aligned_end >= end,
            "case {label}: expected the window to cover {start}..{end}, got \
             {aligned_start}..{aligned_end}"
        );
    }

    // The emulator still answers normally afterwards: nothing was wedged.
    let healthy = call(
        &emulator,
        "GetResourceMetrics",
        metrics_input(IDENTIFIER, &["db.load.avg"], T0, T0 + 3_600, Some(60), None),
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

    for (label, query) in cases {
        let answer = call(
            &emulator,
            "GetResourceMetrics",
            json!({
                "ServiceType": "RDS",
                "Identifier": IDENTIFIER,
                "MetricQueries": [query],
                "StartTime": T0,
                "EndTime": T0 + 3_600,
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
            "StartTime": T0,
            "EndTime": T0 + 3_600,
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

    let created = call(
        &emulator,
        "CreatePerformanceAnalysisReport",
        json!({
            "ServiceType": "RDS",
            "Identifier": IDENTIFIER,
            "StartTime": T0,
            "EndTime": T0 + 3_600,
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
            metrics_input(IDENTIFIER, &["db.load.avg"], T0, T0 + 3_600, Some(60), None),
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

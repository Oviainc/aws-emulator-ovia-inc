// SPDX-License-Identifier: CC0-1.0

//! The aggregate-function rule on a metric name, end to end over the wire.
//!
//! RULE under test: a metric name a request names must carry the aggregate
//! function the series is computed with, and a name that carries none of the
//! documented functions is refused with the modelled
//! `InvalidArgumentException` (HTTP 400) before the request is answered.
//!
//! The functions are the ones `GetResourceMetrics$MetricQueries` documents on
//! the member itself — "You must append the aggregate function to the metric.
//! For example, to find the average for the metric db.load you must use
//! db.load.avg. Valid values for aggregate functions include .avg, .min, .max,
//! and .sum." — and `DescribeDimensionKeys$AdditionalMetrics` narrows its own
//! set to one of them: "The only supported statistic function is .avg."
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over:
//! which function is appended, which metric it is appended to, whether a
//! function is appended at all, where in a list the offending name sits, and
//! which operation names the metric.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator};

/// 2026-01-01T00:00:00Z.
const T0: i64 = 1_767_225_600;

/// A monitored data source, so the aggregate-function rule is the only reason a
/// request below could be refused.
const IDENTIFIER: &str = "db-ABCDEFG1234567";

/// Metric names that DO name an aggregate function: every documented function
/// (`.avg`, `.min`, `.max`, `.sum`), on several of the metrics Performance
/// Insights publishes (`db.load`, `db.sampledload`, the counter metrics
/// `db.SQL.*` / `db.Transactions.*`, and an OS metric), including rows no
/// recording cites.
const ACCEPTED: &[&str] = &[
    "db.load.avg",
    "db.load.min",
    "db.load.max",
    "db.load.sum",
    "db.sampledload.avg",
    "db.SQL.Innodb_rows_read.sum",
    "db.Transactions.xact_commit.max",
    "os.cpuUtilization.total.avg",
    "os.memory.free.min",
];

/// Metric names that name NO aggregate function: the bare metric (the recorded
/// case), a bare metric of another family, a statistic that is not one of the
/// four, a function spelled in another case (metric names are case-sensitive —
/// `db.SQL.Innodb_rows_read.avg` mixes case deliberately), a function with no
/// metric in front of it, and a function with no separating dot.
const REFUSED: &[&str] = &[
    "db.load",
    "db.sampledload",
    "os.cpuUtilization.total",
    "db.load.average",
    "db.load.p99",
    "db.load.count",
    "db.load.AVG",
    ".avg",
    "avg",
    "db.loadavg",
];

fn metrics_input(metrics: &[&str]) -> Value {
    json!({
        "ServiceType": "RDS",
        "Identifier": IDENTIFIER,
        "MetricQueries": metrics
            .iter()
            .map(|metric| json!({"Metric": metric}))
            .collect::<Vec<_>>(),
        "StartTime": T0,
        "EndTime": T0 + 3_600,
        "PeriodInSeconds": 300,
    })
}

fn dimension_keys_input(metric: &str) -> Value {
    json!({
        "ServiceType": "RDS",
        "Identifier": IDENTIFIER,
        "StartTime": T0,
        "EndTime": T0 + 3_600,
        "Metric": metric,
        "GroupBy": {"Group": "db.sql"},
    })
}

/// The sentence this service answers an unknown statistic with. Restated here
/// as the recorded text rather than imported from the crate, so the assertion
/// cannot follow the code wherever it goes.
const UNKNOWN_STATISTIC_MESSAGE: &str = "The specified statistic is not a known statistic";

/// RULE: a `MetricQueries` entry whose metric names no aggregate function is
/// refused; one that names a documented function is answered.
#[tokio::test]
async fn get_resource_metrics_requires_an_aggregate_function_on_every_query() {
    let emulator = emulator();

    for metric in REFUSED {
        let answer = call(&emulator, "GetResourceMetrics", metrics_input(&[metric])).await;
        assert_eq!(
            answer.status, 400,
            "case {metric:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case {metric:?}: expected InvalidArgumentException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.error_message(),
            UNKNOWN_STATISTIC_MESSAGE,
            "case {metric:?}: expected the service's unknown-statistic message, got {:?}",
            answer.body
        );
        // The refusal answers nothing about the series: no metric key AWS
        // would never return comes back beside it.
        assert!(
            answer.member("MetricList").is_null(),
            "case {metric:?}: expected no MetricList in a refusal, got {:?}",
            answer.body
        );
    }

    for metric in ACCEPTED {
        let answer = call(&emulator, "GetResourceMetrics", metrics_input(&[metric])).await;
        assert_eq!(
            answer.status, 200,
            "case {metric:?}: expected 200, got {} {:?}",
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
            Some(*metric),
            "case {metric:?}: expected the series keyed by the requested metric, got {:?}",
            answer.body
        );
    }
}

/// RULE: the rule holds for EVERY query of a list, wherever the offending one
/// sits — one bad metric refuses the whole request, so a client never gets a
/// partial answer it could mistake for a complete one.
#[tokio::test]
async fn one_query_without_an_aggregate_function_refuses_the_whole_request() {
    let emulator = emulator();

    // The rows differ in the position of the offending entry (first, middle,
    // last) and in the length of the list.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("the only query", vec!["db.load"]),
        ("the first of two", vec!["db.load", "db.load.avg"]),
        ("the second of two", vec!["db.load.avg", "db.load"]),
        (
            "the middle of three",
            vec!["db.load.avg", "os.cpuUtilization.total", "db.sampledload.avg"],
        ),
        (
            "the last of five",
            vec![
                "db.load.avg",
                "db.sampledload.avg",
                "db.SQL.Innodb_rows_read.sum",
                "os.cpuUtilization.total.avg",
                "db.Transactions.xact_commit",
            ],
        ),
    ];

    for (label, metrics) in cases {
        let answer = call(&emulator, "GetResourceMetrics", metrics_input(&metrics)).await;
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case {label} ({metrics:?}): expected InvalidArgumentException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // A list in which EVERY metric names a function is answered in full.
    let all_good = vec![
        "db.load.avg",
        "db.sampledload.avg",
        "db.SQL.Innodb_rows_read.sum",
        "os.cpuUtilization.total.avg",
        "db.Transactions.xact_commit.max",
    ];
    let answer = call(&emulator, "GetResourceMetrics", metrics_input(&all_good)).await;
    assert_eq!(
        answer.member("MetricList").as_array().map(Vec::len),
        Some(all_good.len()),
        "case every query names a function: expected one entry per query, got {:?}",
        answer.body
    );
}

/// RULE: the same rule governs `DescribeDimensionKeys$Metric`, whose own
/// documented values (`db.load.avg`, `db.sampledload.avg`) carry the function
/// too.
#[tokio::test]
async fn describe_dimension_keys_requires_an_aggregate_function_on_its_metric() {
    let emulator = emulator();

    for metric in REFUSED {
        let answer = call(&emulator, "DescribeDimensionKeys", dimension_keys_input(metric)).await;
        assert_eq!(
            answer.status, 400,
            "case {metric:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case {metric:?}: expected InvalidArgumentException, got {:?}",
            answer.body
        );
        assert!(
            answer.member("Keys").is_null(),
            "case {metric:?}: expected no Keys in a refusal, got {:?}",
            answer.body
        );
    }

    for metric in ACCEPTED {
        let answer = call(&emulator, "DescribeDimensionKeys", dimension_keys_input(metric)).await;
        assert_eq!(
            answer.status, 200,
            "case {metric:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// RULE: `AdditionalMetrics` narrows the set to `.avg` alone ("The only
/// supported statistic function is .avg"), so a documented function that is not
/// `.avg` is refused THERE while it is accepted on `Metric`.
#[tokio::test]
async fn additional_metrics_accept_only_the_average_function() {
    let emulator = emulator();

    // The rows differ along the function appended and the count of entries.
    let accepted: Vec<Vec<&str>> = vec![
        vec!["db.sql_tokenized.stats.sum_latency_per_sec.avg"],
        vec!["db.sql.stats.calls_per_sec.avg", "db.sql.stats.rows_per_sec.avg"],
    ];
    for metrics in accepted {
        let mut input = dimension_keys_input("db.load.avg");
        input["AdditionalMetrics"] = json!(metrics);
        let answer = call(&emulator, "DescribeDimensionKeys", input).await;
        assert_eq!(
            answer.status, 200,
            "case {metrics:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    let refused: Vec<Vec<&str>> = vec![
        // A documented aggregate function that is not the only supported one.
        vec!["db.sql.stats.calls_per_sec.sum"],
        vec!["db.sql.stats.calls_per_sec.min"],
        vec!["db.sql.stats.calls_per_sec.max"],
        // No function at all.
        vec!["db.sql.stats.calls_per_sec"],
        // One good entry and one bad one: the request is refused as a whole.
        vec![
            "db.sql.stats.calls_per_sec.avg",
            "db.sql.stats.rows_per_sec.sum",
        ],
    ];
    for metrics in refused {
        let mut input = dimension_keys_input("db.load.avg");
        input["AdditionalMetrics"] = json!(metrics);
        let answer = call(&emulator, "DescribeDimensionKeys", input).await;
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case {metrics:?}: expected InvalidArgumentException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

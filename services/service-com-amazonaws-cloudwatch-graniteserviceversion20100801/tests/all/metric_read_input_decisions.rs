// SPDX-License-Identifier: CC0-1.0

//! WHICH metric-read, metric-listing and metric-stream requests this service accepts, and which
//! it refuses — the accept/refuse decision itself, over a table of inputs for each rule.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one request:
//!
//! * a STATISTIC is named by its own spelling: the basic five as the `Statistic` enumerated type
//!   declares them and the percentile form with its lower-case `p`, so a name differing only in
//!   case is no statistic (`MetricStat$Stat` is an unconstrained string, so nothing but the
//!   grammar tells a statistic from a typo);
//! * `GetMetricStatisticsInput$ExtendedStatistics` carries PERCENTILES only ("The percentile
//!   statistics. Specify values between p0.0 and p100."), and a name it cannot compute is
//!   refused naming the element it arrived on;
//! * a `MetricStat.Stat` outside the grammar is refused naming the query the request put it on,
//!   whatever position that is;
//! * a metric-stream filter states the namespace it selects, so one carrying none is refused
//!   naming the filter's own position in the list it arrived in;
//! * the two statistic members an alarm read filters by are mutually exclusive;
//! * a metric READ's range must lie inside the span of instants the service reads;
//! * a range and a granularity spanning more datapoints than one page carries are PAGED, not
//!   refused: each page reports its results partial and hands back a token, and following the
//!   tokens covers the range and terminates;
//! * a metric LISTING applies a dimension filter for every dimension a metric can carry, and a
//!   filter that matches nothing is an empty listing rather than a refusal;
//! * an EMPTY `NextToken` names no page, so it is read as a request for the first one.

use serde_json::{json, Value};

use crate::harness::{
    build_router, call, error_code, error_message, now_secs, ok, DEFAULT_REGION, QUERY_ROLE,
};

/// The namespace every series this suite publishes into lives in.
const NAMESPACE: &str = "MyApp/ReadDecisions";

/// The metric those series carry.
const METRIC: &str = "Latency";

/// A minute, in the epoch seconds a request's range is named in.
const MINUTE: f64 = 60.0;

/// The start of the minute before the one the clock is in, so a published datapoint and the
/// window a read lays over it stay in step.
fn recent() -> f64 {
    (now_secs() / MINUTE).floor() * MINUTE - 10.0 * MINUTE
}

/// A `GetMetricStatistics` request over the suite's metric for the minute at `at`, carrying
/// `patch` on top.
fn statistics_read(at: f64, patch: Value) -> Value {
    crate::harness::merged(
        json!({
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "StartTime": at,
            "EndTime": at + MINUTE,
            "Period": 60,
        }),
        &patch,
    )
}

/// One `MetricStat` query under the id `id`, folded by `stat`.
fn stat_query(id: &str, stat: &str) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": {"Namespace": NAMESPACE, "MetricName": METRIC},
            "Period": 60,
            "Stat": stat,
        },
    })
}

/// A `GetMetricData` request over `queries` for the minute at `at`.
fn data_read(at: f64, queries: Value) -> Value {
    json!({
        "MetricDataQueries": queries,
        "StartTime": at,
        "EndTime": at + MINUTE,
    })
}

/// The refusal a percentile-only member answers an element it cannot compute with, as the
/// recorded sentence writes it: the value, and the element's path down to the list's own target
/// shape.
fn not_supported(value: &str, position: usize) -> String {
    format!(
        "The value {value} for parameter ExtendedStatistics.member.{position}.<list element> is \
         not supported."
    )
}

/// The refusal a query array answers a `Stat` outside the statistic grammar with: the member
/// path the name arrived on, and the terminator the request-validation layer ends its sentences
/// with.
fn not_a_valid_statistic(position: usize) -> String {
    format!(
        "The value for parameter MetricDataQueries.member.{position}.MetricStat.Stat is not a \
         valid statistic.\n"
    )
}

/// The statistic names CloudWatch's "CloudWatch statistics definitions" page lists, in the
/// spellings it writes them in — the source for every row below that names a SUPPORTED
/// statistic.
const DEFINED_STATISTICS: &[&str] = &[
    "SampleCount",
    "Average",
    "Sum",
    "Minimum",
    "Maximum",
    "IQM",
    "p0",
    "p50",
    "p99.9",
    "p100",
    "tm90",
    "wm90",
    "TM(10%:90%)",
    "TC(0.5:2.5)",
    "ts(100:)",
    "PR(:300)",
];

/// Names that are NO statistic of that page: a basic statistic or the interquartile mean in
/// another case, a percentile with an upper-case prefix, a percentile outside `p0.0..p100`, a
/// bounded family with no span at all, and a name of no family.
const UNDEFINED_STATISTICS: &[&str] = &[
    "average",
    "SUM",
    "SampleCOUNT",
    "iqm",
    "P90",
    "P0.0",
    "p101",
    "p",
    "tm",
    "tm110",
    "TM(:)",
    "Median",
    "",
];

#[tokio::test]
async fn a_statistic_is_named_by_its_own_spelling_on_every_read_that_takes_one() {
    let router = build_router();
    let at = recent();
    // One hundred observations at one instant, so every defined statistic of the window has a
    // value and a row that is ACCEPTED is answered from stored data.
    let values: Vec<f64> = (1..=100).map(f64::from).collect();
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{"MetricName": METRIC, "Timestamp": at, "Values": values}],
        }),
    )
    .await;

    for stat in DEFINED_STATISTICS {
        let body = ok(&router, "GetMetricData", &data_read(at, json!([stat_query("m1", stat)]))).await;
        let values = body
            .get("MetricDataResults")
            .and_then(Value::as_array)
            .and_then(|results| results.first().cloned())
            .and_then(|result| result.get("Values").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        assert_eq!(
            values.len(),
            1,
            "case stat {stat:?}: a defined statistic answers the window, got {body}"
        );
    }

    for stat in UNDEFINED_STATISTICS {
        let (status, body) = call(
            &router,
            "GetMetricData",
            &data_read(at, json!([stat_query("m1", stat)])),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case stat {stat:?}: expected the request-validation refusal, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            not_a_valid_statistic(1),
            "case stat {stat:?}: expected the member path named, got {body}"
        );
    }
}

#[tokio::test]
async fn the_refused_query_is_the_one_the_request_put_the_bad_statistic_on() {
    let router = build_router();
    let at = recent();

    // The rows differ along WHERE in the array the offending query sits — first, second, last of
    // three — so the index in the refusal comes from the request rather than from a fixed case.
    for position in 1..=3usize {
        let queries: Vec<Value> = (1..=3)
            .map(|index| {
                let stat = if index == position { "P90" } else { "Sum" };
                stat_query(&format!("m{index}"), stat)
            })
            .collect();
        let (status, body) = call(&router, "GetMetricData", &data_read(at, Value::Array(queries))).await;
        assert_eq!(
            status, 400,
            "case the {position}th query: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            not_a_valid_statistic(position),
            "case the {position}th query: expected that query named, got {body}"
        );
    }
}

#[tokio::test]
async fn extended_statistics_carries_percentiles_and_nothing_else() {
    let router = build_router();
    let at = recent();

    // The percentiles the member documents ("Specify values between p0.0 and p100"), including
    // both published bounds and a fractional one between them, are served.
    for name in ["p0", "p0.0", "p50", "p99.9", "p100"] {
        let (status, body) = call(
            &router,
            "GetMetricStatistics",
            &statistics_read(at, json!({"ExtendedStatistics": [name]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a percentile is served, got {status} with {body}"
        );
    }

    // (case, the list submitted, the element the refusal names, that element's value)
    let refused: Vec<(&str, Vec<&str>, usize, &str)> = vec![
        ("a percentile just past the top", vec!["p100.1"], 1, "p100.1"),
        ("an upper-case prefix", vec!["P90"], 1, "P90"),
        ("a basic statistic", vec!["Average"], 1, "Average"),
        (
            "a bounded family, which the members that take any statistic serve",
            vec!["p90", "tm90"],
            2,
            "tm90",
        ),
        ("the interquartile mean", vec!["IQM"], 1, "IQM"),
        (
            "the tenth element of a list at the member's maximum",
            vec![
                "p10", "p20", "p30", "p40", "p50", "p60", "p70", "p80", "p90", "P95",
            ],
            10,
            "P95",
        ),
    ];
    for (case, extended, position, offending) in refused {
        let (status, body) = call(
            &router,
            "GetMetricStatistics",
            &statistics_read(at, json!({"ExtendedStatistics": extended})),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected the parameter-value fault, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            not_supported(offending, position),
            "case {case:?}: expected {offending:?} named unsupported at element {position}, got \
             {body}"
        );
    }
}

/// A `PutMetricStream` request for `name`, carrying `patch` on top of a configuration this
/// service accepts.
fn stream_request(name: &str, patch: &Value) -> Value {
    crate::harness::merged(
        json!({
            "Name": name,
            "FirehoseArn": format!(
                "arn:aws:firehose:{DEFAULT_REGION}:000000000000:deliverystream/{name}"
            ),
            "RoleArn": QUERY_ROLE,
            "OutputFormat": "json",
        }),
        patch,
    )
}

#[tokio::test]
async fn a_metric_stream_filter_states_the_namespace_it_selects() {
    let router = build_router();

    // The rows differ along which filter list carried the empty filter and WHERE in the list it
    // sat — first, second, third of three — so the refusal's zero-based position comes from the
    // request.
    for member in ["IncludeFilters", "ExcludeFilters"] {
        for position in 0..3usize {
            let filters: Vec<Value> = (0..3)
                .map(|index| {
                    if index == position {
                        json!({"MetricNames": ["Latency"]})
                    } else {
                        json!({"Namespace": format!("MyApp/Stream{index}")})
                    }
                })
                .collect();
            let (status, body) = call(
                &router,
                "PutMetricStream",
                &stream_request("filtered-stream", &json!({member: filters})),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {member} entry {position}: expected the parameter-value fault, got {status} \
                 {body}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "Value null at '{member}.{position}.member.Namespace' failed to satisfy \
                     constraint: Member must not be null"
                ),
                "case {member} entry {position}: expected the filter's own path named, got {body}"
            );
            // Nothing was configured: the stream the refused request named does not exist.
            let (read_status, read) =
                call(&router, "GetMetricStream", &json!({"Name": "filtered-stream"})).await;
            assert_eq!(
                (read_status, error_code(&read).as_str()),
                (404, "ResourceNotFoundException"),
                "case {member} entry {position}: a refused configuration creates no stream, got \
                 {read_status} with {read}"
            );
        }
    }

    // A filter that DOES name its namespace is accepted, and reads back as submitted.
    let created = ok(
        &router,
        "PutMetricStream",
        &stream_request(
            "filtered-stream",
            &json!({"ExcludeFilters": [{"Namespace": "AWS/EC2", "MetricNames": ["CPUUtilization"]}]}),
        ),
    )
    .await;
    assert!(
        created.get("Arn").and_then(Value::as_str).is_some(),
        "a filter naming its namespace is accepted, got {created}"
    );
    let read = ok(&router, "GetMetricStream", &json!({"Name": "filtered-stream"})).await;
    assert_eq!(
        read.get("ExcludeFilters"),
        Some(&json!([{"Namespace": "AWS/EC2", "MetricNames": ["CPUUtilization"]}])),
        "the stored filter reads back as submitted, got {read}"
    );
}

#[tokio::test]
async fn an_alarm_read_filters_by_one_statistic_at_a_time() {
    let router = build_router();

    // The rows differ along the pair submitted: a basic statistic beside each of the percentile
    // forms the other member takes.
    for (statistic, extended) in [
        ("Average", "p90"),
        ("Sum", "p99.9"),
        ("Maximum", "tm90"),
        ("Minimum", "IQM"),
    ] {
        let (status, body) = call(
            &router,
            "DescribeAlarmsForMetric",
            &json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Statistic": statistic,
                "ExtendedStatistic": extended,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {statistic} with {extended}: expected the request-validation refusal, got \
             {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            "DescribeAlarmsForMetric request can not be made with both Statistic and \
             ExtendedStatistic parameters. No more than one of them should be present",
            "case {statistic} with {extended}: expected the recorded sentence, got {body}"
        );
    }

    // Either member ALONE filters, and neither of them filters nothing: the refusal above is
    // about the pair and not about the members being read.
    for filter in [
        json!({"Statistic": "Average"}),
        json!({"ExtendedStatistic": "p90"}),
        json!({}),
    ] {
        let body = ok(
            &router,
            "DescribeAlarmsForMetric",
            &crate::harness::merged(
                json!({"Namespace": NAMESPACE, "MetricName": METRIC}),
                &filter,
            ),
        )
        .await;
        assert!(
            body.get("MetricAlarms").and_then(Value::as_array).is_some(),
            "case {filter}: one statistic member is a filter this read applies, got {body}"
        );
    }
}

#[tokio::test]
async fn a_read_names_an_instant_inside_the_span_the_service_reads() {
    let router = build_router();
    let at = recent();

    // The published span's own ends are readable, and a moment outside either end is not: the
    // rows are each bound, one second inside it, and one second outside.
    const EARLIEST: f64 = -8_520_336_000.0;
    const LATEST: f64 = 253_402_300_799.0;
    const SPAN: &str =
        "must be a value between Fri Jan 01 00:00:00 UTC 1700 and Fri Dec 31 23:59:59 UTC 9999.";

    // (case, the range, the member the refusal names, or None when the range is readable). Each
    // readable row is ONE window wide, so no row is refused for the datapoints it asks for
    // instead of for the instants it names.
    let rows: Vec<(&str, f64, f64, Option<&str>)> = vec![
        (
            "the first window of the published span",
            EARLIEST,
            EARLIEST + MINUTE,
            None,
        ),
        (
            "the last window of it",
            LATEST - MINUTE,
            LATEST,
            None,
        ),
        (
            "a start one second before the span",
            EARLIEST - 1.0,
            EARLIEST + MINUTE,
            Some("StartTime"),
        ),
        ("an end one second past the span", at, LATEST + 1.0, Some("EndTime")),
        ("an end far past the span", at, LATEST * 1_000.0, Some("EndTime")),
        (
            "a start far before the span",
            EARLIEST * 1_000.0,
            EARLIEST,
            Some("StartTime"),
        ),
    ];
    for (case, start, end, refused) in rows {
        for (reader, request) in [
            (
                "GetMetricStatistics",
                statistics_read(at, json!({"StartTime": start, "EndTime": end, "Statistics": ["Sum"]})),
            ),
            (
                "GetMetricData",
                crate::harness::merged(
                    data_read(at, json!([stat_query("m1", "Sum")])),
                    &json!({"StartTime": start, "EndTime": end, "MaxDatapoints": 1}),
                ),
            ),
        ] {
            let (status, body) = call(&router, reader, &request).await;
            match refused {
                None => assert_eq!(
                    status, 200,
                    "case {case:?} on {reader}: an instant inside the span is read, got {status} \
                     with {body}"
                ),
                Some(parameter) => {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, "InvalidParameterValueException"),
                        "case {case:?} on {reader}: expected the parameter-value fault, got \
                         {status} {body}"
                    );
                    assert_eq!(
                        error_message(&body),
                        format!("The parameter {parameter} {SPAN}"),
                        "case {case:?} on {reader}: expected the span named for {parameter}, got \
                         {body}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_window_wider_than_one_page_is_paged_rather_than_refused() {
    let router = build_router();
    let at = (now_secs() / MINUTE).floor() * MINUTE;

    /// The datapoints of a request's range one page carries ("a total of as many as 100,800 data
    /// points", `GetMetricData`).
    const PAGE: f64 = 100_800.0;

    // The rows differ along HOW the request asks for more than one page: a range many times a
    // page at a one-minute granularity, and a range just past a page at an hourly one.
    for (case, period, windows) in [
        ("a thousand pages at a one-minute granularity", 60.0, 1_000.0 * PAGE),
        ("one window past a page at an hourly granularity", 3_600.0, PAGE + 1.0),
    ] {
        let request = json!({
            "MetricDataQueries": [{
                "Id": "m1",
                "MetricStat": {
                    "Metric": {"Namespace": NAMESPACE, "MetricName": "Idle"},
                    "Period": period,
                    "Stat": "Sum",
                },
            }],
            "StartTime": at - windows * period,
            "EndTime": at,
        });
        let body = ok(&router, "GetMetricData", &request).await;
        let results = body
            .get("MetricDataResults")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: the query is answered with its own result, got {body}"
        );
        // A metric nothing published reports no datapoint — and the page is still PARTIAL, with
        // a token to continue from, because the range reaches past what one page covers.
        assert_eq!(
            results[0].get("Timestamps"),
            Some(&json!([])),
            "case {case:?}: an idle series reports no datapoint, got {body}"
        );
        assert_eq!(
            results[0].get("Values"),
            Some(&json!([])),
            "case {case:?}: an idle series reports no value, got {body}"
        );
        assert_eq!(
            results[0].get("StatusCode").and_then(Value::as_str),
            Some("PartialData"),
            "case {case:?}: a page that leaves range unanswered is partial, got {body}"
        );
        assert_eq!(
            body.get("Messages"),
            Some(&json!([])),
            "case {case:?}: nothing is reported about the operation itself, got {body}"
        );
        assert!(
            body.get("NextToken").and_then(Value::as_str).is_some(),
            "case {case:?}: a partial page hands back a token, got {body}"
        );
    }

    // Following the tokens of a two-page range terminates, and the last page is the Complete
    // one: the token is a POSITION in the request rather than a signal repeated forever.
    let request = json!({
        "MetricDataQueries": [{
            "Id": "m1",
            "MetricStat": {
                "Metric": {"Namespace": NAMESPACE, "MetricName": "Idle"},
                "Period": 3_600,
                "Stat": "Sum",
            },
        }],
        "StartTime": at - (PAGE + 1.0) * 3_600.0,
        "EndTime": at,
        "ScanBy": "TimestampAscending",
    });
    let mut statuses: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut page = request.clone();
        if let Some(carried) = &token {
            page["NextToken"] = json!(carried);
        }
        let body = ok(&router, "GetMetricData", &page).await;
        let result = body
            .get("MetricDataResults")
            .and_then(Value::as_array)
            .and_then(|results| results.first().cloned())
            .unwrap_or_default();
        statuses.push(
            result
                .get("StatusCode")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        assert!(
            statuses.len() <= 8,
            "case a two-page range: paging must terminate, got {statuses:?}"
        );
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        statuses,
        vec!["PartialData".to_string(), "Complete".to_string()],
        "case a two-page range: every page but the last is partial, got {statuses:?}"
    );
}

#[tokio::test]
async fn a_listing_applies_a_filter_for_every_dimension_a_metric_can_carry() {
    let router = build_router();

    /// The dimensions one metric can carry ("MetricDatum$Dimensions", length 0..30), which is
    /// what bounds a dimension collection of this service.
    const MAX_DIMENSIONS: usize = 30;

    // The rows differ along how many filters the listing carries: the member's own modelled
    // maximum, one past it, and the per-metric maximum — each of them a filter set a metric
    // could satisfy, so each is applied and matches nothing.
    for count in [10usize, 11, MAX_DIMENSIONS] {
        let filters: Vec<Value> = (0..count)
            .map(|index| json!({"Name": format!("d{index}"), "Value": "v"}))
            .collect();
        let body = ok(
            &router,
            "ListMetrics",
            &json!({"Namespace": NAMESPACE, "Dimensions": filters}),
        )
        .await;
        assert_eq!(
            body.get("Metrics").and_then(Value::as_array),
            Some(&vec![]),
            "case {count} filters: a filter nothing matches is an empty listing, got {body}"
        );
    }

    // One filter past the dimensions a metric can carry could match no metric at all, and is
    // refused against the cap it broke.
    let filters: Vec<Value> = (0..=MAX_DIMENSIONS)
        .map(|index| json!({"Name": format!("d{index}"), "Value": "v"}))
        .collect();
    let (status, body) = call(
        &router,
        "ListMetrics",
        &json!({"Namespace": NAMESPACE, "Dimensions": filters}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValueException"),
        "case one filter past the metric's dimensions: expected the parameter-value fault, got \
         {status} {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("The collection Dimensions must not have a size greater than {MAX_DIMENSIONS}."),
        "case one filter past the metric's dimensions: expected the cap named, got {body}"
    );
}

#[tokio::test]
async fn an_empty_next_token_asks_for_the_first_page() {
    let router = build_router();
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{"MetricName": "Paged", "Value": 1.0}],
        }),
    )
    .await;

    // Every paginated read of this service reads a token by one rule, so the rows span the
    // readers rather than the one the recorded request happened to use.
    let listings: Vec<(&str, Value, &str)> = vec![
        ("ListMetrics", json!({"Namespace": NAMESPACE}), "Metrics"),
        ("DescribeAlarms", json!({}), "MetricAlarms"),
        ("DescribeAlarmHistory", json!({}), "AlarmHistoryItems"),
        ("ListDashboards", json!({}), "DashboardEntries"),
        ("ListMetricStreams", json!({}), "Entries"),
        ("DescribeInsightRules", json!({}), "InsightRules"),
        ("DescribeAnomalyDetectors", json!({}), "AnomalyDetectors"),
    ];
    for (operation, request, member) in listings {
        let first = ok(&router, operation, &request).await;
        let resumed = ok(
            &router,
            operation,
            &crate::harness::merged(request.clone(), &json!({"NextToken": ""})),
        )
        .await;
        assert_eq!(
            resumed.get(member),
            first.get(member),
            "case {operation}: an empty token asks for the first page, got {resumed} after \
             {first}"
        );
    }

    // A token this service did not mint is still refused: the rule is about the EMPTY token
    // naming no page, not about tokens going unchecked.
    let (status, body) = call(
        &router,
        "ListMetrics",
        &json!({"Namespace": NAMESPACE, "NextToken": "not-a-token"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValueException"),
        "a token this service never issued is refused, got {status} {body}"
    );
}

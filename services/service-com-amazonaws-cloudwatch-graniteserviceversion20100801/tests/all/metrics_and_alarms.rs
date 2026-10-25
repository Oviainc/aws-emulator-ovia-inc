// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the CloudWatch metric ingest/read loop and the metric-alarm
//! lifecycle, driven over the wire the service actually speaks through the shared
//! `harness` (`awsJson1_0`, `POST /` with `X-Amz-Target`).
//!
//! Every table row is a distinct input along what the rule under test generalises over
//! (publish form, dimension ordering, period length, requested statistic set, page size),
//! so the recorded happy path is one row among many rather than the specification.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};

/// A fixed epoch second used as the base of every published time stamp, so the aggregation
/// windows a test asserts on never depend on the wall clock.
///
/// Aligned on a whole HOUR, which is the coarsest boundary a read rounds a requested start
/// time down to ("Start time greater than 63 days ago - Round down to the nearest 1-hour clock
/// interval.", `GetMetricStatisticsInput$StartTime`): a request starting here therefore starts
/// exactly where it says, so the rows below assert the bucketing on its own. The rounding
/// itself is asserted by
/// [`a_start_time_off_the_boundary_is_rounded_down_before_the_windows_are_laid`].
const T0: f64 = 1_699_999_200.0;

/// A numeric member, treating an absent member and an explicit null alike.
fn number(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

/// `GetMetricStatistics` datapoints, ordered by time stamp.
fn datapoints(body: &Value) -> Vec<Value> {
    let mut points: Vec<Value> = body
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    points.sort_by(|left, right| {
        number(left, "Timestamp")
            .unwrap_or_default()
            .total_cmp(&number(right, "Timestamp").unwrap_or_default())
    });
    points
}

/// One datum published as a plain `Value`.
fn datum(metric: &str, value: f64, offset: f64) -> Value {
    json!({
        "MetricName": metric,
        "Dimensions": [{"Name": "Service", "Value": "api"}],
        "Value": value,
        "Unit": "Milliseconds",
        "Timestamp": T0 + offset,
    })
}

/// A `GetMetricStatistics` request over the `Service=api` series of `metric`.
fn statistics_request(metric: &str, period: i64, end_offset: f64, statistics: &[&str]) -> Value {
    json!({
        "Namespace": "MyApp",
        "MetricName": metric,
        "Dimensions": [{"Name": "Service", "Value": "api"}],
        "StartTime": T0,
        "EndTime": T0 + end_offset,
        "Period": period,
        "Statistics": statistics,
    })
}

#[tokio::test]
async fn published_datapoints_are_listed_and_aggregated_from_the_store() {
    let router = build_router();

    let put = json!({
        "Namespace": "MyApp",
        "MetricData": [datum("Latency", 100.0, 0.0), datum("Latency", 300.0, 30.0)],
    });
    let put_body = ok(&router, "PutMetricData", &put).await;
    assert!(
        put_body.get("Datapoints").is_none(),
        "PutMetricData: expected an empty output, got {put_body}"
    );

    let listed = ok(&router, "ListMetrics", &json!({"Namespace": "MyApp"})).await;
    assert_eq!(
        listed.get("Metrics"),
        Some(&json!([{
            "Namespace": "MyApp",
            "MetricName": "Latency",
            "Dimensions": [{"Name": "Service", "Value": "api"}],
        }])),
        "ListMetrics: expected exactly the series PutMetricData created, got {listed}"
    );

    let stats = ok(
        &router,
        "GetMetricStatistics",
        &statistics_request(
            "Latency",
            300,
            300.0,
            &["Average", "Sum", "Maximum", "SampleCount"],
        ),
    )
    .await;
    assert_eq!(
        stats.get("Label").and_then(Value::as_str),
        Some("Latency"),
        "GetMetricStatistics: Label must name the metric, got {stats}"
    );
    let points = datapoints(&stats);
    assert_eq!(
        points.len(),
        1,
        "GetMetricStatistics: expected one period window, got {stats}"
    );
    let point = &points[0];
    for (member, expected) in [
        ("SampleCount", 2.0),
        ("Sum", 400.0),
        ("Average", 200.0),
        ("Maximum", 300.0),
    ] {
        assert_eq!(
            number(point, member),
            Some(expected),
            "GetMetricStatistics {member}: expected {expected}, got {point}"
        );
    }
    assert_eq!(
        number(point, "Minimum"),
        None,
        "GetMetricStatistics: Minimum was not requested, so it must be absent: {point}"
    );
    assert_eq!(
        point.get("Unit").and_then(Value::as_str),
        Some("Milliseconds"),
        "GetMetricStatistics: the published unit must survive, got {point}"
    );
}

#[tokio::test]
async fn entity_associated_metric_data_is_ingested_too() {
    // Rows differ in where the metric data rides: the plain `MetricData` list, the
    // `EntityMetricData` list, and both at once — the documented limit of two entity
    // objects is the upper row.
    let cases: Vec<(&str, Value, f64)> = vec![
        (
            "MetricData only",
            json!({"MetricData": [{"MetricName": "Mixed", "Value": 1.0, "Timestamp": T0}]}),
            1.0,
        ),
        (
            "EntityMetricData only",
            json!({
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {"Type": "Service", "Name": "api"}},
                    "MetricData": [{"MetricName": "Mixed", "Value": 2.0, "Timestamp": T0}],
                }],
            }),
            1.0,
        ),
        (
            "both lists, and two entity objects",
            json!({
                "MetricData": [{"MetricName": "Mixed", "Value": 3.0, "Timestamp": T0}],
                "EntityMetricData": [
                    {"MetricData": [{"MetricName": "Mixed", "Value": 4.0, "Timestamp": T0}]},
                    {"MetricData": [{"MetricName": "Mixed", "Value": 5.0, "Timestamp": T0}]},
                ],
            }),
            3.0,
        ),
    ];

    for (case, payload, expected_samples) in cases {
        let router = build_router();
        let mut request = payload;
        request["Namespace"] = json!("MyApp");
        ok(&router, "PutMetricData", &request).await;

        let stats = ok(
            &router,
            "GetMetricStatistics",
            &json!({
                "Namespace": "MyApp",
                "MetricName": "Mixed",
                "StartTime": T0,
                "EndTime": T0 + 60.0,
                "Period": 60,
                "Statistics": ["SampleCount"],
            }),
        )
        .await;
        let samples = datapoints(&stats)
            .first()
            .and_then(|point| number(point, "SampleCount"))
            .unwrap_or(0.0);
        assert_eq!(
            samples, expected_samples,
            "case {case:?}: expected SampleCount {expected_samples}, got {stats}"
        );
    }
}

#[tokio::test]
async fn every_publish_form_folds_into_the_same_aggregation() {
    // Rows differ along the form `MetricDatum` allows (`Value`, `StatisticValues`,
    // `Values`/`Counts`), the number of observations each form carries, and whether the
    // counts are explicit; the expectation is computed from what the row published.
    let cases: Vec<(&str, Value, [f64; 4])> = vec![
        (
            "a plain Value is one observation",
            json!({"MetricName": "Single", "Value": 7.0}),
            [1.0, 7.0, 7.0, 7.0],
        ),
        (
            "a StatisticValues set is stored as published",
            json!({
                "MetricName": "Set",
                "StatisticValues": {"SampleCount": 4.0, "Sum": 20.0, "Minimum": 2.0, "Maximum": 9.0},
            }),
            [4.0, 20.0, 2.0, 9.0],
        ),
        (
            "Values without Counts defaults every count to 1",
            json!({"MetricName": "Values", "Values": [1.0, 2.0, 3.0]}),
            [3.0, 6.0, 1.0, 3.0],
        ),
        (
            "Values with Counts weighs each value by its count",
            json!({"MetricName": "Weighted", "Values": [2.0, 5.0], "Counts": [3.0, 1.0]}),
            [4.0, 11.0, 2.0, 5.0],
        ),
        (
            "a repeated value stays its own minimum and maximum",
            json!({"MetricName": "Repeated", "Values": [4.0, 4.0], "Counts": [2.0, 2.0]}),
            [4.0, 16.0, 4.0, 4.0],
        ),
    ];

    let router = build_router();
    for (case, mut published, [sample_count, sum, minimum, maximum]) in cases {
        let metric = published["MetricName"].as_str().expect("a metric name").to_string();
        published["Timestamp"] = json!(T0);
        published["Dimensions"] = json!([{"Name": "Service", "Value": "api"}]);
        ok(
            &router,
            "PutMetricData",
            &json!({"Namespace": "MyApp", "MetricData": [published]}),
        )
        .await;

        let stats = ok(
            &router,
            "GetMetricStatistics",
            &statistics_request(
                &metric,
                60,
                60.0,
                &["SampleCount", "Sum", "Minimum", "Maximum", "Average"],
            ),
        )
        .await;
        let points = datapoints(&stats);
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: expected one window, got {stats}"
        );
        let point = &points[0];
        for (member, expected) in [
            ("SampleCount", sample_count),
            ("Sum", sum),
            ("Minimum", minimum),
            ("Maximum", maximum),
            ("Average", sum / sample_count),
        ] {
            assert_eq!(
                number(point, member),
                Some(expected),
                "case {case:?}: expected {member} {expected}, got {point}"
            );
        }
    }
}

#[tokio::test]
async fn a_dimension_set_identifies_a_series_regardless_of_order() {
    // Rows differ in how the SAME dimension set is ordered, and in which set is queried, so
    // the assertion is about set identity rather than about one request's ordering.
    let cases: Vec<(&str, Value, Value, f64)> = vec![
        (
            "the publish order is the query order",
            json!([{"Name": "A", "Value": "1"}, {"Name": "B", "Value": "2"}]),
            json!([{"Name": "A", "Value": "1"}, {"Name": "B", "Value": "2"}]),
            1.0,
        ),
        (
            "the query reverses the publish order",
            json!([{"Name": "A", "Value": "1"}, {"Name": "B", "Value": "2"}]),
            json!([{"Name": "B", "Value": "2"}, {"Name": "A", "Value": "1"}]),
            1.0,
        ),
        (
            "a subset of the dimensions is a different series",
            json!([{"Name": "A", "Value": "1"}, {"Name": "B", "Value": "2"}]),
            json!([{"Name": "A", "Value": "1"}]),
            0.0,
        ),
        (
            "a different dimension value is a different series",
            json!([{"Name": "A", "Value": "1"}]),
            json!([{"Name": "A", "Value": "9"}]),
            0.0,
        ),
        (
            "no dimensions at all is its own series",
            json!([]),
            json!([]),
            1.0,
        ),
    ];

    for (index, (case, published, queried, expected_samples)) in cases.into_iter().enumerate() {
        let router = build_router();
        let metric = format!("Keyed{index}");
        ok(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": "MyApp",
                "MetricData": [{
                    "MetricName": metric,
                    "Dimensions": published,
                    "Value": 5.0,
                    "Timestamp": T0,
                }],
            }),
        )
        .await;

        let stats = ok(
            &router,
            "GetMetricStatistics",
            &json!({
                "Namespace": "MyApp",
                "MetricName": metric,
                "Dimensions": queried,
                "StartTime": T0,
                "EndTime": T0 + 60.0,
                "Period": 60,
                "Statistics": ["SampleCount"],
            }),
        )
        .await;
        let points = datapoints(&stats);
        let samples = points
            .first()
            .and_then(|point| number(point, "SampleCount"))
            .unwrap_or(0.0);
        assert_eq!(
            samples, expected_samples,
            "case {case:?}: expected SampleCount {expected_samples}, got {stats}"
        );
    }
}

#[tokio::test]
async fn datapoints_bucket_into_period_windows_anchored_at_the_start_time() {
    // The same three observations, read back at periods spanning the documented range
    // (1, 5, 10, 20, 30, 60 and multiples of 60). A window with no observation is omitted
    // entirely, which is what the `Period = 30` row pins.
    //
    // A row is (case, period, end offset, expected (window offset, sample count) pairs).
    type Row = (&'static str, i64, f64, Vec<(f64, f64)>);
    let cases: Vec<Row> = vec![
        (
            "one window per period covers everything",
            300,
            300.0,
            vec![(0.0, 3.0)],
        ),
        (
            "a one-minute period splits the first two from the third",
            60,
            120.0,
            vec![(0.0, 2.0), (60.0, 1.0)],
        ),
        (
            "an empty window between observations is omitted",
            30,
            120.0,
            vec![(0.0, 1.0), (30.0, 1.0), (90.0, 1.0)],
        ),
        (
            "a one-second period keeps each observation alone",
            1,
            120.0,
            vec![(0.0, 1.0), (30.0, 1.0), (90.0, 1.0)],
        ),
        (
            "a twenty-second period buckets by twenties",
            20,
            120.0,
            vec![(0.0, 1.0), (20.0, 1.0), (80.0, 1.0)],
        ),
        (
            "EndTime is exclusive, so the last observation falls outside",
            60,
            90.0,
            vec![(0.0, 2.0)],
        ),
    ];

    let router = build_router();
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": "MyApp",
            "MetricData": [
                datum("Windowed", 10.0, 0.0),
                datum("Windowed", 20.0, 30.0),
                datum("Windowed", 30.0, 90.0),
            ],
        }),
    )
    .await;

    for (case, period, end_offset, expected) in cases {
        let stats = ok(
            &router,
            "GetMetricStatistics",
            &statistics_request("Windowed", period, end_offset, &["SampleCount"]),
        )
        .await;
        let got: Vec<(f64, f64)> = datapoints(&stats)
            .iter()
            .map(|point| {
                (
                    number(point, "Timestamp").unwrap_or_default() - T0,
                    number(point, "SampleCount").unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected windows {expected:?}, got {got:?} from {stats}"
        );
    }
}

/// A requested start time is rounded DOWN before the period windows are laid over the range,
/// so the datapoints a read returns are stamped on the boundaries CloudWatch lays and
/// aggregate the observations inside those windows.
///
/// "CloudWatch rounds the specified time stamp as follows: Start time less than 15 days ago -
/// Round down to the nearest whole minute. […] Start time between 15 and 63 days ago - Round
/// down to the nearest 5-minute clock interval. […] Start time greater than 63 days ago -
/// Round down to the nearest 1-hour clock interval. […] If you set Period to 5, 10, 20, or 30,
/// the start time of your request is rounded down to the nearest time that corresponds to even
/// 5-, 10-, 20-, or 30-second divisions of a minute." (`GetMetricStatisticsInput$StartTime`)
///
/// The rows span the three age bands and the sub-minute periods that round by themselves, each
/// with a start time deliberately off its own boundary.
#[tokio::test]
async fn a_start_time_off_the_boundary_is_rounded_down_before_the_windows_are_laid() {
    let router = build_router();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
        .floor();
    // A whole hour in the recent past, so a start time near it is "less than 15 days ago", and
    // one 30 days before it is in the five-minute band.
    let recent = (now / 3600.0).floor() * 3600.0 - 3600.0;
    const DAY: f64 = 24.0 * 60.0 * 60.0;

    // (case, the whole-boundary base, how far past it the request starts, the period, the
    // interval the start time rounds to)
    let cases: Vec<(&str, f64, f64, i64, f64)> = vec![
        ("less than 15 days ago rounds to the minute", recent, 150.0, 300, 60.0),
        ("the same band at a one-minute period", recent, 95.0, 60, 60.0),
        (
            "between 15 and 63 days ago rounds to five minutes",
            recent - 30.0 * DAY,
            150.0,
            300,
            300.0,
        ),
        (
            "more than 63 days ago rounds to the hour",
            recent - 100.0 * DAY,
            1_250.0,
            3_600,
            3_600.0,
        ),
        // A sub-minute period rounds to its own division of the minute, whatever the age.
        ("a thirty-second period rounds to thirty seconds", recent, 47.0, 30, 30.0),
        ("a twenty-second period rounds to twenty seconds", recent, 47.0, 20, 20.0),
    ];

    for (case, base, offset, period, interval) in cases {
        let metric = format!("Rounded{period}x{offset}");
        let at = base + offset;
        let expected_start = ((at / interval).floor() * interval).max(0.0);
        ok(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": "MyApp",
                "MetricData": [{
                    "MetricName": metric,
                    "Dimensions": [{"Name": "Service", "Value": "api"}],
                    "Value": 4.0,
                    "Timestamp": at,
                }],
            }),
        )
        .await;
        let stats = ok(
            &router,
            "GetMetricStatistics",
            &json!({
                "Namespace": "MyApp",
                "MetricName": metric,
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "StartTime": at,
                "EndTime": at + f64::from(period as i32),
                "Period": period,
                "Statistics": ["Sum"],
            }),
        )
        .await;
        let stamps: Vec<f64> = datapoints(&stats)
            .iter()
            .filter_map(|point| number(point, "Timestamp"))
            .collect();
        assert_eq!(
            stamps,
            vec![expected_start],
            "case {case:?}: expected the window stamped at the rounded start {expected_start}, \
             got {stamps:?} from {stats}"
        );
    }
}

#[tokio::test]
async fn only_the_requested_statistics_are_emitted() {
    /// Every statistic the model's `Statistic` enum declares (shared_types
    /// `STATISTIC_VALUES`, from the Smithy model).
    const ALL: [&str; 5] = ["Average", "Maximum", "Minimum", "SampleCount", "Sum"];

    // Rows differ in WHICH subset is asked for, including one statistic the recorded happy
    // path never asks for (`Minimum`) and the full set.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("one statistic", vec!["Average"]),
        ("two statistics", vec!["Sum", "SampleCount"]),
        ("the bounds only", vec!["Minimum", "Maximum"]),
        ("the full set", ALL.to_vec()),
        ("the sample count alone", vec!["SampleCount"]),
    ];

    let router = build_router();
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": "MyApp",
            "MetricData": [datum("Selected", 2.0, 0.0), datum("Selected", 8.0, 1.0)],
        }),
    )
    .await;
    let expected = [
        ("Average", 5.0),
        ("Maximum", 8.0),
        ("Minimum", 2.0),
        ("SampleCount", 2.0),
        ("Sum", 10.0),
    ];

    for (case, requested) in cases {
        let stats = ok(
            &router,
            "GetMetricStatistics",
            &statistics_request("Selected", 60, 60.0, &requested),
        )
        .await;
        let points = datapoints(&stats);
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: expected one window, got {stats}"
        );
        let point = &points[0];
        for (member, value) in expected {
            let got = number(point, member);
            if requested.contains(&member) {
                assert_eq!(
                    got,
                    Some(value),
                    "case {case:?}: {member} was requested, expected {value}, got {point}"
                );
            } else {
                assert_eq!(
                    got, None,
                    "case {case:?}: {member} was not requested, so it must be absent: {point}"
                );
            }
        }
    }
}

#[tokio::test]
async fn percentiles_come_from_the_published_values_or_are_not_reported() {
    // Rows walk the percentile range, including both published bounds (p0, p100) and a
    // value the recorded case never asks for.
    let cases: [(&str, &str, f64); 5] = [
        ("the lowest percentile is the smallest value", "p0", 10.0),
        ("a quarter of the way up", "p25", 20.0),
        ("the median", "p50", 30.0),
        ("a high percentile", "p90", 50.0),
        ("the highest percentile is the largest value", "p100", 50.0),
    ];

    let router = build_router();
    let values: Vec<Value> = [10.0, 20.0, 30.0, 40.0, 50.0]
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            #[allow(clippy::cast_precision_loss)]
            let offset = index as f64;
            datum("Percentiles", value, offset)
        })
        .collect();
    ok(
        &router,
        "PutMetricData",
        &json!({"Namespace": "MyApp", "MetricData": values}),
    )
    .await;

    for (case, statistic, expected) in cases {
        let stats = ok(
            &router,
            "GetMetricStatistics",
            &json!({
                "Namespace": "MyApp",
                "MetricName": "Percentiles",
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "StartTime": T0,
                "EndTime": T0 + 60.0,
                "Period": 60,
                "ExtendedStatistics": [statistic],
            }),
        )
        .await;
        let points = datapoints(&stats);
        let got = points
            .first()
            .and_then(|point| point.get("ExtendedStatistics"))
            .and_then(|extended| extended.get(statistic))
            .and_then(Value::as_f64);
        assert_eq!(
            got,
            Some(expected),
            "case {case:?}: expected {statistic} {expected}, got {stats}"
        );
    }

    // An opaque statistic set carries no individual values, and `GetMetricStatistics`
    // documents that percentiles are then unavailable — so none is reported rather than
    // invented.
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": "MyApp",
            "MetricData": [{
                "MetricName": "Opaque",
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "StatisticValues": {"SampleCount": 4.0, "Sum": 20.0, "Minimum": 1.0, "Maximum": 9.0},
                "Timestamp": T0,
            }],
        }),
    )
    .await;
    let stats = ok(
        &router,
        "GetMetricStatistics",
        &json!({
            "Namespace": "MyApp",
            "MetricName": "Opaque",
            "Dimensions": [{"Name": "Service", "Value": "api"}],
            "StartTime": T0,
            "EndTime": T0 + 60.0,
            "Period": 60,
            "ExtendedStatistics": ["p50"],
        }),
    )
    .await;
    let point = datapoints(&stats);
    assert!(
        point
            .first()
            .and_then(|point| point.get("ExtendedStatistics"))
            .is_none(),
        "an opaque statistic set must yield no percentile, got {stats}"
    );
}

#[tokio::test]
async fn list_metrics_filters_only_the_series_that_exist() {
    let router = build_router();
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": "MyApp",
            "MetricData": [
                {"MetricName": "Latency", "Dimensions": [{"Name": "Service", "Value": "api"}], "Value": 1.0, "Timestamp": T0},
                {"MetricName": "Latency", "Dimensions": [{"Name": "Service", "Value": "worker"}], "Value": 1.0, "Timestamp": T0},
                {"MetricName": "Errors", "Dimensions": [{"Name": "Service", "Value": "api"}], "Value": 1.0, "Timestamp": T0},
            ],
        }),
    )
    .await;
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": "Other",
            "MetricData": [{"MetricName": "Latency", "Value": 1.0, "Timestamp": T0}],
        }),
    )
    .await;

    // Rows differ along which filter member is supplied and how selective it is.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no filter lists every series", json!({}), 4),
        (
            "a namespace filter is exact",
            json!({"Namespace": "MyApp"}),
            3,
        ),
        (
            "a metric-name filter is exact",
            json!({"MetricName": "Latency"}),
            3,
        ),
        (
            "namespace and name together",
            json!({"Namespace": "MyApp", "MetricName": "Latency"}),
            2,
        ),
        (
            "a dimension name filter matches any value",
            json!({"Dimensions": [{"Name": "Service"}]}),
            3,
        ),
        (
            "a dimension name and value filter is exact",
            json!({"Dimensions": [{"Name": "Service", "Value": "api"}]}),
            2,
        ),
        (
            "a dimension that no series carries matches nothing",
            json!({"Dimensions": [{"Name": "Region"}]}),
            0,
        ),
        (
            "a namespace nothing published matches nothing",
            json!({"Namespace": "Absent"}),
            0,
        ),
    ];

    for (case, filter, expected) in cases {
        let listed = ok(&router, "ListMetrics", &filter).await;
        let got = listed
            .get("Metrics")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected} metrics, got {listed}"
        );
    }
}

#[tokio::test]
async fn recently_active_lists_only_series_with_fresh_datapoints() {
    let router = build_router();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();

    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": "MyApp",
            "MetricData": [
                {"MetricName": "Fresh", "Value": 1.0, "Timestamp": now - 60.0},
                {"MetricName": "Stale", "Value": 1.0, "Timestamp": now - 4.0 * 60.0 * 60.0},
            ],
        }),
    )
    .await;

    let listed = ok(
        &router,
        "ListMetrics",
        &json!({"Namespace": "MyApp", "RecentlyActive": "PT3H"}),
    )
    .await;
    let names: Vec<&str> = listed
        .get("Metrics")
        .and_then(Value::as_array)
        .map(|metrics| {
            metrics
                .iter()
                .filter_map(|metric| metric.get("MetricName").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["Fresh"],
        "RecentlyActive=PT3H must list only series with datapoints in the past three hours, \
         got {listed}"
    );
}

#[tokio::test]
async fn the_alarm_lifecycle_round_trips_through_the_store() {
    let router = build_router();
    let put = json!({
        "AlarmName": "latency-high",
        "AlarmDescription": "api latency",
        "Namespace": "MyApp",
        "MetricName": "Latency",
        "Dimensions": [{"Name": "Service", "Value": "api"}],
        "Statistic": "Average",
        "Period": 300,
        "EvaluationPeriods": 1,
        "DatapointsToAlarm": 1,
        "Threshold": 250.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "TreatMissingData": "missing",
        "Unit": "Milliseconds",
        "AlarmActions": ["arn:aws:sns:us-east-1:000000000000:oncall"],
        "OKActions": ["arn:aws:sns:us-east-1:000000000000:recovered"],
        "Tags": [{"Key": "team", "Value": "platform"}],
    });
    ok(&router, "PutMetricAlarm", &put).await;

    let described = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["latency-high"]}),
    )
    .await;
    let alarms = described
        .get("MetricAlarms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        alarms.len(),
        1,
        "DescribeAlarms: expected the one stored alarm, got {described}"
    );
    let alarm = &alarms[0];

    // Every configuration member the request carried must come back as it was given.
    for member in [
        "AlarmName",
        "AlarmDescription",
        "Namespace",
        "MetricName",
        "Dimensions",
        "Statistic",
        "Period",
        "EvaluationPeriods",
        "DatapointsToAlarm",
        "Threshold",
        "ComparisonOperator",
        "TreatMissingData",
        "Unit",
        "AlarmActions",
        "OKActions",
    ] {
        assert_eq!(
            alarm.get(member),
            put.get(member),
            "DescribeAlarms {member}: expected {:?}, got {:?}",
            put.get(member),
            alarm.get(member)
        );
    }
    assert_eq!(
        alarm.get("AlarmArn").and_then(Value::as_str),
        Some("arn:aws:cloudwatch:us-east-1:000000000000:alarm:latency-high"),
        "DescribeAlarms: the alarm ARN must name the region, account and alarm, got {alarm}"
    );
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("INSUFFICIENT_DATA"),
        "a newly created alarm starts in INSUFFICIENT_DATA, got {alarm}"
    );
    assert_eq!(
        alarm.get("ActionsEnabled").and_then(Value::as_bool),
        Some(true),
        "ActionsEnabled defaults to TRUE, got {alarm}"
    );
    let created_at = number(alarm, "AlarmConfigurationUpdatedTimestamp")
        .expect("AlarmConfigurationUpdatedTimestamp is set on creation");
    let first_state_update =
        number(alarm, "StateUpdatedTimestamp").expect("StateUpdatedTimestamp is set on creation");
    assert!(
        created_at > 0.0 && first_state_update > 0.0,
        "both timestamps must be real instants, got {alarm}"
    );

    // Rows differ in the state asked for, including setting the state the alarm is
    // already in.
    for (case, state, reason) in [
        ("into ALARM", "ALARM", "manual"),
        ("into OK", "OK", "recovered"),
        ("back into OK", "OK", "still fine"),
        ("into INSUFFICIENT_DATA", "INSUFFICIENT_DATA", "no data"),
    ] {
        ok(
            &router,
            "SetAlarmState",
            &json!({
                "AlarmName": "latency-high",
                "StateValue": state,
                "StateReason": reason,
            }),
        )
        .await;
        let described = ok(
            &router,
            "DescribeAlarms",
            &json!({"AlarmNames": ["latency-high"]}),
        )
        .await;
        let alarm = &described["MetricAlarms"][0];
        assert_eq!(
            alarm.get("StateValue").and_then(Value::as_str),
            Some(state),
            "case {case:?}: expected StateValue {state}, got {alarm}"
        );
        assert_eq!(
            alarm.get("StateReason").and_then(Value::as_str),
            Some(reason),
            "case {case:?}: expected StateReason {reason}, got {alarm}"
        );
        assert!(
            number(alarm, "StateUpdatedTimestamp").unwrap_or_default() >= first_state_update,
            "case {case:?}: StateUpdatedTimestamp must move forward, got {alarm}"
        );
    }

    // Replacing an alarm overwrites its configuration but leaves its state alone.
    let mut replacement = put.clone();
    replacement["Threshold"] = json!(500.0);
    replacement["AlarmDescription"] = json!("relaxed");
    ok(&router, "PutMetricAlarm", &replacement).await;
    let described = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["latency-high"]}),
    )
    .await;
    let alarm = &described["MetricAlarms"][0];
    assert_eq!(
        number(alarm, "Threshold"),
        Some(500.0),
        "a replacement must overwrite the configuration, got {alarm}"
    );
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("INSUFFICIENT_DATA"),
        "a replacement must leave the alarm state unchanged, got {alarm}"
    );

    ok(
        &router,
        "DeleteAlarms",
        &json!({"AlarmNames": ["latency-high"]}),
    )
    .await;
    let described = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["latency-high"]}),
    )
    .await;
    assert_eq!(
        described
            .get("MetricAlarms")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        0,
        "a deleted alarm must not be described, got {described}"
    );
}

#[tokio::test]
async fn describe_alarms_filters_caps_and_pages() {
    let router = build_router();
    for (name, state, action) in [
        ("latency-a", "ALARM", "arn:aws:sns:us-east-1:000000000000:one"),
        ("latency-b", "OK", "arn:aws:sns:us-east-1:000000000000:two"),
        (
            "errors-c",
            "INSUFFICIENT_DATA",
            "arn:aws:sns:us-east-1:000000000000:three",
        ),
    ] {
        ok(
            &router,
            "PutMetricAlarm",
            &json!({
                "AlarmName": name,
                "Namespace": "MyApp",
                "MetricName": "Latency",
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "AlarmActions": [action],
            }),
        )
        .await;
        ok(
            &router,
            "SetAlarmState",
            &json!({"AlarmName": name, "StateValue": state, "StateReason": "seeded"}),
        )
        .await;
    }

    // Rows differ along which filter is used and how many alarms it selects.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter describes every alarm",
            json!({}),
            vec!["errors-c", "latency-a", "latency-b"],
        ),
        (
            "an exact name list",
            json!({"AlarmNames": ["latency-b", "errors-c"]}),
            vec!["errors-c", "latency-b"],
        ),
        (
            "a name prefix",
            json!({"AlarmNamePrefix": "latency"}),
            vec!["latency-a", "latency-b"],
        ),
        (
            "a state filter",
            json!({"StateValue": "OK"}),
            vec!["latency-b"],
        ),
        (
            "an action prefix",
            json!({"ActionPrefix": "arn:aws:sns:us-east-1:000000000000:th"}),
            vec!["errors-c"],
        ),
        (
            "an alarm type this store holds none of",
            json!({"AlarmTypes": ["CompositeAlarm"]}),
            vec![],
        ),
        (
            "a prefix nothing matches",
            json!({"AlarmNamePrefix": "nope"}),
            vec![],
        ),
    ];

    for (case, filter, expected) in cases {
        let described = ok(&router, "DescribeAlarms", &filter).await;
        let names: Vec<&str> = described
            .get("MetricAlarms")
            .and_then(Value::as_array)
            .map(|alarms| {
                alarms
                    .iter()
                    .filter_map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names, expected,
            "case {case:?}: expected {expected:?}, got {described}"
        );
    }

    // Paging: rows differ in page size, so the number of pages and the split differ.
    for (case, page_size, expected_pages) in [
        ("one alarm per page", 1, 3),
        ("two alarms per page", 2, 2),
        ("every alarm in one page", 3, 1),
    ] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"MaxRecords": page_size});
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let described = ok(&router, "DescribeAlarms", &request).await;
            let page = described
                .get("MetricAlarms")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must not exceed MaxRecords, got {described}"
            );
            seen.extend(
                page.iter()
                    .filter_map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
                    .map(ToString::to_string),
            );
            pages += 1;
            token = described
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= 4,
                "case {case:?}: paging must terminate, saw {seen:?}"
            );
        }
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages} over {seen:?}"
        );
        assert_eq!(
            seen,
            vec!["errors-c", "latency-a", "latency-b"],
            "case {case:?}: paging must visit every alarm exactly once, saw {seen:?}"
        );
    }
}

#[tokio::test]
async fn refusals_carry_the_modelled_status_and_code() {
    let router = build_router();
    ok(
        &router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": "known",
            "Namespace": "MyApp",
            "MetricName": "Latency",
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
        }),
    )
    .await;
    ok(
        &router,
        "PutMetricData",
        &json!({"Namespace": "MyApp", "MetricData": [datum("Refused", 1.0, 0.0)]}),
    )
    .await;

    // Rows differ along WHICH documented rule the request breaks, and therefore which
    // status and code answer it.
    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a value larger than 2^360",
            "PutMetricData",
            json!({"Namespace": "MyApp", "MetricData": [{"MetricName": "Big", "Value": 1e300}]}),
            400,
            "InvalidParameterValueException",
        ),
        (
            "Counts shorter than Values",
            "PutMetricData",
            json!({
                "Namespace": "MyApp",
                "MetricData": [{"MetricName": "Mismatched", "Values": [1.0, 2.0], "Counts": [1.0]}],
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a storage resolution the member does not document",
            "PutMetricData",
            json!({
                "Namespace": "MyApp",
                "MetricData": [{"MetricName": "Resolved", "Value": 1.0, "StorageResolution": 7}],
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "two publish forms in one datum",
            "PutMetricData",
            json!({
                "Namespace": "MyApp",
                "MetricData": [{
                    "MetricName": "Both",
                    "Value": 1.0,
                    "StatisticValues": {"SampleCount": 1.0, "Sum": 1.0, "Minimum": 1.0, "Maximum": 1.0},
                }],
            }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "more metrics in one request than the documented 1000",
            "PutMetricData",
            json!({
                "Namespace": "MyApp",
                "MetricData": (0..1001)
                    .map(|index| json!({"MetricName": format!("M{index}"), "Value": 1.0}))
                    .collect::<Vec<Value>>(),
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "more values in one datum than the documented 150",
            "PutMetricData",
            json!({
                "Namespace": "MyApp",
                "MetricData": [{
                    "MetricName": "TooMany",
                    "Values": (0..151).map(|index| json!(f64::from(index))).collect::<Vec<Value>>(),
                }],
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "more EntityMetricData entries than the documented two",
            "PutMetricData",
            json!({
                "Namespace": "MyApp",
                "EntityMetricData": (0..3)
                    .map(|index| json!({
                        "MetricData": [{"MetricName": format!("E{index}"), "Value": 1.0}],
                    }))
                    .collect::<Vec<Value>>(),
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "Counts without any Values",
            "PutMetricData",
            json!({
                "Namespace": "MyApp",
                "MetricData": [{"MetricName": "Orphaned", "Counts": [1.0, 2.0]}],
            }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "a period that is neither sub-minute nor a multiple of 60",
            "GetMetricStatistics",
            statistics_request("Refused", 45, 300.0, &["Sum"]),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an end time at or before the start time",
            "GetMetricStatistics",
            statistics_request("Refused", 60, 0.0, &["Sum"]),
            400,
            "InvalidParameterValueException",
        ),
        (
            "more than 1440 periods in the requested range",
            "GetMetricStatistics",
            statistics_request("Refused", 60, 60.0 * 1441.0, &["Sum"]),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "no statistic at all",
            "GetMetricStatistics",
            json!({
                "Namespace": "MyApp",
                "MetricName": "Refused",
                "StartTime": T0,
                "EndTime": T0 + 60.0,
                "Period": 60,
            }),
            400,
            "MissingRequiredParameterException",
        ),
        (
            "an extended statistic that is not a percentile, over an empty range",
            "GetMetricStatistics",
            json!({
                "Namespace": "MyApp",
                "MetricName": "NothingHere",
                "StartTime": T0,
                "EndTime": T0 + 60.0,
                "Period": 60,
                "ExtendedStatistics": ["notapercentile"],
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a NextToken this service never issued",
            "DescribeAlarms",
            json!({"NextToken": "not-a-token"}),
            400,
            "InvalidNextToken",
        ),
        (
            "a bogus NextToken on the children-of path too",
            "DescribeAlarms",
            json!({"ChildrenOfAlarmName": "known", "NextToken": "not-a-token"}),
            400,
            "InvalidNextToken",
        ),
        (
            "a ListMetrics NextToken this service never issued",
            "ListMetrics",
            json!({"NextToken": "not-a-token"}),
            400,
            "InvalidParameterValueException",
        ),
        (
            "AlarmNamePrefix together with AlarmNames",
            "DescribeAlarms",
            json!({"AlarmNames": ["known"], "AlarmNamePrefix": "kno"}),
            400,
            "ValidationException",
        ),
        (
            "deleting an alarm that does not exist",
            "DeleteAlarms",
            json!({"AlarmNames": ["absent"]}),
            404,
            "ResourceNotFound",
        ),
        (
            "setting the state of an alarm that does not exist",
            "SetAlarmState",
            json!({"AlarmName": "absent", "StateValue": "OK", "StateReason": "why"}),
            404,
            "ResourceNotFound",
        ),
    ];

    for (case, operation, body, status, code) in cases {
        let (got_status, got_body) = call(&router, operation, &body).await;
        assert_eq!(
            got_status, status,
            "case {case:?}: expected HTTP {status}, got {got_status} with body {got_body}"
        );
        assert_eq!(
            error_code(&got_body),
            code,
            "case {case:?}: expected error code {code}, got {got_body}"
        );
    }
}

#[tokio::test]
async fn a_failed_put_ingests_nothing_from_the_batch() {
    let router = build_router();
    // The first datum is sound and the second breaks a documented rule; the operation
    // documents that such a request fails entirely and "no data will be ingested".
    let (status, body) = call(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": "MyApp",
            "MetricData": [
                datum("Atomic", 1.0, 0.0),
                {"MetricName": "Atomic", "Value": f64::MAX},
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a batch with an unsupported value must be refused, got {status} with {body}"
    );

    let listed = ok(&router, "ListMetrics", &json!({"Namespace": "MyApp"})).await;
    assert_eq!(
        listed
            .get("Metrics")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        0,
        "a refused PutMetricData must leave the store untouched, got {listed}"
    );
}

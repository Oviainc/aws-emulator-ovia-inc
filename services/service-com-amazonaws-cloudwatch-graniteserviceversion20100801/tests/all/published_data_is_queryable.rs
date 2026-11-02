// SPDX-License-Identifier: CC0-1.0

//! What a client publishes, a client can read back — through both readers of the metric data
//! plane.
//!
//! Two rules, both about a publish and the reads over it:
//!
//! 1. **A read answers with the history there IS over the range it asks for.** The granularity
//!    a range can be read at is the one its start time's retention tier still holds: "If the
//!    StartTime parameter specifies a time stamp that is greater than 3 hours ago, you must
//!    specify the period as follows or no data points in that time range is returned: Start
//!    time between 3 hours and 15 days ago - Use a multiple of 60 seconds (1 minute). Start
//!    time between 15 and 63 days ago - Use a multiple of 300 seconds (5 minutes). Start time
//!    greater than 63 days ago - Use a multiple of 3600 seconds (1 hour)."
//!    (`GetMetricStatisticsInput$Period`, in the same words on `MetricStat$Period`). Those
//!    bands cover every start time that can select a tier, because "Data points with a period
//!    of 3600 seconds (1 hour) are available for 455 days (15 months)" is as far back as a
//!    series is held at all — so a range beginning before that horizon is read FROM the
//!    horizon, at the granularity the oldest tier holds, rather than answered with nothing
//!    because its start reaches past where every tier ends.
//! 2. **A namespace is refused only for the space AWS reserves for its own services'
//!    metrics.** The modelled `@pattern "^[^:]"` on `Namespace`
//!    (`smithy_query(query="shape",shape="Namespace")`) is not a constraint the service
//!    applies: a publish into a namespace beginning with any other character is accepted, and
//!    the series it created is then listed and read back. Only the reserved `AWS/` prefix is
//!    refused (`PutMetricDataInput$Namespace`).
//!
//! Both rules are asserted through BOTH readers: `GetMetricStatistics`, which aggregates one
//! metric into period windows, and `GetMetricData`, which answers a query array — one store,
//! two read paths, so a rule has to hold in both.

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, now_secs, ok};

/// The dimension set every series in this suite is published with, so a read names the same
/// metric identity the publish created.
fn dimensions() -> Value {
    json!([{"Name": "Service", "Value": "api"}])
}

/// One `PutMetricData` request publishing `value` into `namespace` at `at`, stored at
/// one-second granularity so a sub-minute read sees the instant it was published at
/// ("Setting this to 1 specifies this metric as a high-resolution metric",
/// `MetricDatum$StorageResolution`).
fn publish(namespace: &str, metric: &str, at: f64, value: f64) -> Value {
    json!({
        "Namespace": namespace,
        "MetricData": [{
            "MetricName": metric,
            "Dimensions": dimensions(),
            "Timestamp": at,
            "Value": value,
            "Unit": "Count",
            "StorageResolution": 1,
        }],
    })
}

/// A `GetMetricStatistics` request over the series `namespace`/`metric`.
fn statistics_request(namespace: &str, metric: &str, start: f64, end: f64, period: i64) -> Value {
    json!({
        "Namespace": namespace,
        "MetricName": metric,
        "Dimensions": dimensions(),
        "StartTime": start,
        "EndTime": end,
        "Period": period,
        "Statistics": ["Sum", "SampleCount", "Minimum", "Maximum"],
    })
}

/// A `GetMetricData` request over the same series, as one `MetricStat` query.
fn data_request(namespace: &str, metric: &str, start: f64, end: f64, period: i64) -> Value {
    json!({
        "MetricDataQueries": [{
            "Id": "m1",
            "MetricStat": {
                "Metric": {
                    "Namespace": namespace,
                    "MetricName": metric,
                    "Dimensions": dimensions(),
                },
                "Period": period,
                "Stat": "Sum",
            },
        }],
        "StartTime": start,
        "EndTime": end,
    })
}

/// The `(Sum, Unit)` of every datapoint a `GetMetricStatistics` answer carries.
fn datapoints(body: &Value) -> Vec<(f64, String)> {
    body["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| {
            (
                point["Sum"].as_f64().unwrap_or_default(),
                point["Unit"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `(Timestamps, Values)` the first result of a `GetMetricData` answer carries.
fn series(body: &Value) -> (Vec<f64>, Vec<f64>) {
    let numbers = |member: &str| -> Vec<f64> {
        body["MetricDataResults"][0][member]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|value| value.as_f64().unwrap_or_default())
            .collect()
    };
    (numbers("Timestamps"), numbers("Values"))
}

/// Three hours, the span the sub-minute tier holds a datapoint for.
const HOUR: f64 = 60.0 * 60.0;

/// One day, the unit the two coarser tiers are published in.
const DAY: f64 = 24.0 * HOUR;

#[tokio::test]
async fn a_read_answers_with_the_datapoints_its_range_still_holds() {
    let router = build_router();
    let namespace = "Probe/Retention";
    let now = now_secs();

    // The rows differ along what the rule generalises over: how far back the requested window
    // BEGINS, how fine the granularity it is read at, and how old the DATAPOINT in it is —
    // each published tier boundary covered from both sides, plus a start reaching back past
    // the horizon every tier ends at. Each row's window ends after the datapoint it published,
    // and spans no more than the 1,440 datapoints one `GetMetricStatistics` call may return
    // ("The maximum number of data points returned from a single call is 1,440.").
    //
    // (case, the datapoint's age, how far back the window starts, the period, whether it shows)
    let rows: Vec<(&str, f64, f64, i64, bool)> = vec![
        (
            "a window inside the three-hour tier, read at a minute",
            60.0,
            10.0 * 60.0,
            60,
            true,
        ),
        (
            "a window inside the three-hour tier, read at a sub-minute period",
            60.0,
            10.0 * 60.0,
            30,
            true,
        ),
        (
            "a window starting past the three-hour tier, read at a sub-minute period that tier \
             no longer holds",
            60.0,
            4.0 * HOUR,
            30,
            false,
        ),
        (
            "the same window, read at the minute its tier does hold",
            60.0,
            4.0 * HOUR,
            60,
            true,
        ),
        (
            "a window starting past the fifteen-day tier, read at a granularity that tier no \
             longer holds",
            60.0,
            20.0 * DAY,
            1_260,
            false,
        ),
        (
            "the same window, read at the five minutes its tier does hold",
            60.0,
            20.0 * DAY,
            1_500,
            true,
        ),
        (
            "a window starting past the sixty-three-day tier, read at a granularity that tier \
             no longer holds",
            60.0,
            70.0 * DAY,
            4_260,
            false,
        ),
        (
            "the same window, read at the hours its tier does hold",
            60.0,
            70.0 * DAY,
            7_200,
            true,
        ),
        (
            "a window reaching back past the horizon every tier ends at, read at the hours the \
             oldest tier holds",
            60.0,
            500.0 * DAY,
            86_400,
            true,
        ),
        (
            "a datapoint of its own past the three-hour tier, read at the minute its tier holds",
            4.0 * HOUR,
            5.0 * HOUR,
            60,
            true,
        ),
    ];

    for (index, (case, age, window, period, visible)) in rows.into_iter().enumerate() {
        let metric = format!("Retention{index}");
        let at = now - age;
        let start = now - window;
        let end = now + f64::from(i32::try_from(period).unwrap_or(60));
        ok(&router, "PutMetricData", &publish(namespace, &metric, at, 7.0)).await;

        let statistics = ok(
            &router,
            "GetMetricStatistics",
            &statistics_request(namespace, &metric, start, end, period),
        )
        .await;
        let points = datapoints(&statistics);
        let expected: Vec<(f64, String)> = if visible {
            vec![(7.0, "Count".to_string())]
        } else {
            Vec::new()
        };
        assert_eq!(
            points, expected,
            "case {case:?}: expected GetMetricStatistics to answer {expected:?}, got {points:?} \
             from {statistics}"
        );

        let data = ok(
            &router,
            "GetMetricData",
            &data_request(namespace, &metric, start, end, period),
        )
        .await;
        let (timestamps, values) = series(&data);
        // "The number of timestamps always matches the number of values." (`MetricDataResult`)
        assert_eq!(
            timestamps.len(),
            values.len(),
            "case {case:?}: a result's timestamps and values must be in step, got \
             {timestamps:?} and {values:?}"
        );
        assert_eq!(
            values,
            if visible { vec![7.0] } else { Vec::new() },
            "case {case:?}: expected GetMetricData to answer the same series, got {values:?} \
             from {data}"
        );
    }
}

#[tokio::test]
async fn a_namespace_is_refused_only_for_the_space_aws_reserves() {
    let now = now_secs();
    // The rows differ along what the rule generalises over: the character a namespace begins
    // with, where a colon sits in it, and whether it names the reserved space at all — in
    // either case. The reserved prefix is `PutMetricDataInput$Namespace`'s ("you should not
    // specify a namespace that begins with AWS/").
    //
    // (case, the namespace, whether a publish into it is accepted)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a plain custom namespace", "Probe/Namespaces", true),
        ("a namespace beginning with a colon", ":probe-namespaces", true),
        ("a namespace carrying a colon inside it", "probe:namespaces", true),
        ("a namespace that is only a colon", ":", true),
        ("a namespace with a space in it", "probe namespaces", true),
        ("the reserved space", "AWS/ES", false),
        ("the reserved space in another case", "aws/es", false),
        ("a namespace that merely mentions the reserved space", "my-AWS/ES", true),
    ];

    for (index, (case, namespace, accepted)) in rows.into_iter().enumerate() {
        // A fresh emulator per row, so what a row reads back is what that row published.
        let router = build_router();
        let metric = format!("Published{index}");
        let at = now - 60.0;
        let start = now - 600.0;
        let end = now + 60.0;

        let (status, body) = call(&router, "PutMetricData", &publish(namespace, &metric, at, 3.0)).await;
        if !accepted {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the publish to be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected the parameter-value fault, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected the publish to be accepted, got {status} with {body}"
        );

        // The series it created is in the catalogue …
        let listed = ok(&router, "ListMetrics", &json!({"Namespace": namespace})).await;
        let names: Vec<&str> = listed["Metrics"]
            .as_array()
            .map(|metrics| {
                metrics
                    .iter()
                    .filter_map(|held| held["MetricName"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec![metric.as_str()],
            "case {case:?}: expected the catalogue to list the published metric, got {listed}"
        );

        // … and its datapoint reads back through both readers.
        let statistics = ok(
            &router,
            "GetMetricStatistics",
            &statistics_request(namespace, &metric, start, end, 60),
        )
        .await;
        let points = datapoints(&statistics);
        assert_eq!(
            points,
            vec![(3.0, "Count".to_string())],
            "case {case:?}: expected the published datapoint back, got {points:?} from \
             {statistics}"
        );
        let data = ok(
            &router,
            "GetMetricData",
            &data_request(namespace, &metric, start, end, 60),
        )
        .await;
        let (timestamps, values) = series(&data);
        assert_eq!(
            timestamps.len(),
            values.len(),
            "case {case:?}: a result's timestamps and values must be in step, got \
             {timestamps:?} and {values:?}"
        );
        assert_eq!(
            values,
            vec![3.0],
            "case {case:?}: expected GetMetricData to answer the published series, got {data}"
        );
    }
}

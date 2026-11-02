// SPDX-License-Identifier: CC0-1.0

//! The GRANULARITY a stored datapoint is read at, end to end over the wire.
//!
//! ONE rule, asserted over a table: **a datapoint published at REGULAR resolution is stored at
//! one-minute granularity — "Setting this to 1 specifies this metric as a high-resolution
//! metric, so that CloudWatch stores the metric with sub-minute resolution down to one second.
//! Setting this to 60 specifies this metric as regular resolution, which CloudWatch stores at
//! 1-minute resolution."
//! (`smithy_query(query="doc",shape="MetricDatum",member="StorageResolution")`) — so however
//! short a period a read asks for, such a datapoint is aggregated in the MINUTE its own time
//! stamp falls in: a sub-minute-period read over a regular-resolution series answers with
//! whole-minute instants and folds every observation of one minute together. Only a
//! high-resolution datapoint (`StorageResolution` 1) is aggregated at a sub-minute period.**
//!
//! A read cannot report detail finer than the resolution the data is stored at, so the period it
//! asks for bounds the granularity from above and the datapoint's own `StorageResolution` bounds
//! it from below. `StorageResolution` is optional and "if you omit this, the default of 60 is
//! used", so an omitted member reads exactly like 60.
//!
//! The rows differ along what the rule generalises over: the resolution the series was published
//! at (1, 60, and omitted), the period the read asks for (1, 5, 10, 20 and 30 seconds — every
//! sub-minute period `MetricStat$Period` admits — plus 60 and 300, where the two resolutions
//! cannot differ), and where inside their minutes the observations fall. Both readers of a
//! series are driven, because one fold answers for both.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, ok};

/// One minute, in the epoch seconds a published time stamp carries.
const MINUTE: f64 = 60.0;

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/StorageResolution";

/// A minute-aligned instant ten minutes in the past: recent enough to sit inside every retention
/// tier, and aligned so the documented start-time rounding ("Start time less than 15 days ago -
/// Round down to the nearest whole minute", `GetMetricDataInput$StartTime`) leaves a request's
/// start exactly where it put it.
fn base_time() -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    (now / MINUTE).floor() * MINUTE - 10.0 * MINUTE
}

/// The offsets, in seconds from the minute-aligned base, that every row publishes one
/// observation at: two inside the first minute and one inside the second, so a read can tell a
/// minute-wide fold from a finer one.
const OFFSETS: [f64; 3] = [0.0, 10.0, 70.0];

/// One row of the table.
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The `StorageResolution` the series is published with, or `None` for a publish that omits
    /// the member and takes the documented default.
    storage_resolution: Option<i64>,
    /// The `Period` the read asks for, in seconds.
    period: i64,
    /// The `(offset from the base, SampleCount)` buckets the read must answer with, oldest
    /// first.
    buckets: &'static [(f64, f64)],
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded case: a regular-resolution series read at a ten-second period answers
        // with the MINUTE each observation was published in, so both observations of the first
        // minute are one datapoint and every instant is on a whole minute.
        case: "regular resolution read at ten seconds",
        storage_resolution: Some(60),
        period: 10,
        buckets: &[(0.0, 2.0), (60.0, 1.0)],
    },
    Row {
        // The same series published at high resolution: the sub-minute detail is stored, so the
        // read sees the ten-second buckets it asked for.
        case: "high resolution read at ten seconds",
        storage_resolution: Some(1),
        period: 10,
        buckets: &[(0.0, 1.0), (10.0, 1.0), (70.0, 1.0)],
    },
    Row {
        // "If you omit this, the default of 60 is used" — an omitted member is regular
        // resolution, and reads exactly like the first row.
        case: "an omitted StorageResolution read at ten seconds",
        storage_resolution: None,
        period: 10,
        buckets: &[(0.0, 2.0), (60.0, 1.0)],
    },
    Row {
        case: "regular resolution read at one second",
        storage_resolution: Some(60),
        period: 1,
        buckets: &[(0.0, 2.0), (60.0, 1.0)],
    },
    Row {
        case: "high resolution read at one second",
        storage_resolution: Some(1),
        period: 1,
        buckets: &[(0.0, 1.0), (10.0, 1.0), (70.0, 1.0)],
    },
    Row {
        case: "regular resolution read at five seconds",
        storage_resolution: Some(60),
        period: 5,
        buckets: &[(0.0, 2.0), (60.0, 1.0)],
    },
    Row {
        case: "regular resolution read at twenty seconds",
        storage_resolution: Some(60),
        period: 20,
        buckets: &[(0.0, 2.0), (60.0, 1.0)],
    },
    Row {
        case: "high resolution read at twenty seconds",
        storage_resolution: Some(1),
        period: 20,
        // The two observations of the first minute fall in ONE twenty-second bucket, and the
        // third in the bucket sixty seconds on: a window is the period asked for, not the
        // spacing of the data.
        buckets: &[(0.0, 2.0), (60.0, 1.0)],
    },
    Row {
        case: "regular resolution read at thirty seconds",
        storage_resolution: Some(60),
        period: 30,
        buckets: &[(0.0, 2.0), (60.0, 1.0)],
    },
    Row {
        // At a one-minute period the two resolutions cannot differ: the window IS the minute the
        // regular-resolution datapoint is stored in.
        case: "regular resolution read at a minute",
        storage_resolution: Some(60),
        period: 60,
        buckets: &[(0.0, 2.0), (60.0, 1.0)],
    },
    Row {
        case: "high resolution read at a minute",
        storage_resolution: Some(1),
        period: 60,
        buckets: &[(0.0, 2.0), (60.0, 1.0)],
    },
    Row {
        // A period coarser than a minute folds both minutes together, whichever resolution the
        // series is stored at.
        case: "regular resolution read at five minutes",
        storage_resolution: Some(60),
        period: 300,
        buckets: &[(0.0, 3.0)],
    },
    Row {
        case: "high resolution read at five minutes",
        storage_resolution: Some(1),
        period: 300,
        buckets: &[(0.0, 3.0)],
    },
];

/// Publish this row's observations into a series of its own, each one offset from the
/// minute-aligned `base`.
async fn publish(router: &MultiServiceRouter, metric: &str, row: &Row, base: f64) {
    let data: Vec<Value> = OFFSETS
        .iter()
        .map(|offset| {
            let mut datum = json!({
                "MetricName": metric,
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "Value": 1.0,
                "Unit": "Count",
                "Timestamp": base + offset,
            });
            if let Some(resolution) = row.storage_resolution {
                datum["StorageResolution"] = json!(resolution);
            }
            datum
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": data}),
    )
    .await;
}

/// The `(offset from `base`, SampleCount)` buckets `GetMetricStatistics` answers with, oldest
/// first.
async fn statistics_buckets(
    router: &MultiServiceRouter,
    metric: &str,
    row: &Row,
    base: f64,
) -> Vec<(f64, f64)> {
    let body = ok(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{"Name": "Service", "Value": "api"}],
            "StartTime": base,
            "EndTime": base + 2.0 * MINUTE,
            "Period": row.period,
            "Statistics": ["SampleCount"],
        }),
    )
    .await;
    let mut buckets: Vec<(f64, f64)> = body["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| {
            (
                point["Timestamp"].as_f64().unwrap_or_default() - base,
                point["SampleCount"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    buckets.sort_by(|left, right| left.0.total_cmp(&right.0));
    buckets
}

/// The same buckets as `GetMetricData` answers with, oldest first.
async fn metric_data_buckets(
    router: &MultiServiceRouter,
    metric: &str,
    row: &Row,
    base: f64,
) -> Vec<(f64, f64)> {
    let body = ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [{
                "Id": "m1",
                "MetricStat": {
                    "Metric": {
                        "Namespace": NAMESPACE,
                        "MetricName": metric,
                        "Dimensions": [{"Name": "Service", "Value": "api"}],
                    },
                    "Period": row.period,
                    "Stat": "SampleCount",
                },
            }],
            "StartTime": base,
            "EndTime": base + 2.0 * MINUTE,
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    let result = &body["MetricDataResults"][0];
    let stamps: Vec<f64> = result["Timestamps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    let values: Vec<f64> = result["Values"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    assert_eq!(
        stamps.len(),
        values.len(),
        "a result's Timestamps and Values stay in step, got {result}"
    );
    stamps
        .into_iter()
        .map(|stamp| stamp - base)
        .zip(values)
        .collect()
}

#[tokio::test]
async fn a_read_sees_a_series_at_the_granularity_it_is_stored_at() {
    let router = build_router();
    let base = base_time();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let metric = format!("Resolution{index}");
        publish(&router, &metric, row, base).await;

        for (reader, got) in [
            (
                "GetMetricStatistics",
                statistics_buckets(&router, &metric, row, base).await,
            ),
            (
                "GetMetricData",
                metric_data_buckets(&router, &metric, row, base).await,
            ),
        ] {
            assert_eq!(
                got,
                row.buckets.to_vec(),
                "case {case:?}: expected {reader} to fold the series into {:?}, got {got:?}",
                row.buckets
            );
            // A regular-resolution series has no sub-minute detail to report, so every instant
            // such a read answers with is on a whole minute whatever period it asked for.
            if row.storage_resolution.unwrap_or(60) > 1 {
                for (offset, _) in &got {
                    assert!(
                        (base + offset).rem_euclid(MINUTE) == 0.0,
                        "case {case:?}: {reader} must answer with whole-minute instants over a \
                         regular-resolution series, got the offset {offset} from {base}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_range_inside_one_minute_holds_a_datapoint_published_inside_it_at_either_resolution() {
    // What the requested range is judged against is the SPAN the datapoint is stored as, at
    // either resolution: a range opening forty seconds into a minute holds an observation
    // published fifty seconds into it — the second it is stored in for a high-resolution
    // series, and the minute it is stored in for a regular-resolution one. What the resolution
    // decides is the bucket the selected observation is REPORTED under: the second it was
    // published at where that detail is stored, and the whole minute it was published in
    // otherwise, which is the only instant such a datapoint has even where that minute began
    // before the range did.
    let router = build_router();
    let base = base_time();

    // (case, the StorageResolution published, the offset from the base the read reports)
    for (case, resolution, reported_at) in [
        ("regular resolution", 60, 0.0),
        ("high resolution", 1, 50.0),
    ] {
        let metric = format!("Ranged{resolution}");
        ok(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "MetricData": [{
                    "MetricName": metric,
                    "Dimensions": [{"Name": "Service", "Value": "api"}],
                    "Value": 1.0,
                    "Unit": "Count",
                    "StorageResolution": resolution,
                    // Fifty seconds into the minute, so a range opening forty seconds into it
                    // contains the publish instant but not the start of the minute.
                    "Timestamp": base + 50.0,
                }],
            }),
        )
        .await;
        let body = ok(
            &router,
            "GetMetricStatistics",
            &json!({
                "Namespace": NAMESPACE,
                "MetricName": metric,
                "Dimensions": [{"Name": "Service", "Value": "api"}],
                "StartTime": base + 40.0,
                "EndTime": base + 60.0,
                "Period": 10,
                "Statistics": ["SampleCount"],
            }),
        )
        .await;
        let points = body["Datapoints"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: expected the twenty-second range inside the minute to hold the one \
             observation published inside it, got {body}"
        );
        assert_eq!(
            points[0]["Timestamp"].as_f64().map(|stamp| stamp - base),
            Some(reported_at),
            "case {case:?}: expected the datapoint reported at the offset {reported_at} the \
             series stores it at, got {body}"
        );
    }
}

#[tokio::test]
async fn an_alarm_grades_a_regular_resolution_series_by_the_minute() {
    // The alarm evaluation reads the same series the readers above do, so it folds it at the
    // same granularity: an alarm with a sub-minute period sees two observations of ONE minute as
    // two windows only where the series is stored at that resolution, and as the single minute
    // they were published in otherwise.
    let router = build_router();
    // The previous whole minute: both observations below are stamped inside it and in the past,
    // so each falls in a ten-second bucket of its own and both fall in one minute, whatever
    // phase of a minute the test runs in.
    let minute = {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64();
        (now / MINUTE).floor() * MINUTE - MINUTE
    };

    // (case, the StorageResolution published, the state the evaluation must compute)
    //
    // Two breaching windows are needed, and the observations can only be two windows where the
    // sub-minute detail is stored; folded by the minute they are ONE breaching window, which
    // reaches neither the ALARM minimum nor the OK one.
    let rows: [(&str, Option<i64>, &str); 3] = [
        ("high resolution", Some(1), "ALARM"),
        ("regular resolution", Some(60), "INSUFFICIENT_DATA"),
        ("an omitted StorageResolution", None, "INSUFFICIENT_DATA"),
    ];

    for (case, resolution, state) in rows {
        let name = format!("minute-graded-{}", resolution.unwrap_or_default());
        let data: Vec<Value> = [10.0, 20.0]
            .iter()
            .map(|offset| {
                let mut datum = json!({
                    "MetricName": "Graded",
                    "Dimensions": [{"Name": "Case", "Value": name}],
                    "Value": 99.0,
                    "Timestamp": minute + offset,
                });
                if let Some(resolution) = resolution {
                    datum["StorageResolution"] = json!(resolution);
                }
                datum
            })
            .collect();
        ok(
            &router,
            "PutMetricData",
            &json!({"Namespace": NAMESPACE, "MetricData": data}),
        )
        .await;
        ok(
            &router,
            "PutMetricAlarm",
            &json!({
                "AlarmName": name,
                "Namespace": NAMESPACE,
                "MetricName": "Graded",
                "Dimensions": [{"Name": "Case", "Value": name}],
                "Statistic": "Maximum",
                "Period": 10,
                "EvaluationPeriods": 6,
                "DatapointsToAlarm": 2,
                "Threshold": 50.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "TreatMissingData": "missing",
            }),
        )
        .await;
        crate::harness::wait_for_evaluation();

        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(state),
            "case {case:?}: expected StateValue {state} over the windows a \
             {resolution:?}-second series has, got {reported}"
        );
    }
}

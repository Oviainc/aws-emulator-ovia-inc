// SPDX-License-Identifier: CC0-1.0

//! WHICH periods a metric alarm's evaluation grades, and the watermark its decision record
//! reports, end to end over the wire.
//!
//! ONE rule, asserted over a table: **a period an evaluation reports as graded is one whose
//! observations it really had, so the instant it reports as the one it queried the series at is
//! never earlier than the END of a period it says it graded** — `queryDate >=
//! max(evaluatedDatapoints[].timestamp) + period` in the machine-readable half of the decision
//! ("An explanation for the alarm state, in JSON format.",
//! `smithy_query(query="shape",shape="MetricAlarm")`), and in the
//! same document as `DescribeAlarmHistory` records it ("Data about the alarm, in JSON format.",
//! `smithy_query(query="shape",shape="AlarmHistoryItem")`).
//!
//! A period is aggregated into one datapoint out of EVERY observation stamped inside it —
//! "CloudWatch aggregates all data points with time stamps that fall within each one-hour period"
//! (`smithy_query(query="doc",shape="GetMetricStatistics")`) — so which period an observation
//! belongs to is decided by its own time stamp, and a period an evaluation grades is one its
//! rolling range has already reached. That is what makes the query instant a watermark a client
//! can reconcile an alarm decision against the metric data with. Every row below publishes into
//! periods that are OVER, which is why each of them names a closed period as the newest it
//! graded.
//!
//! The rows differ along what the rule generalises over: the length of the period (10, 20, 30 and
//! 60 seconds — the high-resolution cadences and the regular one), the statistic that aggregates
//! a window, the comparison operator, the state the evaluation computed, how many windows it
//! grades and how many of them hold a real datapoint, and the `TreatMissingData` arm. The second
//! test is the other side of the same rule for a HIGH-RESOLUTION alarm, whose rolling range lags
//! the clock by its own period: the period the clock is in is graded by the evaluation that
//! follows its CLOSE, not before.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router, document_instant_secs, graded_window_stamp, now_secs, ok, wait_for_evaluation,
    wait_for_period_room, PAST_EVALUATION_DELAY,
};

/// The customer namespace every row of the table watches.
const NAMESPACE: &str = "Custom/ClosedPeriods";

/// The metric every row watches.
const METRIC: &str = "Load";

/// How far past the test's own clock reading a document's instant may sit: a document holds an
/// instant to the MILLISECOND, so the one it writes for the instant an evaluation ran is that
/// instant rounded, not the instant itself.
const INSTANT_TOLERANCE_SECS: f64 = 0.0015;

/// One datapoint a row publishes: its value and WHICH period it belongs to, counting back from
/// the newest period an evaluation grades.
#[derive(Clone, Copy, Debug)]
struct Placed {
    /// The value published.
    value: f64,
    /// How many periods back from the newest period an evaluation grades the datapoint's time
    /// stamp falls in.
    periods_back: u32,
}

/// A datapoint of `value` stamped inside the `periods_back`th period back from the newest one an
/// evaluation grades.
const fn placed(value: f64, periods_back: u32) -> Placed {
    Placed { value, periods_back }
}

/// One row of the table.
#[derive(Clone, Debug)]
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The alarm's `Period`, in seconds.
    period: i32,
    /// The alarm's `Statistic`.
    statistic: &'static str,
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The alarm's `Threshold`.
    threshold: f64,
    /// The alarm's `EvaluationPeriods`.
    evaluation_periods: i32,
    /// The alarm's `DatapointsToAlarm`, where the row names one.
    datapoints_to_alarm: Option<i32>,
    /// The alarm's `TreatMissingData`, where the row names one.
    treat_missing_data: Option<&'static str>,
    /// The datapoints the row publishes, each placed in a period of its own choosing.
    data: &'static [Placed],
    /// The state the evaluation must put the alarm in.
    state: &'static str,
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded case: a high-resolution `Maximum` over three consecutive 10-second
        // buckets, all three breaching.
        case: "a Maximum over three breaching 10-second buckets",
        period: 10,
        statistic: "Maximum",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 3,
        datapoints_to_alarm: Some(3),
        treat_missing_data: Some("missing"),
        data: &[placed(87.0, 2), placed(87.0, 1), placed(87.0, 0)],
        state: "ALARM",
    },
    Row {
        case: "an Average over one 60-second bucket",
        period: 60,
        statistic: "Average",
        comparison: "GreaterThanOrEqualToThreshold",
        threshold: 50.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        treat_missing_data: None,
        data: &[placed(50.0, 0)],
        state: "ALARM",
    },
    Row {
        // The OK direction: the record of a state no datapoint breached carries the same
        // watermark.
        case: "a Minimum over a 30-second bucket that does not cross",
        period: 30,
        statistic: "Minimum",
        comparison: "LessThanThreshold",
        threshold: 10.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        treat_missing_data: None,
        data: &[placed(50.0, 0)],
        state: "OK",
    },
    Row {
        // Gaps inside the window: the graded buckets are two periods apart rather than
        // consecutive, so the newest of them is not the oldest.
        case: "three breaching 20-second buckets of five, with gaps",
        period: 20,
        statistic: "Sum",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 5,
        datapoints_to_alarm: Some(3),
        treat_missing_data: Some("missing"),
        data: &[placed(99.0, 4), placed(99.0, 2), placed(99.0, 0)],
        state: "ALARM",
    },
    Row {
        // A window the missing-data arm decides: the record names no graded bucket at all, and
        // the watermark it carries is still the instant the query ran.
        case: "a breaching arm over a series with no datapoints at all",
        period: 60,
        statistic: "Maximum",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 2,
        datapoints_to_alarm: Some(2),
        treat_missing_data: Some("breaching"),
        data: &[],
        state: "ALARM",
    },
];

/// The alarm request one row configures, watching its own dimension set so every row is its own
/// time series and all of them share one emulator.
fn alarm_of(row: &Row, name: &str) -> Value {
    let mut alarm = json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Case", "Value": name}],
        "Statistic": row.statistic,
        "Period": row.period,
        "EvaluationPeriods": row.evaluation_periods,
        "Threshold": row.threshold,
        "ComparisonOperator": row.comparison,
        "ActionsEnabled": false,
    });
    let members = alarm.as_object_mut().expect("the request is an object");
    if let Some(count) = row.datapoints_to_alarm {
        members.insert("DatapointsToAlarm".to_string(), json!(count));
    }
    if let Some(arm) = row.treat_missing_data {
        members.insert("TreatMissingData".to_string(), json!(arm));
    }
    alarm
}

/// Publish a row's datapoints, each stamped in the middle of the period it names, counting back
/// from the newest period the evaluation grades.
async fn publish_placed(router: &MultiServiceRouter, name: &str, row: &Row) {
    if row.data.is_empty() {
        return;
    }
    let period = f64::from(row.period);
    let newest_graded = graded_window_stamp(period);
    let data: Vec<Value> = row
        .data
        .iter()
        .map(|datum| {
            json!({
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Case", "Value": name}],
                "Value": datum.value,
                // A high-resolution series: a sub-minute period reads only data stored at that
                // resolution, because regular resolution stores a datapoint at 1-minute
                // granularity ("Setting this to 1 specifies this metric as a high-resolution
                // metric", `MetricDatum$StorageResolution`).
                "StorageResolution": 1,
                "Timestamp": newest_graded - f64::from(datum.periods_back) * period,
            })
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": data}),
    )
    .await;
}

/// One instant a decision record carries, as epoch seconds.
fn instant_of(case: &str, record: &Value, member: &str) -> f64 {
    record
        .get(member)
        .and_then(Value::as_str)
        .and_then(document_instant_secs)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} instant, got {record}"))
}

/// The instants the record reports for the datapoints the verdict rests on, as epoch seconds —
/// an entry standing for a window a missing-data arm substituted a datapoint for carries none.
fn graded_instants(record: &Value) -> Vec<f64> {
    record
        .get("evaluatedDatapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            entry
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(document_instant_secs)
        })
        .collect()
}

/// Assert THE RULE over one decision record: the instant the evaluation queried the series at is
/// at or after the end of the newest period the record says it graded, and every instant it names
/// is the start of one of the alarm's periods.
fn assert_query_is_the_watermark(case: &str, what: &str, record: &Value, period: f64) {
    let query_date = instant_of(case, record, "queryDate");
    assert_eq!(
        record.get("period").and_then(Value::as_f64),
        Some(period),
        "case {case:?}: {what} must report the period it graded ({period}), got {record}"
    );
    let graded = graded_instants(record);
    for instant in &graded {
        assert!(
            instant.rem_euclid(period) == 0.0,
            "case {case:?}: {what} must name the START of a {period}-second bucket, got \
             {instant} in {record}"
        );
        assert!(
            query_date >= instant + period,
            "case {case:?}: {what} must not report a period as graded before that period is \
             over, so its queryDate ({query_date}) must be at or after the end of the bucket it \
             names ({}), got {record}",
            instant + period
        );
    }
    if let Some(newest) = graded.iter().copied().reduce(f64::max) {
        // The oldest bucket the record opens its span with is one of the graded ones, so it is
        // behind the watermark too.
        let start_date = instant_of(case, record, "startDate");
        assert!(
            start_date <= newest && query_date >= start_date + period,
            "case {case:?}: {what} must open its span at the oldest bucket it graded and no \
             later than the newest ({newest}), got {record}"
        );
    }
    // Nothing the evaluation reports was read from the future: the query instant is one that
    // has happened, give or take the millisecond the document rounds it to.
    assert!(
        query_date <= now_secs() + INSTANT_TOLERANCE_SECS,
        "case {case:?}: {what} must name a query instant that has already happened, got \
         {query_date} in {record}"
    );
}

/// The `StateReasonData` of a described alarm, parsed: it is "an explanation for the alarm state,
/// in JSON format", so a consumer reads it as a document.
fn reason_data(case: &str, alarm: &Value) -> Value {
    let text = alarm
        .get("StateReasonData")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected StateReasonData, got {alarm}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("case {case:?}: StateReasonData is not JSON: {error}"))
}

#[tokio::test]
async fn an_alarms_query_instant_is_never_before_the_end_of_the_newest_period_it_graded() {
    // Every row watches its own series, so all of them are configured on ONE emulator and the
    // evaluation delay is waited out once rather than per row.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("closed-periods-{index}");
        ok(&router, "PutMetricAlarm", &alarm_of(row, &name)).await;
        publish_placed(&router, &name, row).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in ROWS.iter().zip(&names) {
        let case = row.case;
        let period = f64::from(row.period);
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(row.state),
            "case {case:?}: expected StateValue {}, got {reported}",
            row.state
        );
        let record = reason_data(case, reported);
        assert_query_is_the_watermark(case, "the decision record", &record, period);

        // The same document as the history records it: this is the one a client reads the
        // decision out of after the fact (`DescribeAlarmHistory`).
        let history = ok(
            &router,
            "DescribeAlarmHistory",
            &json!({
                "AlarmName": name,
                "HistoryItemType": "StateUpdate",
                "ScanBy": "TimestampAscending",
            }),
        )
        .await;
        let items = history["AlarmHistoryItems"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !items.is_empty(),
            "case {case:?}: expected a StateUpdate item for the transition, got {history}"
        );
        for item in &items {
            let data: Value = item
                .get("HistoryData")
                .and_then(Value::as_str)
                .map(|text| {
                    serde_json::from_str(text).unwrap_or_else(|error| {
                        panic!("case {case:?}: HistoryData is not JSON: {error}")
                    })
                })
                .unwrap_or_else(|| panic!("case {case:?}: expected HistoryData, got {item}"));
            let recorded = data
                .pointer("/newState/stateReasonData")
                .map(|embedded| match embedded {
                    // The record is embedded as the document it is; a consumer that reads it as
                    // quoted text parses that text.
                    Value::String(text) => serde_json::from_str::<Value>(text)
                        .unwrap_or_else(|error| {
                            panic!("case {case:?}: the recorded reason data is not JSON: {error}")
                        }),
                    document => document.clone(),
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the recorded decision record, got {data}")
                });
            assert_query_is_the_watermark(case, "the recorded document", &recorded, period);
        }
    }
}

/// The alarm the second test drives: the shortest period an alarm may be evaluated on ("Valid
/// values are 10, 20, 30, and any multiple of 60.",
/// `smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`), so the close of a
/// period is a span a test can wait out — and a HIGH-RESOLUTION period, whose rolling range lags
/// the clock by the period itself, so the period the clock is in is graded only once it is over.
const SHORTEST_PERIOD_SECS: i32 = 10;

/// How much of the period must still be ahead when the datapoint is published: enough for the
/// evaluation the publish makes due to be read while that period is STILL OPEN.
const OPEN_WINDOW_ROOM_SECS: f64 = 5.0;

#[tokio::test]
async fn a_datapoint_of_the_period_the_clock_is_in_is_graded_once_that_period_closes() {
    let router = build_router();
    let name = "unclosed-period";
    let period = f64::from(SHORTEST_PERIOD_SECS);
    ok(
        &router,
        "PutMetricAlarm",
        &ok_alarm_request(name, SHORTEST_PERIOD_SECS),
    )
    .await;

    // A breaching datapoint stamped inside the period the clock is IN, with most of that period
    // still ahead, so the reading below happens before it closes.
    wait_for_period_room(period, OPEN_WINDOW_ROOM_SECS).await;
    let published_at = now_secs();
    let open_window_start = (published_at / period).floor() * period;
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Case", "Value": name}],
                "Value": 87.0,
                "StorageResolution": 1,
                "Timestamp": published_at,
            }],
        }),
    )
    .await;

    // The period it landed in is still receiving datapoints, so the evaluation the publish made
    // due has nothing complete to grade: the alarm is still the state its creation left it in,
    // and `missing` substitutes nothing for the windows it has no datapoint for.
    wait_for_evaluation();
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    let reported = &described["MetricAlarms"][0];
    assert_eq!(
        reported.get("StateValue").and_then(Value::as_str),
        Some("INSUFFICIENT_DATA"),
        "a datapoint of the period the clock is in is not graded while that period is still \
         open, got {reported}"
    );

    // Once the period is over, the same datapoint IS graded — and the instant the evaluation
    // reports is at or after the end of the bucket it names.
    std::thread::sleep(Duration::from_secs_f64(period) + PAST_EVALUATION_DELAY);
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    let reported = &described["MetricAlarms"][0];
    assert_eq!(
        reported.get("StateValue").and_then(Value::as_str),
        Some("ALARM"),
        "the datapoint must be graded by the evaluation that follows the close of its period, \
         got {reported}"
    );
    let case = "a datapoint published into the period the clock was in";
    let record = reason_data(case, reported);
    assert_eq!(
        graded_instants(&record),
        vec![open_window_start],
        "the evaluation must grade the bucket the datapoint was really stamped in \
         ({open_window_start}), got {record}"
    );
    assert_query_is_the_watermark(case, "the decision record", &record, period);
}

/// The alarm the second test configures: one breaching datapoint of its own period is enough to
/// fire it, and a window with no datapoint at all leaves it undecided.
fn ok_alarm_request(name: &str, period: i32) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Case", "Value": name}],
        "Statistic": "Maximum",
        "Period": period,
        "EvaluationPeriods": 1,
        "DatapointsToAlarm": 1,
        "Threshold": 50.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "TreatMissingData": "missing",
        "ActionsEnabled": false,
    })
}

// SPDX-License-Identifier: CC0-1.0

//! WHEN a metric alarm may reach an evaluated verdict, end to end over the wire.
//!
//! ONE rule, asserted over a table: **a missing-data arm stands in only for a period the series
//! an alarm reads really hold no datapoint for, so while those series hold a datapoint newer
//! than the end of the range an evaluation covers, that evaluation has graded nothing and
//! computes no state — the alarm keeps the state it has, and the FIRST decision record it ever
//! reports is one computed over that datapoint.** "Sets how this alarm is to handle missing
//! data points" (`smithy_query(query="doc",shape="MetricAlarm",member="TreatMissingData")`) is
//! about the periods that are missing data; a period the client has just published into is not
//! one of them, it is one the alarm's rolling range ("the evaluated range advances each time
//! the alarm is evaluated, forming a rolling time window",
//! `smithy_query(query="doc",shape="SlidingWindow")`) has not reached yet. So no verdict is ever
//! reached from the arm alone over an EMPTY decision window while the metric is being published
//! into, and every decision record an alarm reports names the datapoints it graded —
//! `StateReasonData` is "an explanation for the alarm state, in JSON format"
//! (`smithy_query(query="shape",shape="MetricAlarm")`), and an explanation with an empty
//! `recentDatapoints` explains nothing a consumer can tell from an alarm that has no data at
//! all.
//!
//! The rows differ along what the rule generalises over: the alarm's configuration (a `Metrics`
//! metric-math array over TWO source metrics, an array over one, and a scalar single-metric
//! alarm), the expression (a scaled quotient, a sum, a product, a halving), the `Stat` each
//! source is folded by, the `TreatMissingData` arm (`notBreaching`, `breaching` and `missing` —
//! each of which would otherwise produce a DIFFERENT premature verdict), the comparison
//! operator, the threshold, the "M out of N" shape and the state the computed verdict reaches.
//!
//! The second test is the recorded case at its own cadence: a one-minute metric-math alarm over
//! two source metrics, created in the phase of the minute in which the period the client
//! published into is still outside the evaluation's range, driven through both legs — the OK
//! verdict over the computed ratio, and the `ALARM` one the raised numerator produces while
//! every raw input stays below the threshold — and read back out of `DescribeAlarmHistory`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    alarm_history_items, build_router, described_alarm, now_secs, ok, wait_for_evaluation,
    wait_for_period_room, AlarmKind,
};

/// The customer namespace every row publishes into.
const NAMESPACE: &str = "Custom/HeldVerdicts";

/// The shortest cadence an alarm may be evaluated on ("Valid values are 10, 20, 30, and any
/// multiple of 60", `smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`).
///
/// Every row of the table is evaluated on it, because a HIGH-RESOLUTION alarm's rolling range
/// lags the clock by its own period (`crate::state::alarm_evaluation`'s `evaluation_lag_secs`):
/// the period the clock is in is therefore always past the end of that range, so a datapoint
/// published into it is one no evaluation running now has reached — the table needs no phase of
/// the clock to arrange that.
const SHORT_PERIOD_SECS: i32 = 10;

/// How much of the period must still be ahead when a row publishes: enough for the publish, the
/// alarm write and the first read after the evaluation delay to all happen while the period the
/// datapoint landed in is STILL open.
const OPEN_WINDOW_ROOM_SECS: f64 = 6.0;

/// How long a row's datapoint may take to be graded once its period closes, and how often a
/// poll looks.
const GRADED_WITHIN: Duration = Duration::from_secs(30);
/// See [`GRADED_WITHIN`].
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// One source metric a row's array reads.
struct Source {
    /// The `Id` the query carries, which the expression refers to.
    id: &'static str,
    /// The metric the query names.
    metric: &'static str,
    /// The `Stat` the query folds it by.
    stat: &'static str,
    /// The value published into it, one datapoint inside the period the clock is in.
    value: f64,
}

/// One row of the table.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The metrics the row publishes and its array reads, each `ReturnData=false`.
    sources: &'static [Source],
    /// The returning expression — `Id` and metric math — or `None` for a SCALAR alarm, which
    /// names its one metric through `Namespace`/`MetricName`/`Statistic` instead.
    expression: Option<(&'static str, &'static str)>,
    /// The alarm's `TreatMissingData`.
    treat_missing_data: &'static str,
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The alarm's `Threshold`.
    threshold: f64,
    /// The alarm's `EvaluationPeriods` (the N).
    evaluation_periods: i32,
    /// The alarm's `DatapointsToAlarm` (the M).
    datapoints_to_alarm: i32,
    /// The `StateValue` the verdict computed over the published datapoints reaches.
    state: &'static str,
    /// The values that verdict's `recentDatapoints` must hold, oldest first.
    recent: &'static [f64],
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded shape: a ratio of two sources, scaled, under the arm that would
        // otherwise answer an empty window with OK — the same state the graded verdict
        // reaches, so only the decision record tells the two apart.
        case: "a scaled quotient of two sources that does not breach",
        sources: &[
            Source {
                id: "errors",
                metric: "Errors",
                stat: "Maximum",
                value: 5.0,
            },
            Source {
                id: "requests",
                metric: "Requests",
                stat: "Maximum",
                value: 20.0,
            },
        ],
        expression: Some(("rate", "(errors / requests) * 100")),
        treat_missing_data: "notBreaching",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        state: "OK",
        recent: &[25.0],
    },
    Row {
        // The same expression breaching, from inputs that both stay far below the threshold:
        // only the computed value can reach this state.
        case: "a scaled quotient that breaches from inputs below the threshold",
        sources: &[
            Source {
                id: "errors",
                metric: "Errors",
                stat: "Maximum",
                value: 15.0,
            },
            Source {
                id: "requests",
                metric: "Requests",
                stat: "Maximum",
                value: 20.0,
            },
        ],
        expression: Some(("rate", "(errors / requests) * 100")),
        treat_missing_data: "notBreaching",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        state: "ALARM",
        recent: &[75.0],
    },
    Row {
        // The BREACHING arm, which would answer an empty window with the opposite state to
        // the one the published datapoints compute.
        case: "a sum of two sources under the breaching arm",
        sources: &[
            Source {
                id: "errors",
                metric: "Faults",
                stat: "Sum",
                value: 5.0,
            },
            Source {
                id: "requests",
                metric: "Calls",
                stat: "Sum",
                value: 20.0,
            },
        ],
        expression: Some(("total", "errors + requests")),
        treat_missing_data: "breaching",
        comparison: "GreaterThanThreshold",
        threshold: 100.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        state: "OK",
        recent: &[25.0],
    },
    Row {
        // One source and the MISSING arm, which substitutes nothing: an empty window reports
        // insufficient data, which is the state the alarm already has — so here it is the
        // decision record, and not the state, that says whether anything was graded.
        case: "a product of one source under the missing arm",
        sources: &[Source {
            id: "m1",
            metric: "Load",
            stat: "Maximum",
            value: 4.0,
        }],
        expression: Some(("e1", "m1 * 3")),
        treat_missing_data: "missing",
        comparison: "GreaterThanThreshold",
        threshold: 10.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        state: "ALARM",
        recent: &[12.0],
    },
    Row {
        // A SCALAR alarm, which names its metric through its own members rather than through
        // a query array: the rule is about the datapoints an evaluation has, so it holds for
        // the single-metric path too.
        case: "a scalar single-metric alarm",
        sources: &[Source {
            id: "unused",
            metric: "Depth",
            stat: "Maximum",
            value: 7.0,
        }],
        expression: None,
        treat_missing_data: "notBreaching",
        comparison: "LessThanThreshold",
        threshold: 50.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        state: "ALARM",
        recent: &[7.0],
    },
    Row {
        // "M out of N" over the expression's output: one breaching datapoint decides a 1-of-2
        // alarm, and the window it decided names the one value it really had.
        case: "one breaching expression datapoint of a 1-of-2 window",
        sources: &[Source {
            id: "m1",
            metric: "Bytes",
            stat: "Minimum",
            value: 80.0,
        }],
        expression: Some(("e1", "m1 / 2")),
        treat_missing_data: "notBreaching",
        comparison: "GreaterThanThreshold",
        threshold: 30.0,
        evaluation_periods: 2,
        datapoints_to_alarm: 1,
        state: "ALARM",
        recent: &[40.0],
    },
];

/// The dimension set the row at `index` publishes and watches: its own, so every row has its own
/// time series and all the rows share one emulator.
fn dimensions(index: usize) -> Value {
    json!([{"Name": "Case", "Value": format!("row-{index}")}])
}

/// The `MetricData` of one row: one datapoint per source, stamped at `at` and stored at
/// one-second granularity, so a sub-minute period can see it at all ("Setting this to 1
/// specifies this metric as a high-resolution metric",
/// `smithy_query(query="doc",shape="MetricDatum",member="StorageResolution")`).
fn metric_data(row: &Row, index: usize, at: f64) -> Value {
    Value::Array(
        row.sources
            .iter()
            .map(|source| {
                json!({
                    "MetricName": source.metric,
                    "Dimensions": dimensions(index),
                    "Value": source.value,
                    "StorageResolution": 1,
                    "Timestamp": at,
                })
            })
            .collect(),
    )
}

/// The alarm request one row configures: a `Metrics` query array for a row that names an
/// expression, and the scalar members for the one that does not.
fn alarm_request(row: &Row, index: usize, name: &str) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "Threshold": row.threshold,
        "ComparisonOperator": row.comparison,
        "EvaluationPeriods": row.evaluation_periods,
        "DatapointsToAlarm": row.datapoints_to_alarm,
        "TreatMissingData": row.treat_missing_data,
        "ActionsEnabled": false,
    });
    let members = request.as_object_mut().expect("the request is an object");
    match row.expression {
        Some((id, expression)) => {
            let mut queries: Vec<Value> = row
                .sources
                .iter()
                .map(|source| {
                    json!({
                        "Id": source.id,
                        "ReturnData": false,
                        "MetricStat": {
                            "Metric": {
                                "Namespace": NAMESPACE,
                                "MetricName": source.metric,
                                "Dimensions": dimensions(index),
                            },
                            "Period": SHORT_PERIOD_SECS,
                            "Stat": source.stat,
                        },
                    })
                })
                .collect();
            queries.push(json!({
                "Id": id,
                "Expression": expression,
                "Label": format!("{id} of {}", row.case),
                "ReturnData": true,
            }));
            members.insert("Metrics".to_string(), Value::Array(queries));
        }
        None => {
            let source = row
                .sources
                .first()
                .expect("a scalar row names the metric it watches");
            members.insert("Namespace".to_string(), json!(NAMESPACE));
            members.insert("MetricName".to_string(), json!(source.metric));
            members.insert("Dimensions".to_string(), dimensions(index));
            members.insert("Statistic".to_string(), json!(source.stat));
            members.insert("Period".to_string(), json!(SHORT_PERIOD_SECS));
        }
    }
    request
}

/// The decision record a described alarm reports, parsed, or `None` where it reports none —
/// which is what an alarm no evaluation has decided yet looks like.
fn decision_record(case: &str, alarm: &Value) -> Option<Value> {
    let text = alarm.get("StateReasonData").and_then(Value::as_str)?;
    Some(
        serde_json::from_str(text)
            .unwrap_or_else(|_| panic!("case {case:?}: StateReasonData must be JSON, got {text}")),
    )
}

/// The numbers a document member holds, as an array.
fn numbers(document: &Value, member: &str) -> Vec<f64> {
    document
        .get(member)
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// Assert THE RULE over one reading: a decision record an alarm reports names the datapoints the
/// evaluation graded, so it is never one with an empty `recentDatapoints`.
fn assert_record_graded_something(case: &str, alarm: &Value) {
    if let Some(record) = decision_record(case, alarm) {
        assert!(
            !numbers(&record, "recentDatapoints").is_empty(),
            "case {case:?}: an alarm whose series holds a datapoint must not report a verdict \
             reached over an empty decision window, got {record}"
        );
    }
}

#[tokio::test]
async fn a_verdict_waits_for_the_datapoint_the_evaluation_has_not_reached_yet() {
    let router = build_router();
    let period = f64::from(SHORT_PERIOD_SECS);

    // Every row publishes into the period the clock is IN, with most of that period still
    // ahead: a high-resolution alarm's range ends at the start of that period, so the datapoint
    // is one no evaluation running now has reached.
    wait_for_period_room(period, OPEN_WINDOW_ROOM_SECS).await;
    let published_at = now_secs();
    let mut names: Vec<String> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("held-verdict-{index}");
        ok(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "MetricData": metric_data(row, index, published_at),
            }),
        )
        .await;
        ok(&router, "PutMetricAlarm", &alarm_request(row, index, &name)).await;
        names.push(name);
    }

    // The evaluation the write made due has now run, and it had nothing to grade: the period
    // its datapoints are in is still open. So no arm of any row's treatment decided anything —
    // every row is still the state its creation left it in, with no decision record at all.
    wait_for_evaluation();
    for (row, name) in ROWS.iter().zip(&names) {
        let case = row.case;
        let alarm = described_alarm(&router, AlarmKind::Metric, name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeAlarms reports no alarm {name}"));
        assert_eq!(
            alarm.get("StateValue").and_then(Value::as_str),
            Some("INSUFFICIENT_DATA"),
            "case {case:?}: an evaluation that has reached none of the series' datapoints must \
             leave the alarm in the state its creation set, got {alarm}"
        );
        assert_eq!(
            alarm.get("StateReasonData").and_then(Value::as_str),
            None,
            "case {case:?}: an evaluation that graded nothing must report no decision record, \
             got {alarm}"
        );
    }

    // Once the period those datapoints are in has closed, each row's verdict IS computed over
    // them — and no reading in between ever carried a record of an empty window.
    let deadline = Instant::now() + GRADED_WITHIN;
    let mut graded: Vec<Option<Value>> = vec![None; ROWS.len()];
    loop {
        for ((index, row), name) in ROWS.iter().enumerate().zip(&names) {
            if graded[index].is_some() {
                continue;
            }
            let alarm = described_alarm(&router, AlarmKind::Metric, name)
                .await
                .unwrap_or_else(|| panic!("case {}: no alarm {name}", row.case));
            assert_record_graded_something(row.case, &alarm);
            if decision_record(row.case, &alarm).is_some() {
                graded[index] = Some(alarm);
            }
        }
        if graded.iter().all(Option::is_some) || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }

    for ((index, row), name) in ROWS.iter().enumerate().zip(&names) {
        let case = row.case;
        let alarm = graded[index].clone().unwrap_or_else(|| {
            panic!("case {case:?}: the alarm {name} never graded the datapoint published for it")
        });
        assert_eq!(
            alarm.get("StateValue").and_then(Value::as_str),
            Some(row.state),
            "case {case:?}: expected the verdict over the published datapoints to be {}, got \
             {alarm}",
            row.state
        );
        let record = decision_record(case, &alarm)
            .unwrap_or_else(|| panic!("case {case:?}: expected a decision record, got {alarm}"));
        assert_eq!(
            numbers(&record, "recentDatapoints"),
            row.recent.to_vec(),
            "case {case:?}: expected the window the verdict was decided over to be {:?}, got \
             {record}",
            row.recent
        );
    }
}

/// The period the recorded case's alarm is evaluated on.
const MINUTE_PERIOD_SECS: i32 = 60;

/// How far into a minute the recorded leg may start.
///
/// A one-minute alarm's rolling range lags the clock by the store's scheduled pass
/// (`crate::state::alarm_evaluation`'s `evaluation_lag_secs`), so the minute the clock is in is
/// outside that range for its first seconds — which is the phase in which a client publishing
/// into that minute and polling at once reads a verdict nothing graded.
const FRESH_MINUTE_SECS: f64 = 2.0;

/// Wait until the clock is inside the first [`FRESH_MINUTE_SECS`] of a minute.
async fn wait_for_a_fresh_minute() {
    while now_secs().rem_euclid(f64::from(MINUTE_PERIOD_SECS)) >= FRESH_MINUTE_SECS {
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The recorded case's query array, over the two source metrics of `namespace`.
fn recorded_metrics(namespace: &str) -> Value {
    json!([
        {
            "Id": "errors",
            "ReturnData": false,
            "MetricStat": {
                "Metric": {"Namespace": namespace, "MetricName": "Errors"},
                "Period": MINUTE_PERIOD_SECS,
                "Stat": "Maximum",
            },
        },
        {
            "Id": "requests",
            "ReturnData": false,
            "MetricStat": {
                "Metric": {"Namespace": namespace, "MetricName": "Requests"},
                "Period": MINUTE_PERIOD_SECS,
                "Stat": "Maximum",
            },
        },
        {
            "Id": "rate",
            "Expression": "(errors / requests) * 100",
            "Label": "ErrorRatePercent",
            "ReturnData": true,
        },
    ])
}

/// Publish the two source datapoints of the recorded case, stamped at `at`.
async fn publish_sources(router: &MultiServiceRouter, namespace: &str, errors: f64, at: f64) {
    ok(
        router,
        "PutMetricData",
        &json!({
            "Namespace": namespace,
            "MetricData": [
                {"MetricName": "Errors", "Value": errors, "Unit": "Count", "Timestamp": at},
                {"MetricName": "Requests", "Value": 20.0, "Unit": "Count", "Timestamp": at},
            ],
        }),
    )
    .await;
}

/// Poll `name` until its decision record reports `wanted` values, asserting THE RULE on every
/// reading, and answer with the alarm as it then reads.
async fn awaited_window(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
    wanted: &[f64],
) -> Value {
    let deadline = Instant::now() + GRADED_WITHIN;
    loop {
        let alarm = described_alarm(router, AlarmKind::Metric, name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeAlarms reports no alarm {name}"));
        assert_record_graded_something(case, &alarm);
        let reached = decision_record(case, &alarm)
            .map(|record| numbers(&record, "recentDatapoints"))
            .unwrap_or_default();
        if reached == wanted || Instant::now() >= deadline {
            return alarm;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

#[tokio::test]
async fn a_minute_long_metric_math_alarm_reaches_its_verdicts_over_the_computed_expression() {
    let router = build_router();
    let namespace = "Custom/HeldMinuteVerdicts";
    let name = "held-minute-rate";
    let case = "a one-minute alarm on a ratio of two sources";

    // The phase the recorded flow starts in: the minute the client is about to publish into has
    // only just opened, so it is still outside the range the next evaluation covers.
    wait_for_a_fresh_minute().await;
    let minute =
        (now_secs() / f64::from(MINUTE_PERIOD_SECS)).floor() * f64::from(MINUTE_PERIOD_SECS);
    publish_sources(&router, namespace, 5.0, minute).await;
    ok(
        &router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Metrics": recorded_metrics(namespace),
            "Threshold": 50.0,
            "ComparisonOperator": "GreaterThanThreshold",
            "EvaluationPeriods": 1,
            "DatapointsToAlarm": 1,
            "TreatMissingData": "notBreaching",
            "ActionsEnabled": false,
        }),
    )
    .await;

    // The first evaluation has run and reached none of the published data, so it decided
    // nothing: the alarm is still un-evaluated rather than OK over an empty window.
    wait_for_evaluation();
    let alarm = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: DescribeAlarms reports no alarm {name}"));
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("INSUFFICIENT_DATA"),
        "case {case:?}: the alarm must stay un-evaluated while the minute it was published into \
         is outside the evaluation's range, got {alarm}"
    );
    assert_eq!(
        alarm.get("StateReasonData").and_then(Value::as_str),
        None,
        "case {case:?}: an evaluation that graded nothing must report no decision record, got \
         {alarm}"
    );

    // The OK verdict is computed over the expression: one value per evaluation period, the
    // ratio of the two sources, with the alarm's own threshold echoed beside it.
    let alarm = awaited_window(&router, name, case, &[25.0]).await;
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("OK"),
        "case {case:?}: expected the computed ratio to put the alarm in OK, got {alarm}"
    );
    let record = decision_record(case, &alarm)
        .unwrap_or_else(|| panic!("case {case:?}: expected a decision record, got {alarm}"));
    assert_eq!(
        numbers(&record, "recentDatapoints"),
        vec![25.0],
        "case {case:?}: expected the OK verdict's window to hold the computed ratio, got {record}"
    );
    assert_eq!(
        record.get("threshold").and_then(Value::as_f64),
        Some(50.0),
        "case {case:?}: expected the record to echo the alarm's threshold, got {record}"
    );

    // The breaching leg: the numerator is raised while BOTH raw inputs stay below the
    // threshold, so only the computed value can fire the alarm.
    publish_sources(&router, namespace, 15.0, minute).await;
    let alarm = awaited_window(&router, name, case, &[75.0]).await;
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("ALARM"),
        "case {case:?}: expected the breaching expression value to fire the alarm, got {alarm}"
    );

    // The transition is recorded, from the state it came FROM, with the same computed value.
    let items = alarm_history_items(&router, AlarmKind::Metric, name, "StateUpdate").await;
    let recorded = items
        .iter()
        .filter_map(|item| item.get("HistoryData").and_then(Value::as_str))
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find(|data| data.pointer("/newState/stateValue").and_then(Value::as_str) == Some("ALARM"))
        .unwrap_or_else(|| panic!("case {case:?}: expected a StateUpdate item, got {items:?}"));
    assert_eq!(
        recorded
            .pointer("/oldState/stateValue")
            .and_then(Value::as_str),
        Some("OK"),
        "case {case:?}: expected the record to name the state the alarm came from, got {recorded}"
    );
    let newest = recorded
        .pointer("/newState/stateReasonData")
        .map(|embedded| match embedded {
            Value::String(text) => serde_json::from_str::<Value>(text).unwrap_or(Value::Null),
            document => document.clone(),
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected the recorded record, got {recorded}"));
    assert_eq!(
        numbers(&newest, "recentDatapoints"),
        vec![75.0],
        "case {case:?}: expected the recorded transition to carry the computed value, got \
         {newest}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! A metric-math alarm's evaluation ADVANCES, end to end over the wire: an alarm configured
//! with a `Metrics` query array is graded again from the stored series every time an
//! evaluation comes due, so its first verdict is never its last one.
//!
//! ONE rule, asserted over a table: **a metric alarm is a standing comparison, not a
//! one-shot one — every evaluation grades the periods of the series its configuration names
//! as the store holds them at that instant, so a datapoint that lands after an earlier
//! verdict is graded by the next evaluation and moves the alarm's `StateValue` whenever the
//! value it computes crosses the threshold the other way** ("The arithmetic operation to use
//! when comparing the specified statistic and threshold",
//! `smithy_query(query="shape",shape="MetricAlarm")`, graded over the series the query
//! carrying `ReturnData=true` resolves to: "one must have true as the value for ReturnData.
//! The result of this expression is the value the alarm watches",
//! `smithy_query(query="doc",shape="MetricDataQuery")`). Each such move stamps
//! `StateUpdatedTimestamp` / `StateTransitionedTimestamp` ("The date and time that the
//! alarm's `StateValue` most recently changed") and records its own `StateUpdate` item whose
//! `HistoryData` carries the state it came FROM and the expression values the new verdict
//! rests on.
//!
//! The rows differ along what the rule generalises over: how many metrics the array reads
//! (one and two), the kind of expression (a conditional, a ratio of two series, a product, a
//! sum), the statistic each input is folded by, the width of the period, the comparison
//! operator, the direction the second verdict moves in (`OK` -> `ALARM`, `ALARM` -> `OK`,
//! and one row that does not move at all) and the "M out of N" shape. The two recorded cases
//! the issue was filed about — `IF(m1 > 100, 0, 1000)` over a `Minimum`, and `(m1/m2)*100`
//! over two `Maximum` inputs — are two rows of it.
//!
//! The other two tests are the WHEN of the same rule, over the range an evaluation grades: an
//! alarm's evaluated range "advances each time the alarm is evaluated, forming a rolling time
//! window ... Choose a sliding window when you need the fastest detection and the calendar
//! boundaries of the data don't matter" (`smithy_query(query="doc",shape="SlidingWindow")`,
//! the window an alarm whose configuration names none uses —
//! `smithy_query(query="doc",shape="EvaluationWindow")`). So a datapoint is graded within one
//! evaluation cadence of the write that stored it rather than after the calendar boundary of
//! its period: an alarm whose period is longer than that cadence reaches its verdicts while
//! the period the datapoint landed in is still OPEN (the third test, over two period widths),
//! while a high-resolution alarm — whose period is no longer than the cadence — still grades
//! only periods that have closed (the second test).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    alarm_stamp, build_router, described_alarm, document_instant_secs, graded_window_stamp,
    now_secs, ok, wait_for_evaluation, wait_for_period_room, AlarmKind, PAST_EVALUATION_DELAY,
};

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/MathAlarmAdvance";

/// How close two computed values have to be to count as the same number.
const TOLERANCE: f64 = 1e-9;

/// One metric the array of a row reads.
struct Input {
    /// The `Id` the query carries, which the expression refers to.
    id: &'static str,
    /// The metric name the query names.
    metric: &'static str,
    /// The `Stat` the query folds that metric by.
    stat: &'static str,
}

/// One datapoint a stage publishes: the metric it belongs to, the value, and WHICH of the
/// alarm's periods it is stamped in.
struct Published {
    /// The metric the datapoint is published under.
    metric: &'static str,
    /// The value published.
    value: f64,
    /// How many periods back from the newest window an evaluation grades the datapoint is
    /// stamped in.
    back: u32,
}

/// What one evaluation of a row's alarm must answer with: the state it reaches, and the values
/// of the returning query it reports.
struct Verdict {
    /// The `StateValue` the verdict reaches.
    state: &'static str,
    /// The values the evaluation had, OLDEST first — `recentDatapoints`, "the values the
    /// evaluation really had".
    recent: &'static [f64],
    /// The values the verdict RESTS ON, newest first — `evaluatedDatapoints`, the datapoints
    /// that satisfied the direction the state was decided in, as many as that direction
    /// needed.
    rests_on: &'static [f64],
}

/// One row of the table.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The metric queries of the array, each `ReturnData=false`.
    inputs: &'static [Input],
    /// The returning expression: its `Id` and the metric math it computes.
    expression: (&'static str, &'static str),
    /// The width of each input's window, in seconds (`MetricStat$Period`).
    period: i32,
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The alarm's `Threshold`.
    threshold: f64,
    /// The alarm's `EvaluationPeriods` (the N).
    evaluation_periods: i32,
    /// The alarm's `DatapointsToAlarm` (the M).
    datapoints_to_alarm: i32,
    /// The datapoints published before the alarm is created, and the verdict over them.
    first: (&'static [Published], Verdict),
    /// The datapoints published INTO THE SAME period after that verdict, and the verdict the
    /// next evaluation must reach over the series they leave behind. An evaluation that
    /// reaches the state the alarm already reports changes nothing, so a row that does not
    /// move repeats the explanation its first verdict wrote.
    then: (&'static [Published], Verdict),
    /// The values `GetMetricData` answers the SAME array with over those periods once both
    /// stages are published, newest first: the query path and the evaluator compute the
    /// expression over one store.
    reads_back: &'static [f64],
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded case: a conditional over one `Minimum` input. The later datapoint
        // lowers the minimum, which INVERTS the conditional's output, so the alarm has to
        // leave the `OK` its first evaluation computed.
        case: "a conditional whose output inverts when a lower datapoint lands",
        inputs: &[Input {
            id: "m1",
            metric: "Load",
            stat: "Minimum",
        }],
        expression: ("e1", "IF(m1 > 100, 0, 1000)"),
        period: 60,
        comparison: "GreaterThanThreshold",
        threshold: 500.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        first: (
            &[Published {
                metric: "Load",
                value: 700.0,
                back: 0,
            }],
            Verdict {
                state: "OK",
                recent: &[0.0],
                rests_on: &[0.0],
            },
        ),
        then: (
            &[Published {
                metric: "Load",
                value: 50.0,
                back: 0,
            }],
            Verdict {
                state: "ALARM",
                recent: &[1000.0],
                rests_on: &[1000.0],
            },
        ),
        reads_back: &[1000.0],
    },
    Row {
        // The other recorded case: a RATIO of two metrics. Both raw inputs are above the
        // threshold throughout, so the only value that can decide this alarm is the
        // expression's.
        case: "a ratio of two metrics whose numerator rises",
        inputs: &[
            Input {
                id: "m1",
                metric: "Errors",
                stat: "Maximum",
            },
            Input {
                id: "m2",
                metric: "Requests",
                stat: "Maximum",
            },
        ],
        expression: ("e1", "(m1/m2)*100"),
        period: 60,
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        first: (
            &[
                Published {
                    metric: "Errors",
                    value: 80.0,
                    back: 0,
                },
                Published {
                    metric: "Requests",
                    value: 400.0,
                    back: 0,
                },
            ],
            Verdict {
                state: "OK",
                recent: &[20.0],
                rests_on: &[20.0],
            },
        ),
        then: (
            &[Published {
                metric: "Errors",
                value: 300.0,
                back: 0,
            }],
            Verdict {
                state: "ALARM",
                recent: &[75.0],
                rests_on: &[75.0],
            },
        ),
        reads_back: &[75.0],
    },
    Row {
        // The other direction: an alarm that has already fired leaves `ALARM` once the
        // expression's value stops breaching.
        case: "a product that stops breaching when a lower datapoint lands",
        inputs: &[Input {
            id: "m1",
            metric: "Depth",
            stat: "Minimum",
        }],
        expression: ("e1", "m1 * 2"),
        period: 60,
        comparison: "GreaterThanThreshold",
        threshold: 100.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        first: (
            &[Published {
                metric: "Depth",
                value: 80.0,
                back: 0,
            }],
            Verdict {
                state: "ALARM",
                recent: &[160.0],
                rests_on: &[160.0],
            },
        ),
        then: (
            &[Published {
                metric: "Depth",
                value: 10.0,
                back: 0,
            }],
            Verdict {
                state: "OK",
                recent: &[20.0],
                rests_on: &[20.0],
            },
        ),
        reads_back: &[20.0],
    },
    Row {
        // A five-minute window rather than a one-minute one, over a Sum: the later datapoint
        // is aggregated INTO the window the first one is in, so the graded value changes
        // without any new window.
        case: "a five-minute window whose sum grows",
        inputs: &[Input {
            id: "m1",
            metric: "Bytes",
            stat: "Sum",
        }],
        expression: ("e1", "m1 / 2"),
        period: 300,
        comparison: "GreaterThanOrEqualToThreshold",
        threshold: 20.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        first: (
            &[Published {
                metric: "Bytes",
                value: 20.0,
                back: 0,
            }],
            Verdict {
                state: "OK",
                recent: &[10.0],
                rests_on: &[10.0],
            },
        ),
        then: (
            &[Published {
                metric: "Bytes",
                value: 60.0,
                back: 0,
            }],
            Verdict {
                state: "ALARM",
                recent: &[40.0],
                rests_on: &[40.0],
            },
        ),
        reads_back: &[40.0],
    },
    Row {
        // An "M out of N" alarm over TWO periods: the first verdict needs both of them not to
        // breach, and one breaching datapoint of the newer period is all the second needs.
        case: "one breaching expression datapoint of a 1-of-2 window",
        inputs: &[Input {
            id: "m1",
            metric: "Latency",
            stat: "Maximum",
        }],
        expression: ("e1", "m1 + 1"),
        period: 60,
        comparison: "GreaterThanThreshold",
        threshold: 100.0,
        evaluation_periods: 2,
        datapoints_to_alarm: 1,
        first: (
            &[
                Published {
                    metric: "Latency",
                    value: 9.0,
                    back: 0,
                },
                Published {
                    metric: "Latency",
                    value: 19.0,
                    back: 1,
                },
            ],
            Verdict {
                state: "OK",
                recent: &[20.0, 10.0],
                rests_on: &[10.0, 20.0],
            },
        ),
        then: (
            &[Published {
                metric: "Latency",
                value: 400.0,
                back: 0,
            }],
            Verdict {
                state: "ALARM",
                recent: &[20.0, 401.0],
                rests_on: &[401.0],
            },
        ),
        reads_back: &[401.0, 20.0],
    },
    Row {
        // The control row: a later datapoint that does NOT take the expression across the
        // threshold leaves the state where it was, so the rule is "graded again", not
        // "changed on every write".
        case: "a later datapoint that keeps the expression on the same side",
        inputs: &[Input {
            id: "m1",
            metric: "Queue",
            stat: "Maximum",
        }],
        expression: ("e1", "m1 + 1"),
        period: 60,
        comparison: "GreaterThanThreshold",
        threshold: 1000.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        first: (
            &[Published {
                metric: "Queue",
                value: 5.0,
                back: 0,
            }],
            Verdict {
                state: "OK",
                recent: &[6.0],
                rests_on: &[6.0],
            },
        ),
        then: (
            &[Published {
                metric: "Queue",
                value: 9.0,
                back: 0,
            }],
            // The window now folds to 10.0, which is still under the threshold: the
            // evaluation reaches the state the alarm already reports, so no transition is
            // recorded and neither state stamp moves — while the EXPLANATION is this
            // evaluation's, over the value it graded, because the one the alarm carries was
            // written by an evaluation too.
            Verdict {
                state: "OK",
                recent: &[10.0],
                rests_on: &[10.0],
            },
        ),
        reads_back: &[10.0],
    },
];

/// How much of the row's period must still be ahead when it starts: every publish and every
/// read of one row happens inside one period, so the window its datapoints are stamped in is
/// the same window every evaluation of that row grades.
const ROW_ROOM_SECS: f64 = 10.0;

/// The dimension set the row at `index` publishes and watches: its own, so every row has its
/// own time series and the rows share one emulator.
fn dimensions(index: usize) -> Value {
    json!([{"Name": "Case", "Value": format!("row-{index}")}])
}

/// The `Metrics` query array of one row: each input under its own `Id` with its data not
/// returned, then the one expression the alarm watches.
fn metrics(row: &Row, index: usize) -> Value {
    let mut queries: Vec<Value> = row
        .inputs
        .iter()
        .map(|input| {
            json!({
                "Id": input.id,
                "ReturnData": false,
                "MetricStat": {
                    "Metric": {
                        "Namespace": NAMESPACE,
                        "MetricName": input.metric,
                        "Dimensions": dimensions(index),
                    },
                    "Period": row.period,
                    "Stat": input.stat,
                },
            })
        })
        .collect();
    let (id, expression) = row.expression;
    queries.push(json!({
        "Id": id,
        "Expression": expression,
        "Label": "Graded",
        "ReturnData": true,
    }));
    Value::Array(queries)
}

/// Publish one stage's datapoints, each stamped inside the window its own `back` names,
/// counting back from the window at `newest`.
async fn publish(
    router: &MultiServiceRouter,
    index: usize,
    data: &[Published],
    newest: f64,
    period: f64,
) {
    let series: Vec<Value> = data
        .iter()
        .map(|datum| {
            json!({
                "MetricName": datum.metric,
                "Dimensions": dimensions(index),
                "Value": datum.value,
                "Unit": "Count",
                "Timestamp": newest - f64::from(datum.back) * period,
            })
        })
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": series}),
    )
    .await;
}

/// The value a reason renders a measured quantity as: a decimal that always carries a
/// fraction, the form an alarm explanation writes one in.
fn decimal(value: f64) -> String {
    if value.fract() == 0.0 {
        return format!("{value:.1}");
    }
    format!("{value}")
}

/// The `StateReasonData` document of a described alarm, parsed.
fn reason_data(alarm: &Value, case: &str) -> Value {
    let text = alarm
        .get("StateReasonData")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a StateReasonData member, got {alarm}"));
    serde_json::from_str(text)
        .unwrap_or_else(|_| panic!("case {case:?}: StateReasonData must be JSON, got {text}"))
}

/// The numbers a document member holds as an array.
fn numbers(document: &Value, member: &str) -> Vec<f64> {
    document
        .get(member)
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// The values the datapoints a verdict rests on carry, as its document records them.
fn evaluated_values(document: &Value) -> Vec<f64> {
    document
        .get("evaluatedDatapoints")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("value").and_then(Value::as_f64))
                .collect()
        })
        .unwrap_or_default()
}

/// Every `StateUpdate` item recorded for `name`, oldest first, each as the parsed document its
/// `HistoryData` carries.
async fn state_updates(router: &MultiServiceRouter, name: &str) -> Vec<Value> {
    let body = ok(
        router,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": name,
            "AlarmTypes": ["MetricAlarm"],
            "HistoryItemType": "StateUpdate",
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    body.get("AlarmHistoryItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item.get("HistoryData").and_then(Value::as_str))
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .collect()
}

/// One member of a recorded state snapshot, read whether the record embeds the decision
/// document or carries it as the quoted text an alarm reports it in.
fn recorded_reason_data(record: &Value, side: &str) -> Value {
    match record.pointer(&format!("/{side}/stateReasonData")) {
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or(Value::Null),
        Some(document) => document.clone(),
        None => Value::Null,
    }
}

/// Assert the verdict a described alarm reports: the state, and the expression values it cites
/// in its explanation and in both arrays of its decision document.
fn assert_verdict(case: &str, stage: &str, alarm: &Value, verdict: &Verdict) {
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some(verdict.state),
        "case {case:?}: expected {stage} to report StateValue {:?} over the graded values \
         {:?}, got {alarm}",
        verdict.state,
        verdict.recent
    );
    let reason = alarm
        .get("StateReason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    for value in verdict.recent {
        assert!(
            reason.contains(&format!("{} (", decimal(*value))),
            "case {case:?}: expected {stage} to cite the graded value {} in its StateReason, \
             got {reason:?}",
            decimal(*value)
        );
    }
    let document = reason_data(alarm, case);
    assert_eq!(
        numbers(&document, "recentDatapoints"),
        verdict.recent.to_vec(),
        "case {case:?}: expected {stage} to report recentDatapoints {:?}, got {document}",
        verdict.recent
    );
    assert_eq!(
        evaluated_values(&document),
        verdict.rests_on.to_vec(),
        "case {case:?}: expected {stage} to rest on the graded values {:?}, got {document}",
        verdict.rests_on
    );
}

/// What `GetMetricData` answers the row's own array with over the periods its alarm evaluates,
/// ending with the window at `at`: the values of the returning query, newest first.
async fn read_back(router: &MultiServiceRouter, row: &Row, index: usize, at: f64) -> Vec<f64> {
    let period = f64::from(row.period);
    let window = (at / period).floor() * period;
    let span = f64::from(row.evaluation_periods.max(1)) * period;
    let mut queries = metrics(row, index);
    for query in queries.as_array_mut().into_iter().flatten() {
        if let Value::Object(members) = query {
            members.insert("ReturnData".to_string(), Value::Bool(true));
        }
    }
    let body = ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": queries,
            "StartTime": window + period - span,
            "EndTime": window + period,
            "ScanBy": "TimestampDescending",
        }),
    )
    .await;
    body.get("MetricDataResults")
        .and_then(Value::as_array)
        .and_then(|results| {
            results
                .iter()
                .find(|result| {
                    result.get("Id").and_then(Value::as_str) == Some(row.expression.0)
                })
                .cloned()
        })
        .map(|result| numbers(&result, "Values"))
        .unwrap_or_default()
}

#[tokio::test]
async fn a_metrics_array_alarm_is_graded_again_whenever_its_series_changes() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("advancing-math-alarm-{index}");
        let period = f64::from(row.period);
        // Every publish and read of this row lands inside one period, so the window its
        // datapoints are stamped in stays the window the evaluations grade.
        wait_for_period_room(period, ROW_ROOM_SECS).await;
        // The datapoints go into the newest window an evaluation grades — the newest one that
        // has closed — because an evaluation grades the periods that are complete.
        let stamp = graded_window_stamp(period);

        let (first_data, first) = &row.first;
        publish(&router, index, first_data, stamp, period).await;
        ok(
            &router,
            "PutMetricAlarm",
            &json!({
                "AlarmName": name,
                "Metrics": metrics(row, index),
                "Threshold": row.threshold,
                "ComparisonOperator": row.comparison,
                "EvaluationPeriods": row.evaluation_periods,
                "DatapointsToAlarm": row.datapoints_to_alarm,
                "TreatMissingData": "missing",
                "ActionsEnabled": false,
            }),
        )
        .await;
        wait_for_evaluation();

        let alarm = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeAlarms reports no alarm {name}"));
        assert_verdict(case, "the first verdict", &alarm, first);
        let first_updated = alarm_stamp(&alarm, "StateUpdatedTimestamp")
            .unwrap_or_else(|| panic!("case {case:?}: no state stamp in {alarm}"));

        // The later datapoints, into the same period: the series the array reads now folds to
        // a different value, so the next evaluation has a different verdict to reach.
        let (then_data, then) = &row.then;
        publish(&router, index, then_data, stamp, period).await;
        wait_for_evaluation();

        let alarm = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: DescribeAlarms reports no alarm {name}"));
        assert_verdict(case, "the verdict after the later datapoint", &alarm, then);

        // A verdict that MOVED the state stamps the alarm and records its own transition; one
        // that reached the same state again moves neither.
        let moved = then.state != first.state;
        let then_updated = alarm_stamp(&alarm, "StateUpdatedTimestamp")
            .unwrap_or_else(|| panic!("case {case:?}: no state stamp in {alarm}"));
        assert_eq!(
            then_updated > first_updated,
            moved,
            "case {case:?}: expected StateUpdatedTimestamp to {} ({first_updated} -> \
             {then_updated}), got {alarm}",
            if moved { "advance" } else { "stand" }
        );
        assert_eq!(
            alarm_stamp(&alarm, "StateTransitionedTimestamp"),
            Some(then_updated),
            "case {case:?}: expected the transition stamp to be the one the state moved at, \
             got {alarm}"
        );

        let recorded = state_updates(&router, &name).await;
        let wanted = if moved { 2 } else { 1 };
        assert_eq!(
            recorded.len(),
            wanted,
            "case {case:?}: expected {wanted} StateUpdate item(s) for this alarm, got \
             {recorded:?}"
        );
        if moved {
            let latest = recorded.last().unwrap_or(&Value::Null);
            assert_eq!(
                latest.pointer("/oldState/stateValue").and_then(Value::as_str),
                Some(first.state),
                "case {case:?}: expected the recorded transition to come from {:?}, got {latest}",
                first.state
            );
            assert_eq!(
                latest.pointer("/newState/stateValue").and_then(Value::as_str),
                Some(then.state),
                "case {case:?}: expected the recorded transition to reach {:?}, got {latest}",
                then.state
            );
            assert_eq!(
                numbers(&recorded_reason_data(latest, "newState"), "recentDatapoints"),
                then.recent.to_vec(),
                "case {case:?}: expected the record to carry the graded values {:?}, got \
                 {latest}",
                then.recent
            );
        }

        // The same array read through `GetMetricData` computes the values the evaluator
        // graded, so the two paths agree over the same windows of the same series. The read
        // answers newest first, which is the order `recentDatapoints` reverses.
        let read = read_back(&router, row, index, stamp).await;
        let expected: Vec<f64> = row.reads_back.to_vec();
        assert!(
            read.len() == expected.len()
                && read
                    .iter()
                    .zip(&expected)
                    .all(|(answered, wanted)| (answered - wanted).abs() <= TOLERANCE),
            "case {case:?}: expected GetMetricData over the same array to answer {expected:?}, \
             got {read:?}"
        );
    }
}

/// The period the second test grades in: the shortest an alarm may be evaluated on ("Valid
/// values are 10, 20, 30, and any multiple of 60.",
/// `smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`) — a
/// high-resolution period, no wider than the store's evaluation cadence, so its rolling range
/// grades only periods that have closed and the close of one is a span a test can wait out.
const SHORTEST_PERIOD_SECS: i32 = 10;

/// How much of the period must still be ahead when the second datapoint is published: enough
/// for the alarm to be read while that period is STILL OPEN.
const OPEN_WINDOW_ROOM_SECS: f64 = 8.0;

/// The array the period tests' alarms watch: the recorded conditional over the series `case`
/// publishes, whose output inverts when a lower datapoint lands.
fn inverting_array(case: &str, metric: &str, period: i32) -> Value {
    json!([
        {
            "Id": "m1",
            "ReturnData": false,
            "MetricStat": {
                "Metric": {
                    "Namespace": NAMESPACE,
                    "MetricName": metric,
                    "Dimensions": [{"Name": "Case", "Value": case}],
                },
                "Period": period,
                "Stat": "Minimum",
            },
        },
        {
            "Id": "e1",
            "Expression": "IF(m1 > 100, 0, 1000)",
            "Label": "Inverted",
            "ReturnData": true,
        },
    ])
}

#[tokio::test]
async fn a_high_resolution_metrics_array_alarm_grades_its_period_once_that_period_closes() {
    let router = build_router();
    let name = "next-period-math-alarm";
    let metric = "Load";
    let case_series = "next-period";
    let period = f64::from(SHORTEST_PERIOD_SECS);

    // The datapoint the first verdict is computed from: in the newest window that has closed,
    // with most of the current window still ahead so the whole first leg lands in it.
    wait_for_period_room(period, OPEN_WINDOW_ROOM_SECS).await;
    let closed_window = graded_window_stamp(period);
    let publish_datapoint = |value: f64, at: f64| {
        let router = &router;
        async move {
            ok(
                router,
                "PutMetricData",
                &json!({
                    "Namespace": NAMESPACE,
                    "MetricData": [{
                        "MetricName": metric,
                        "Dimensions": [{"Name": "Case", "Value": case_series}],
                        "Value": value,
                        // A sub-minute period reads only data stored at that resolution
                        // ("Setting this to 1 specifies this metric as a high-resolution
                        // metric", `MetricDatum$StorageResolution`).
                        "StorageResolution": 1,
                        "Timestamp": at,
                    }],
                }),
            )
            .await;
        }
    };
    publish_datapoint(700.0, closed_window).await;
    ok(
        &router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Metrics": inverting_array(case_series, metric, SHORTEST_PERIOD_SECS),
            "Threshold": 500.0,
            "ComparisonOperator": "GreaterThanThreshold",
            "EvaluationPeriods": 1,
            "DatapointsToAlarm": 1,
            "TreatMissingData": "missing",
            "ActionsEnabled": false,
        }),
    )
    .await;
    wait_for_evaluation();
    let alarm = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));
    let case = "a datapoint published into the period the clock was in";
    assert_verdict(
        case,
        "the first verdict",
        &alarm,
        &Verdict {
            state: "OK",
            recent: &[0.0],
            rests_on: &[0.0],
        },
    );
    let first_updated = alarm_stamp(&alarm, "StateUpdatedTimestamp")
        .unwrap_or_else(|| panic!("case {case:?}: no state stamp in {alarm}"));

    // A datapoint whose minimum makes the conditional's output breach, stamped inside the
    // period the clock is IN. That period is still receiving datapoints, so no statistic over
    // it is final and the alarm keeps the verdict it has.
    let published_at = now_secs();
    let open_window_start = (published_at / period).floor() * period;
    publish_datapoint(50.0, published_at).await;
    wait_for_evaluation();
    let alarm = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("OK"),
        "case {case:?}: a datapoint of the period the clock is in is not graded while that \
         period is still open, got {alarm}"
    );

    // Once that period has closed, the SAME datapoint is graded and the alarm advances.
    let closes_at = open_window_start + period;
    let wait = (closes_at - now_secs()).max(0.0);
    std::thread::sleep(Duration::from_secs_f64(wait) + PAST_EVALUATION_DELAY);
    // The verdict is materialised by the evaluation the clock owes; a reader asks for it.
    let alarm = described_alarm(&router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("DescribeAlarms reports no alarm {name}"));
    assert_verdict(
        case,
        "the verdict of the period that has just closed",
        &alarm,
        &Verdict {
            state: "ALARM",
            recent: &[1000.0],
            rests_on: &[1000.0],
        },
    );
    let then_updated = alarm_stamp(&alarm, "StateUpdatedTimestamp")
        .unwrap_or_else(|| panic!("case {case:?}: no state stamp in {alarm}"));
    assert!(
        then_updated > first_updated,
        "case {case:?}: expected StateUpdatedTimestamp to advance past the first verdict \
         ({first_updated} -> {then_updated}), got {alarm}"
    );
    let document = reason_data(&alarm, case);
    assert!(
        document.get("startDate").and_then(Value::as_str).is_some(),
        "case {case:?}: expected the decision document to open its span, got {document}"
    );

    let recorded = state_updates(&router, name).await;
    let latest = recorded.last().unwrap_or(&Value::Null);
    assert_eq!(
        latest.pointer("/oldState/stateValue").and_then(Value::as_str),
        Some("OK"),
        "case {case:?}: expected the recorded transition to come from OK, got {recorded:?}"
    );
    assert_eq!(
        numbers(&recorded_reason_data(latest, "newState"), "recentDatapoints"),
        vec![1000.0],
        "case {case:?}: expected the record to carry the graded value 1000, got {latest}"
    );
    assert_eq!(
        graded_instants(&document),
        vec![open_window_start],
        "case {case:?}: expected the verdict to name the period the datapoint was stamped in \
         ({open_window_start}), got {document}"
    );
}

/// The instants a decision document names for the datapoints its verdict rests on, as epoch
/// seconds.
fn graded_instants(document: &Value) -> Vec<f64> {
    document
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

/// How long the third test waits for a verdict the evaluation cadence owes.
///
/// The cadence is the store's scheduled pass, so a datapoint is graded within it plus the span
/// a transition the clock owes takes; this budget is a small multiple of that, and the test
/// asserts WHERE in the period the verdict landed rather than how long it waited.
const CADENCE_BUDGET: Duration = Duration::from_secs(15);

/// How often the third test polls for the verdict it is waiting for.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// How much of the period must still be ahead when the third test starts publishing: enough for
/// both of its verdicts to be reached while the period the datapoints land in is OPEN.
const OPEN_PERIOD_ROOM_SECS: f64 = 20.0;

/// Poll `DescribeAlarms` the way a client waiting for an alarm does, until the alarm reports
/// `state` or [`CADENCE_BUDGET`] runs out; answers the alarm and the instant it was read at.
async fn await_state(
    router: &MultiServiceRouter,
    name: &str,
    state: &str,
    case: &str,
) -> (Value, f64) {
    let deadline = now_secs() + CADENCE_BUDGET.as_secs_f64();
    let mut last = Value::Null;
    while now_secs() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        let at = now_secs();
        let Some(alarm) = described_alarm(router, AlarmKind::Metric, name).await else {
            continue;
        };
        if alarm.get("StateValue").and_then(Value::as_str) == Some(state) {
            return (alarm, at);
        }
        last = alarm;
    }
    panic!(
        "case {case:?}: expected the alarm {name} to report StateValue {state:?} within \
         {CADENCE_BUDGET:?}, last saw {last}"
    );
}

#[tokio::test]
async fn a_metrics_array_alarm_grades_the_open_period_within_the_evaluation_cadence() {
    let router = build_router();

    // The rows differ in the width of the period the datapoints land in — the recorded
    // one-minute window and a five-minute one — both wider than the store's evaluation
    // cadence, which is what makes the verdicts observable inside the open period.
    let rows: [(&str, i32); 2] = [
        ("a one-minute period", 60),
        ("a five-minute period", 300),
    ];
    for (case, period_secs) in rows {
        let period = f64::from(period_secs);
        let name = format!("open-period-math-alarm-{period_secs}");
        let series = format!("open-{period_secs}");
        let metric = "Load";
        // Enough of the period ahead that both verdicts land inside it.
        wait_for_period_room(period, OPEN_PERIOD_ROOM_SECS).await;
        let published_at = now_secs();
        let open_window_start = (published_at / period).floor() * period;
        let closes_at = open_window_start + period;
        let publish = |value: f64| {
            let router = &router;
            let series = series.clone();
            async move {
                ok(
                    router,
                    "PutMetricData",
                    &json!({
                        "Namespace": NAMESPACE,
                        "MetricData": [{
                            "MetricName": metric,
                            "Dimensions": [{"Name": "Case", "Value": series}],
                            "Value": value,
                            "Unit": "Count",
                            "Timestamp": now_secs(),
                        }],
                    }),
                )
                .await;
            }
        };

        // A datapoint in the period the clock is IN, and an alarm over the conditional that
        // grades it as a value under the threshold.
        publish(700.0).await;
        ok(
            &router,
            "PutMetricAlarm",
            &json!({
                "AlarmName": name,
                "Metrics": inverting_array(&series, metric, period_secs),
                "Threshold": 500.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "EvaluationPeriods": 1,
                "DatapointsToAlarm": 1,
                "TreatMissingData": "missing",
                "ActionsEnabled": false,
            }),
        )
        .await;

        // The first verdict is reached from the data of that still-open period, within the
        // evaluation cadence of the write that stored it.
        let (alarm, at) = await_state(&router, &name, "OK", case).await;
        assert!(
            at < closes_at,
            "case {case:?}: expected the first verdict while the period starting \
             {open_window_start} was still open (before {closes_at}), read it at {at}"
        );
        assert_verdict(
            case,
            "the first verdict",
            &alarm,
            &Verdict {
                state: "OK",
                recent: &[0.0],
                rests_on: &[0.0],
            },
        );
        let document = reason_data(&alarm, case);
        assert_eq!(
            graded_instants(&document),
            vec![open_window_start],
            "case {case:?}: expected the verdict to name the open period it graded \
             ({open_window_start}), got {document}"
        );
        let first_updated = alarm_stamp(&alarm, "StateUpdatedTimestamp")
            .unwrap_or_else(|| panic!("case {case:?}: no state stamp in {alarm}"));

        // A later datapoint of the SAME open period takes the conditional's output across the
        // threshold, and the alarm moves on it — still inside that period.
        publish(50.0).await;
        let (alarm, at) = await_state(&router, &name, "ALARM", case).await;
        assert!(
            at < closes_at,
            "case {case:?}: expected the transition while the period starting \
             {open_window_start} was still open (before {closes_at}), read it at {at}"
        );
        assert_verdict(
            case,
            "the verdict after the later datapoint",
            &alarm,
            &Verdict {
                state: "ALARM",
                recent: &[1000.0],
                rests_on: &[1000.0],
            },
        );
        let then_updated = alarm_stamp(&alarm, "StateUpdatedTimestamp")
            .unwrap_or_else(|| panic!("case {case:?}: no state stamp in {alarm}"));
        assert!(
            then_updated > first_updated,
            "case {case:?}: expected StateUpdatedTimestamp to advance past the first verdict \
             ({first_updated} -> {then_updated}), got {alarm}"
        );

        let recorded = state_updates(&router, &name).await;
        let latest = recorded.last().unwrap_or(&Value::Null);
        assert_eq!(
            latest.pointer("/oldState/stateValue").and_then(Value::as_str),
            Some("OK"),
            "case {case:?}: expected the recorded transition to come from OK, got {recorded:?}"
        );
        assert_eq!(
            numbers(&recorded_reason_data(latest, "newState"), "recentDatapoints"),
            vec![1000.0],
            "case {case:?}: expected the record to carry the graded value 1000, got {latest}"
        );
    }
}

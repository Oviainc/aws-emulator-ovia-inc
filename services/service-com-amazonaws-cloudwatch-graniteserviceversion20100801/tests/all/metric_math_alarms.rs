// SPDX-License-Identifier: CC0-1.0

//! The METRIC-MATH ALARM slice, end to end over the wire: an alarm configured with a `Metrics`
//! query array is evaluated against the series that array's returning query COMPUTES, not
//! against the metrics the array reads.
//!
//! One rule, asserted over a table whose rows differ along what it generalises over:
//!
//! * A metric alarm whose configuration is a `Metrics` query array ("Use the `Metrics`
//!   parameter to create an alarm based on a metric math expression",
//!   `smithy_query(query="doc",shape="PutMetricAlarmInput",member="Metrics")`) is evaluated
//!   like every other metric alarm — the same `ComparisonOperator` against the same
//!   `Threshold`, the same "M out of N" predicate, the same `StateReason` and the same
//!   `StateReasonData` — over the series the query with `ReturnData=true` resolves to. So the
//!   values it grades, cites and records are the EXPRESSION's output over the datapoints
//!   `PutMetricData` stored, which is also what `GetMetricData` answers the identical array
//!   with.
//!
//! Rows differ in the expression (a conditional, a product, a quotient, a chain of two
//! expressions, and no expression at all), the statistic the array folds its metric by, the
//! window width, the comparison operator, the threshold, the number of evaluation periods and
//! the state the verdict reaches. The recorded case the issue was filed about — `IF(m1 > 100,
//! 0, 1000)` over a `Minimum` of 700, graded against 500 — is one row of it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    alarm_history_items, alarm_stamp, build_router, call, described_alarm, error_code,
    error_message, graded_window_stamp, now_secs, ok, wait_for_evaluation, AlarmKind,
};

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/MathAlarms";

/// The metric every row of the table publishes into its own series of.
const METRIC: &str = "Load";

/// How close two computed values have to be to count as the same number.
const TOLERANCE: f64 = 1e-9;

/// One row of the table: a query array, the datapoint its metric holds, and the verdict the
/// alarm over it must reach.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The value published into this row's own series, one datapoint at the current instant.
    published: f64,
    /// The statistic the array's `MetricStat` query folds that series by.
    stat: &'static str,
    /// The width of the array's window, in seconds (`MetricStat$Period`).
    period: i32,
    /// The expression queries of the array, in the order the request lists them, each as
    /// `(Id, Expression)`. The LAST of them is the query that returns data — the one the alarm
    /// grades — and an empty list leaves the metric query itself returning the data.
    expressions: &'static [(&'static str, &'static str)],
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The alarm's `Threshold`.
    threshold: f64,
    /// The alarm's `EvaluationPeriods` (the N).
    evaluation_periods: i32,
    /// The alarm's `DatapointsToAlarm` (the M).
    datapoints_to_alarm: i32,
    /// The value the returning query takes over the published datapoint — what the alarm must
    /// grade, cite and record.
    graded: f64,
    /// The `StateValue` the verdict over that value reaches.
    state: &'static str,
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded case: the conditional INVERTS the input, so an input far above the
        // threshold is graded as a value far below it.
        case: "a conditional grading a high input as a low value",
        published: 700.0,
        stat: "Minimum",
        period: 60,
        expressions: &[("e1", "IF(m1 > 100, 0, 1000)")],
        comparison: "GreaterThanThreshold",
        threshold: 500.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        graded: 0.0,
        state: "OK",
    },
    Row {
        // The same array over a low input: the conditional grades it as the high value, which
        // breaches.
        case: "the same conditional grading a low input as a high value",
        published: 50.0,
        stat: "Minimum",
        period: 60,
        expressions: &[("e1", "IF(m1 > 100, 0, 1000)")],
        comparison: "GreaterThanThreshold",
        threshold: 500.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        graded: 1000.0,
        state: "ALARM",
    },
    Row {
        // Arithmetic rather than a conditional, and the sum rather than the minimum.
        case: "a quotient of the published sum",
        published: 300.0,
        stat: "Sum",
        period: 60,
        expressions: &[("e1", "m1 / 2")],
        comparison: "GreaterThanThreshold",
        threshold: 100.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        graded: 150.0,
        state: "ALARM",
    },
    Row {
        // A comparison in the other direction, over a product.
        case: "a product graded by a LessThanThreshold comparison",
        published: 2.0,
        stat: "Maximum",
        period: 60,
        expressions: &[("e1", "m1 * 3")],
        comparison: "LessThanThreshold",
        threshold: 10.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        graded: 6.0,
        state: "ALARM",
    },
    Row {
        // A CHAIN: the returning expression reads another expression, which reads the metric.
        case: "a chain of two expressions",
        published: 4.0,
        stat: "Average",
        period: 60,
        expressions: &[("e1", "m1 * 2"), ("e2", "e1 + 1")],
        comparison: "GreaterThanOrEqualToThreshold",
        threshold: 9.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        graded: 9.0,
        state: "ALARM",
    },
    Row {
        // No expression at all: a query array can grade the metric query itself, and then the
        // graded value is the statistic over the window.
        case: "a query array that grades its metric query itself",
        published: 7.0,
        stat: "Maximum",
        period: 60,
        expressions: &[],
        comparison: "GreaterThanThreshold",
        threshold: 10.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        graded: 7.0,
        state: "OK",
    },
    Row {
        // A five-minute window rather than a one-minute one.
        case: "a five-minute window",
        published: 101.0,
        stat: "Average",
        period: 300,
        expressions: &[("e1", "m1 - 1")],
        comparison: "GreaterThanThreshold",
        threshold: 100.0,
        evaluation_periods: 1,
        datapoints_to_alarm: 1,
        graded: 100.0,
        state: "OK",
    },
    Row {
        // "M out of N" over the expression's output: one breaching datapoint is all a 1-of-2
        // alarm needs.
        case: "one breaching expression datapoint of a 1-of-2 window",
        published: 700.0,
        stat: "Minimum",
        period: 60,
        expressions: &[("e1", "IF(m1 > 100, 1000, 0)")],
        comparison: "GreaterThanThreshold",
        threshold: 500.0,
        evaluation_periods: 2,
        datapoints_to_alarm: 1,
        graded: 1000.0,
        state: "ALARM",
    },
];

/// The dimension set the row at `index` publishes and watches: its own, so every row has its
/// own time series and the rows share one emulator.
fn dimensions(index: usize) -> Value {
    json!([{"Name": "Instance", "Value": format!("row-{index}")}])
}

/// The `Metrics` query array of one row: the metric query under the id `m1`, then the row's
/// expression queries, the last of which is the one that returns data.
fn metrics(row: &Row, index: usize) -> Value {
    let graded_is_the_metric = row.expressions.is_empty();
    let mut queries = vec![json!({
        "Id": "m1",
        "ReturnData": graded_is_the_metric,
        "MetricStat": {
            "Metric": {
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions(index),
            },
            "Period": row.period,
            "Stat": row.stat,
        },
    })];
    for (position, (id, expression)) in row.expressions.iter().enumerate() {
        queries.push(json!({
            "Id": id,
            "Expression": expression,
            "Label": format!("{id} of {}", row.case),
            "ReturnData": position + 1 == row.expressions.len(),
        }));
    }
    Value::Array(queries)
}

/// The `Id` of the query the row's alarm grades.
fn graded_id(row: &Row) -> &'static str {
    row.expressions.last().map_or("m1", |(id, _)| *id)
}

/// The value a reason or a document renders a measured quantity as: a decimal that always
/// carries a fraction, the form an alarm explanation writes one in.
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

/// What `GetMetricData` answers the row's OWN query array with, over the windows the alarm
/// graded: the values of the graded query, newest first. `at` is an instant inside the window
/// the row published into, so the read covers the window the alarm graded.
async fn read_back(router: &MultiServiceRouter, row: &Row, index: usize, at: f64) -> Vec<f64> {
    let period = f64::from(row.period);
    let window = (at / period).floor() * period;
    // Every query returns its data here: the read is of the same array the alarm is configured
    // with, and a reader asks for the value the alarm graded.
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
            "StartTime": window - 2.0 * period,
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
                .find(|result| result.get("Id").and_then(Value::as_str) == Some(graded_id(row)))
                .cloned()
        })
        .map(|result| numbers(&result, "Values"))
        .unwrap_or_default()
}

#[tokio::test]
async fn a_metrics_array_alarm_grades_the_series_its_returning_query_computes() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("math-alarm-{index}");
        // The datapoint is stamped inside the newest window the alarm's evaluation GRADES — the
        // newest one that has closed — because an evaluation grades the periods that are
        // complete rather than the one the clock is in.
        let stamp = graded_window_stamp(f64::from(row.period));
        ok(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "MetricData": [{
                    "MetricName": METRIC,
                    "Dimensions": dimensions(index),
                    "Value": row.published,
                    "Unit": "Count",
                    "Timestamp": stamp,
                }],
            }),
        )
        .await;
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

        // The state the verdict over the EXPRESSION's output reaches — not the one the input
        // metric's own value would reach.
        assert_eq!(
            alarm.get("StateValue").and_then(Value::as_str),
            Some(row.state),
            "case {case:?}: expected StateValue {:?} over the graded value {}, got {alarm}",
            row.state,
            row.graded
        );

        // The explanation names the datapoints it graded, the comparison it made and the
        // threshold it made it against.
        let reason = alarm
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            reason.starts_with("Threshold Crossed"),
            "case {case:?}: expected a graded StateReason, got {reason:?}"
        );
        assert!(
            reason.contains(&format!("{} (", decimal(row.graded))),
            "case {case:?}: expected the StateReason to cite the graded value {}, got {reason:?}",
            decimal(row.graded)
        );
        assert!(
            reason.contains(&format!("the threshold ({})", decimal(row.threshold))),
            "case {case:?}: expected the StateReason to name the threshold {}, got {reason:?}",
            decimal(row.threshold)
        );

        // The document reports the values the evaluation had, and the datapoints the verdict
        // rests on — both the expression's output, so a client parsing them can tell which
        // value was graded.
        let document = reason_data(&alarm, case);
        assert_eq!(
            numbers(&document, "recentDatapoints"),
            vec![row.graded],
            "case {case:?}: expected recentDatapoints to be the graded value, got {document}"
        );
        let evaluated = document
            .get("evaluatedDatapoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            evaluated
                .iter()
                .filter_map(|entry| entry.get("value").and_then(Value::as_f64))
                .collect::<Vec<f64>>(),
            vec![row.graded],
            "case {case:?}: expected the verdict to rest on the graded value, got {document}"
        );
        assert_eq!(
            document.get("threshold").and_then(Value::as_f64),
            Some(row.threshold),
            "case {case:?}: expected the document to name the threshold, got {document}"
        );
        assert_eq!(
            document.get("period").and_then(Value::as_f64),
            Some(f64::from(row.period)),
            "case {case:?}: expected the document to name the graded period, got {document}"
        );

        // The evaluation happened AFTER the write that configured the alarm, which is what
        // tells an evaluated alarm from one nothing has evaluated yet.
        let configured = alarm_stamp(&alarm, "AlarmConfigurationUpdatedTimestamp")
            .unwrap_or_else(|| panic!("case {case:?}: no configuration stamp in {alarm}"));
        let updated = alarm_stamp(&alarm, "StateUpdatedTimestamp")
            .unwrap_or_else(|| panic!("case {case:?}: no state stamp in {alarm}"));
        assert!(
            updated > configured,
            "case {case:?}: expected the state to be updated after the configuration \
             ({updated} > {configured}), got {alarm}"
        );

        // The transition is recorded, and the record carries the same graded values.
        let items = alarm_history_items(&router, AlarmKind::Metric, &name, "StateUpdate").await;
        let recorded = items
            .first()
            .and_then(|item| item.get("HistoryData").and_then(Value::as_str))
            .unwrap_or_else(|| panic!("case {case:?}: expected a StateUpdate item, got {items:?}"));
        let history: Value = serde_json::from_str(recorded)
            .unwrap_or_else(|_| panic!("case {case:?}: HistoryData must be JSON, got {recorded}"));
        assert_eq!(
            history
                .get("newState")
                .and_then(|state| state.get("stateValue"))
                .and_then(Value::as_str),
            Some(row.state),
            "case {case:?}: expected the recorded new state to be {:?}, got {history}",
            row.state
        );
        assert_eq!(
            history
                .get("newState")
                .and_then(|state| state.get("stateReasonData"))
                .map(|data| numbers(data, "recentDatapoints"))
                .unwrap_or_default(),
            vec![row.graded],
            "case {case:?}: expected the record to carry the graded value, got {history}"
        );

        // The same array read through `GetMetricData` computes the same value, so the alarm
        // cited what a reader of its own configuration sees.
        let read = read_back(&router, row, index, stamp).await;
        assert_eq!(
            read.len(),
            1,
            "case {case:?}: expected GetMetricData to answer the graded query with one value, \
             got {read:?}"
        );
        let answered = read.first().copied().unwrap_or_default();
        assert!(
            (answered - row.graded).abs() <= TOLERANCE,
            "case {case:?}: expected GetMetricData over the same array to answer {}, got \
             {answered}",
            row.graded
        );
    }
}

/// One `MetricStat` query under the id `id`, over the row-independent series `instance`, whose
/// own data is not returned.
fn retrieving_query(id: &str, instance: &str) -> Value {
    json!({
        "Id": id,
        "ReturnData": false,
        "MetricStat": {
            "Metric": {
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Instance", "Value": instance}],
            },
            "Period": 60,
            "Stat": "Sum",
        },
    })
}

/// The same query, with its data RETURNED — so the result it contributes carries the page's
/// `StatusCode`.
fn returning_query(id: &str, instance: &str) -> Value {
    let mut query = retrieving_query(id, instance);
    query["ReturnData"] = json!(true);
    query
}

/// The work ONE query array may ask for is bounded before any of it is done: an array carrying
/// more queries than one call may include is refused outright. A wide RANGE is not refused —
/// a range and a period that together span more datapoints than one page carries are answered
/// one slice of the range at a time — so the rows below are the bound on the array itself,
/// and the blocks after them are the two answers a legal request gets: paged when the range
/// asks for more than one page, whole when it does not.
#[tokio::test]
async fn an_array_asking_for_more_work_than_one_resolution_does_is_refused() {
    let router = build_router();
    let period = 60.0;
    let now = now_secs();
    let window = (now / period).floor() * period;
    // One real datapoint, so the rows that ARE answered are answered from stored data.
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Instance", "Value": "bounds"}],
                "Value": 5.0,
                "Unit": "Count",
                "Timestamp": now,
            }],
        }),
    )
    .await;

    /// A day, in the epoch seconds a request's range is named in.
    const DAY: f64 = 86_400.0;

    // (case, the queries, the range in seconds back from the current window, the words the
    // refusal must carry)
    let refused: Vec<(&str, Value, f64, &str)> = vec![(
        "more queries than one call may include",
        Value::Array(
            (0..501)
                .map(|index| retrieving_query(&format!("m{index}"), "bounds"))
                .collect(),
        ),
        3.0 * period,
        "one call can include up to 500",
    )];
    for (case, queries, span, says) in refused {
        let (status, body) = call(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": queries,
                "StartTime": window - span,
                "EndTime": window + period,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code == "InvalidParameterValueException"
                || code == "InvalidParameterCombinationException",
            "case {case:?}: expected a parameter refusal, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(says),
            "case {case:?}: expected the refusal to say {says:?}, got {message:?}"
        );
    }

    // A range that asks for more datapoints than one page carries is answered, not refused:
    // the page covers the slice of the range that fits, reports it as partial and hands back a
    // token. Rows differ in which factor makes the request exceed one page — the range alone,
    // and the product of a range and a query count that each stay inside it.
    let paged: Vec<(&str, Value, f64)> = vec![
        (
            "one query over a range of a thousand days at a one-minute period",
            json!([returning_query("m1", "bounds")]),
            1_000.0 * DAY,
        ),
        (
            // Neither factor crosses one page on its own: five queries, and a range of
            // 30,000 one-minute windows each. Their PRODUCT does.
            "five queries each asking for a third of the datapoints one page carries",
            Value::Array(
                (0..5)
                    .map(|index| returning_query(&format!("m{index}"), "bounds"))
                    .collect(),
            ),
            30_000.0 * period,
        ),
    ];
    for (case, queries, span) in paged {
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": queries,
                "StartTime": window - span,
                "EndTime": window + period,
            }),
        )
        .await;
        assert!(
            body.get("NextToken").and_then(Value::as_str).is_some(),
            "case {case:?}: a range wider than one page must hand back a token, got {body}"
        );
        let status = body
            .get("MetricDataResults")
            .and_then(Value::as_array)
            .and_then(|results| results.first().cloned())
            .and_then(|result| {
                result
                    .get("StatusCode")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            });
        assert_eq!(
            status.as_deref(),
            Some("PartialData"),
            "case {case:?}: every result of a cut page is partial, got {body}"
        );
    }

    // A range and a period that ask for no more than one page are answered whole, however
    // sparse the series behind them is: what decides the page is what the request ASKS FOR, and
    // this one asks for exactly the datapoints one page carries.
    let inside = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [{
                "Id": "m1",
                "ReturnData": true,
                "MetricStat": {
                    "Metric": {
                        "Namespace": NAMESPACE,
                        "MetricName": METRIC,
                        "Dimensions": [{"Name": "Instance", "Value": "bounds"}],
                    },
                    "Period": 3_600,
                    "Stat": "Sum",
                },
            }],
            "StartTime": window - 100_800.0 * 3_600.0,
            "EndTime": window,
        }),
    )
    .await;
    assert!(
        inside
            .get("MetricDataResults")
            .and_then(Value::as_array)
            .is_some_and(|results| results.len() == 1),
        "a request asking for exactly the datapoint limit must be answered, got {inside}"
    );
    assert!(
        inside.get("NextToken").and_then(Value::as_str).is_none(),
        "a request inside one page mints no token, got {inside}"
    );

    // A long but bounded CHAIN of expressions, each reading the one before it: the ordering
    // walk follows the references the array defines without recursing on this thread's stack,
    // so a deep array is answered rather than refused — and the value it answers with is the
    // whole chain applied to the stored datapoint.
    let chain = 100;
    let mut queries = vec![retrieving_query("m1", "bounds")];
    for step in 1..=chain {
        let reads = if step == 1 {
            "m1".to_string()
        } else {
            format!("e{}", step - 1)
        };
        queries.push(json!({
            "Id": format!("e{step}"),
            "Expression": format!("{reads} + 1"),
            "ReturnData": step == chain,
        }));
    }
    let body = ok(
        &router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": queries,
            "StartTime": window - 2.0 * period,
            "EndTime": window + period,
            "ScanBy": "TimestampDescending",
        }),
    )
    .await;
    let answered: Vec<f64> = body
        .get("MetricDataResults")
        .and_then(Value::as_array)
        .and_then(|results| results.first().cloned())
        .map(|result| numbers(&result, "Values"))
        .unwrap_or_default();
    assert_eq!(
        answered,
        vec![5.0 + f64::from(chain)],
        "case \"a chain of {chain} expressions\": expected the stored datapoint with the whole \
         chain applied, got {body}"
    );
}

#[tokio::test]
async fn a_newly_created_alarm_reports_the_un_evaluated_seed_reason() {
    let router = build_router();

    // Every kind of alarm this service writes seeds the same un-evaluated state, so the rule
    // is asserted over all three rather than over the one the issue cited.
    for kind in [AlarmKind::Metric, AlarmKind::Composite, AlarmKind::Log] {
        let name = format!("seeded-{}", kind.alarm_type());
        kind.create(&router, &name, Some(false)).await;
        let alarm = described_alarm(&router, kind, &name)
            .await
            .unwrap_or_else(|| panic!("case {kind:?}: DescribeAlarms reports no alarm {name}"));
        assert_eq!(
            alarm.get("StateReason").and_then(Value::as_str),
            Some("Unchecked: Initial alarm creation"),
            "case {kind:?}: expected the service's un-evaluated seed reason, got {alarm}"
        );
    }
}

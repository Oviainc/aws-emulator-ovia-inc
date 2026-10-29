// SPDX-License-Identifier: CC0-1.0

//! The LOW-SAMPLE policy of a PERCENTILE metric alarm, end to end over the wire.
//!
//! ONE rule, asserted over a table: an alarm whose statistic is a percentile moves its state
//! from a window only when that window holds at least ten observations per unit of the nearer
//! distribution tail the percentile cuts off — a hundred observations at the ninetieth
//! percentile (and at the tenth), a thousand at the ninety-ninth — and only while the alarm is
//! configured to withhold under-sampled periods: "If you specify `ignore`, the alarm state does
//! not change during periods with too few data points to be statistically significant. If you
//! specify `evaluate` or omit this parameter, the alarm is always evaluated and possibly
//! changes state no matter how many data points are available."
//! (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="EvaluateLowSampleCountPercentile")`).
//! The member is "used only for alarms based on percentiles", so an alarm folded by one of the
//! `Statistic` values is graded on whatever its window holds whatever it says, and so is a
//! percentile no floor is published for.
//!
//! The rows differ along what the rule generalises over: which percentile the alarm names, how
//! the window's sample count stands against that percentile's floor (one below it, exactly on
//! it, past it), which policy is in force (`ignore`, `evaluate`, omitted), which member carried
//! the statistic, and which direction the verdict goes in. Beside the state, each row that
//! moves pins the machine-readable explanation the move rests on — the statistic it folded and
//! the one datapoint it graded, with the observations that datapoint stood for — and each row
//! that is withheld pins that NOTHING moved: the creation state, the creation reason, no
//! explanation document and no state-update history item.

use serde_json::{json, Value};

use crate::harness::{
    alarm_history_items, build_router, graded_window_stamp, now_secs, ok, wait_for_evaluation,
    AlarmKind,
};

/// The namespace every row publishes into and watches.
const NAMESPACE: &str = "Custom/Percentiles";

/// The metric every row publishes into and watches.
const METRIC: &str = "Latency";

/// The dimension name a row's own series is told apart by: each row watches the series its own
/// alarm name labels, so the rows share one emulator without sharing a series.
const SERIES_DIMENSION: &str = "Series";

/// The `Period` every row is evaluated on, in seconds — one whole minute, the shortest regular
/// resolution a series is stored at.
const PERIOD_SECS: i64 = 60;

/// The threshold every row compares its folded percentile against.
const THRESHOLD: f64 = 10.0;

/// The unit every row publishes under and every alarm selects on, one value of the modelled
/// `StandardUnit` enum (`smithy_query(query="shape",shape="StandardUnit")`).
const UNIT: &str = "Milliseconds";

/// The value every observation of a BREACHING row takes: past [`THRESHOLD`], so every
/// percentile over the window folds to it and breaches.
const BREACHING_VALUE: f64 = 100.0;

/// The value every observation of a NON-breaching row takes: under [`THRESHOLD`].
const CLEAR_VALUE: f64 = 1.0;

/// The state `PutMetricAlarm` leaves a new metric alarm in: "the alarm state is immediately set
/// to INSUFFICIENT_DATA"
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="AlarmName")`).
const CREATED_STATE: &str = "INSUFFICIENT_DATA";

/// The reason a newly created alarm reports, which a withheld evaluation leaves untouched.
const CREATED_REASON: &str = "Unchecked: Initial alarm creation";

/// The history item type a state transition is recorded under, the one a withheld evaluation
/// must append none of.
const STATE_UPDATE: &str = "StateUpdate";

/// One row of the table.
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The member the row's statistic arrives on, and the name it spells there: a percentile on
    /// `ExtendedStatistic`, or one of the five on `Statistic`.
    statistic: (&'static str, &'static str),
    /// The `EvaluateLowSampleCountPercentile` the row configures, or `None` to omit the member.
    policy: Option<&'static str>,
    /// How many observations the row's one window holds.
    samples: f64,
    /// The value every one of those observations took.
    value: f64,
    /// The state the alarm must report once the evaluation is due: the state the comparison puts
    /// it in for a row the alarm grades, and the creation state for a row it withholds.
    expected_state: &'static str,
    /// Whether the evaluation moved the alarm at all. `false` is the withheld case: the state,
    /// the reason, the explanation document and the history are all left as creation left them.
    moves: bool,
}

/// A row whose statistic is the percentile `percentile`.
fn percentile(
    case: &'static str,
    percentile: &'static str,
    policy: Option<&'static str>,
    samples: f64,
    expected_state: &'static str,
) -> Row {
    Row {
        case,
        statistic: ("ExtendedStatistic", percentile),
        policy,
        samples,
        value: BREACHING_VALUE,
        expected_state,
        moves: expected_state != CREATED_STATE,
    }
}

impl Row {
    /// The same row with every observation under the threshold, so the verdict goes the other
    /// way.
    fn clear(mut self, expected_state: &'static str) -> Self {
        self.value = CLEAR_VALUE;
        self.expected_state = expected_state;
        self
    }
}

/// The alarm `name` this row configures: one period, one evaluation period, one breaching
/// datapoint needed, and the row's own statistic and low-sample policy.
fn alarm_request(name: &str, row: &Row) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": SERIES_DIMENSION, "Value": name}],
        "Period": PERIOD_SECS,
        "EvaluationPeriods": 1,
        "DatapointsToAlarm": 1,
        "Threshold": THRESHOLD,
        "ComparisonOperator": "GreaterThanThreshold",
        "TreatMissingData": "missing",
        "Unit": UNIT,
        "ActionsEnabled": false,
    });
    let (member, statistic) = row.statistic;
    request[member] = Value::from(statistic);
    if let Some(policy) = row.policy {
        request["EvaluateLowSampleCountPercentile"] = Value::from(policy);
    }
    request
}

/// Publish ONE statistic entry into the window `periods_back` before the newest window an
/// alarm evaluation grades: `samples` observations, every one of them `value`, which keeps the
/// individual values a percentile needs recoverable
/// (`smithy_query(query="doc",shape="MetricDatum",member="Values")`).
///
/// The newest window an evaluation grades is the newest one that has CLOSED
/// ([`graded_window_stamp`]), because an alarm is evaluated over complete periods.
async fn publish_window(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    value: f64,
    samples: f64,
    periods_back: f64,
) {
    #[allow(clippy::cast_precision_loss)]
    let period = PERIOD_SECS as f64;
    ok(
        router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": SERIES_DIMENSION, "Value": name}],
                "Values": [value],
                "Counts": [samples],
                "Unit": UNIT,
                "Timestamp": graded_window_stamp(period) - periods_back * period,
            }],
        }),
    )
    .await;
}

/// The row's window, in the newest period the evaluation grades.
async fn publish(router: &smithy_server_runtime::MultiServiceRouter, name: &str, row: &Row) {
    publish_window(router, name, row.value, row.samples, 0.0).await;
}

/// The `StateReasonData` of a described alarm, parsed.
fn reason_data(case: &str, alarm: &Value) -> Value {
    let text = alarm
        .get("StateReasonData")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a StateReasonData member, got {alarm}"));
    serde_json::from_str(text)
        .unwrap_or_else(|_| panic!("case {case:?}: StateReasonData must be JSON, got {text:?}"))
}

#[tokio::test]
async fn a_percentile_alarm_withholds_its_state_only_below_the_published_sample_floor() {
    // The floors the rule publishes for the percentiles below: ten over the weight of the nearer
    // tail the percentile cuts off — p50 -> 20, p90 and p10 -> 100, p95 -> 200, p99 -> 1000,
    // p99.9 -> 10000 — each probed on both sides of its own floor, plus the two ends of the
    // "between p0.0 and p100" range, whose nearer tail carries no weight and which therefore have
    // no floor (`smithy_query(query="doc",shape="MetricAlarm",member="ExtendedStatistic")`), the
    // two policies that evaluate regardless, and a basic statistic, for which the member is inert.
    let rows: Vec<Row> = vec![
        percentile(
            "p90 one observation under its hundred-sample floor, ignoring low samples",
            "p90",
            Some("ignore"),
            99.0,
            CREATED_STATE,
        ),
        percentile(
            "p90 exactly on its hundred-sample floor, ignoring low samples",
            "p90",
            Some("ignore"),
            100.0,
            "ALARM",
        ),
        percentile(
            "p90 past its hundred-sample floor, ignoring low samples",
            "p90",
            Some("ignore"),
            101.0,
            "ALARM",
        ),
        percentile(
            "p90 on its floor with every observation under the threshold",
            "p90",
            Some("ignore"),
            100.0,
            "ALARM",
        )
        .clear("OK"),
        percentile(
            "p90 under its floor but asked to evaluate anyway",
            "p90",
            Some("evaluate"),
            99.0,
            "ALARM",
        ),
        percentile(
            "p90 under its floor with the policy member omitted",
            "p90",
            None,
            99.0,
            "ALARM",
        ),
        percentile(
            "p95 one observation under its two-hundred-sample floor",
            "p95",
            Some("ignore"),
            199.0,
            CREATED_STATE,
        ),
        percentile(
            "p95 exactly on its two-hundred-sample floor",
            "p95",
            Some("ignore"),
            200.0,
            "ALARM",
        ),
        percentile(
            "p99 far under its thousand-sample floor",
            "p99",
            Some("ignore"),
            200.0,
            CREATED_STATE,
        ),
        percentile(
            "p99 exactly on its thousand-sample floor",
            "p99",
            Some("ignore"),
            1000.0,
            "ALARM",
        ),
        percentile(
            "p99 far under its floor but asked to evaluate anyway",
            "p99",
            Some("evaluate"),
            200.0,
            "ALARM",
        ),
        percentile(
            "p50 one observation under its twenty-sample floor",
            "p50",
            Some("ignore"),
            19.0,
            CREATED_STATE,
        ),
        percentile(
            "p50 exactly on its twenty-sample floor",
            "p50",
            Some("ignore"),
            20.0,
            "ALARM",
        ),
        percentile(
            "p99.9 under its ten-thousand-sample floor",
            "p99.9",
            Some("ignore"),
            1000.0,
            CREATED_STATE,
        ),
        percentile(
            "p10, whose nearer tail is the lower one, under its hundred-sample floor",
            "p10",
            Some("ignore"),
            99.0,
            CREATED_STATE,
        ),
        percentile(
            "p10 exactly on its hundred-sample floor",
            "p10",
            Some("ignore"),
            100.0,
            "ALARM",
        ),
        percentile(
            "the bottom of the percentile range, whose nearer tail carries no weight",
            "p0.0",
            Some("ignore"),
            1.0,
            "ALARM",
        ),
        percentile(
            "the top of the percentile range, whose nearer tail carries no weight",
            "p100",
            Some("ignore"),
            1.0,
            "ALARM",
        ),
        Row {
            case: "a basic statistic, for which the low-sample member is inert",
            statistic: ("Statistic", "Average"),
            policy: Some("ignore"),
            samples: 1.0,
            value: BREACHING_VALUE,
            expected_state: "ALARM",
            moves: true,
        },
    ];

    // Every row watches its own series, so all of them are configured on ONE emulator and the
    // evaluation delay is waited out once.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("low-sample-{index}");
        ok(&router, "PutMetricAlarm", &alarm_request(&name, row)).await;
        publish(&router, &name, row).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(row.expected_state),
            "case {case:?}: {} observations of {} under {:?} must leave the alarm in {}, got {}",
            row.samples,
            row.statistic.1,
            row.policy,
            row.expected_state,
            reported
        );

        let history = alarm_history_items(&router, AlarmKind::Metric, name, STATE_UPDATE).await;
        if !row.moves {
            // A withheld evaluation leaves everything the creation wrote: the state does not
            // change, so neither the reason nor the explanation nor the history moves with it.
            assert_eq!(
                reported.get("StateReason").and_then(Value::as_str),
                Some(CREATED_REASON),
                "case {case:?}: a withheld evaluation leaves the creation reason, got {reported}"
            );
            assert_eq!(
                reported.get("StateReasonData"),
                None,
                "case {case:?}: a withheld evaluation reports no explanation, got {reported}"
            );
            assert!(
                history.is_empty(),
                "case {case:?}: a withheld evaluation records no {STATE_UPDATE} item, got \
                 {history:?}"
            );
            continue;
        }

        assert_eq!(
            history.len(),
            1,
            "case {case:?}: a moved alarm records one {STATE_UPDATE} item, got {history:?}"
        );
        let document = reason_data(case, reported);
        assert_eq!(
            document.get("statistic").and_then(Value::as_str),
            Some(row.statistic.1),
            "case {case:?}: expected the document to name the statistic it folded, got {document}"
        );
        let evaluated = document
            .get("evaluatedDatapoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            evaluated.len(),
            1,
            "case {case:?}: one evaluation period rests on one datapoint, got {document}"
        );
        let entry = &evaluated[0];
        assert_eq!(
            entry.get("sampleCount").and_then(Value::as_f64),
            Some(row.samples),
            "case {case:?}: expected the observations the window held ({}), got {document}",
            row.samples
        );
        assert_eq!(
            entry.get("value").and_then(Value::as_f64),
            Some(row.value),
            "case {case:?}: expected the percentile the window folded to ({}), got {document}",
            row.value
        );
    }
}

/// One row of the multi-period table: what each of the two graded windows holds, which
/// missing-data arm is in force, and what the alarm must end up reporting.
struct MixedRow {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// Observations in the newest graded window, and in the one before it.
    samples: (f64, f64),
    /// The value every observation of both windows took.
    value: f64,
    /// The `TreatMissingData` arm in force.
    treat_missing: &'static str,
    /// The alarm's `DatapointsToAlarm` — the M of its "M out of N".
    datapoints_to_alarm: i64,
    /// The state the alarm must report once the evaluation is due.
    expected_state: &'static str,
}

#[tokio::test]
async fn a_withheld_period_is_not_stood_in_for_by_the_missing_data_arm() {
    // Two evaluation periods, so a row can hold ONE under-sampled window beside one the alarm
    // grades. A window the alarm withholds was published into, so no arm of the missing-data
    // treatment decided anything for it: the arms that SUBSTITUTE a datapoint
    // (`breaching`, `notBreaching`,
    // `smithy_query(query="doc",shape="MetricAlarm",member="TreatMissingData")`) must not fill
    // the slot it came from, which is what would otherwise move the state on the strength of
    // exactly the under-sampled period the alarm asked to ignore. The rows differ in the arm,
    // in which window is under-sampled, in the M of the predicate and in the direction the
    // well-sampled window points.
    let rows: Vec<MixedRow> = vec![
        MixedRow {
            case: "an under-sampled newest window beside a clear one, breaching arm",
            samples: (99.0, 100.0),
            value: CLEAR_VALUE,
            treat_missing: "breaching",
            datapoints_to_alarm: 1,
            expected_state: CREATED_STATE,
        },
        MixedRow {
            case: "an under-sampled newest window beside a breaching one, notBreaching arm",
            samples: (99.0, 100.0),
            value: BREACHING_VALUE,
            treat_missing: "notBreaching",
            datapoints_to_alarm: 2,
            expected_state: CREATED_STATE,
        },
        MixedRow {
            case: "both windows well sampled and clear, breaching arm",
            samples: (100.0, 100.0),
            value: CLEAR_VALUE,
            treat_missing: "breaching",
            datapoints_to_alarm: 1,
            expected_state: "OK",
        },
        MixedRow {
            case: "both windows well sampled and breaching, notBreaching arm",
            samples: (100.0, 100.0),
            value: BREACHING_VALUE,
            treat_missing: "notBreaching",
            datapoints_to_alarm: 2,
            expected_state: "ALARM",
        },
    ];

    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("mixed-{index}");
        ok(
            &router,
            "PutMetricAlarm",
            &json!({
                "AlarmName": name,
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": [{"Name": SERIES_DIMENSION, "Value": name}],
                "ExtendedStatistic": "p90",
                "EvaluateLowSampleCountPercentile": "ignore",
                "Period": PERIOD_SECS,
                "EvaluationPeriods": 2,
                "DatapointsToAlarm": row.datapoints_to_alarm,
                "Threshold": THRESHOLD,
                "ComparisonOperator": "GreaterThanThreshold",
                "TreatMissingData": row.treat_missing,
                "Unit": UNIT,
                "ActionsEnabled": false,
            }),
        )
        .await;
        publish_window(&router, &name, row.value, row.samples.0, 0.0).await;
        publish_window(&router, &name, row.value, row.samples.1, 1.0).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(row.expected_state),
            "case {case:?}: windows of {:?} observations under the {} arm must leave the alarm \
             in {}, got {}",
            row.samples,
            row.treat_missing,
            row.expected_state,
            reported
        );
        if row.expected_state == CREATED_STATE {
            assert!(
                alarm_history_items(&router, AlarmKind::Metric, name, STATE_UPDATE)
                    .await
                    .is_empty(),
                "case {case:?}: a withheld period records no {STATE_UPDATE} item"
            );
        }
    }
}

#[tokio::test]
async fn a_withheld_percentile_is_still_answered_on_the_query_path() {
    // The policy governs the ALARM, not the metric: "Used only for alarms based on percentiles"
    // — so the same percentile the alarm withholds is still computed for a reader of the series,
    // which is how a client tells a deliberately withheld alarm from a broken one.
    let router = build_router();
    let name = "withheld-but-readable";
    let row = percentile(
        "p99 far under its thousand-sample floor",
        "p99",
        Some("ignore"),
        200.0,
        CREATED_STATE,
    );
    ok(&router, "PutMetricAlarm", &alarm_request(name, &row)).await;
    publish(&router, name, &row).await;
    wait_for_evaluation();

    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some(CREATED_STATE),
        "the under-sampled alarm keeps the state its creation set, got {described}"
    );

    let now = now_secs();
    #[allow(clippy::cast_precision_loss)]
    let window = PERIOD_SECS as f64;
    let read = ok(
        &router,
        "GetMetricData",
        &json!({
            "StartTime": now - 3.0 * window,
            "EndTime": now + window,
            "MetricDataQueries": [
                {
                    "Id": "p99",
                    "MetricStat": {
                        "Metric": {
                            "Namespace": NAMESPACE,
                            "MetricName": METRIC,
                            "Dimensions": [{"Name": SERIES_DIMENSION, "Value": name}],
                        },
                        "Period": PERIOD_SECS,
                        "Stat": "p99",
                        "Unit": UNIT,
                    },
                },
                {
                    "Id": "samples",
                    "MetricStat": {
                        "Metric": {
                            "Namespace": NAMESPACE,
                            "MetricName": METRIC,
                            "Dimensions": [{"Name": SERIES_DIMENSION, "Value": name}],
                        },
                        "Period": PERIOD_SECS,
                        "Stat": "SampleCount",
                        "Unit": UNIT,
                    },
                },
            ],
        }),
    )
    .await;
    let results = read["MetricDataResults"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    // (the query's id, the value its series must carry)
    let expected = [("p99", BREACHING_VALUE), ("samples", row.samples)];
    for (id, value) in expected {
        let result = results
            .iter()
            .find(|result| result.get("Id").and_then(Value::as_str) == Some(id))
            .unwrap_or_else(|| panic!("case {id:?}: expected a result for the query, got {read}"));
        assert_eq!(
            result.get("StatusCode").and_then(Value::as_str),
            Some("Complete"),
            "case {id:?}: expected a complete read, got {result}"
        );
        let values: Vec<f64> = result["Values"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        assert_eq!(
            values,
            vec![value],
            "case {id:?}: expected the series to carry {value}, got {result}"
        );
    }
}

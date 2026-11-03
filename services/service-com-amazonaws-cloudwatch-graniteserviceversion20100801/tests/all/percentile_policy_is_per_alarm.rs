// SPDX-License-Identifier: CC0-1.0

//! Whose property the low-sample policy of a PERCENTILE alarm is, end to end over the wire.
//!
//! ONE rule, asserted over a table: whether an under-sampled period moves a percentile alarm
//! is decided by THAT alarm's own `EvaluateLowSampleCountPercentile` and its own percentile's
//! significance floor — ten observations per unit of the nearer distribution tail the
//! percentile cuts off — and never by the series, so several alarms watching ONE series each
//! follow their own setting, and the series itself stays fully readable on the query path.
//!
//! "Used only for alarms based on percentiles. If you specify `ignore`, the alarm state does
//! not change during periods with too few data points to be statistically significant. If you
//! specify `evaluate` or omit this parameter, the alarm is always evaluated and possibly
//! changes state no matter how many data points are available."
//! (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="EvaluateLowSampleCountPercentile")`).
//!
//! The rows differ along what the rule generalises over: which SERIES an alarm watches (and so
//! how many observations each of its periods holds, below / exactly on / past the floor), which
//! percentile the alarm names, which policy is in force (`ignore`, `evaluate`, omitted), which
//! member carried the statistic, and how many evaluation periods the verdict covers. Several
//! rows share one series and disagree only in their own configuration, which is the case the
//! per-series reading cannot tell apart from the per-alarm one.
//!
//! Each row that moves pins the machine-readable explanation its move rests on — the statistic
//! it folded, and the datapoint it graded with the observations THAT series published per
//! period; each row that is withheld pins that nothing moved at all: the creation state, the
//! creation reason, no explanation document and no state-update history item.

use serde_json::{json, Value};

use crate::harness::{
    alarm_history_items, build_router, graded_window_stamp, ok, wait_for_evaluation, AlarmKind,
};

/// The namespace every series of this suite is published into.
const NAMESPACE: &str = "Custom/PercentilePolicy";

/// The metric every series of this suite is published into. The series are told apart by the
/// one dimension below, so an alarm selects a series by naming it.
const METRIC: &str = "Latency";

/// The dimension name a series is told apart by.
const SERIES_DIMENSION: &str = "Series";

/// The `Period` every alarm is evaluated on, in seconds.
const PERIOD_SECS: i64 = 60;

/// The threshold every alarm compares its folded percentile against.
const THRESHOLD: f64 = 10.0;

/// The unit every observation carries and every alarm selects on, one value of the modelled
/// `StandardUnit` enum (`smithy_query(query="shape",shape="StandardUnit")`).
const UNIT: &str = "Milliseconds";

/// The value every observation takes: past [`THRESHOLD`], so every percentile over a window
/// folds to it and breaches.
const BREACHING_VALUE: f64 = 100.0;

/// How many periods of the series each row publishes, newest graded window first: a series
/// republished every period, the way a client feeding a percentile alarm publishes one.
const PUBLISHED_PERIODS: usize = 2;

/// The state `PutMetricAlarm` leaves a new metric alarm in: "the alarm state is immediately set
/// to INSUFFICIENT_DATA"
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="AlarmName")`).
const CREATED_STATE: &str = "INSUFFICIENT_DATA";

/// The reason a newly created alarm reports, which a withheld evaluation leaves untouched.
const CREATED_REASON: &str = "Unchecked: Initial alarm creation";

/// The history item type a state transition is recorded under, the one a withheld evaluation
/// must append none of.
const STATE_UPDATE: &str = "StateUpdate";

/// One series every alarm that names it watches: its dimension value and how many observations
/// each of its periods holds.
#[derive(Clone, Copy)]
struct Series {
    /// The dimension value that tells this series apart.
    label: &'static str,
    /// Observations published into EACH period of it.
    samples: f64,
}

/// Past the hundred-sample floor of p90 and far under the thousand-sample floor of p99, so the
/// two percentiles disagree over one series.
const BUSY: Series = Series {
    label: "busy",
    samples: 200.0,
};

/// Exactly ON the hundred-sample floor of p90.
const AT_P90_FLOOR: Series = Series {
    label: "at-p90-floor",
    samples: 100.0,
};

/// One observation UNDER the hundred-sample floor of p90.
const UNDER_P90_FLOOR: Series = Series {
    label: "under-p90-floor",
    samples: 99.0,
};

/// One row of the table.
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The series it watches.
    series: Series,
    /// The member its statistic arrives on, and the name it spells there.
    statistic: (&'static str, &'static str),
    /// The `EvaluateLowSampleCountPercentile` it configures, or `None` to omit the member.
    policy: Option<&'static str>,
    /// How many periods its verdict covers — its `EvaluationPeriods`.
    evaluation_periods: i64,
    /// The state it must report once the evaluation is due: `ALARM` for a row the alarm grades,
    /// and the creation state for one it withholds.
    expected_state: &'static str,
}

impl Row {
    /// Whether the evaluation moved this alarm at all.
    fn moves(&self) -> bool {
        self.expected_state != CREATED_STATE
    }
}

/// A row whose statistic is the percentile `percentile`.
fn percentile(
    case: &'static str,
    series: Series,
    percentile: &'static str,
    policy: Option<&'static str>,
    expected_state: &'static str,
) -> Row {
    Row {
        case,
        series,
        statistic: ("ExtendedStatistic", percentile),
        policy,
        evaluation_periods: 1,
        expected_state,
    }
}

/// The alarm `name` this row configures over its own series.
fn alarm_request(name: &str, row: &Row) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": SERIES_DIMENSION, "Value": row.series.label}],
        "Period": PERIOD_SECS,
        "EvaluationPeriods": row.evaluation_periods,
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

/// Publish [`PUBLISHED_PERIODS`] periods of `series`, newest graded window first.
///
/// Each entry carries `Values`/`Counts`, which is what keeps the individual values a percentile
/// needs recoverable (`smithy_query(query="doc",shape="MetricDatum",member="Values")`).
async fn publish(router: &smithy_server_runtime::MultiServiceRouter, series: Series) {
    #[allow(clippy::cast_precision_loss)]
    let period = PERIOD_SECS as f64;
    for periods_back in 0..PUBLISHED_PERIODS {
        #[allow(clippy::cast_precision_loss)]
        let back = periods_back as f64;
        ok(
            router,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "MetricData": [{
                    "MetricName": METRIC,
                    "Dimensions": [{"Name": SERIES_DIMENSION, "Value": series.label}],
                    "Values": [BREACHING_VALUE],
                    "Counts": [series.samples],
                    "Unit": UNIT,
                    "Timestamp": graded_window_stamp(period) - back * period,
                }],
            }),
        )
        .await;
    }
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
async fn the_low_sample_policy_is_read_off_each_alarm_rather_than_off_the_series() {
    // Three series, each republished every period, and eleven alarms over them: the rows that
    // share a series differ only in the percentile they name and the policy they carry, so a
    // reading that took the policy from the series — or that never graded a percentile at all —
    // cannot produce this table.
    let rows: Vec<Row> = vec![
        percentile(
            "p90 past its hundred-sample floor, ignoring low samples",
            BUSY,
            "p90",
            Some("ignore"),
            "ALARM",
        ),
        percentile(
            "p90 past its floor, evaluating low samples, over the same series",
            BUSY,
            "p90",
            Some("evaluate"),
            "ALARM",
        ),
        percentile(
            "p99 under its thousand-sample floor on the same series, ignoring low samples",
            BUSY,
            "p99",
            Some("ignore"),
            CREATED_STATE,
        ),
        percentile(
            "p99 under its floor on the same series, asked to evaluate anyway",
            BUSY,
            "p99",
            Some("evaluate"),
            "ALARM",
        ),
        percentile(
            "p99 under its floor on the same series, with the policy member omitted",
            BUSY,
            "p99",
            None,
            "ALARM",
        ),
        percentile(
            "p99.9 under its ten-thousand-sample floor on the same series",
            BUSY,
            "p99.9",
            Some("ignore"),
            CREATED_STATE,
        ),
        Row {
            case: "p90 past its floor over two evaluation periods of the same series",
            series: BUSY,
            statistic: ("ExtendedStatistic", "p90"),
            policy: Some("ignore"),
            evaluation_periods: 2,
            expected_state: "ALARM",
        },
        percentile(
            "p90 exactly on its hundred-sample floor",
            AT_P90_FLOOR,
            "p90",
            Some("ignore"),
            "ALARM",
        ),
        percentile(
            "p99 on p90's floor, which is under its own",
            AT_P90_FLOOR,
            "p99",
            Some("ignore"),
            CREATED_STATE,
        ),
        percentile(
            "p90 one observation under its hundred-sample floor",
            UNDER_P90_FLOOR,
            "p90",
            Some("ignore"),
            CREATED_STATE,
        ),
        percentile(
            "p90 under its floor on the same series, asked to evaluate anyway",
            UNDER_P90_FLOOR,
            "p90",
            Some("evaluate"),
            "ALARM",
        ),
        Row {
            case: "a basic statistic over the under-sampled series, for which the member is inert",
            series: UNDER_P90_FLOOR,
            statistic: ("Statistic", "Maximum"),
            policy: Some("ignore"),
            evaluation_periods: 1,
            expected_state: "ALARM",
        },
    ];

    // Every series is published once and every alarm is configured on ONE emulator, so the
    // evaluation delay is waited out once and the rows that share a series really do share it.
    let router = build_router();
    for series in [BUSY, AT_P90_FLOOR, UNDER_P90_FLOOR] {
        publish(&router, series).await;
    }
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("policy-{index}");
        ok(&router, "PutMetricAlarm", &alarm_request(&name, row)).await;
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
            "case {case:?}: {} of {} over a series of {} observations per period must leave the \
             alarm in {}, got {}",
            row.statistic.1,
            row.series.label,
            row.series.samples,
            row.expected_state,
            reported
        );

        let history = alarm_history_items(&router, AlarmKind::Metric, name, STATE_UPDATE).await;
        if !row.moves() {
            // Nothing moved: the state, the reason, the explanation and the history are all
            // left as the creation wrote them.
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
            "case {case:?}: expected the document to name the statistic this alarm folded, got \
             {document}"
        );
        let evaluated = document
            .get("evaluatedDatapoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            evaluated.len(),
            1,
            "case {case:?}: one breaching datapoint is what this verdict needed, got {document}"
        );
        let entry = &evaluated[0];
        assert_eq!(
            entry.get("sampleCount").and_then(Value::as_f64),
            Some(row.series.samples),
            "case {case:?}: expected the observations this series holds per period ({}), got \
             {document}",
            row.series.samples
        );
        assert_eq!(
            entry.get("value").and_then(Value::as_f64),
            Some(BREACHING_VALUE),
            "case {case:?}: expected the percentile the window folded to ({BREACHING_VALUE}), \
             got {document}"
        );
    }
}

#[tokio::test]
async fn every_series_an_alarm_withholds_is_still_answered_on_the_query_path() {
    // The policy governs an ALARM, not a metric: the same percentile a p99 alarm withholds over
    // these series is still computed for a reader of them, beside the per-period observation
    // count it was withheld for — which is how a client tells a deliberately withheld alarm
    // from a broken one.
    let router = build_router();
    let series = [BUSY, AT_P90_FLOOR, UNDER_P90_FLOOR];
    for one in series {
        publish(&router, one).await;
    }

    #[allow(clippy::cast_precision_loss)]
    let period = PERIOD_SECS as f64;
    let newest = graded_window_stamp(period);
    #[allow(clippy::cast_precision_loss)]
    let oldest = newest - (PUBLISHED_PERIODS as f64) * period;
    // The statistics this read asks for: the percentile the p99 alarms withheld, and the
    // per-period observation count they withheld it for.
    for one in series {
        for statistic in ["p99", "SampleCount"] {
            // What that statistic takes over every published period of this series.
            let expected = match statistic {
                "SampleCount" => one.samples,
                _ => BREACHING_VALUE,
            };
            let case = format!("{} as {statistic}", one.label);
            let read = ok(
                &router,
                "GetMetricData",
                &json!({
                    "StartTime": oldest,
                    "EndTime": newest + period,
                    "MetricDataQueries": [{
                        "Id": "read",
                        "MetricStat": {
                            "Metric": {
                                "Namespace": NAMESPACE,
                                "MetricName": METRIC,
                                "Dimensions": [
                                    {"Name": SERIES_DIMENSION, "Value": one.label},
                                ],
                            },
                            "Period": PERIOD_SECS,
                            "Stat": statistic,
                            "Unit": UNIT,
                        },
                    }],
                }),
            )
            .await;
            let result = &read["MetricDataResults"][0];
            assert_eq!(
                result.get("StatusCode").and_then(Value::as_str),
                Some("Complete"),
                "case {case:?}: expected a complete read, got {read}"
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
                vec![expected; PUBLISHED_PERIODS],
                "case {case:?}: expected every published period to carry {expected}, got \
                 {result}"
            );
        }
    }
}

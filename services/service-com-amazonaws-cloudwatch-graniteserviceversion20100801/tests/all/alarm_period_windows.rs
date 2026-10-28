// SPDX-License-Identifier: CC0-1.0

//! The PERIODS a metric alarm's evaluation grades, and the sentence it explains the graded
//! state with, end to end over the wire.
//!
//! ONE rule, asserted over a table: **a metric alarm's evaluation aggregates the series into
//! periods ANCHORED at multiples of its `Period` — "CloudWatch aggregates all data points with
//! time stamps that fall within each one-hour period"
//! (`smithy_query(query="doc",shape="GetMetricStatistics")`) — so which period a datapoint is
//! graded in is decided by the datapoint's own time stamp and not by the instant the alarm is
//! read, and the state it computes is explained in the sentence CloudWatch publishes for it:
//! `Threshold Crossed: <n> out of the last <m> datapoints [<value> (<instant>), …] was|were
//! [not] <comparison> the threshold (<threshold>) (minimum <k> datapoint(s) for <from> -> <to>
//! transition).`**
//!
//! Two datapoints seconds apart are therefore graded as the SEPARATE periods their time stamps
//! fall in rather than folded into one window that happens to hold both — which is what lets a
//! clearing datapoint published just after a period boundary decide the alarm's state — and
//! every instant the reason names is the START of a period.
//!
//! The rows differ along what the rule generalises over: the length of the period (10, 20, 30
//! and 60 seconds, which span the sub-minute cadences and the regular one), the statistic that
//! aggregates a period, the comparison operator and whether it held, how the datapoints are
//! distributed across the period boundary, how many evaluation periods the alarm covers and how
//! many of them breach. The recorded case the issue was filed about — a `Maximum` over
//! 10-second high-resolution periods driven past and then back under its threshold — is the
//! first two rows.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, graded_window_stamp, ok, wait_for_evaluation};

/// The customer namespace every row of the table watches.
const NAMESPACE: &str = "Custom/PeriodWindows";

/// The metric every row watches.
const METRIC: &str = "Latency";

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
    Placed {
        value,
        periods_back,
    }
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
    /// The alarm's `DatapointsToAlarm`, when the row names one.
    datapoints_to_alarm: Option<i32>,
    /// The datapoints the row publishes, each placed in a period of its own choosing.
    data: &'static [Placed],
    /// The state the evaluation must put the alarm in.
    state: &'static str,
    /// The whole `StateReason` the evaluation must report, with `{instants}` standing for the
    /// bracketed datapoint list the row cannot know the instants of.
    reason: &'static str,
    /// The values the reason must name as graded, newest first.
    graded: &'static [&'static str],
}

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
    });
    if let Some(count) = row.datapoints_to_alarm {
        alarm["DatapointsToAlarm"] = json!(count);
    }
    alarm
}

/// Publish a row's datapoints, each stamped INSIDE the period it names.
///
/// The time stamps are placed on the period grid, in the MIDDLE of the period they name, counting
/// back from the newest period an alarm's evaluation grades — the newest one that has CLOSED
/// ([`graded_window_stamp`]) — so the period a datapoint belongs to is the one the row asked for
/// whatever phase of a period the publish falls in, and the row's newest datapoint is one the
/// evaluation grades rather than one still being published into.
///
/// A period boundary crossed between the publish and the reading below shifts every one of a
/// row's periods back together, and the evaluation grades the most recent periods that HOLD a
/// datapoint, so neither the state, the counts nor the instants the reason names change under
/// such a shift; a row's oldest datapoint would have to shift past the whole evaluation range,
/// which is periods away, for anything to.
async fn publish_placed(router: &MultiServiceRouter, name: &str, row: &Row) {
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
                // resolution ("Data points with a period of 10 seconds are available only for
                // custom metrics defined with a StorageResolution of 1"), because regular
                // resolution stores a datapoint at 1-minute granularity.
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

/// The bracketed datapoint list of a reason, and the sentence with that list replaced by
/// `{instants}` — so a row can state the whole sentence without knowing the instants.
fn split_reason(reason: &str) -> Option<(String, String)> {
    let open = reason.find('[')?;
    let close = reason[open..].find(']')? + open;
    let listed = reason[open + 1..close].to_string();
    let shape = format!("{}{{instants}}{}", &reason[..open], &reason[close + 1..]);
    Some((shape, listed))
}

/// Whether `instant` is the START of a `period`-second period: the form an alarm explanation
/// writes an instant in (`dd/mm/yy HH:MM:SS`), whose time of day is a whole number of periods
/// since midnight.
///
/// Every period an alarm may be evaluated on divides a day, so a period that starts at a
/// multiple of the period since the epoch also starts at a multiple of it since midnight.
fn is_period_start(instant: &str, period: i64) -> bool {
    let Some((_, time)) = instant.split_once(' ') else {
        return false;
    };
    let fields: Vec<i64> = time
        .split(':')
        .map(|field| field.parse().unwrap_or(-1))
        .collect();
    let [hours, minutes, seconds] = fields[..] else {
        return false;
    };
    if hours < 0 || minutes < 0 || seconds < 0 {
        return false;
    }
    (hours * 3_600 + minutes * 60 + seconds) % period == 0
}

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded case: a high-resolution `Maximum` over 10-second periods, cleared by a
        // datapoint published after the boundary. A window that slid with the read instant
        // would still hold the breaching datapoint and grade the alarm ALARM.
        case: "a clearing datapoint in the period after a breaching one",
        period: 10,
        statistic: "Maximum",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        data: &[placed(211.0, 1), placed(1.0, 0)],
        state: "OK",
        reason: "Threshold Crossed: 1 out of the last 1 datapoints {instants} was not greater \
                 than the threshold (50.0) (minimum 1 datapoint for ALARM -> OK \
                 transition).",
        graded: &["1.0"],
    },
    Row {
        // The same two values inside ONE period: there the statistic really does aggregate
        // both, and the `Maximum` of them breaches.
        case: "a clearing datapoint in the same period as a breaching one",
        period: 10,
        statistic: "Maximum",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        data: &[placed(211.0, 0), placed(1.0, 0)],
        state: "ALARM",
        reason: "Threshold Crossed: 1 out of the last 1 datapoints {instants} was greater than \
                 the threshold (50.0) (minimum 1 datapoint for OK -> ALARM \
                 transition).",
        graded: &["211.0"],
    },
    Row {
        case: "a Minimum over regular periods that no longer crosses",
        period: 60,
        statistic: "Minimum",
        comparison: "LessThanThreshold",
        threshold: 10.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        data: &[placed(1.0, 1), placed(50.0, 0)],
        state: "OK",
        reason: "Threshold Crossed: 1 out of the last 1 datapoints {instants} was not less than \
                 the threshold (10.0) (minimum 1 datapoint for ALARM -> OK \
                 transition).",
        graded: &["50.0"],
    },
    Row {
        // The Average of the current period alone crosses; the Average of both periods folded
        // together would not.
        case: "an Average of the current period only",
        period: 20,
        statistic: "Average",
        comparison: "GreaterThanOrEqualToThreshold",
        threshold: 100.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        data: &[placed(10.0, 1), placed(150.0, 0)],
        state: "ALARM",
        reason: "Threshold Crossed: 1 out of the last 1 datapoints {instants} was greater than \
                 or equal to the threshold (100.0) (minimum 1 datapoint for OK \
                 -> ALARM transition).",
        graded: &["150.0"],
    },
    Row {
        case: "a Maximum under a LessThanOrEqualToThreshold threshold",
        period: 30,
        statistic: "Maximum",
        comparison: "LessThanOrEqualToThreshold",
        threshold: 5.0,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        data: &[placed(100.0, 1), placed(1.0, 0)],
        state: "ALARM",
        reason: "Threshold Crossed: 1 out of the last 1 datapoints {instants} was less than or \
                 equal to the threshold (5.0) (minimum 1 datapoint for OK -> \
                 ALARM transition).",
        graded: &["1.0"],
    },
    Row {
        // Two periods, one of them breaching, and one breaching datapoint is enough.
        case: "one of two periods breaches and one datapoint is enough",
        period: 10,
        statistic: "Maximum",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 2,
        datapoints_to_alarm: Some(1),
        data: &[placed(211.0, 1), placed(1.0, 0)],
        state: "ALARM",
        reason: "Threshold Crossed: 1 out of the last 2 datapoints {instants} was greater than \
                 the threshold (50.0) (minimum 1 datapoint for OK -> ALARM \
                 transition).",
        graded: &["1.0", "211.0"],
    },
    Row {
        // Both periods breach, so the counts and the verbs are plural and the minimum is the
        // alarm's whole evaluation period count.
        case: "both of two periods breach",
        period: 10,
        statistic: "Maximum",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 2,
        datapoints_to_alarm: None,
        data: &[placed(211.0, 1), placed(212.0, 0)],
        state: "ALARM",
        reason: "Threshold Crossed: 2 out of the last 2 datapoints {instants} were greater than \
                 the threshold (50.0) (minimum 2 datapoints for OK -> ALARM \
                 transition).",
        graded: &["212.0", "211.0"],
    },
    Row {
        // Neither of two periods breaches: the sentence states the comparison negated and
        // counts the datapoints that read that way. The minimum it names is the OK direction's
        // — `EvaluationPeriods - DatapointsToAlarm + 1`, one clean datapoint here — not the two
        // breaching ones the ALARM direction would have needed.
        case: "neither of two periods breaches",
        period: 10,
        statistic: "Maximum",
        comparison: "GreaterThanThreshold",
        threshold: 50.0,
        evaluation_periods: 2,
        datapoints_to_alarm: None,
        data: &[placed(3.0, 1), placed(4.0, 0)],
        state: "OK",
        reason: "Threshold Crossed: 2 out of the last 2 datapoints {instants} were not greater \
                 than the threshold (50.0) (minimum 1 datapoint for ALARM -> OK \
                 transition).",
        graded: &["4.0", "3.0"],
    },
];

#[tokio::test]
async fn an_alarm_grades_the_periods_its_datapoints_fall_in_and_says_so() {
    // Every row watches its own series, so all of them are configured on ONE emulator and the
    // evaluation delay is waited out once rather than per row.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("period-row-{index}");
        ok(&router, "PutMetricAlarm", &alarm_of(row, &name)).await;
        publish_placed(&router, &name, row).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in ROWS.iter().zip(&names) {
        let case = row.case;
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(row.state),
            "case {case:?}: expected StateValue {}, got {reported}",
            row.state
        );
        let reason = reported
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (shape, listed) = split_reason(&reason).unwrap_or_else(|| {
            panic!("case {case:?}: no bracketed datapoint list in {reason:?}")
        });
        assert_eq!(
            shape, row.reason,
            "case {case:?}: expected the StateReason {:?}, got {shape:?}",
            row.reason
        );

        // The datapoints the reason names, newest first, each with the START of the period it
        // was aggregated out of.
        let entries: Vec<&str> = listed.split(", ").collect();
        assert_eq!(
            entries.len(),
            row.graded.len(),
            "case {case:?}: expected the reason to name {} datapoints, got {entries:?}",
            row.graded.len()
        );
        for (entry, expected) in entries.iter().zip(row.graded) {
            let (value, instant) = entry
                .split_once(" (")
                .unwrap_or_else(|| panic!("case {case:?}: {entry:?} names no instant"));
            assert_eq!(
                value, *expected,
                "case {case:?}: expected the reason to grade {expected:?}, got {entry:?}"
            );
            let instant = instant
                .strip_suffix(')')
                .unwrap_or_else(|| panic!("case {case:?}: {entry:?} is not a closed instant"));
            assert!(
                is_period_start(instant, i64::from(row.period)),
                "case {case:?}: {instant:?} must be the start of a {}-second period, in \
                 {reason:?}",
                row.period
            );
        }
    }
}

#[tokio::test]
async fn reading_an_alarm_again_grades_the_same_periods() {
    // An evaluation anchored at multiples of the period is a function of the stored datapoints,
    // so re-reading the alarm inside the period it was graded in reports the same state, the
    // same reason and the same instants — nothing moves because a reader called again.
    let router = build_router();
    let row = &ROWS[0];
    let name = "period-stability";
    ok(&router, "PutMetricAlarm", &alarm_of(row, name)).await;
    publish_placed(&router, name, row).await;
    wait_for_evaluation();

    let first = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    wait_for_evaluation();
    let second = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    for member in ["StateValue", "StateReason", "StateUpdatedTimestamp"] {
        assert_eq!(
            second["MetricAlarms"][0].get(member),
            first["MetricAlarms"][0].get(member),
            "member {member:?}: reading the alarm again must report the same evaluation, got \
             {second} after {first}"
        );
    }
}

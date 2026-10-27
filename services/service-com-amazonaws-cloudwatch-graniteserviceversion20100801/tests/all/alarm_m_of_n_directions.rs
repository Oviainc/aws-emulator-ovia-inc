// SPDX-License-Identifier: CC0-1.0

//! The TWO DIRECTIONS of a metric alarm's "M out of N" predicate, and the decision record each
//! one reports, end to end over the wire.
//!
//! ONE rule, asserted over a table: **an alarm's predicate has a minimum per direction, both
//! built from its own configuration — `DatapointsToAlarm` breaching datapoints ("the number of
//! data points that must be breaching to trigger the alarm […] this value is the M",
//! `smithy_query(query="doc",shape="PutMetricAlarmInput",member="DatapointsToAlarm")`) put the
//! alarm in `ALARM`, and `EvaluationPeriods - DatapointsToAlarm + 1` non-breaching ones (the
//! count that makes M breaching datapoints unreachable inside the same window, where
//! `EvaluationPeriods` "is the N") put it in `OK`; a window the datapoints present reach neither
//! minimum over decides nothing, so the alarm reports `INSUFFICIENT_DATA`. The `StateReason`
//! counts the datapoints that satisfied the direction being reported and closes with that
//! direction's minimum, and `StateReasonData` reports the window the evaluation graded: one
//! datapoint per evaluated period, each with the instant of the `Period`-second bucket it was
//! aggregated out of.**
//!
//! The rows differ along what the rule generalises over: the length of the period (10, 20, 30
//! and 60 seconds — the high-resolution cadences and the regular one), N, M, which direction the
//! window satisfied, how many of the evaluated periods carry a real datapoint and the
//! `TreatMissingData` arm in force. The recorded cases the issue was filed about are the
//! high-resolution 3-of-3 rows (a 10-second window three buckets wide) and the 3-of-5 row that
//! holds `OK` one datapoint short of M.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, now_secs, ok, wait_for_evaluation};

/// The customer namespace every row of the table watches.
const NAMESPACE: &str = "Custom/MofN";

/// The metric every row watches.
const METRIC: &str = "Errors";

/// The threshold every row compares against, with `GreaterThanThreshold`: a breaching value is
/// above it and a clean one below.
const THRESHOLD: f64 = 10.0;

/// A breaching value.
const BREACHING: f64 = 99.0;

/// A clean value.
const CLEAN: f64 = 1.0;

/// One datapoint a row publishes: its value and WHICH period of the alarm it belongs to,
/// counting back from the period the publish falls in.
#[derive(Clone, Copy, Debug)]
struct Placed {
    /// The value published.
    value: f64,
    /// How many periods back from the current one the datapoint's time stamp falls in.
    periods_back: u32,
}

/// A datapoint of `value` stamped inside the `periods_back`th period back from the current one.
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
    /// The alarm's `EvaluationPeriods` — the N.
    evaluation_periods: i32,
    /// The alarm's `DatapointsToAlarm` — the M, where the row names one.
    datapoints_to_alarm: Option<i32>,
    /// The alarm's `TreatMissingData`, where the row names one.
    treat_missing_data: Option<&'static str>,
    /// The datapoints the row publishes, each placed in a period of its own choosing.
    data: &'static [Placed],
    /// The state the evaluation must put the alarm in.
    state: &'static str,
    /// The whole `StateReason` the evaluation must report, with `{instants}` standing for the
    /// bracketed datapoint list the row cannot know the instants of, and empty where the row
    /// reports a state nothing was graded for.
    reason: &'static str,
    /// The `StateReason` fragments the row reports instead, where it names no whole sentence.
    reason_fragments: &'static [&'static str],
    /// How many values `StateReasonData.recentDatapoints` must hold: one per period a REAL
    /// datapoint was found for, which is what the missing-data treatment cannot substitute.
    recent_datapoints: usize,
    /// Whether this evaluation wrote a decision record at all: an evaluation that computes the
    /// state the alarm already reports changes nothing, so a window that decides nothing leaves
    /// a newly created alarm exactly as its creation left it.
    writes_record: bool,
    /// How many datapoints `StateReasonData.evaluatedDatapoints` must name: the ones the verdict
    /// RESTS ON, which is the minimum of the direction the state was decided in — M for `ALARM`
    /// and `EvaluationPeriods - DatapointsToAlarm + 1` for `OK`.
    verdict_datapoints: usize,
    /// Whether those datapoints are REAL and consecutive, so each one names the instant of its
    /// own `Period`-second bucket and the buckets are exactly one period apart.
    named_instants: bool,
}

/// How far two instants a record names may be from the period apart and still count as exactly
/// one period apart: the instants are whole seconds on the period grid, so this leaves room for
/// nothing but the rendering's own rounding.
const INSTANT_TOLERANCE_SECS: f64 = 1e-6;

/// The `StateReason` `PutMetricAlarm` leaves a newly created alarm with, before any evaluation
/// has decided anything ("When this operation creates an alarm, the alarm state is immediately
/// set to INSUFFICIENT_DATA",
/// `smithy_query(query="doc",shape="PutMetricAlarm")`).
const CREATION_REASON: &str = "Unchecked: initial alarm creation";

/// The table: one row per shape the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        // The recorded high-resolution case: three consecutive 10-second buckets, every one of
        // them breaching, is the whole window a 3-of-3 alarm needs.
        case: "a high-resolution 3-of-3 window of three breaching buckets",
        period: 10,
        evaluation_periods: 3,
        datapoints_to_alarm: Some(3),
        treat_missing_data: None,
        data: &[
            placed(BREACHING, 0),
            placed(BREACHING, 1),
            placed(BREACHING, 2),
        ],
        state: "ALARM",
        reason: "Threshold Crossed: 3 out of the last 3 datapoints {instants} were greater than \
                 the threshold (10.0) (minimum 3 datapoints for OK -> ALARM transition).",
        reason_fragments: &[],
        recent_datapoints: 3,
        writes_record: true,
        verdict_datapoints: 3,
        named_instants: true,
    },
    Row {
        // The same alarm one bucket short: two breaching datapoints are below its M, and no
        // clean datapoint satisfies the OK direction, so the window decides nothing and the
        // alarm is left exactly as its creation left it.
        case: "a high-resolution 3-of-3 window two buckets wide",
        period: 10,
        evaluation_periods: 3,
        datapoints_to_alarm: Some(3),
        treat_missing_data: None,
        data: &[placed(BREACHING, 0), placed(BREACHING, 1)],
        state: "INSUFFICIENT_DATA",
        reason: "",
        reason_fragments: &[CREATION_REASON],
        recent_datapoints: 0,
        writes_record: false,
        verdict_datapoints: 0,
        named_instants: false,
    },
    Row {
        // The recorded M-of-N case: two breaching datapoints of five is one short of M, and the
        // three clean ones are exactly the OK direction's minimum (5 - 3 + 1).
        case: "a 3-of-5 alarm one breaching datapoint short of M",
        period: 60,
        evaluation_periods: 5,
        datapoints_to_alarm: Some(3),
        treat_missing_data: Some("notBreaching"),
        data: &[
            placed(BREACHING, 0),
            placed(BREACHING, 1),
            placed(CLEAN, 2),
            placed(CLEAN, 3),
            placed(CLEAN, 4),
        ],
        state: "OK",
        reason: "Threshold Crossed: 3 out of the last 5 datapoints {instants} were not greater \
                 than the threshold (10.0) (minimum 3 datapoints for ALARM -> OK transition).",
        reason_fragments: &[],
        recent_datapoints: 5,
        writes_record: true,
        verdict_datapoints: 3,
        named_instants: true,
    },
    Row {
        // The same alarm at M: the third breaching datapoint reaches the ALARM direction's
        // minimum, and the sentence counts the breaching ones.
        case: "a 3-of-5 alarm at M",
        period: 60,
        evaluation_periods: 5,
        datapoints_to_alarm: Some(3),
        treat_missing_data: Some("notBreaching"),
        data: &[
            placed(BREACHING, 0),
            placed(BREACHING, 1),
            placed(BREACHING, 2),
            placed(CLEAN, 3),
            placed(CLEAN, 4),
        ],
        state: "ALARM",
        reason: "Threshold Crossed: 3 out of the last 5 datapoints {instants} were greater than \
                 the threshold (10.0) (minimum 3 datapoints for OK -> ALARM transition).",
        reason_fragments: &[],
        recent_datapoints: 5,
        writes_record: true,
        verdict_datapoints: 3,
        named_instants: true,
    },
    Row {
        // M defaulting to N: a 5-of-5 alarm needs just ONE clean datapoint to be in OK, so its
        // reason names a minimum of one however many evaluation periods it covers.
        case: "a 5-of-5 alarm with one breaching datapoint",
        period: 60,
        evaluation_periods: 5,
        datapoints_to_alarm: None,
        treat_missing_data: None,
        data: &[
            placed(BREACHING, 0),
            placed(CLEAN, 1),
            placed(CLEAN, 2),
            placed(CLEAN, 3),
            placed(CLEAN, 4),
        ],
        state: "OK",
        reason: "Threshold Crossed: 4 out of the last 5 datapoints {instants} were not greater \
                 than the threshold (10.0) (minimum 1 datapoint for ALARM -> OK transition).",
        reason_fragments: &[],
        recent_datapoints: 5,
        writes_record: true,
        verdict_datapoints: 1,
        named_instants: true,
    },
    Row {
        // One breaching datapoint of a 1-of-2 window at the shortest sub-minute cadence but
        // one: the ALARM direction's minimum is one, so the window crosses.
        case: "a 1-of-2 alarm over 20-second periods with one breaching bucket",
        period: 20,
        evaluation_periods: 2,
        datapoints_to_alarm: Some(1),
        treat_missing_data: None,
        data: &[placed(CLEAN, 0), placed(BREACHING, 1)],
        state: "ALARM",
        reason: "Threshold Crossed: 1 out of the last 2 datapoints {instants} was greater than \
                 the threshold (10.0) (minimum 1 datapoint for OK -> ALARM transition).",
        reason_fragments: &[],
        recent_datapoints: 2,
        writes_record: true,
        verdict_datapoints: 1,
        named_instants: true,
    },
    Row {
        // A 2-of-4 window over 30-second periods: the two newest buckets breach, which is
        // exactly M, and the two clean ones are one short of the OK direction's three.
        case: "a 2-of-4 alarm over 30-second periods at M",
        period: 30,
        evaluation_periods: 4,
        datapoints_to_alarm: Some(2),
        treat_missing_data: None,
        data: &[
            placed(BREACHING, 0),
            placed(BREACHING, 1),
            placed(CLEAN, 2),
            placed(CLEAN, 3),
        ],
        state: "ALARM",
        reason: "Threshold Crossed: 2 out of the last 4 datapoints {instants} were greater than \
                 the threshold (10.0) (minimum 2 datapoints for OK -> ALARM transition).",
        reason_fragments: &[],
        recent_datapoints: 4,
        writes_record: true,
        verdict_datapoints: 2,
        named_instants: true,
    },
    Row {
        // `breaching` SUBSTITUTES a datapoint for every period that held none, so the window is
        // whole again: the two substituted ones reach this alarm's M even though only one real
        // datapoint — a clean one — was ever published, and only that one is a recent datapoint.
        case: "a 2-of-3 alarm whose empty periods are treated as breaching",
        period: 60,
        evaluation_periods: 3,
        datapoints_to_alarm: Some(2),
        treat_missing_data: Some("breaching"),
        data: &[placed(CLEAN, 0)],
        state: "ALARM",
        reason: "",
        reason_fragments: &[
            "Threshold Crossed: 2 out of the last 3 datapoints",
            "(minimum 2 datapoints for OK -> ALARM transition).",
            "2 missing datapoints were treated as [Breaching].",
        ],
        recent_datapoints: 1,
        writes_record: true,
        // The verdict rests on the two datapoints the arm substituted, and nothing was measured
        // over the periods they stand for: those entries name the arm, not an instant.
        verdict_datapoints: 2,
        named_instants: false,
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
        "Statistic": "Maximum",
        "Period": row.period,
        "EvaluationPeriods": row.evaluation_periods,
        "Threshold": THRESHOLD,
        "ComparisonOperator": "GreaterThanThreshold",
        "ActionsEnabled": false,
    });
    if let Some(count) = row.datapoints_to_alarm {
        alarm["DatapointsToAlarm"] = json!(count);
    }
    if let Some(arm) = row.treat_missing_data {
        alarm["TreatMissingData"] = json!(arm);
    }
    alarm
}

/// Publish a row's datapoints, each stamped INSIDE the period it names.
///
/// The time stamps are placed on the period grid, in the MIDDLE of the period they name, so the
/// period a datapoint belongs to is the one the row asked for whatever phase of a period the
/// publish falls in — which is what makes a row's window a property of its datapoints rather
/// than of the moment the test runs. A period boundary crossed between the publish and the
/// reading shifts every one of a row's periods back together, and the evaluation grades the most
/// recent periods that hold a datapoint, so neither the state nor the counts change under it.
async fn publish_placed(router: &MultiServiceRouter, name: &str, row: &Row) {
    let period = f64::from(row.period);
    let current_period_start = (now_secs() / period).floor() * period;
    let data: Vec<Value> = row
        .data
        .iter()
        .map(|datum| {
            let start = current_period_start - f64::from(datum.periods_back) * period;
            json!({
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Case", "Value": name}],
                "Value": datum.value,
                // A high-resolution series: the sub-minute rows need the resolution their
                // period reads at ("Data points with a period of 10 seconds are available only
                // for custom metrics defined with a StorageResolution of 1").
                "StorageResolution": 1,
                "Timestamp": start + period / 2.0,
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
fn without_instants(reason: &str) -> Option<String> {
    let open = reason.find('[')?;
    let close = reason[open..].find(']')? + open;
    Some(format!(
        "{}{{instants}}{}",
        &reason[..open],
        &reason[close + 1..]
    ))
}

/// Publish ONE datapoint of `value` into the series `name` watches, stamped in the middle of the
/// `periods_back`th `period`-second period back from the one the publish falls in.
async fn publish_one(
    router: &MultiServiceRouter,
    name: &str,
    period: f64,
    periods_back: u32,
    value: f64,
) {
    let start =
        (now_secs() / period).floor() * period - f64::from(periods_back) * period + period / 2.0;
    ok(
        router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Case", "Value": name}],
                "Value": value,
                "Timestamp": start,
            }],
        }),
    )
    .await;
}

/// One instant of a decision record as epoch seconds: the record writes every instant in the
/// modelled timestamp type's own rendering, so it is read back through that type rather than
/// through a second date implementation.
fn record_instant(case: &str, timestamp: &Value) -> f64 {
    let text = timestamp
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an instant, got {timestamp}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {text:?} is not a modelled timestamp: {error}")
        })
        .as_secs_f64()
}

#[tokio::test]
async fn an_alarm_is_decided_by_the_minimum_of_the_direction_it_reports() {
    // Every row watches its own series, so all of them are configured on ONE emulator and the
    // evaluation delay is waited out once rather than per row.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("m-of-n-{index}");
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
        if !row.reason.is_empty() {
            let shape = without_instants(&reason).unwrap_or_else(|| {
                panic!("case {case:?}: no bracketed datapoint list in {reason:?}")
            });
            assert_eq!(
                shape, row.reason,
                "case {case:?}: expected the StateReason {:?}, got {shape:?}",
                row.reason
            );
        }
        for fragment in row.reason_fragments {
            assert!(
                reason.contains(fragment),
                "case {case:?}: expected the StateReason to report {fragment:?}, got {reason:?}"
            );
        }

        // The decision record: the window the evaluation graded, as the periods it was laid
        // over and the datapoints it found in them. An evaluation that decided nothing wrote
        // none, so the alarm still carries no record at all.
        let written = reported.get("StateReasonData").and_then(Value::as_str);
        if !row.writes_record {
            assert!(
                written.is_none(),
                "case {case:?}: an evaluation that decided nothing must write no decision \
                 record, got {written:?}"
            );
            continue;
        }
        let record: Value = serde_json::from_str(written.unwrap_or_default()).unwrap_or_else(
            |error| panic!("case {case:?}: StateReasonData must be JSON, got {reported}: {error}"),
        );
        assert_eq!(
            record.get("period").and_then(Value::as_f64),
            Some(f64::from(row.period)),
            "case {case:?}: the record must report the period it graded, got {record}"
        );
        let recent = record
            .get("recentDatapoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            recent.len(),
            row.recent_datapoints,
            "case {case:?}: expected {} recent datapoints, got {record}",
            row.recent_datapoints
        );
        let evaluated = record
            .get("evaluatedDatapoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            evaluated.len(),
            row.verdict_datapoints,
            "case {case:?}: the record must name the {} datapoints the verdict rests on, got \
             {record}",
            row.verdict_datapoints
        );
        if !row.named_instants {
            continue;
        }

        // Real, consecutive datapoints: the record names the instant of each one's own
        // `Period`-second bucket, newest first, beside the value and the sample count of the
        // period it was aggregated out of.
        let instants: Vec<f64> = evaluated
            .iter()
            .map(|entry| record_instant(case, &entry["timestamp"]))
            .collect();
        for pair in instants.windows(2) {
            let [newer, older] = pair else {
                continue;
            };
            assert!(
                (newer - older - f64::from(row.period)).abs() < INSTANT_TOLERANCE_SECS,
                "case {case:?}: expected the graded buckets exactly {} seconds apart, got \
                 {instants:?}",
                row.period
            );
        }
        for entry in &evaluated {
            assert_eq!(
                entry.get("sampleCount").and_then(Value::as_f64),
                Some(1.0),
                "case {case:?}: each graded period reports the observations it aggregated, got \
                 {entry}"
            );
            let value = entry.get("value").and_then(Value::as_f64);
            assert!(
                value == Some(BREACHING) || value == Some(CLEAN),
                "case {case:?}: each graded period reports the value its statistic took, got \
                 {entry}"
            );
        }
    }
}

#[tokio::test]
async fn a_window_short_of_both_minima_stops_deciding_an_alarm_that_had_crossed() {
    // Each arm's alarm is driven into ALARM first over a window the `breaching` treatment made
    // whole, and then re-configured onto a treatment that substitutes NOTHING — a
    // `PutMetricAlarm` over an alarm that exists keeps the state it has. Its window is then two
    // real breaching datapoints of a 3-of-3 predicate: one short of M, with no clean datapoint
    // for the OK direction, so neither minimum is reached and the window decides nothing.
    //
    // (the arm, the state the evaluation that follows must report, what its reason must carry)
    let rows = [
        (
            "missing",
            "INSUFFICIENT_DATA",
            "Insufficient Data: 1 datapoint was unknown.",
        ),
        // "ignore — the current alarm state is maintained": the alarm keeps the state it had
        // and the explanation of it, because this evaluation decided nothing.
        ("ignore", "ALARM", "minimum 3 datapoints for OK -> ALARM transition"),
    ];

    let router = build_router();
    // The window every arm starts from, as a `const` so the slice of placed datapoints lives as
    // long as the row that names it.
    const CROSSING: Row = Row {
        case: "two breaching buckets of three, the third treated as breaching",
        period: 10,
        evaluation_periods: 3,
        datapoints_to_alarm: Some(3),
        treat_missing_data: Some("breaching"),
        data: &[placed(BREACHING, 0), placed(BREACHING, 1)],
        state: "ALARM",
        reason: "",
        reason_fragments: &[],
        recent_datapoints: 2,
        writes_record: true,
        verdict_datapoints: 3,
        named_instants: false,
    };
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (arm, _, _) in rows {
        let name = format!("undecidable-{arm}");
        ok(&router, "PutMetricAlarm", &alarm_of(&CROSSING, &name)).await;
        publish_placed(&router, &name, &CROSSING).await;
        names.push(name);
    }
    wait_for_evaluation();

    for ((arm, _, _), name) in rows.iter().zip(&names) {
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        assert_eq!(
            described["MetricAlarms"][0]
                .get("StateValue")
                .and_then(Value::as_str),
            Some("ALARM"),
            "case {arm:?}: the substituted window must put the alarm in ALARM first, got \
             {described}"
        );
        // The same alarm, now with the arm that substitutes nothing.
        let mut reconfigured = CROSSING;
        reconfigured.treat_missing_data = Some(arm);
        ok(&router, "PutMetricAlarm", &alarm_of(&reconfigured, name)).await;
    }
    wait_for_evaluation();

    for ((arm, state, reported), name) in rows.iter().zip(&names) {
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let alarm = &described["MetricAlarms"][0];
        assert_eq!(
            alarm.get("StateValue").and_then(Value::as_str),
            Some(*state),
            "case {arm:?}: expected StateValue {state}, got {alarm}"
        );
        let reason = alarm
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            reason.contains(reported),
            "case {arm:?}: expected the StateReason to report {reported:?}, got {reason:?}"
        );
    }
}

#[tokio::test]
async fn an_m_of_n_alarm_holds_ok_one_datapoint_short_of_m_and_crosses_at_m() {
    // The recorded M-of-N case as a SEQUENCE on one alarm: a 3-of-5 alarm over 60-second periods
    // whose window holds two breaching datapoints — one short of M — reports OK, naming the
    // non-breaching datapoints that satisfied the OK direction and the minimum it needed; the
    // next breaching datapoint reaches M and the same alarm crosses into ALARM, naming the
    // breaching ones and the minimum of that direction.
    //
    // The window is kept HOLE-FREE: every one of the five periods it grades carries a datapoint,
    // the newest of them the period the publishes fall in.
    const PERIOD: f64 = 60.0;
    const PERIODS: i32 = 5;
    const TO_ALARM: i32 = 3;
    let name = "m-of-n-sequence";
    let row = Row {
        case: name,
        period: 60,
        evaluation_periods: PERIODS,
        datapoints_to_alarm: Some(TO_ALARM),
        treat_missing_data: Some("notBreaching"),
        data: &[],
        state: "OK",
        reason: "",
        reason_fragments: &[],
        recent_datapoints: 5,
        writes_record: true,
        verdict_datapoints: 3,
        named_instants: true,
    };

    let router = build_router();
    ok(&router, "PutMetricAlarm", &alarm_of(&row, name)).await;
    // Oldest first: three clean periods, then two breaching ones, and the period the publish
    // falls in kept filled with a clean datapoint.
    for (periods_back, value) in [(4, CLEAN), (3, CLEAN), (2, BREACHING), (1, BREACHING), (0, CLEAN)]
    {
        publish_one(&router, name, PERIOD, periods_back, value).await;
    }
    wait_for_evaluation();

    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    let alarm = &described["MetricAlarms"][0];
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("OK"),
        "two breaching datapoints of five are one short of M, so the alarm must hold OK, got \
         {described}"
    );
    let reason = alarm
        .get("StateReason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let expected = format!(
        "Threshold Crossed: 3 out of the last 5 datapoints {{instants}} were not greater than \
         the threshold (10.0) (minimum {} datapoints for ALARM -> OK transition).",
        PERIODS - TO_ALARM + 1
    );
    assert_eq!(
        without_instants(&reason).unwrap_or_default(),
        expected,
        "expected the OK reason {expected:?}, got {reason:?}"
    );
    let record: Value = serde_json::from_str(
        alarm
            .get("StateReasonData")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .expect("the decision record is JSON");
    assert_eq!(
        record["recentDatapoints"].as_array().map(Vec::len),
        Some(5),
        "the record must report one datapoint per evaluation period, got {record}"
    );
    let breaching = record["recentDatapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|value| value.as_f64().is_some_and(|value| value > THRESHOLD))
        .count();
    assert_eq!(
        breaching, 2,
        "exactly two of the window's datapoints breach, got {record}"
    );

    // One more breaching datapoint, in the period the publish falls in: the window now holds
    // three breaching datapoints, which is M, so the same alarm crosses.
    publish_one(&router, name, PERIOD, 0, BREACHING).await;
    wait_for_evaluation();
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    let alarm = &described["MetricAlarms"][0];
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("ALARM"),
        "the datapoint that reaches M must cross the alarm, got {described}"
    );
    let reason = alarm
        .get("StateReason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let expected = format!(
        "Threshold Crossed: 3 out of the last 5 datapoints {{instants}} were greater than the \
         threshold (10.0) (minimum {TO_ALARM} datapoints for OK -> ALARM transition)."
    );
    assert_eq!(
        without_instants(&reason).unwrap_or_default(),
        expected,
        "expected the ALARM reason {expected:?}, got {reason:?}"
    );
}

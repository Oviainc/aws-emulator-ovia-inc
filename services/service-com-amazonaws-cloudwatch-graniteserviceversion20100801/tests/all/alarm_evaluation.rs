// SPDX-License-Identifier: CC0-1.0

//! The METRIC-ALARM EVALUATION slice, end to end over the wire: a metric alarm's
//! `StateValue` computed from the datapoints `PutMetricData` stored, the history item the
//! transition records, and the SNS action the transition fires — plus the namespace a
//! publisher may not write into.
//!
//! Two rules are asserted here, each over a TABLE whose rows differ along what the rule
//! generalises over rather than along a name:
//!
//! 1. `PutMetricData` refuses a `Namespace` in the AWS-reserved namespace space (one
//!    beginning with `AWS/`) with HTTP 400 `InvalidParameterValue` and stores nothing, and
//!    accepts every other namespace. Rows differ in which reserved namespace is named, how
//!    the prefix is spelled and where it appears.
//! 2. A metric alarm is in `ALARM` once at least `DatapointsToAlarm` of the datapoints its
//!    evaluation found in the range it searches satisfy its `ComparisonOperator` against
//!    `Threshold` — or, where the missing-data treatment left it with fewer datapoints than
//!    that, once every datapoint it has does — in `OK` when fewer do and at least one
//!    datapoint was evaluated, and in `INSUFFICIENT_DATA` when none was. Rows differ in the
//!    comparison, the statistic, the datapoint values and their count, how many of the
//!    evaluated periods carry one, the age of a datapoint, its unit, its dimension set and
//!    the `TreatMissingData` setting.
//! 3. The `StateReason` of an evaluated state explains how that state was decided: a window
//!    that held no datapoint of the metric is reported as the hole it was, naming the
//!    missing-data arm applied to it, and only the datapoints the evaluation really had are
//!    reported as graded. Rows differ in the arm in force, how many of the evaluated windows
//!    hold a real datapoint, the number of evaluation periods and whether the comparison held.
//!
//! The recorded case the issue was filed about (a `SearchableDocuments` datapoint driven
//! past an alarm's threshold, and a forged `AWS/ES` datapoint) is one row of each table.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, error_code, ok};

/// The customer namespace the alarm rows below watch.
const NAMESPACE: &str = "Custom/OpenSearch";

/// The metric the alarm rows below watch.
const METRIC: &str = "SearchableDocuments";

/// Long enough for a metric alarm's first evaluation to come due.
///
/// `PutMetricAlarm` sets a new alarm to `INSUFFICIENT_DATA` and the evaluated state is owed a
/// span later (`crate::state::alarm_evaluation::EVALUATION_DELAY_SECS`, inside the
/// framework's 250-750 ms window for a transition the clock owes), so a test that wants the
/// EVALUATED state waits past the top of that window.
const PAST_EVALUATION_DELAY: Duration = Duration::from_millis(900);

/// Wait for every alarm evaluation started before now to be due.
///
/// A thread sleep rather than `tokio::time::sleep`: this workspace enables tokio's `macros`
/// and `rt` features only, so there is no timer driver to await on. Each test here drives one
/// router with no concurrent task, so blocking its own runtime thread holds nothing up.
fn wait_for_evaluation() {
    std::thread::sleep(PAST_EVALUATION_DELAY);
}

/// The shortest `Period` an alarm may be evaluated on: "Valid values are 10, 20, 30, and any
/// multiple of 60." (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`).
///
/// The rows that wait out a span measured in evaluation periods use it, so they wait the
/// shortest span an alarm can be configured for.
const SHORTEST_PERIOD_SECS: i32 = 10;

/// Wall-clock now as epoch seconds, the clock the evaluation windows are measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// The alarm every row of the evaluation table starts from: one 60-second period, one
/// evaluation period, a static threshold of 10 and a `GreaterThanThreshold` comparison.
///
/// Each row watches its OWN dimension set — the one its `DomainName` dimension names — so
/// every row has its own time series and the rows share one emulator.
fn base_alarm(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "DomainName", "Value": name}],
        "Statistic": "Average",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// One datapoint a row publishes: its value, how many seconds before the publish its time
/// stamp lies, and the extra members that make the row what it is (another `Unit`, another
/// dimension set).
#[derive(Clone, Debug)]
struct Datum {
    value: f64,
    age_secs: f64,
    extra: Value,
}

/// A datapoint of `value`, stamped `age_secs` before the publish.
fn datum(value: f64, age_secs: f64) -> Datum {
    Datum {
        value,
        age_secs,
        extra: json!({}),
    }
}

impl Datum {
    /// The same datapoint with `extra` merged into its `MetricDatum`.
    fn with(mut self, extra: Value) -> Self {
        self.extra = extra;
        self
    }

    /// This datapoint as a `MetricDatum` of the series `domain` names, stamped against `now`.
    fn rendered(&self, now: f64, domain: &str) -> Value {
        let mut rendered = json!({
            "MetricName": METRIC,
            "Dimensions": [{"Name": "DomainName", "Value": domain}],
            "Value": self.value,
            "Timestamp": now - self.age_secs,
        });
        for (member, value) in self.extra.as_object().cloned().unwrap_or_default() {
            rendered[member] = value;
        }
        rendered
    }
}

/// Publish `data` as the series the `DomainName` dimension `domain` names, in one call.
async fn publish(router: &MultiServiceRouter, domain: &str, data: &[Datum]) {
    if data.is_empty() {
        return;
    }
    let now = now_secs();
    let rendered: Vec<Value> = data
        .iter()
        .map(|datum| datum.rendered(now, domain))
        .collect();
    ok(
        router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": rendered}),
    )
    .await;
}

/// Apply a row's patch to the base alarm: every member the row names replaces the base one,
/// and a member the row names `null` is REMOVED — an alarm carries `Statistic` or
/// `ExtendedStatistic`, never both, so a row that names one has to drop the other.
fn patched(name: &str, patch: &Value) -> Value {
    let mut alarm = base_alarm(name);
    for (member, value) in patch.as_object().cloned().unwrap_or_default() {
        if value.is_null() {
            alarm
                .as_object_mut()
                .expect("the alarm is a JSON object")
                .remove(&member);
        } else {
            alarm[member] = value;
        }
    }
    alarm
}

#[tokio::test]
async fn an_alarms_state_is_evaluated_from_the_datapoints_it_watches() {
    // (case, the members this row changes on the base alarm, the datapoints it publishes,
    //  the state the evaluation must put the alarm in)
    let rows: Vec<(&str, Value, Vec<Datum>, &str)> = vec![
        (
            "a datapoint past a GreaterThanThreshold threshold",
            json!({}),
            vec![datum(42.0, 0.0)],
            "ALARM",
        ),
        (
            "a datapoint exactly at a GreaterThanThreshold threshold",
            json!({}),
            vec![datum(10.0, 0.0)],
            "OK",
        ),
        (
            "a datapoint exactly at a GreaterThanOrEqualToThreshold threshold",
            json!({"ComparisonOperator": "GreaterThanOrEqualToThreshold"}),
            vec![datum(10.0, 0.0)],
            "ALARM",
        ),
        (
            "a datapoint under a LessThanThreshold threshold",
            json!({"ComparisonOperator": "LessThanThreshold"}),
            vec![datum(3.0, 0.0)],
            "ALARM",
        ),
        (
            "a datapoint exactly at a LessThanOrEqualToThreshold threshold",
            json!({"ComparisonOperator": "LessThanOrEqualToThreshold"}),
            vec![datum(10.0, 0.0)],
            "ALARM",
        ),
        (
            "a datapoint over a LessThanOrEqualToThreshold threshold",
            json!({"ComparisonOperator": "LessThanOrEqualToThreshold"}),
            vec![datum(11.0, 0.0)],
            "OK",
        ),
        (
            "the Average of two datapoints of one period crosses",
            json!({}),
            vec![datum(2.0, 0.0), datum(30.0, 1.0)],
            "ALARM",
        ),
        (
            "the Minimum of the same two datapoints does not",
            json!({"Statistic": "Minimum"}),
            vec![datum(2.0, 0.0), datum(30.0, 1.0)],
            "OK",
        ),
        (
            "the Maximum of the same two datapoints does",
            json!({"Statistic": "Maximum"}),
            vec![datum(2.0, 0.0), datum(30.0, 1.0)],
            "ALARM",
        ),
        (
            "the Sum of the same two datapoints does",
            json!({"Statistic": "Sum"}),
            vec![datum(2.0, 0.0), datum(30.0, 1.0)],
            "ALARM",
        ),
        (
            "the SampleCount of two datapoints is under a threshold of 10",
            json!({"Statistic": "SampleCount"}),
            vec![datum(2.0, 0.0), datum(30.0, 1.0)],
            "OK",
        ),
        (
            "the p90 extended statistic of ten datapoints crosses",
            json!({"Statistic": Value::Null, "ExtendedStatistic": "p90"}),
            (0..10).map(|i| datum(f64::from(i) * 3.0, 0.0)).collect(),
            "ALARM",
        ),
        (
            "the p10 extended statistic of the same ten datapoints does not",
            json!({"Statistic": Value::Null, "ExtendedStatistic": "p10"}),
            (0..10).map(|i| datum(f64::from(i) * 3.0, 0.0)).collect(),
            "OK",
        ),
        (
            "one of three periods breaches, and one datapoint is enough",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 1}),
            vec![datum(42.0, 0.0), datum(1.0, 70.0), datum(1.0, 130.0)],
            "ALARM",
        ),
        (
            "one of three periods breaches, and two datapoints are needed",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 2}),
            vec![datum(42.0, 0.0), datum(1.0, 70.0), datum(1.0, 130.0)],
            "OK",
        ),
        (
            "two of three periods breach, and two datapoints are needed",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 2}),
            vec![datum(42.0, 0.0), datum(43.0, 70.0), datum(1.0, 130.0)],
            "ALARM",
        ),
        (
            "every period breaches, and no DatapointsToAlarm is named",
            json!({"EvaluationPeriods": 2}),
            vec![datum(42.0, 0.0), datum(43.0, 70.0)],
            "ALARM",
        ),
        (
            "only the newest of two periods breaches, and no DatapointsToAlarm is named",
            json!({"EvaluationPeriods": 2}),
            vec![datum(42.0, 0.0), datum(1.0, 70.0)],
            "OK",
        ),
        (
            "no datapoint at all, with the default missing-data treatment",
            json!({}),
            Vec::new(),
            "INSUFFICIENT_DATA",
        ),
        (
            "no datapoint at all, with missing data treated as breaching",
            json!({"TreatMissingData": "breaching"}),
            Vec::new(),
            "ALARM",
        ),
        (
            "no datapoint at all, with missing data treated as not breaching",
            json!({"TreatMissingData": "notBreaching"}),
            Vec::new(),
            "OK",
        ),
        (
            // "ignore — the current alarm state is maintained" speaks about MISSING data: a
            // datapoint that is present still decides the state.
            "a breaching period beside a missing one, with missing data ignored",
            json!({"EvaluationPeriods": 2, "DatapointsToAlarm": 1, "TreatMissingData": "ignore"}),
            vec![datum(42.0, 0.0)],
            "ALARM",
        ),
        (
            "a non-breaching period beside a missing one, with missing data ignored",
            json!({"EvaluationPeriods": 2, "DatapointsToAlarm": 1, "TreatMissingData": "ignore"}),
            vec![datum(1.0, 0.0)],
            "OK",
        ),
        (
            "a breaching period beside a missing one, with missing data missing",
            json!({"EvaluationPeriods": 2, "DatapointsToAlarm": 1, "TreatMissingData": "missing"}),
            vec![datum(42.0, 0.0)],
            "ALARM",
        ),
        (
            // The missing-data treatments that substitute NOTHING leave the evaluation with
            // fewer datapoints than the alarm's N, and it is decided by the datapoints it
            // has: both of the real ones breach, so the alarm crosses even though its M is
            // three.
            "two of three periods breach and the third is missing, with three needed",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 3}),
            vec![datum(42.0, 0.0), datum(43.0, 70.0)],
            "ALARM",
        ),
        (
            "one of two periods breaches and the other is missing, with no DatapointsToAlarm",
            json!({"EvaluationPeriods": 2}),
            vec![datum(42.0, 0.0)],
            "ALARM",
        ),
        (
            // `notBreaching` SUBSTITUTES a datapoint for the missing window, so the
            // evaluation has both of its datapoints and the alarm's M is asked for in full.
            "one of two periods breaches and the other is treated as not breaching",
            json!({"EvaluationPeriods": 2, "TreatMissingData": "notBreaching"}),
            vec![datum(42.0, 0.0)],
            "OK",
        ),
        (
            // The evaluation range reaches back past the look-back, so a series published a
            // little slower than the alarm's period is still evaluated on what it published.
            "a breaching datapoint one period older than the look-back",
            json!({}),
            vec![datum(42.0, 90.0)],
            "ALARM",
        ),
        (
            // … and it is the MOST RECENT datapoints of the range that are evaluated: the
            // older breaching one is not among the one datapoint this alarm evaluates.
            "a non-breaching datapoint newer than a breaching one",
            json!({}),
            vec![datum(1.0, 0.0), datum(42.0, 90.0)],
            "OK",
        ),
        (
            "a breaching datapoint older than the whole evaluation range",
            json!({}),
            vec![datum(42.0, 180.0)],
            "INSUFFICIENT_DATA",
        ),
        (
            "a breaching datapoint of the unit the alarm watches",
            json!({"Unit": "Count"}),
            vec![datum(42.0, 0.0).with(json!({"Unit": "Count"}))],
            "ALARM",
        ),
        (
            "a breaching datapoint of another unit than the alarm watches",
            json!({"Unit": "Count"}),
            vec![datum(42.0, 0.0).with(json!({"Unit": "Bytes"}))],
            "INSUFFICIENT_DATA",
        ),
        (
            "a breaching datapoint of another dimension set than the alarm watches",
            json!({}),
            vec![datum(42.0, 0.0).with(json!({
                "Dimensions": [{"Name": "DomainName", "Value": "another-domain"}]
            }))],
            "INSUFFICIENT_DATA",
        ),
        (
            "a breaching datapoint of a metric the alarm does not name",
            json!({"MetricName": "SearchLatency"}),
            vec![datum(42.0, 0.0)],
            "INSUFFICIENT_DATA",
        ),
        (
            // An anomaly-detection band is compared against a model this emulator does not
            // compute, so the alarm keeps the state its creation left it in rather than
            // being answered with a state nothing evaluated.
            "an anomaly-detection band comparison",
            json!({
                "ComparisonOperator": "GreaterThanUpperThreshold",
                "Threshold": Value::Null,
                "ThresholdMetricId": "ad1",
            }),
            vec![datum(42.0, 0.0)],
            "INSUFFICIENT_DATA",
        ),
    ];

    // Every row's alarm watches its own series (its `DomainName` dimension is its own name),
    // so all of them are configured on ONE emulator and the evaluation delay is waited out
    // once rather than per row.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, (_, patch, data, _)) in rows.iter().enumerate() {
        let name = format!("alarm-{index}");
        ok(&router, "PutMetricAlarm", &patched(&name, patch)).await;
        publish(&router, &name, data).await;
        names.push(name);
    }
    wait_for_evaluation();

    for ((case, _, _, expected), name) in rows.iter().zip(&names) {
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(*expected),
            "case {case:?}: expected StateValue {expected}, got {reported}"
        );
    }
}

/// One row of the `StateReason` table.
struct ReasonRow {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The members this row changes on the base alarm.
    patch: Value,
    /// The datapoints it publishes into its own series.
    data: Vec<Datum>,
    /// The state the evaluation must put the alarm in.
    state: &'static str,
    /// The fragments its `StateReason` must carry.
    reported: Vec<&'static str>,
    /// The fragments its `StateReason` must NOT carry.
    not_reported: Vec<&'static str>,
}

/// One row of the `StateReason` table, in the order the table below reads.
fn reason_row(
    case: &'static str,
    patch: Value,
    data: Vec<Datum>,
    state: &'static str,
    reported: Vec<&'static str>,
    not_reported: Vec<&'static str>,
) -> ReasonRow {
    ReasonRow {
        case,
        patch,
        data,
        state,
        reported,
        not_reported,
    }
}

#[tokio::test]
async fn an_evaluated_state_reason_reports_an_empty_window_as_a_hole_and_names_the_arm() {
    // The rows differ in the missing-data arm in force, how many of the evaluated windows hold
    // a real datapoint, how many periods the alarm evaluates and whether the comparison held —
    // the things the rule generalises over. The recorded case the issue was filed about (one
    // evaluation period, `notBreaching`, no datapoint published at all) is the first row.
    let rows: Vec<ReasonRow> = vec![
        reason_row(
            "no datapoint at all over one period, missing data treated as not breaching",
            json!({"TreatMissingData": "notBreaching"}),
            Vec::new(),
            "OK",
            vec![
                "no datapoints were received for 1 period",
                "1 missing datapoint was treated as [NonBreaching]",
            ],
            vec!["out of the last"],
        ),
        reason_row(
            "no datapoint at all over three periods, missing data treated as not breaching",
            json!({"EvaluationPeriods": 3, "TreatMissingData": "notBreaching"}),
            Vec::new(),
            "OK",
            vec![
                "no datapoints were received for 3 periods",
                "3 missing datapoints were treated as [NonBreaching]",
            ],
            vec!["out of the last"],
        ),
        reason_row(
            "no datapoint at all, missing data treated as breaching",
            json!({"TreatMissingData": "breaching"}),
            Vec::new(),
            "ALARM",
            vec![
                "no datapoints were received for 1 period",
                "1 missing datapoint was treated as [Breaching]",
            ],
            vec!["out of the last", "NonBreaching"],
        ),
        reason_row(
            "one real non-breaching datapoint beside a window treated as not breaching",
            json!({"EvaluationPeriods": 2, "TreatMissingData": "notBreaching"}),
            vec![datum(1.0, 0.0)],
            "OK",
            vec![
                "0 out of the last 2 datapoints",
                "1 missing datapoint was treated as [NonBreaching]",
            ],
            vec!["no datapoints were received"],
        ),
        reason_row(
            "one real breaching datapoint beside a window treated as breaching",
            json!({"EvaluationPeriods": 2, "TreatMissingData": "breaching"}),
            vec![datum(42.0, 0.0)],
            "ALARM",
            vec![
                "2 out of the last 2 datapoints",
                "1 missing datapoint was treated as [Breaching]",
            ],
            vec!["no datapoints were received"],
        ),
        reason_row(
            "every evaluated datapoint is real and breaching",
            json!({}),
            vec![datum(42.0, 0.0)],
            "ALARM",
            vec!["Threshold Crossed: 1 out of the last 1 datapoints"],
            vec!["missing datapoint", "no datapoints were received"],
        ),
        reason_row(
            "every evaluated datapoint is real and none breaches",
            json!({}),
            vec![datum(1.0, 0.0)],
            "OK",
            vec!["Threshold Crossed: 0 out of the last 1 datapoints"],
            vec![
                "Threshold not Crossed",
                "missing datapoint",
                "no datapoints were received",
            ],
        ),
        reason_row(
            // `missing` substitutes no datapoint, so the two real ones are all the evaluation
            // had and that is the count it reports — no arm decided anything here.
            "two real breaching datapoints and a third window the default treatment leaves out",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 3}),
            vec![datum(42.0, 0.0), datum(43.0, 70.0)],
            "ALARM",
            vec!["2 out of the last 2 datapoints"],
            vec!["missing datapoint", "no datapoints were received"],
        ),
    ];

    // Every row's alarm watches its own series, so all of them are configured on ONE emulator
    // and the evaluation delay is waited out once rather than per row.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("reason-{index}");
        ok(&router, "PutMetricAlarm", &patched(&name, &row.patch)).await;
        publish(&router, &name, &row.data).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        let state = row.state;
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(state),
            "case {case:?}: expected StateValue {state}, got {reported}"
        );
        let reason = reported
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for fragment in &row.reported {
            assert!(
                reason.contains(fragment),
                "case {case:?}: expected StateReason to report {fragment:?}, got {reason:?}"
            );
        }
        for fragment in &row.not_reported {
            assert!(
                !reason.contains(fragment),
                "case {case:?}: expected StateReason not to claim {fragment:?}, got {reason:?}"
            );
        }
        // `StateReason` is `@length 0..1023`
        // (`smithy_query(query="shape",shape="StateReason")`).
        assert!(
            reason.len() <= 1023,
            "case {case:?}: StateReason must stay inside its modelled length, got {} \
             characters: {reason:?}",
            reason.len()
        );
    }
}

#[tokio::test]
async fn an_evaluation_with_nothing_to_grade_reports_the_datapoints_it_did_not_have() {
    // (case, the alarm's `EvaluationPeriods`, the reason the evaluation must report once its
    //  datapoint has fallen out of the range)
    //
    // The shortest period an alarm may be evaluated on (`Period`: "Valid values are 10, 20,
    // 30, and any multiple of 60."), with each row's datapoint published close to the end of
    // the evaluation range its alarm searches — so the range it falls out of is short enough
    // to wait out: no datapoint is left in it, the default `missing` treatment substitutes
    // none, and the evaluation reports the datapoints it does not have instead of naming an
    // arm.
    let rows = [
        (
            "one evaluation period",
            1,
            "Insufficient Data: 1 datapoint was unknown.",
        ),
        (
            "two evaluation periods",
            2,
            "Insufficient Data: 2 datapoints were unknown.",
        ),
    ];

    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, (_, periods, _)) in rows.iter().enumerate() {
        let name = format!("aged-out-{index}");
        let alarm = patched(
            &name,
            &json!({"Period": SHORTEST_PERIOD_SECS, "EvaluationPeriods": periods}),
        );
        ok(&router, "PutMetricAlarm", &alarm).await;
        // The evaluation range reaches twice the look-back back, so a datapoint three
        // seconds short of it is still evaluated now and out of the range a few seconds on.
        let age = f64::from(SHORTEST_PERIOD_SECS * 2 * periods) - 3.0;
        publish(&router, &name, &[datum(42.0, age)]).await;
        names.push(name);
    }
    wait_for_evaluation();

    // The datapoint is still in the range, so each alarm is evaluated into ALARM first.
    for ((case, _, _), name) in rows.iter().zip(&names) {
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        assert_eq!(
            described["MetricAlarms"][0]
                .get("StateValue")
                .and_then(Value::as_str),
            Some("ALARM"),
            "case {case:?}: the breaching datapoint must put the alarm in ALARM, got {described}"
        );
    }

    // Past the end of every one of these alarms' evaluation ranges, every window they search
    // is empty: each row's datapoint was published three seconds short of its range.
    std::thread::sleep(Duration::from_millis(4_600));
    for ((case, _, expected), name) in rows.iter().zip(&names) {
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some("INSUFFICIENT_DATA"),
            "case {case:?}: with no datapoint left in the range the alarm must report \
             INSUFFICIENT_DATA, got {reported}"
        );
        let reason = reported
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            reason, *expected,
            "case {case:?}: expected StateReason {expected:?}, got {reason:?}"
        );
    }
}

#[tokio::test]
async fn an_evaluated_transition_is_recorded_in_the_alarm_history() {
    let router = build_router();
    let alarm = base_alarm("searchable-documents-high");
    ok(&router, "PutMetricAlarm", &alarm).await;
    publish(&router, "searchable-documents-high", &[datum(42.0, 0.0)]).await;
    wait_for_evaluation();

    let described = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["searchable-documents-high"]}),
    )
    .await;
    let reported = &described["MetricAlarms"][0];
    assert_eq!(
        reported.get("StateValue").and_then(Value::as_str),
        Some("ALARM"),
        "the evaluation must move the alarm into ALARM, got {described}"
    );
    let reason = reported
        .get("StateReason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        reason.contains("1 out of the last 1 datapoints"),
        "the state reason must report what the evaluation counted, got {reason:?}"
    );
    let reason_data: Value = serde_json::from_str(
        reported
            .get("StateReasonData")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .expect("StateReasonData is JSON");
    assert_eq!(
        reason_data["evaluatedDatapoints"][0]["value"].as_f64(),
        Some(42.0),
        "the reason data must carry the datapoint that was evaluated, got {reason_data}"
    );

    let history = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({"AlarmName": "searchable-documents-high", "HistoryItemType": "StateUpdate"}),
    )
    .await;
    let items = history["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "the transition must record exactly one StateUpdate item, got {history}"
    );
    let data: Value = serde_json::from_str(
        items[0]
            .get("HistoryData")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .expect("HistoryData is JSON");
    assert_eq!(
        data["oldState"]["stateValue"].as_str(),
        Some("INSUFFICIENT_DATA"),
        "the item must record the state the alarm moved out of, got {data}"
    );
    assert_eq!(
        data["newState"]["stateValue"].as_str(),
        Some("ALARM"),
        "the item must record the state the alarm moved into, got {data}"
    );
}

#[tokio::test]
async fn a_forced_state_holds_until_the_next_evaluation_is_due() {
    let router = build_router();
    // The shortest period an alarm may be evaluated on, so the span a forced state stands for
    // — one evaluation period of the alarm — is the shortest one there is. (`Period`: "Valid
    // values are 10, 20, 30, and any multiple of 60.")
    let alarm = patched("forced", &json!({"Period": SHORTEST_PERIOD_SECS}));
    ok(&router, "PutMetricAlarm", &alarm).await;
    ok(
        &router,
        "SetAlarmState",
        &json!({"AlarmName": "forced", "StateValue": "ALARM", "StateReason": "testing"}),
    )
    .await;

    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["forced"]})).await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("ALARM"),
        "a forced state must be visible to the caller that reads straight after it, got \
         {described}"
    );

    // Past the forced span, the evaluation owns the state again: a datapoint under the
    // threshold puts the alarm in OK. The span is one evaluation period of the alarm.
    std::thread::sleep(Duration::from_millis(10_500));
    publish(&router, "forced", &[datum(1.0, 0.0)]).await;
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["forced"]})).await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("OK"),
        "once the forced span has passed, the evaluation must take the state back, got \
         {described}"
    );
}

#[tokio::test]
async fn ignored_missing_data_maintains_the_state_but_a_datapoint_still_decides_it() {
    let router = build_router();
    // The shortest period an alarm may be evaluated on, so the span a forced state stands for
    // is the shortest one there is and the evaluation then runs with the look-back window
    // holding no datapoint at all.
    let alarm = patched(
        "ignoring",
        &json!({"Period": SHORTEST_PERIOD_SECS, "TreatMissingData": "ignore"}),
    );
    ok(&router, "PutMetricAlarm", &alarm).await;
    ok(
        &router,
        "SetAlarmState",
        &json!({"AlarmName": "ignoring", "StateValue": "ALARM", "StateReason": "testing"}),
    )
    .await;
    // One evaluation period of the alarm, the span the forced state stands for.
    std::thread::sleep(Duration::from_millis(10_500));

    // "ignore — the current alarm state is maintained": with no datapoint to decide on, the
    // evaluation leaves the alarm in the state it has.
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["ignoring"]})).await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("ALARM"),
        "with every datapoint missing and TreatMissingData=ignore, the state must be \
         maintained, got {described}"
    );

    // A datapoint that IS present decides the state, whatever the missing-data setting — at
    // the next evaluation, which the read above re-armed a span away.
    publish(&router, "ignoring", &[datum(1.0, 0.0)]).await;
    wait_for_evaluation();
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["ignoring"]})).await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("OK"),
        "a datapoint under the threshold must decide the state even under \
         TreatMissingData=ignore, got {described}"
    );
}

#[tokio::test]
async fn the_alarms_of_one_metric_are_reported_with_the_state_the_evaluation_computed() {
    // The dimension set the matching alarms watch, and the one series a datapoint is
    // published into.
    let domain = "described-for-metric";
    let dimensions = json!([{"Name": "DomainName", "Value": domain}]);
    let other_dimensions = json!([{"Name": "DomainName", "Value": "another-domain"}]);

    let router = build_router();
    // The alarms this metric carries, each differing in exactly one of the members
    // `DescribeAlarmsForMetric` selects on.
    for (name, patch) in [
        ("average-60", json!({})),
        ("maximum-60", json!({"Statistic": "Maximum"})),
        ("average-300", json!({"Period": 300})),
        ("count-unit", json!({"Unit": "Count"})),
        (
            "p90",
            json!({"Statistic": Value::Null, "ExtendedStatistic": "p90"}),
        ),
        ("another-metric", json!({"MetricName": "SearchLatency"})),
        ("another-dimension-set", json!({"Dimensions": other_dimensions})),
        (
            // "It does not return alarms based on math expressions that use the specified
            // metric" — an alarm carrying a `Metrics` query array is one of those.
            "metric-math",
            json!({
                "Statistic": Value::Null,
                "Namespace": Value::Null,
                "MetricName": Value::Null,
                "Dimensions": Value::Null,
                "Period": Value::Null,
                "Metrics": [{
                    "Id": "m1",
                    "MetricStat": {
                        "Metric": {
                            "Namespace": NAMESPACE,
                            "MetricName": METRIC,
                            "Dimensions": dimensions,
                        },
                        "Period": 60,
                        "Stat": "Average",
                    },
                    "ReturnData": true,
                }],
            }),
        ),
    ] {
        let mut alarm = patched(name, &patch);
        if alarm.get("Dimensions").is_some() && name != "another-dimension-set" {
            alarm["Dimensions"] = dimensions.clone();
        }
        ok(&router, "PutMetricAlarm", &alarm).await;
    }
    // One breaching datapoint of the series the matching alarms watch.
    publish(&router, domain, &[datum(42.0, 0.0)]).await;
    wait_for_evaluation();

    // (case, the query, the alarms it must report, newest first by name)
    let rows: &[(&str, Value, &[&str])] = &[
        (
            "every standard alarm of the metric",
            json!({"Namespace": NAMESPACE, "MetricName": METRIC, "Dimensions": dimensions}),
            &["average-300", "average-60", "count-unit", "maximum-60", "p90"],
        ),
        (
            "filtered by statistic",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Statistic": "Maximum",
            }),
            &["maximum-60"],
        ),
        (
            "filtered by extended statistic",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "ExtendedStatistic": "p90",
            }),
            &["p90"],
        ),
        (
            "filtered by period",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Period": 300,
            }),
            &["average-300"],
        ),
        (
            "filtered by unit",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Unit": "Count",
            }),
            &["count-unit"],
        ),
        (
            "another dimension set names another metric",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": other_dimensions,
            }),
            &["another-dimension-set"],
        ),
        (
            "no dimensions at all names the dimensionless metric",
            json!({"Namespace": NAMESPACE, "MetricName": METRIC}),
            &[],
        ),
        (
            "another metric of the same namespace",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": "SearchLatency",
                "Dimensions": dimensions,
            }),
            &["another-metric"],
        ),
        (
            "a metric no alarm watches",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": "IndexingLatency",
                "Dimensions": dimensions,
            }),
            &[],
        ),
        (
            "a period no alarm of the metric uses",
            json!({
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions,
                "Period": 10,
            }),
            &[],
        ),
    ];

    for (case, query, expected) in rows {
        let reported = ok(&router, "DescribeAlarmsForMetric", query).await;
        let names: Vec<String> = reported["MetricAlarms"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|alarm| {
                alarm
                    .get("AlarmName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names,
            expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?}: expected exactly those alarms, got {reported}"
        );
    }

    // The state this reader reports is the EVALUATED one, the same state `DescribeAlarms`
    // reports: which call an alarm is read through cannot change the state a client is told.
    let for_metric = ok(
        &router,
        "DescribeAlarmsForMetric",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": dimensions,
            "Statistic": "Average",
            "Period": 60,
        }),
    )
    .await;
    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": ["average-60"]})).await;
    assert_eq!(
        for_metric["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("ALARM"),
        "the crossing datapoint must be reported through this reader too, got {for_metric}"
    );
    assert_eq!(
        for_metric["MetricAlarms"][0].get("StateValue"),
        described["MetricAlarms"][0].get("StateValue"),
        "both readers must report one state, got {for_metric} and {described}"
    );
}

#[tokio::test]
async fn a_publish_into_the_reserved_namespace_is_refused_and_stores_nothing() {
    // (case, the namespace published into, whether the publish is accepted)
    //
    // The reserved space is the one `PutMetricDataInput$Namespace` names: "To avoid conflicts
    // with Amazon Web Services service namespaces, you should not specify a namespace that
    // begins with AWS/". The accepted rows are the ones just outside it.
    let rows: &[(&str, &str, bool)] = &[
        ("the namespace of the recorded case", "AWS/ES", false),
        ("a namespace of another AWS service", "AWS/EC2", false),
        ("a namespace of a third AWS service", "AWS/Lambda", false),
        ("the bare reserved prefix", "AWS/", false),
        ("a deeper reserved namespace", "AWS/ApplicationELB/Target", false),
        ("a customer namespace", "MyApplication", true),
        ("the reserved prefix in another case", "aws/es", true),
        ("the reserved prefix without its separator", "AWSMyApp", true),
        ("the reserved prefix inside a namespace", "MyApp/AWS/Metrics", true),
        ("a namespace that only starts with the letter A", "A/WS", true),
    ];

    for (case, namespace, accepted) in rows {
        let router = build_router();
        let request = json!({
            "Namespace": namespace,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [
                    {"Name": "ClientId", "Value": "000000000000"},
                    {"Name": "DomainName", "Value": "obs-opensearch-2"},
                ],
                "Value": 42.0,
            }],
        });
        let (status, body) = call(&router, "PutMetricData", &request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {namespace} must be accepted, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {namespace} must be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected the parameter-value fault, got {body}"
            );
        }

        // Nothing is stored for a refused publish, and exactly the one series for an
        // accepted one: the refusal happens before any datapoint is written.
        let listed = ok(&router, "ListMetrics", &json!({"Namespace": namespace})).await;
        let metrics = listed["Metrics"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            metrics.len(),
            usize::from(*accepted),
            "case {case:?}: expected {} stored series, got {listed}",
            usize::from(*accepted)
        );
    }
}

/// The emulator the end-to-end test drives: CloudWatch, SNS and SQS on ONE engine, because
/// an alarm's action is an SNS topic and the notification is asserted where a client sees
/// it — on the queue subscribed to that topic.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ))
    ;
    Emulator { router }
}

/// The Region every end-to-end call below is signed for.
const REGION: &str = "us-east-1";

/// One `awsJson` RPC call against the service whose signing name is `signing_name` — the
/// credential scope is what routes it.
async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with body {parsed}"
    );
    parsed
}

/// One awsQuery form call against SNS, the protocol that service serves.
async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    body.push(char::from(byte));
                }
                other => body.push_str(&format!("%{other:02X}")),
            }
        }
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/sns/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(status, 200, "{action}: expected HTTP 200, got {status} / {text}");
    text
}

/// The text of the first `<tag>…</tag>` element of an awsQuery response.
fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no <{tag}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("no </{tag}> in {body}"));
    body[start..start + end].to_string()
}

#[tokio::test]
async fn an_alarm_that_crosses_its_threshold_notifies_its_sns_action() {
    let emulator = emulator();

    // A queue subscribed to a topic, the path an alarm notification travels.
    let queue = rpc(
        &emulator,
        "sqs",
        "AmazonSQS",
        "CreateQueue",
        &json!({"QueueName": "alarm-notifications"}),
    )
    .await;
    let queue_url = queue["QueueUrl"]
        .as_str()
        .expect("CreateQueue answers a QueueUrl")
        .to_string();
    let attributes = rpc(
        &emulator,
        "sqs",
        "AmazonSQS",
        "GetQueueAttributes",
        &json!({"QueueUrl": queue_url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .expect("GetQueueAttributes answers the QueueArn")
        .to_string();
    let topic_arn = element(&sns(&emulator, "CreateTopic", &[("Name", "alarms")]).await, "TopicArn");
    sns(
        &emulator,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
        ],
    )
    .await;

    // An alarm on a customer metric, with the topic as its ALARM action.
    let mut alarm = base_alarm("searchable-documents-high");
    alarm["AlarmActions"] = json!([topic_arn]);
    alarm["AlarmDescription"] = json!("the searchable document count crossed its threshold");
    rpc(
        &emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "PutMetricAlarm",
        &alarm,
    )
    .await;

    // The crossing datapoint, published the way the metric's publisher publishes it.
    wait_for_evaluation();
    rpc(
        &emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [datum(4_200.0, 0.0).rendered(now_secs(), "searchable-documents-high")],
        }),
    )
    .await;

    let described = rpc(
        &emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "DescribeAlarms",
        &json!({"AlarmNames": ["searchable-documents-high"]}),
    )
    .await;
    assert_eq!(
        described["MetricAlarms"][0]
            .get("StateValue")
            .and_then(Value::as_str),
        Some("ALARM"),
        "the alarm must reach ALARM once the published datapoint crosses its threshold, got \
         {described}"
    );

    // The notification the transition fired, where the subscriber sees it.
    let received = rpc(
        &emulator,
        "sqs",
        "AmazonSQS",
        "ReceiveMessage",
        &json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        messages.len(),
        1,
        "the alarm's SNS action must deliver exactly one notification to the subscribed \
         queue, got {received}"
    );
    let envelope: Value = serde_json::from_str(
        messages[0]
            .get("Body")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .expect("the delivered body is the SNS notification envelope");
    assert_eq!(
        envelope["TopicArn"].as_str(),
        Some(topic_arn.as_str()),
        "the envelope must name the topic the alarm published to, got {envelope}"
    );
    let notification: Value = serde_json::from_str(
        envelope["Message"]
            .as_str()
            .expect("the envelope carries the published message"),
    )
    .expect("the alarm notification is JSON");
    assert_eq!(
        notification["AlarmName"].as_str(),
        Some("searchable-documents-high"),
        "the notification must name the alarm, got {notification}"
    );
    assert_eq!(
        notification["NewStateValue"].as_str(),
        Some("ALARM"),
        "the notification must carry the state the alarm moved to, got {notification}"
    );
    assert_eq!(
        notification["OldStateValue"].as_str(),
        Some("INSUFFICIENT_DATA"),
        "the notification must carry the state the alarm moved from, got {notification}"
    );
    assert_eq!(
        notification["Trigger"]["MetricName"].as_str(),
        Some(METRIC),
        "the notification must name the metric that was evaluated, got {notification}"
    );

    // And the alarm's own history records that the action ran.
    let history = rpc(
        &emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "DescribeAlarmHistory",
        &json!({"AlarmName": "searchable-documents-high", "HistoryItemType": "Action"}),
    )
    .await;
    let items = history["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "executing the one action must record one Action item, got {history}"
    );
    let summary = items[0]
        .get("HistorySummary")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        summary.contains(&topic_arn) && summary.starts_with("Successfully executed action"),
        "the Action item must name the action that ran, got {summary:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! WHERE the evaluation that reclaims a forced metric-alarm state puts the alarm, and how the
//! history tells a client-forced transition from an evaluated one.
//!
//! The rule: `SetAlarmState` writes a TRANSIENT override — "Temporarily sets the state of an
//! alarm for testing purposes. […] Metric alarms returns to their actual state quickly, often
//! within seconds. Because the metric alarm state change happens quickly, it is typically only
//! visible in the alarm's History tab […] or through DescribeAlarmHistory"
//! (`smithy_query(query="operation",operation="SetAlarmState")`) — so the alarm's own
//! evaluation owns `StateValue` again at its next pass, and the state that pass computes is
//! decided by the datapoints inside the range it grades under the alarm's missing-data
//! treatment: "The valid values are breaching, notBreaching, ignore, and missing. […] If this
//! parameter is omitted, the default behavior of missing is used"
//! (`smithy_query(query="doc",shape="MetricAlarm",member="TreatMissingData")`), of which
//! `ignore` maintains the current alarm state — so under that one arm, and only under it, a
//! forced state is NOT taken back and the caller's own reason text still stands. Every
//! transition, whoever wrote it, is recorded as a `StateUpdate` history item whose `HistoryData`
//! names the state before and after it and carries `stateReasonData` only for a state that has
//! one, which is what lets a client tell a forced state from a derived one.
//!
//! The rows differ along what the rule generalises over: the missing-data arm (every documented
//! value, plus the member left out so the default applies), the state forced (every modelled
//! `StateValue`), and whether the evaluation's own verdict agrees with the forced state. The
//! recorded scenario — a forced `ALARM` reclaimed to `OK`, a never-graded `missing` sibling
//! reclaimed to `INSUFFICIENT_DATA`, an `ignore` sibling left alone — is three rows among them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, described_alarm, merged, now_secs, ok, wait_for_evaluation, AlarmKind};

/// The namespace every alarm in this suite watches.
const NAMESPACE: &str = "Custom/ForcedReclaim";

/// The metric every alarm in this suite watches.
const METRIC: &str = "Faults";

/// The threshold every alarm in this suite compares a datapoint against.
const THRESHOLD: f64 = 10.0;

/// A value below [`THRESHOLD`]: a datapoint that does not breach, so an alarm evaluated on it
/// is in `OK`.
const CLEAR: f64 = 1.0;

/// The period every alarm here is evaluated on: the shortest regular-resolution one of the
/// documented set ("Valid values are 10, 20, 30, and any multiple of 60",
/// `PutMetricAlarmInput$Period`).
const PERIOD: i32 = 60;

/// How far before now a datapoint is stamped to land OUTSIDE the range an alarm of [`PERIOD`]
/// seconds and one evaluation period grades, while staying well inside the two weeks
/// `PutMetricData` accepts ("You can specify time stamps that are as much as two weeks before
/// the current date"). Such a datapoint is still an arrival in the series the alarm watches, so
/// it is what makes the alarm's next evaluation due — with nothing for that evaluation to grade.
const STALE_AGE_SECS: f64 = 3_600.0;

/// The states a caller may force: the modelled `StateValue` members
/// (`smithy_query(query="shape",shape="StateValue")`).
const FORCED_STATES: &[&str] = &["ALARM", "OK", "INSUFFICIENT_DATA"];

/// Every documented `TreatMissingData` value, plus the member left out, and the state an
/// evaluation that could grade NO datapoint computes under each — `None` where the arm computes
/// no state at all, so the alarm keeps the one it has.
///
/// The arms are the documented set
/// (`smithy_query(query="doc",shape="MetricAlarm",member="TreatMissingData")`); the state each
/// one yields for an alarm of one evaluation period and one datapoint to alarm follows from the
/// datapoint the arm substitutes for the hole: a breaching one reaches the `ALARM` minimum, a
/// non-breaching one reaches the `OK` minimum, and the two arms that substitute nothing leave
/// the window short of either.
const ARMS: &[(Option<&str>, Option<&str>)] = &[
    (Some("breaching"), Some("ALARM")),
    (Some("notBreaching"), Some("OK")),
    (Some("missing"), Some("INSUFFICIENT_DATA")),
    // "If this parameter is omitted, the default behavior of missing is used."
    (None, Some("INSUFFICIENT_DATA")),
    // "ignore — the current alarm state is maintained."
    (Some("ignore"), None),
];

/// The alarm this suite forces: a static threshold over one series, with the row's missing-data
/// arm and its own name as the dimension value, so every row watches a series of its own.
fn alarm_request(name: &str, arm: Option<&str>) -> Value {
    let base = json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Row", "Value": name}],
        "Statistic": "Maximum",
        "Period": PERIOD,
        "EvaluationPeriods": 1,
        "DatapointsToAlarm": 1,
        "Threshold": THRESHOLD,
        "ComparisonOperator": "GreaterThanThreshold",
        "ActionsEnabled": false,
    });
    // A row that names no arm leaves the member out, so the documented default applies.
    merged(base, &json!({"TreatMissingData": arm.map_or(Value::Null, Value::from)}))
}

/// Publish one datapoint of the series the alarm `name` watches, stamped `age_secs` before now.
async fn publish(router: &MultiServiceRouter, name: &str, value: f64, age_secs: f64) {
    ok(
        router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Row", "Value": name}],
                "Value": value,
                "Timestamp": now_secs() - age_secs,
            }],
        }),
    )
    .await;
}

/// Force `name` into `state`, with `reason` as the caller's own explanation.
async fn force(router: &MultiServiceRouter, name: &str, state: &str, reason: &str) {
    ok(
        router,
        "SetAlarmState",
        &json!({
            "AlarmName": name,
            "StateValue": state,
            "StateReason": reason,
        }),
    )
    .await;
}

/// The alarm's `StateValue` and `StateReason` as `DescribeAlarms` reports them.
async fn reported(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let alarm = described_alarm(router, AlarmKind::Metric, name)
        .await
        .unwrap_or_else(|| panic!("{name} is a stored metric alarm"));
    (member_text(&alarm, "StateValue"), member_text(&alarm, "StateReason"))
}

/// One string member of a reported alarm, or the empty string where it carries none.
fn member_text(alarm: &Value, member: &str) -> String {
    alarm
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// What the pass that follows a force leaves behind in `StateReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Explanation {
    /// The caller's own text still stands, because nothing re-derived the state.
    TheForcedOne,
    /// The evaluation explained the state it moved the alarm to.
    TheEvaluationsOwn,
    /// Not asserted here: the evaluation computed the state the alarm already reported, so
    /// whether it re-explains an unchanged state is not what this suite pins.
    NotAsserted,
}

#[tokio::test]
async fn the_missing_data_arm_decides_where_a_forced_state_is_reclaimed_to() {
    let router = build_router();

    // One alarm per (state forced, missing-data arm) cell, each over a series of its own, so
    // the whole matrix is driven through one emulator and one clock span.
    let mut cells: Vec<(String, String, &str, String, Explanation)> = Vec::new();
    for forced in FORCED_STATES {
        for (arm, computed) in ARMS {
            let case =
                format!("{forced} forced with TreatMissingData={}", arm.unwrap_or("omitted"));
            let name = format!("row-{forced}-{}", arm.unwrap_or("default"));
            ok(&router, "PutMetricAlarm", &alarm_request(&name, *arm)).await;
            let expected = computed.unwrap_or(forced);
            let explanation = match computed {
                // The arm maintains the state the alarm has, so the force's own reason stands.
                None => Explanation::TheForcedOne,
                // The evaluation moved the alarm, so it explains where it moved it to.
                Some(state) if state != forced => Explanation::TheEvaluationsOwn,
                Some(_) => Explanation::NotAsserted,
            };
            cells.push((case, name, forced, expected.to_string(), explanation));
        }
    }

    // Force every cell, and read each one back: a caller reading inside the span the force
    // stands for sees the state it forced, whatever the arm.
    for (case, name, forced, _, _) in &cells {
        force(&router, name, forced, &forced_reason(name)).await;
        let (straight_after, _) = reported(&router, name).await;
        assert_eq!(
            &straight_after, forced,
            "case {case:?}: the caller reading straight after the force must see the state it \
             forced, got {straight_after:?}"
        );
    }

    // A datapoint of each series arrives, which is what makes that alarm's next evaluation
    // due — stamped outside the range the evaluation grades, so that evaluation has no
    // datapoint of its own and the arm alone decides what it computes.
    for (_, name, _, _, _) in &cells {
        publish(&router, name, CLEAR, STALE_AGE_SECS).await;
    }
    wait_for_evaluation();

    for (case, name, _, expected, explanation) in &cells {
        let (settled, reason) = reported(&router, name).await;
        assert_eq!(
            &settled, expected,
            "case {case:?}: expected {expected} once the evaluation the datapoint made due is \
             owed, got {settled:?} ({reason:?})"
        );
        let forced = forced_reason(name);
        match explanation {
            Explanation::TheForcedOne => assert_eq!(
                reason, forced,
                "case {case:?}: an arm that maintains the state leaves the caller's own reason \
                 in place, got {reason:?}"
            ),
            Explanation::TheEvaluationsOwn => assert!(
                reason != forced && !reason.is_empty(),
                "case {case:?}: the evaluation that moved the alarm must explain the state it \
                 computed rather than report {forced:?}, got {reason:?}"
            ),
            Explanation::NotAsserted => {}
        }
    }
}

/// The caller's own explanation for the force on `name`: text no evaluation would ever produce,
/// so a reason still reading this way is one nothing re-derived.
fn forced_reason(name: &str) -> String {
    format!("forced by the test on {name}")
}

/// Which writer a `StateUpdate` history item came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    /// `SetAlarmState`: the caller's own state and reason, with no decision record.
    TheClient,
    /// The alarm's evaluation: the state its datapoints decide, explained and recorded.
    TheEvaluation,
}

/// One `StateUpdate` item a row expects: the state the item moved the alarm out of, the state
/// it moved it to, and the writer that made it.
type ExpectedItem = (&'static str, &'static str, Writer);

/// One row of the history table: its case, the state it forces, and the items the alarm's
/// history then holds, oldest first.
type HistoryRow = (&'static str, &'static str, &'static [ExpectedItem]);

#[tokio::test]
async fn the_history_tells_a_client_forced_transition_from_an_evaluated_one() {
    // (case, the state forced, the StateUpdate items the alarm's history then holds, oldest
    //  first, as (state moved out of, state moved to, writer))
    let rows: &[HistoryRow] = &[
        (
            "a forced ALARM the evaluation takes back",
            "ALARM",
            &[
                ("INSUFFICIENT_DATA", "OK", Writer::TheEvaluation),
                ("OK", "ALARM", Writer::TheClient),
                ("ALARM", "OK", Writer::TheEvaluation),
            ],
        ),
        (
            "a forced INSUFFICIENT_DATA the evaluation takes back",
            "INSUFFICIENT_DATA",
            &[
                ("INSUFFICIENT_DATA", "OK", Writer::TheEvaluation),
                ("OK", "INSUFFICIENT_DATA", Writer::TheClient),
                ("INSUFFICIENT_DATA", "OK", Writer::TheEvaluation),
            ],
        ),
        (
            // A force into the state the evaluation already put the alarm in is still a write
            // the history records, and the evaluation that follows it computes that same state,
            // so there is nothing further to record.
            "a force into the state the alarm already reports",
            "OK",
            &[
                ("INSUFFICIENT_DATA", "OK", Writer::TheEvaluation),
                ("OK", "OK", Writer::TheClient),
            ],
        ),
    ];

    for (case, forced, expected) in rows {
        let router = build_router();
        let name = "forced-history";
        // The baseline is an EVALUATED state, not the creation state: the series holds a
        // datapoint under the threshold before the alarm is written, so the alarm's first
        // evaluation moves it out of the state `PutMetricAlarm` created it in.
        publish(&router, name, CLEAR, 0.0).await;
        ok(&router, "PutMetricAlarm", &alarm_request(name, Some("missing"))).await;
        wait_for_evaluation();
        let (baseline, baseline_reason) = reported(&router, name).await;
        assert_eq!(
            baseline, "OK",
            "case {case:?}: the published datapoint under the threshold decides the baseline, \
             got {baseline:?} ({baseline_reason:?})"
        );

        let reason = forced_reason(name);
        force(&router, name, forced, &reason).await;
        let (right_after, right_after_reason) = reported(&router, name).await;
        assert_eq!(
            &right_after, forced,
            "case {case:?}: the force takes effect verbatim, got {right_after:?} \
             ({right_after_reason:?})"
        );

        // The metric keeps publishing, with no further control-plane call to the alarm.
        publish(&router, name, CLEAR, 0.0).await;
        wait_for_evaluation();
        let (settled, settled_reason) = reported(&router, name).await;
        assert_eq!(
            settled, "OK",
            "case {case:?}: the datapoint under the threshold decides the state again, got \
             {settled:?} ({settled_reason:?})"
        );

        let items = state_update_history(&router, name).await;
        assert_eq!(
            items.len(),
            expected.len(),
            "case {case:?}: expected {} StateUpdate items, got {items:?}",
            expected.len()
        );
        for ((from, to, writer), item) in expected.iter().zip(&items) {
            let document = history_data(case, item);
            assert_eq!(
                document.pointer("/oldState/stateValue").and_then(Value::as_str),
                Some(*from),
                "case {case:?}: the item records the state it moved out of, got {document}"
            );
            assert_eq!(
                document.pointer("/newState/stateValue").and_then(Value::as_str),
                Some(*to),
                "case {case:?}: the item records the state it moved to, got {document}"
            );
            match writer {
                Writer::TheClient => {
                    assert_eq!(
                        document.pointer("/newState/stateReason").and_then(Value::as_str),
                        Some(reason.as_str()),
                        "case {case:?}: a client-forced item carries the caller's own reason, \
                         got {document}"
                    );
                    // The caller sent no `StateReasonData`, so the forced state has none and
                    // the item records none: this is what tells a forced state from a derived
                    // one, which always carries its decision record.
                    assert!(
                        document.pointer("/newState/stateReasonData").is_none(),
                        "case {case:?}: a client-forced item whose caller sent no reason data \
                         records none, got {document}"
                    );
                }
                Writer::TheEvaluation => assert_evaluated_state(case, &document),
            }
        }
    }
}

/// The `StateUpdate` items of one alarm's history, oldest first.
async fn state_update_history(router: &MultiServiceRouter, name: &str) -> Vec<Value> {
    let described = ok(
        router,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": name,
            "HistoryItemType": "StateUpdate",
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    described
        .get("AlarmHistoryItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `HistoryData` of one item, parsed.
fn history_data(case: &str, item: &Value) -> Value {
    let raw = item
        .get("HistoryData")
        .and_then(Value::as_str)
        .unwrap_or_default();
    serde_json::from_str::<Value>(raw)
        .unwrap_or_else(|_| panic!("case {case:?}: every item carries a JSON HistoryData, got {item}"))
}

/// Assert the `newState` of `document` is one the EVALUATION wrote: it explains the state over
/// the datapoints it graded, and it carries the decision record a derived state always has.
fn assert_evaluated_state(case: &str, document: &Value) {
    let reason = document
        .pointer("/newState/stateReason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for expected in [
        // The sentence an evaluated state is explained with, the count of datapoints that read
        // the way the state's own direction needs, over the alarm's evaluation periods, and the
        // threshold they were compared against.
        "Threshold Crossed",
        "1 out of the last 1 datapoints",
        &format!("({THRESHOLD:.1})"),
    ] {
        assert!(
            reason.contains(expected),
            "case {case:?}: an evaluated reason must report {expected:?}, got {reason:?}"
        );
    }
    let record = document
        .pointer("/newState/stateReasonData")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        record.get("threshold").and_then(Value::as_f64),
        Some(THRESHOLD),
        "case {case:?}: the decision record names the threshold it compared against, got {record}"
    );
    assert_eq!(
        record.get("recentDatapoints"),
        Some(&json!([CLEAR])),
        "case {case:?}: the decision record names the datapoints the evaluation had, got {record}"
    );
    assert_eq!(
        record.pointer("/evaluatedDatapoints/0/value").and_then(Value::as_f64),
        Some(CLEAR),
        "case {case:?}: the decision record names the datapoint the verdict rests on, got \
         {record}"
    );
}

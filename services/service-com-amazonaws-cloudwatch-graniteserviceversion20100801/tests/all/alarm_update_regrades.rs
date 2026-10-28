// SPDX-License-Identifier: CC0-1.0

//! An IN-PLACE configuration update of a metric alarm, end to end over the wire.
//!
//! ONE rule, asserted over a table: re-writing an existing alarm name is an update of that one
//! alarm — "When you update an existing alarm, its state is left unchanged, but the update
//! completely overwrites the previous configuration of the alarm"
//! (`smithy_query(query="doc",shape="PutMetricAlarm")`) — and the write hands the alarm's state
//! back to its own evaluation, which re-grades the series ALREADY published under the NEW
//! configuration: "The alarm is then evaluated and its state is set appropriately"
//! (`smithy_query(query="doc",shape="PutMetricAlarm")`), restated for a state a caller forced as
//! "It is also reevaluated if you update its configuration"
//! (`smithy_query(query="doc",shape="SetAlarmState")`). So an alarm repaired in place resolves
//! without anything being published again, and a value arriving afterwards is graded by the
//! configuration the repair left behind.
//!
//! The rows differ along WHICH member of the configuration the update changes — the comparison,
//! the threshold, the statistic, the "M out of N" counts, and a re-write that changes nothing —
//! and therefore along the direction the re-grade goes: back to `OK`, on to a state too few
//! datapoints can decide, or nowhere at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::alarm_evaluation::{datum, patched, publish, Datum};
use crate::harness::{build_router, ok, wait_for_evaluation};

/// The values every row publishes into its own series, inside one window: their `Average` is
/// past the base alarm's threshold, their `Minimum` is under it and their `Maximum` past it, so
/// which statistic the update names decides the direction the re-grade goes.
fn published() -> Vec<Datum> {
    vec![datum(42.0, 0.0), datum(1.0, 0.0)]
}

/// The state every row's alarm is in BEFORE the update: the base alarm compares the 60-second
/// `Average` of [`published`] (21.5) against a threshold of 10 with `GreaterThanThreshold`.
const BEFORE: &str = "ALARM";

/// The state and the reason an alarm reports, as `DescribeAlarms` answers them, plus how many
/// alarms that name matched.
async fn reported(router: &MultiServiceRouter, name: &str) -> (usize, String, String) {
    let described = ok(router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    let alarms = described["MetricAlarms"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let first = alarms.first().cloned().unwrap_or(Value::Null);
    (
        alarms.len(),
        first
            .get("StateValue")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        first
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// One row of the table.
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The members the SECOND write changes on the base alarm.
    patch: Value,
    /// The state the evaluation the update made due must put the alarm in.
    after: &'static str,
}

/// One row of the table, in the order it reads.
fn row(case: &'static str, patch: Value, after: &'static str) -> Row {
    Row { case, patch, after }
}

#[tokio::test]
async fn an_in_place_update_regrades_the_series_already_published() {
    let rows: Vec<Row> = vec![
        row(
            // The recorded case: the comparison is the only member the repair changes.
            "only the comparison operator changes",
            json!({"ComparisonOperator": "LessThanThreshold"}),
            "OK",
        ),
        row(
            "only the threshold changes",
            json!({"Threshold": 100.0}),
            "OK",
        ),
        row(
            "only the statistic changes, to one the window folds under the threshold",
            json!({"Statistic": "Minimum"}),
            "OK",
        ),
        row(
            "only the statistic changes, to one the window still folds past the threshold",
            json!({"Statistic": "Maximum"}),
            "ALARM",
        ),
        row(
            // Two breaching datapoints are needed where the series has one real one, and the
            // default missing-data treatment substitutes none, so neither direction's minimum
            // is reached.
            "the M out of N counts change",
            json!({"EvaluationPeriods": 2, "DatapointsToAlarm": 2}),
            "INSUFFICIENT_DATA",
        ),
        row(
            "the re-write changes nothing",
            json!({}),
            "ALARM",
        ),
    ];

    // Every row's alarm watches its own series (its `DomainName` dimension is its own name), so
    // all of them share one emulator and each evaluation delay is waited out once.
    let router = build_router();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, _) in rows.iter().enumerate() {
        let name = format!("repaired-{index}");
        ok(&router, "PutMetricAlarm", &patched(&name, &json!({}))).await;
        publish(&router, &name, &published()).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        let (matched, state, reason) = reported(&router, name).await;
        assert_eq!(
            (matched, state.as_str()),
            (1, BEFORE),
            "case {case:?}: the published series must put the one alarm in {BEFORE} before the \
             update, got {matched} alarm(s) in {state:?} ({reason:?})"
        );
    }

    // The update itself: one write per row, naming the alarm that already exists.
    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        ok(&router, "PutMetricAlarm", &patched(name, &row.patch)).await;
        // "When you update an existing alarm, its state is left unchanged": the write itself
        // moves nothing, it hands the state back to the evaluation.
        let (matched, state, reason) = reported(&router, name).await;
        assert_eq!(
            (matched, state.as_str()),
            (1, BEFORE),
            "case {case:?}: the update must leave one alarm in the state it found, got \
             {matched} alarm(s) in {state:?} ({reason:?})"
        );
    }
    wait_for_evaluation();

    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        let after = row.after;
        let (matched, state, reason) = reported(&router, name).await;
        assert_eq!(
            (matched, state.as_str()),
            (1, after),
            "case {case:?}: the evaluation the update made due must re-grade the published \
             series to {after}, got {matched} alarm(s) in {state:?} ({reason:?})"
        );
        // The configuration the update wrote is the one the alarm now carries, and the update
        // is recorded beside the creation rather than replacing it.
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let alarm = &described["MetricAlarms"][0];
        for (member, value) in row.patch.as_object().cloned().unwrap_or_default() {
            assert_eq!(
                alarm.get(&member).and_then(Value::as_f64),
                value.as_f64(),
                "case {case:?}: expected {member} to read back as {value}, got {alarm}"
            );
            assert_eq!(
                alarm.get(&member).and_then(Value::as_str),
                value.as_str(),
                "case {case:?}: expected {member} to read back as {value}, got {alarm}"
            );
        }
        let history = ok(
            &router,
            "DescribeAlarmHistory",
            &json!({"AlarmName": name, "HistoryItemType": "ConfigurationUpdate"}),
        )
        .await;
        assert_eq!(
            history["AlarmHistoryItems"].as_array().map(Vec::len),
            Some(2),
            "case {case:?}: the creation and the update are two configuration items, got \
             {history}"
        );
    }
}

#[tokio::test]
async fn a_value_arriving_after_the_update_is_graded_by_the_updated_configuration() {
    // The recorded repair loop, end to end: an alarm that fired on the series it watches, its
    // comparison fixed in place, and then one more value — which the UPDATED comparison grades.
    let router = build_router();
    let name = "repaired-then-breached";
    ok(&router, "PutMetricAlarm", &patched(name, &json!({}))).await;
    publish(&router, name, &published()).await;
    wait_for_evaluation();
    let (_, state, reason) = reported(&router, name).await;
    assert_eq!(
        state, BEFORE,
        "the published series must fire the alarm first, got {state:?} ({reason:?})"
    );

    ok(
        &router,
        "PutMetricAlarm",
        &patched(name, &json!({"ComparisonOperator": "LessThanThreshold"})),
    )
    .await;
    wait_for_evaluation();
    let (_, state, reason) = reported(&router, name).await;
    assert_eq!(
        state, "OK",
        "the repair must resolve the alarm with nothing published since, got {state:?} \
         ({reason:?})"
    );

    // One value that drags the window's average under the threshold: the repaired comparison
    // breaches on it, which the alarm the repair left behind is the one to report.
    publish(&router, name, &[datum(-1_000.0, 0.0)]).await;
    wait_for_evaluation();
    let (_, state, reason) = reported(&router, name).await;
    assert_eq!(
        state, "ALARM",
        "a value under the threshold must fire the repaired alarm, got {state:?} ({reason:?})"
    );
    assert!(
        reason.contains("was less than the threshold"),
        "the reason must report the comparison the repair left behind, got {reason:?}"
    );
}

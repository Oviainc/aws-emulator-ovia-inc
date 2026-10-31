// SPDX-License-Identifier: CC0-1.0

//! How long a state `SetAlarmState` FORCES on a metric alarm stands, and what takes it back.
//!
//! The rule: a forced state stands for its own span, whatever the series the alarm watches
//! holds. An alarm is evaluated from the datapoints of the metric it watches — "the length, in
//! seconds, used each time the metric specified in `MetricName` is evaluated"
//! (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`) — and what takes
//! the forced state back is the evaluation at the end of that span ("Metric alarms returns to
//! their actual state quickly, often within seconds",
//! `smithy_query(query="operation",operation="SetAlarmState")`), which grades whatever the
//! series holds by then: a datapoint arriving meanwhile does not cut the span short, because a
//! forced state nothing could read would be no force at all.
//!
//! The rows differ along what the rule generalises over: the alarm's `Period`, whether the
//! metric the alarm watches holds a datapoint at all, whether that datapoint is inside the
//! range the evaluation grades, whether it arrived before or after the force, and which state
//! was forced — and where the state lands once the span has passed. The recorded scenarios are
//! two rows among them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router, graded_window_stamp, ok, wait_for_evaluation, wait_for_forced_state_reclaim,
};

/// The namespace every alarm in this suite watches.
const NAMESPACE: &str = "Custom/ForcedLifetime";

/// The metric every alarm in this suite watches.
const METRIC: &str = "Errors";

/// The threshold every alarm in this suite compares against.
const THRESHOLD: f64 = 10.0;

/// A value below [`THRESHOLD`]: a datapoint that does not breach, so an alarm evaluated on it
/// is in `OK`.
const CLEAR: f64 = 1.0;

/// How far before the newest window an evaluation grades a datapoint is stamped to fall OUTSIDE
/// the range an alarm of [`SHORT_PERIOD`] seconds and one evaluation period grades, and well
/// inside the two weeks `PutMetricData` accepts ("You can specify time stamps that are as much as
/// two weeks before the current date").
const STALE_AGE_SECS: f64 = 3_600.0;

/// The shortest `Period` an alarm may be evaluated on: "Valid values are 10, 20, 30, and any
/// multiple of 60." (`PutMetricAlarmInput$Period`)
const SHORTEST_PERIOD: i32 = 10;

/// The shortest regular-resolution period, the one most alarms are written with.
const SHORT_PERIOD: i32 = 60;

/// A long period: `Period` x `EvaluationPeriods` may reach one day for a sub-hour period, so a
/// five-minute period is an ordinary configuration rather than an extreme one.
const LONG_PERIOD: i32 = 300;

/// The alarm this suite forces: a static threshold over one series, with the row's `Period`
/// and its own name as the dimension value, so each row watches a series of its own.
fn alarm_request(name: &str, period: i32) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Row", "Value": name}],
        "Statistic": "Maximum",
        "Period": period,
        "EvaluationPeriods": 1,
        "DatapointsToAlarm": 1,
        "Threshold": THRESHOLD,
        "ComparisonOperator": "GreaterThanThreshold",
        "ActionsEnabled": false,
        "TreatMissingData": "missing",
    })
}

/// Publish one datapoint of the series `name`'s alarm watches, stamped `age_secs` before the
/// newest `period`-second window that alarm's evaluation grades.
///
/// An evaluation grades the periods that have CLOSED, so a datapoint meant to be graded is
/// stamped inside the newest closed one ([`graded_window_stamp`]) rather than inside the period
/// the clock is in.
async fn publish(
    router: &MultiServiceRouter,
    name: &str,
    period: i32,
    value: f64,
    age_secs: f64,
) {
    ok(
        router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Row", "Value": name}],
                "Value": value,
                "Timestamp": graded_window_stamp(f64::from(period)) - age_secs,
            }],
        }),
    )
    .await;
}

/// Force `name` into `state`.
async fn force(router: &MultiServiceRouter, name: &str, state: &str) {
    ok(
        router,
        "SetAlarmState",
        &json!({
            "AlarmName": name,
            "StateValue": state,
            "StateReason": "driven by the test",
        }),
    )
    .await;
}

/// The alarm's `StateValue` and `StateReason` as `DescribeAlarms` reports them.
async fn reported(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let described = ok(router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    let alarm = &described["MetricAlarms"][0];
    (
        alarm
            .get("StateValue")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        alarm
            .get("StateReason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// What a row publishes into the series its alarm watches, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Data {
    /// Nothing at all: a metric no caller writes.
    None,
    /// A datapoint inside the range the evaluation grades, stored BEFORE the force.
    FreshBeforeTheForce,
    /// A datapoint inside that range, stored AFTER the force.
    FreshAfterTheForce,
    /// A datapoint of the series, stamped far enough back to be outside the range the
    /// evaluation grades, stored after the force.
    StaleAfterTheForce,
}

#[tokio::test]
async fn a_forced_state_stands_until_the_alarms_next_evaluation() {
    // (case, Period, what the series holds, the state forced, the state the alarm reports once
    //  the forced span has passed — the one the evaluation computes over whatever the series
    //  holds by then)
    let rows: &[(&str, i32, Data, &str, &str)] = &[
        // No datapoint to grade: the evaluation at the end of the span has nothing to decide
        // on, so it reports insufficient data — whatever the period, and whichever state was
        // forced.
        (
            "ALARM forced on an alarm over a metric nothing writes",
            LONG_PERIOD,
            Data::None,
            "ALARM",
            "INSUFFICIENT_DATA",
        ),
        (
            "OK forced on an alarm over a metric nothing writes",
            SHORT_PERIOD,
            Data::None,
            "OK",
            "INSUFFICIENT_DATA",
        ),
        (
            "INSUFFICIENT_DATA forced on the shortest period",
            SHORTEST_PERIOD,
            Data::None,
            "INSUFFICIENT_DATA",
            "INSUFFICIENT_DATA",
        ),
        // A datapoint the evaluation can grade: the state the alarm lands in once the forced
        // span has passed is the one that datapoint decides, whichever side of the force it
        // arrived on.
        (
            "ALARM forced on a metric already publishing",
            SHORT_PERIOD,
            Data::FreshBeforeTheForce,
            "ALARM",
            "OK",
        ),
        (
            "ALARM forced, then the metric publishes",
            SHORT_PERIOD,
            Data::FreshAfterTheForce,
            "ALARM",
            "OK",
        ),
        (
            "ALARM forced on a long period, then the metric publishes",
            LONG_PERIOD,
            Data::FreshAfterTheForce,
            "ALARM",
            "OK",
        ),
        // A datapoint of the series arriving outside the range the evaluation grades is no
        // data for that evaluation to grade either — the state it computes is the data-less
        // one.
        (
            "a stale datapoint of the series is no data the evaluation grades",
            SHORT_PERIOD,
            Data::StaleAfterTheForce,
            "ALARM",
            "INSUFFICIENT_DATA",
        ),
    ];

    // One emulator for the whole table, so the rows are set up together and ONE clock span
    // covers every reclaim: each row watches a series of its own and forces an alarm of its
    // own, so nothing of one row reaches another.
    let router = build_router();
    for (case, period, data, state, _) in rows {
        let name = format!("row-{}", case.replace(' ', "-"));
        ok(&router, "PutMetricAlarm", &alarm_request(&name, *period)).await;
        if *data == Data::FreshBeforeTheForce {
            publish(&router, &name, *period, CLEAR, 0.0).await;
        }

        force(&router, &name, state).await;
        let (straight_after, _) = reported(&router, &name).await;
        assert_eq!(
            &straight_after, state,
            "case {case:?}: the caller reading straight after the force must see the state it \
             forced, got {straight_after:?}"
        );

        match data {
            Data::FreshAfterTheForce => publish(&router, &name, *period, CLEAR, 0.0).await,
            Data::StaleAfterTheForce => {
                publish(&router, &name, *period, CLEAR, STALE_AGE_SECS).await;
            }
            Data::None | Data::FreshBeforeTheForce => {}
        }
    }

    // Inside the span the force stands for, every row still reports the state it forced: a
    // datapoint arriving meanwhile does not cut it short.
    wait_for_evaluation();
    for (case, _, _, state, _) in rows {
        let name = format!("row-{}", case.replace(' ', "-"));
        let (inside_the_span, reason) = reported(&router, &name).await;
        assert_eq!(
            &inside_the_span, state,
            "case {case:?}: the forced state must stand for its whole span, got \
             {inside_the_span:?} ({reason:?})"
        );
    }

    // Past it, the evaluation owns the state again: with one datapoint under the threshold
    // inside the range it grades it is OK, and with none INSUFFICIENT_DATA.
    wait_for_forced_state_reclaim();
    for (case, _, _, _, settled_state) in rows {
        let name = format!("row-{}", case.replace(' ', "-"));
        let (settled, reason) = reported(&router, &name).await;
        assert_eq!(
            &settled, settled_state,
            "case {case:?}: the evaluation at the end of the forced span must report the state \
             the series decides, got {settled:?} ({reason:?})"
        );
    }
}

/// The recorded shape of cause 2: a forced ALARM on an alarm whose metric then publishes one
/// datapoint under the threshold is reclaimed, and the reason reports the reclaimed state's own
/// direction over the datapoints the evaluation graded.
#[tokio::test]
async fn the_reclaiming_evaluation_reports_the_datapoints_it_graded() {
    let router = build_router();
    let name = "reclaimed";
    ok(&router, "PutMetricAlarm", &alarm_request(name, SHORT_PERIOD)).await;
    force(&router, name, "ALARM").await;
    publish(&router, name, SHORT_PERIOD, CLEAR, 0.0).await;
    wait_for_forced_state_reclaim();

    let (state, reason) = reported(&router, name).await;
    assert_eq!(
        state, "OK",
        "the evaluation must take the forced ALARM back to the state the datapoint decides, \
         got {state:?} ({reason:?})"
    );
    for expected in [
        // The numerator is the number of datapoints satisfying the direction REPORTED, over
        // `EvaluationPeriods`.
        "1 out of the last 1 datapoints",
        // The datapoint the evaluation graded, and the threshold it was compared against, both
        // taken from the store and the request.
        &format!("[{CLEAR:.1} ("),
        &format!("({THRESHOLD:.1})"),
    ] {
        assert!(
            reason.contains(expected),
            "the reclaiming reason must report {expected:?}, got {reason:?}"
        );
    }
}

/// A state forced on an alarm the next write RECONFIGURES is taken back by that alarm's own
/// next evaluation, which the configuration write makes due: "the alarm is then evaluated and
/// its state is set appropriately" (`PutMetricAlarm`), and "It is also reevaluated if you
/// update its configuration" (`SetAlarmState`, of a composite alarm).
///
/// The rows differ along WHICH configuration write is made — the three operations that write an
/// alarm's configuration — and, for the action switch, on which setting it asks for.
#[tokio::test]
async fn a_configuration_write_hands_the_forced_state_back_to_the_evaluation() {
    // (case, the operation that reconfigures the alarm, its request, the state a clock span
    //  later: the evaluated one when the write really changed the configuration, and the
    //  forced one when the write changed nothing)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "PutMetricAlarm with a different threshold",
            "PutMetricAlarm",
            {
                let mut replacement = alarm_request("reconfigured", LONG_PERIOD);
                replacement["Threshold"] = json!(THRESHOLD * 2.0);
                replacement["ActionsEnabled"] = json!(true);
                replacement
            },
            "INSUFFICIENT_DATA",
        ),
        (
            "DisableAlarmActions, which writes ActionsEnabled",
            "DisableAlarmActions",
            json!({"AlarmNames": ["reconfigured"]}),
            "INSUFFICIENT_DATA",
        ),
        (
            // "Asking for the setting an alarm already has changes nothing": no configuration
            // was written, so nothing handed the state back either, and the force stands for
            // its own span.
            "EnableAlarmActions on an alarm whose actions are already on",
            "EnableAlarmActions",
            json!({"AlarmNames": ["reconfigured"]}),
            "ALARM",
        ),
    ];

    for (case, operation, request, expected) in &rows {
        let router = build_router();
        let name = "reconfigured";
        // The alarm's actions start ON, so one row's write really changes the member and
        // another's does not. Its metric is one no caller publishes, so the only thing that
        // can take the forced state back inside the span asserted here is the configuration
        // write itself.
        let mut configured = alarm_request(name, LONG_PERIOD);
        configured["ActionsEnabled"] = json!(true);
        ok(&router, "PutMetricAlarm", &configured).await;
        force(&router, name, "ALARM").await;

        ok(&router, operation, request).await;
        let (carried, _) = reported(&router, name).await;
        assert_eq!(
            carried, "ALARM",
            "case {case:?}: the configuration write must leave the state it found, got \
             {carried:?}"
        );

        wait_for_evaluation();
        let (settled, reason) = reported(&router, name).await;
        assert_eq!(
            &settled, expected,
            "case {case:?}: expected {expected} a clock span after the write, got {settled:?} \
             ({reason:?})"
        );
    }
}

/// The datapoints an evaluation grades are the ones the alarm's `Unit` selects: "If you specify
/// a unit, the operation returns only data that was collected with that unit specified." So a
/// series published under another unit is no datapoint for THIS alarm, and the evaluation that
/// takes the forced state back at the end of its span reports insufficient data rather than the
/// state that datapoint would decide.
#[tokio::test]
async fn a_datapoint_of_another_unit_is_not_data_this_alarm_grades() {
    // (case, the unit the alarm selects, the unit the datapoint carries, whether it is
    //  published before the force, whether it is a datapoint this alarm GRADES)
    let rows: &[(&str, &str, &str, bool, bool)] = &[
        ("the unit the alarm selects, before the force", "Count", "Count", true, true),
        ("the unit the alarm selects, after the force", "Count", "Count", false, true),
        ("another unit, before the force", "Count", "Milliseconds", true, false),
        ("another unit, after the force", "Count", "Milliseconds", false, false),
        ("no unit on the datapoint", "Count", "", true, false),
        // An alarm naming no unit grades the series' own unit.
        ("an alarm naming no unit", "", "Milliseconds", false, true),
    ];

    // One emulator for the whole table, so ONE clock span covers every row's reclaim: each row
    // forces an alarm of its own over a series of its own.
    let router = build_router();
    for (index, (case, selected, published, before_the_force, _)) in rows.iter().enumerate() {
        let name = format!("unit-selected-{index}");
        let mut configured = alarm_request(&name, LONG_PERIOD);
        if !selected.is_empty() {
            configured["Unit"] = json!(selected);
        }
        ok(&router, "PutMetricAlarm", &configured).await;

        let mut datum = json!({
            "MetricName": METRIC,
            "Dimensions": [{"Name": "Row", "Value": name}],
            "Value": CLEAR,
            "Timestamp": graded_window_stamp(f64::from(LONG_PERIOD)),
        });
        if !published.is_empty() {
            datum["Unit"] = json!(published);
        }
        let write = json!({"Namespace": NAMESPACE, "MetricData": [datum]});
        if *before_the_force {
            ok(&router, "PutMetricData", &write).await;
        }
        force(&router, &name, "ALARM").await;
        if !*before_the_force {
            ok(&router, "PutMetricData", &write).await;
        }
        let (straight_after, _) = reported(&router, &name).await;
        assert_eq!(
            straight_after, "ALARM",
            "case {case:?}: the caller reading straight after the force must see the state it \
             forced, got {straight_after:?}"
        );
    }

    wait_for_forced_state_reclaim();
    for (index, (case, _, _, _, graded)) in rows.iter().enumerate() {
        let name = format!("unit-selected-{index}");
        let (settled, reason) = reported(&router, &name).await;
        // The evaluation at the end of the forced span grades what the alarm's own unit
        // selects: the datapoint under the threshold where it selects it, and nothing at all
        // where it does not.
        let expected = if *graded { "OK" } else { "INSUFFICIENT_DATA" };
        assert_eq!(
            settled, expected,
            "case {case:?}: expected {expected} once the forced span has passed, got \
             {settled:?} ({reason:?})"
        );
    }
}

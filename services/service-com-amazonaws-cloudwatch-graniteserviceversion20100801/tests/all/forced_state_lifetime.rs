// SPDX-License-Identifier: CC0-1.0

//! How long a state `SetAlarmState` FORCES on a metric alarm stands, and what takes it back.
//!
//! The rule: a forced state stands until the alarm's next evaluation. An alarm is evaluated
//! from the datapoints of the metric it watches — "the length, in seconds, used each time the
//! metric specified in `MetricName` is evaluated"
//! (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`) — so a metric that
//! is being published is evaluated again as soon as its next datapoint lands ("Metric alarms
//! returns to their actual state quickly, often within seconds",
//! `smithy_query(query="operation",operation="SetAlarmState")`), while an alarm over a metric
//! nothing writes keeps the forced state until its next SCHEDULED pass, one `Period` away.
//!
//! The rows differ along what the rule generalises over: the alarm's `Period` (the span
//! itself), whether the metric the alarm watches holds a datapoint at all, whether that
//! datapoint is inside the range the evaluation grades, whether it arrived before or after the
//! force, and which state was forced. The recorded scenarios are two rows among them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, graded_window_stamp, ok, wait_for_evaluation};

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
    // (case, Period, what the series holds, the state forced, whether the evaluation has taken
    //  the state back once every evaluation started before now is due)
    let rows: &[(&str, i32, Data, &str, bool)] = &[
        // No datapoint to grade: nothing triggers an evaluation before the alarm's own next
        // scheduled pass, which is a whole period away — so the forced state is still what a
        // caller reads a clock span later, whatever the period.
        (
            "ALARM forced on an alarm over a metric nothing writes",
            LONG_PERIOD,
            Data::None,
            "ALARM",
            false,
        ),
        (
            "OK forced on an alarm over a metric nothing writes",
            SHORT_PERIOD,
            Data::None,
            "OK",
            false,
        ),
        (
            "INSUFFICIENT_DATA forced on the shortest period",
            SHORTEST_PERIOD,
            Data::None,
            "INSUFFICIENT_DATA",
            false,
        ),
        // A datapoint the evaluation can grade: the alarm is evaluated again a clock span
        // later, whichever side of the force the datapoint arrived on.
        (
            "ALARM forced on a metric already publishing",
            SHORT_PERIOD,
            Data::FreshBeforeTheForce,
            "ALARM",
            true,
        ),
        (
            "ALARM forced, then the metric publishes",
            SHORT_PERIOD,
            Data::FreshAfterTheForce,
            "ALARM",
            true,
        ),
        (
            "ALARM forced on a long period, then the metric publishes",
            LONG_PERIOD,
            Data::FreshAfterTheForce,
            "ALARM",
            true,
        ),
        // A datapoint of the series arriving outside the range the evaluation grades still
        // triggers that evaluation — the arrival is what the evaluation is owed from — and the
        // state it computes is the data-less one.
        (
            "a stale datapoint of the series still triggers the evaluation",
            SHORT_PERIOD,
            Data::StaleAfterTheForce,
            "ALARM",
            true,
        ),
    ];

    for (case, period, data, state, reclaimed) in rows {
        let router = build_router();
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

        wait_for_evaluation();
        let (settled, reason) = reported(&router, &name).await;
        if *reclaimed {
            // The evaluation owns the state again. With one datapoint under the threshold it
            // is OK; with none inside the range it graded, INSUFFICIENT_DATA.
            let expected = if *data == Data::StaleAfterTheForce {
                "INSUFFICIENT_DATA"
            } else {
                "OK"
            };
            assert_eq!(
                settled, expected,
                "case {case:?}: the evaluation the datapoint made due must take the forced \
                 state back, got {settled:?} ({reason:?})"
            );
        } else {
            assert_eq!(
                &settled, state,
                "case {case:?}: with no datapoint to grade the forced state must stand until \
                 the alarm's next scheduled pass, got {settled:?} ({reason:?})"
            );
        }
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
    wait_for_evaluation();

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

/// The datapoints that decide whether the metric is being published are the ones the alarm's
/// own evaluation would grade: "If you specify a unit, the operation returns only data that
/// was collected with that unit specified." So a series published under another unit is no
/// datapoint for THIS alarm — neither when it was already there before the force nor when it
/// arrives after it — and the state it forced stands.
#[tokio::test]
async fn a_datapoint_of_another_unit_is_not_data_this_alarm_grades() {
    // (case, the unit the alarm selects, the unit the datapoint carries, whether it is
    //  published before the force, whether the evaluation has taken the forced state back a
    //  clock span later)
    let rows: &[(&str, &str, &str, bool, bool)] = &[
        ("the unit the alarm selects, before the force", "Count", "Count", true, true),
        ("the unit the alarm selects, after the force", "Count", "Count", false, true),
        ("another unit, before the force", "Count", "Milliseconds", true, false),
        ("another unit, after the force", "Count", "Milliseconds", false, false),
        ("no unit on the datapoint", "Count", "", true, false),
        // An alarm naming no unit grades every datapoint of the series it watches.
        ("an alarm naming no unit", "", "Milliseconds", false, true),
    ];

    for (case, selected, published, before_the_force, reclaimed) in rows {
        let router = build_router();
        let name = "unit-selected";
        let mut configured = alarm_request(name, LONG_PERIOD);
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
        force(&router, name, "ALARM").await;
        if !*before_the_force {
            ok(&router, "PutMetricData", &write).await;
        }
        wait_for_evaluation();

        let (settled, reason) = reported(&router, name).await;
        let expected = if *reclaimed { "OK" } else { "ALARM" };
        assert_eq!(
            settled, expected,
            "case {case:?}: expected {expected} a clock span after the force, got {settled:?} \
             ({reason:?})"
        );
    }
}

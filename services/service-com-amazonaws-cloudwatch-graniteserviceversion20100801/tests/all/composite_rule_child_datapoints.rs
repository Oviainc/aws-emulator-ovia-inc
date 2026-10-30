// SPDX-License-Identifier: CC0-1.0

//! The same rule as [`crate::composite_rule_over_published_children`] — a composite alarm's
//! state is its `AlarmRule` over the current states of the alarms it references, re-derived
//! whenever one of those alarms changes state — driven the way a CLIENT drives it: datapoints
//! published with no time stamp of their own, so each lands in the period window the clock is
//! in and is graded once that window is inside its alarm's evaluation range, and the whole
//! ALARM/OK cycle of the rule is reached without the publisher placing anything.
//!
//! What this adds to the table suite is that no step of the cycle depends on where the
//! publisher put its data: the sibling suite places each datapoint in a window the evaluation
//! already grades, which makes the rows fast and the windows exact; here the clock has to carry
//! each child's transition, and the parent follows its children whenever their own evaluation
//! moves them. That is how the recorded scenario reaches the rule, and it is the only way the
//! wait between a publish and the parent's transition is exercised at all.

use serde_json::json;

use crate::composite_rule_over_published_children::{
    alarm_of, assert_transition, await_state, awaited_notifications, child_alarm, cw,
    given_topic_with_subscribed_queue, Child, ALARM, BREACHING, CLEARING, COMPOSITE,
    INSUFFICIENT_DATA, METRIC, NAMESPACE, OK,
};
use crate::harness::{emulator, Emulator};

/// The children's `Period`, in seconds: the shortest the documentation allows, which is also
/// the recorded one — "Specifying 10, 20, or 30 also sets this alarm as a high-resolution
/// alarm" (`PutMetricAlarmInput$Period`).
const PERIOD_SECS: f64 = 10.0;

/// The rule under test: both children have to be in ALARM.
fn rule() -> String {
    format!(
        "ALARM(\"{}\") AND ALARM(\"{}\")",
        Child::A.name(),
        Child::B.name()
    )
}

/// Publish `value` for `child` with no time stamp, the way a client publishes what is happening
/// now: "If you omit Timestamp, CloudWatch uses the time the data was received."
/// (`smithy_query(query="doc",shape="MetricDatum",member="Timestamp")`) The resolution is the
/// one a sub-minute period needs, so the datapoint keeps the instant it was published at.
async fn publish(emulator: &Emulator, child: Child, value: f64) {
    cw(
        emulator,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Child", "Value": child.name()}],
                "Value": value,
                "StorageResolution": 1,
            }],
        }),
    )
    .await;
}

/// The runtime flavour the booted emulator serves on, so the evaluation pass and the alarm
/// clock run the way they do in production rather than inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conjunction_completes_its_cycle_over_datapoints_published_as_they_happen() {
    let case = "two children published as they happen";
    let reads = [Child::A, Child::B];
    let emulator = emulator();
    let (topic, url) = given_topic_with_subscribed_queue(&emulator, "composite-live").await;

    for child in reads {
        cw(
            &emulator,
            "PutMetricAlarm",
            &child_alarm(PERIOD_SECS, child.name()),
        )
        .await;
    }
    let rule = rule();
    cw(
        &emulator,
        "PutCompositeAlarm",
        &json!({
            "AlarmName": COMPOSITE,
            "AlarmRule": rule,
            "AlarmActions": [topic.clone()],
            "OKActions": [topic.clone()],
        }),
    )
    .await;
    // Both children are in INSUFFICIENT_DATA, so the conjunction is false and the first
    // evaluation puts the parent in OK and executes the actions of that state.
    await_state(&emulator, case, "CompositeAlarms", COMPOSITE, OK).await;
    awaited_notifications(&emulator, case, &url, 1).await;

    // Only one child breaches: the conjunction stays false, so the parent holds OK and
    // executes nothing.
    publish(&emulator, Child::A, BREACHING).await;
    await_state(&emulator, case, "MetricAlarms", Child::A.name(), ALARM).await;
    let held = await_state(&emulator, case, "CompositeAlarms", COMPOSITE, OK).await;
    assert_eq!(
        alarm_of(&emulator, "MetricAlarms", Child::B.name()).await["StateValue"].as_str(),
        Some(INSUFFICIENT_DATA),
        "case {case:?}: the child nothing was published for must still report insufficient data"
    );
    awaited_notifications(&emulator, case, &url, 0).await;

    // The second child breaches: the conjunction becomes true and the parent transitions once,
    // attributing the change to the child that completed the rule.
    publish(&emulator, Child::B, BREACHING).await;
    await_state(&emulator, case, "MetricAlarms", Child::B.name(), ALARM).await;
    let fired = await_state(&emulator, case, "CompositeAlarms", COMPOSITE, ALARM).await;
    assert_ne!(
        fired["StateTransitionedTimestamp"], held["StateTransitionedTimestamp"],
        "case {case:?}: the transition must move the alarm's transition instant, got {fired}"
    );
    assert_transition(
        &emulator,
        case,
        &reads,
        &fired,
        Child::B,
        ALARM,
        OK,
        &rule,
        &url,
    )
    .await;

    // The first child clears: the conjunction is false again, so the parent returns to OK and
    // the actions of THAT state are executed once.
    publish(&emulator, Child::A, CLEARING).await;
    await_state(&emulator, case, "MetricAlarms", Child::A.name(), OK).await;
    let cleared = await_state(&emulator, case, "CompositeAlarms", COMPOSITE, OK).await;
    assert_transition(
        &emulator,
        case,
        &reads,
        &cleared,
        Child::A,
        OK,
        ALARM,
        &rule,
        &url,
    )
    .await;
}

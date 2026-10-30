// SPDX-License-Identifier: CC0-1.0

//! The alarm evaluation as a LIVE client drives it, end to end over the wire through the real
//! `MultiServiceRouter` with CloudWatch, SNS and SQS on one engine.
//!
//! The RULE asserted here: **a metric alarm's state is computed from the datapoints published
//! for the series it watches as the publisher stamps them — the current instant, which is what a
//! client publishing its own measurements does — so the alarm transitions, records the
//! transition in its history and executes the actions that transition fires without any caller
//! setting the state or back-dating a datapoint; `ActionsEnabled` ("Indicates whether actions
//! should be executed during any changes to the alarm state", `MetricAlarm$ActionsEnabled`,
//! written by `DisableAlarmActions` / `EnableAlarmActions`) governs ONLY that execution, so an
//! alarm whose actions are off transitions and records its `StateUpdate` while publishing
//! nothing and recording no `Action` item, and turning them back on restores publication at that
//! alarm's next transition.**
//!
//! What this suite adds to [`crate::actions_disabled_evaluation_delivery`], which asserts the
//! same switch over datapoints placed in a chosen window, is the client's own drive and the
//! client's own readers: nothing here names a time stamp or a window — each datapoint is
//! published at the instant the publish happens, repeatedly, and the alarms are read back
//! through `DescribeAlarms` naming BOTH alarm kinds ("Use this parameter to specify whether you
//! want the operation to return metric alarms, composite alarms, or log alarms",
//! `DescribeAlarmsInput$AlarmTypes`) and through `DescribeAlarmHistory` naming the metric-alarm
//! kind and one `HistoryItemType`. So what is asserted is reachable by a client that only
//! publishes and polls.
//!
//! The queue the notifications are read off carries the grant AWS documents for a subscribed
//! queue, scoped by `aws:SourceArn` to the topic the alarms act on, so every delivery asserted
//! here is one the queue's own policy had to allow.
//!
//! Two alarms over ONE series, differing only in that switch, are what makes the difference
//! observable: the alarm that kept its actions is the positive control, so a queue that stays
//! empty for the muted one cannot be a system that delivers nothing at all.
//!
//! The rows differ along what the rule generalises over — the statistic the windows are
//! aggregated with (a basic one, two others and a percentile), the direction of the comparison,
//! whether the breaching value is past the threshold or exactly on it, the missing-data
//! treatment (including the documented default, left out of the request) and the series the
//! alarm watches — because none of those changes what publishing a value does: it is graded, and
//! the transition it causes is delivered or withheld by the switch alone. The recorded
//! high-resolution `Maximum` case is one row.
//!
//! Every row runs on its own emulator, and the rows run CONCURRENTLY: a transition the clock
//! owes a high-resolution alarm is owed within a period of the publish that implies it, so the
//! waits here are what the suite spends its time on, and rows sharing nothing can spend it at
//! once. Each assertion names its case.

use std::time::Duration;

use serde_json::{json, Value};

use crate::actions_disabled_evaluation_delivery::{
    await_history, cw, history, recorded_state, switch_actions, Mailbox,
};
use crate::harness::{
    element, emulator, given_queue, grant_sns_delivery_from, sns, subscribe_queue, Emulator,
    PAST_EVALUATION_DELAY,
};

/// The alarm whose actions stay on: the positive control.
const ENABLED: &str = "live-actions-enabled-twin";

/// The alarm whose actions `DisableAlarmActions` turns off.
const MUTED: &str = "live-actions-disabled-twin";

/// The period every row's alarm evaluates, in seconds.
///
/// The shortest cadence a high-resolution alarm is evaluated on, so a transition the published
/// data implies is owed soonest — which is what keeps a suite that waits for three of them
/// bounded. "Specifying 10, 20, or 30 also sets this alarm as a high-resolution alarm"
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`).
const PERIOD: i64 = 10;

/// The `StorageResolution` the datapoints of a high-resolution series are published with: "Be
/// sure to specify 10, 20, or 30 only for metrics that are stored by a PutMetricData call with a
/// StorageResolution of 1"
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`).
const STORAGE_RESOLUTION: i64 = 1;

/// How long a wait for a state the published data implies, or for a delivery that follows one,
/// keeps looking.
///
/// A high-resolution alarm's rolling range reaches the window a datapoint was published into
/// once that window has closed, so the state a drive implies is owed within a couple of periods
/// of it; this is many of those, so a failure here is a failure rather than a flake on a busy
/// host.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long one turn of a drive waits before publishing again and re-reading.
const LOOK_AGAIN: Duration = Duration::from_millis(250);

/// How long the absence of a delivery is measured over: a whole evaluation period, so the window
/// spans the cadence at which an evaluation of these alarms could publish anything at all, and a
/// notification the muted alarm would have published has had every chance to arrive.
const SILENCE: Duration = Duration::from_secs(PERIOD.unsigned_abs());

/// One row of the table: a series, how its windows are graded, and the two values that drive the
/// alarm across its threshold in each direction.
struct Row {
    /// What this row is, for every assertion message.
    case: &'static str,
    /// The namespace the row's series lives in.
    namespace: &'static str,
    /// The metric the row's alarm watches.
    metric: &'static str,
    /// The statistic members the alarm carries: `Statistic` or `ExtendedStatistic`.
    statistic: Value,
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The alarm's `Threshold`.
    threshold: f64,
    /// The alarm's `TreatMissingData`, or `None` to leave the member out and take the documented
    /// default.
    treat_missing: Option<&'static str>,
    /// A value the comparison puts in `ALARM`.
    breaching: f64,
    /// A value the comparison puts in `OK`.
    clearing: f64,
    /// The state the row's alarms report before any datapoint is published, which the row's
    /// missing-data treatment decides: the state the first transition moves OUT of.
    ///
    /// A new alarm starts in `INSUFFICIENT_DATA` and is evaluated over a window holding no
    /// datapoint at all, so the arm applied to that hole is what the state becomes —
    /// `notBreaching` substitutes a datapoint that does not breach, so the alarm reads `OK`;
    /// `ignore` leaves the state it had; and the default treatment reports the missing data as
    /// what it is (`TreatMissingData`, `smithy_query(query="doc",shape="PutMetricAlarmInput",
    /// member="TreatMissingData")`).
    before_breach: &'static str,
}

impl Row {
    /// The dimension set this row's series is identified by.
    fn dimensions(&self) -> Value {
        json!([{"Name": "Run", "Value": self.case}])
    }

    /// The `PutMetricAlarm` request for one of the row's twins.
    fn alarm(&self, name: &str, topic: &str) -> Value {
        let mut request = json!({
            "AlarmName": name,
            "Namespace": self.namespace,
            "MetricName": self.metric,
            "Dimensions": self.dimensions(),
            "Period": PERIOD,
            "EvaluationPeriods": 1,
            "Threshold": self.threshold,
            "ComparisonOperator": self.comparison,
            "AlarmActions": [topic],
        });
        for (member, value) in self.statistic.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        if let Some(treat_missing) = self.treat_missing {
            request["TreatMissingData"] = json!(treat_missing);
        }
        request
    }

    /// Publish `value` into the row's series the way its publisher does: one observation whose
    /// instant is the instant of the call, because the request names no `Timestamp` — "The time
    /// the metric data was received"
    /// (`smithy_query(query="doc",shape="MetricDatum",member="Timestamp")`), which for a datum
    /// that states none is when the service received it.
    async fn publish(&self, emulator: &Emulator, value: f64) {
        cw(
            emulator,
            "PutMetricData",
            &json!({
                "Namespace": self.namespace,
                "MetricData": [{
                    "MetricName": self.metric,
                    "Dimensions": self.dimensions(),
                    "Value": value,
                    "StorageResolution": STORAGE_RESOLUTION,
                }],
            }),
        )
        .await;
    }
}

/// Both twins as a client reads them: ONE `DescribeAlarms` naming both alarms and both alarm
/// kinds, which is the reader a client polling an alarm's state uses.
async fn reported_alarms(emulator: &Emulator) -> Value {
    cw(
        emulator,
        "DescribeAlarms",
        &json!({
            "AlarmNames": [ENABLED, MUTED],
            "AlarmTypes": ["CompositeAlarm", "MetricAlarm"],
        }),
    )
    .await
}

/// The metric alarm that read reports under `name`, or `Value::Null` when it reports none.
fn metric_alarm(reported: &Value, name: &str) -> Value {
    reported["MetricAlarms"]
        .as_array()
        .and_then(|alarms| {
            alarms
                .iter()
                .find(|alarm| alarm["AlarmName"].as_str() == Some(name))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

/// The state value each twin reports, read in ONE call so both are read of the same answer.
async fn both_states(emulator: &Emulator) -> (String, String) {
    let reported = reported_alarms(emulator).await;
    let state_of = |name| {
        metric_alarm(&reported, name)["StateValue"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    (state_of(ENABLED), state_of(MUTED))
}

/// The state each twin reports once the evaluations the WRITE made due have settled.
///
/// A new alarm is stored in `INSUFFICIENT_DATA` and evaluated a span later over a window holding
/// no datapoint at all, and what that evaluation makes of an empty window is the row's
/// missing-data treatment — `notBreaching` substitutes an `OK` for it. So the state a breaching
/// datapoint moves the alarm OUT of is the one that has stopped changing, not the one the write
/// left behind; waiting for two reads a span apart to agree is how a client sees that.
async fn settled_states(emulator: &Emulator) -> (String, String) {
    let deadline = std::time::Instant::now() + DEADLINE;
    let mut reported = both_states(emulator).await;
    loop {
        tokio::time::sleep(PAST_EVALUATION_DELAY).await;
        let again = both_states(emulator).await;
        if again == reported || std::time::Instant::now() >= deadline {
            return again;
        }
        reported = again;
    }
}

/// Publish `value` into the row's series and read both twins, over and over, until each reports
/// `state` or the deadline passes; answers what they last reported.
///
/// This is the whole drive: a client publishing a measurement every turn and polling the alarms
/// it configured over it. Keeping the value published is what the series an alarm watches really
/// looks like — a window an evaluation grades holds what was published into it, and the windows
/// keep moving — and every publish of one drive carries the SAME value, so what the windows
/// aggregate stays the state being waited for and no extra transition can come of it.
async fn drive_until(emulator: &Emulator, row: &Row, value: f64, state: &str) -> (String, String) {
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        row.publish(emulator, value).await;
        let reported = both_states(emulator).await;
        if (reported.0 == state && reported.1 == state) || std::time::Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Assert that `notification` reports the transition `old` -> `new` of `alarm` over the row's
/// series.
fn assert_reports_transition(
    notification: &Value,
    alarm: &str,
    old: &str,
    new: &str,
    row: &Row,
) {
    let case = row.case;
    assert_eq!(
        notification["AlarmName"].as_str(),
        Some(alarm),
        "case {case:?}: the notification must name the alarm that fired it, got {notification}"
    );
    assert_eq!(
        (
            notification["OldStateValue"].as_str(),
            notification["NewStateValue"].as_str()
        ),
        (Some(old), Some(new)),
        "case {case:?}: expected the state moved {old:?} -> {new:?}, got {notification}"
    );
    let trigger = &notification["Trigger"];
    assert_eq!(
        (
            trigger["Namespace"].as_str(),
            trigger["MetricName"].as_str()
        ),
        (Some(row.namespace), Some(row.metric)),
        "case {case:?}: the Trigger must name the series evaluated, got {trigger}"
    );
    // The dimension set the notification reports, as a notification spells a dimension.
    assert_eq!(
        trigger["Dimensions"],
        json!([{"name": "Run", "value": row.case}]),
        "case {case:?}: the Trigger must name the dimensions evaluated, got {trigger}"
    );
    assert!(
        notification["NewStateReason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "case {case:?}: the notification must carry the reason the state was reached, got \
         {notification}"
    );
}

/// Assert that `reason` explains an `ALARM` the row's breaching value decided: the datapoint it
/// graded, and the threshold it was graded against.
///
/// The fixed text is the evaluation's own; what is asserted here is that the VARIABLE parts are
/// the published value and the configured threshold, each written as the measurement it is.
fn assert_cites_datapoint(reason: &str, row: &Row) {
    let case = row.case;
    let cited = format!("[{:.1} (", row.breaching);
    assert!(
        reason.contains(&cited),
        "case {case:?}: the state reason must cite the datapoint it graded ({cited}), got \
         {reason:?}"
    );
    let threshold = format!("the threshold ({:.1})", row.threshold);
    assert!(
        reason.contains(&threshold),
        "case {case:?}: the state reason must name the threshold it graded against \
         ({threshold}), got {reason:?}"
    );
}

/// Drive ONE row end to end, on an emulator of its own.
async fn run(row: &Row) {
    let case = row.case;
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "alarm-notifications")]).await,
        "TopicArn",
    );
    let (url, queue_arn) = given_queue(&emulator, "subscriber").await;
    grant_sns_delivery_from(&emulator, &url, &queue_arn, &[&topic]).await;
    subscribe_queue(&emulator, &topic, &queue_arn).await;
    let mut mailbox = Mailbox::over(&url);

    for name in [ENABLED, MUTED] {
        cw(&emulator, "PutMetricAlarm", &row.alarm(name, &topic)).await;
    }
    switch_actions(&emulator, MUTED, false).await;

    // The reader a client polls with asks for both kinds of alarm and gets each alarm under the
    // kind it is: the twins are metric alarms, so a composite-alarm list is no place for them.
    let reported = reported_alarms(&emulator).await;
    for (name, actions_enabled) in [(ENABLED, true), (MUTED, false)] {
        let alarm = metric_alarm(&reported, name);
        assert_eq!(
            alarm["AlarmName"].as_str(),
            Some(name),
            "case {case:?}: a reader naming both alarm kinds must report the metric alarm \
             {name} under MetricAlarms, got {reported}"
        );
        assert_eq!(
            alarm.get("ActionsEnabled").and_then(Value::as_bool),
            Some(actions_enabled),
            "case {case:?}: expected ActionsEnabled {actions_enabled} on {name}, got {alarm}"
        );
    }
    assert_eq!(
        reported["CompositeAlarms"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: no composite alarm exists, so the composite list must be empty, got \
         {reported}"
    );

    // The state each twin reports before anything breaches: what the transition moves FROM,
    // which the row's missing-data treatment decides.
    let before = settled_states(&emulator).await;
    assert_eq!(
        before,
        (row.before_breach.to_string(), row.before_breach.to_string()),
        "case {case:?}: expected both twins to report {:?} before anything is published, got \
         {before:?}",
        row.before_breach
    );

    // The published measurement alone moves BOTH twins: nothing sets a state and nothing names
    // a window.
    let reached = drive_until(&emulator, row, row.breaching, "ALARM").await;
    assert_eq!(
        reached,
        ("ALARM".to_string(), "ALARM".to_string()),
        "case {case:?}: published data must move BOTH twins to ALARM whatever ActionsEnabled \
         says, got {reached:?}"
    );
    let described = reported_alarms(&emulator).await;
    for name in [ENABLED, MUTED] {
        let alarm = metric_alarm(&described, name);
        let reason = alarm["StateReason"].as_str().unwrap_or_default();
        assert_cites_datapoint(reason, row);
    }

    // The control alarm delivered its notification, so the queue is not simply silent.
    let delivered = mailbox.await_for(&emulator, ENABLED, 1).await;
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: the alarm that kept its actions must deliver exactly one notification, \
         got {delivered:?}"
    );
    assert_reports_transition(&delivered[0], ENABLED, row.before_breach, "ALARM", row);
    let envelopes = mailbox.envelopes_of(ENABLED);
    assert_eq!(
        envelopes.first().map(|envelope| envelope["TopicArn"].clone()),
        Some(json!(topic)),
        "case {case:?}: the notification must be delivered through the topic the alarm acts on, \
         got {envelopes:?}"
    );
    let executed = await_history(&emulator, ENABLED, "Action", 1).await;
    assert_eq!(
        executed.len(),
        1,
        "case {case:?}: the executed action must be recorded once, got {executed:?}"
    );

    // The muted alarm transitioned — its history says so — and published nothing.
    let updates = history(&emulator, MUTED, "StateUpdate").await;
    assert!(
        updates.iter().any(|item| recorded_state(item) == "ALARM"),
        "case {case:?}: the muted alarm's history must record the state update into ALARM, got \
         {updates:?}"
    );
    mailbox.read_over(&emulator, SILENCE).await;
    let muted_deliveries = mailbox.of(MUTED);
    assert!(
        muted_deliveries.is_empty(),
        "case {case:?}: an alarm whose actions are off must deliver nothing, got \
         {muted_deliveries:?}"
    );
    let muted_actions = history(&emulator, MUTED, "Action").await;
    assert!(
        muted_actions.is_empty(),
        "case {case:?}: a withheld notification must record no Action item, got {muted_actions:?}"
    );

    // Actions back on: the transition they were off for is not replayed …
    switch_actions(&emulator, MUTED, true).await;
    mailbox.read_over(&emulator, SILENCE).await;
    let replayed = mailbox.of(MUTED);
    assert!(
        replayed.is_empty(),
        "case {case:?}: turning the actions on must not notify for a transition that already \
         happened, got {replayed:?}"
    );

    // … and the alarm's NEXT transitions are the published data's too: the clearing value takes
    // both twins to OK, and the breaching one brings them back, which is the transition the
    // re-enabled alarm delivers.
    let cleared = drive_until(&emulator, row, row.clearing, "OK").await;
    assert_eq!(
        cleared,
        ("OK".to_string(), "OK".to_string()),
        "case {case:?}: a clearing datapoint must move both twins to OK, got {cleared:?}"
    );
    let again = drive_until(&emulator, row, row.breaching, "ALARM").await;
    assert_eq!(
        again,
        ("ALARM".to_string(), "ALARM".to_string()),
        "case {case:?}: a breaching datapoint must move both twins back to ALARM, got {again:?}"
    );

    let delivered = mailbox.await_for(&emulator, MUTED, 1).await;
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: the re-enabled alarm must deliver its next transition, got {delivered:?}"
    );
    assert_reports_transition(&delivered[0], MUTED, "OK", "ALARM", row);
    let muted_actions = await_history(&emulator, MUTED, "Action", 1).await;
    assert_eq!(
        muted_actions.len(),
        1,
        "case {case:?}: the re-enabled alarm's executed action must be recorded, got \
         {muted_actions:?}"
    );
    // Only `AlarmActions` are configured, so an OK transition fires nothing: the control alarm
    // has exactly one delivery per ALARM transition. Awaited up to that count, because the
    // twins' transitions are materialised by the evaluation each alarm's own due instant makes
    // due and the notification each owes is published after it — then asserted to be exactly
    // it, so nothing the OK transition might have fired can pass unnoticed.
    let control = mailbox.await_for(&emulator, ENABLED, 2).await;
    assert_eq!(
        control.len(),
        2,
        "case {case:?}: the control alarm must deliver one notification per ALARM transition and \
         nothing for its OK transition, got {control:?}"
    );
    assert_reports_transition(&control[1], ENABLED, "OK", "ALARM", row);
}

/// The runtime flavour the booted emulator serves on, so the evaluation pass and the alarm clock
/// run the way they do in production rather than inline. One worker per row, plus one for the
/// service's own clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_published_data_transitions_both_twins_and_only_the_enabled_one_delivers() {
    let rows = [
        Row {
            case: "a high-resolution Maximum above its threshold",
            namespace: "Custom/Latency",
            metric: "latency",
            statistic: json!({"Statistic": "Maximum"}),
            comparison: "GreaterThanThreshold",
            threshold: 50.0,
            treat_missing: Some("ignore"),
            breaching: 211.0,
            clearing: 2.0,
            before_breach: "INSUFFICIENT_DATA",
        },
        Row {
            case: "a Minimum below its threshold",
            namespace: "Custom/Capacity",
            metric: "FreeSlots",
            statistic: json!({"Statistic": "Minimum"}),
            comparison: "LessThanThreshold",
            threshold: 5.0,
            treat_missing: Some("notBreaching"),
            breaching: 1.0,
            clearing: 42.0,
            before_breach: "OK",
        },
        Row {
            case: "a percentile exactly on its threshold, with the default missing-data arm",
            namespace: "Custom/Checkout",
            metric: "ResponseTime",
            statistic: json!({"ExtendedStatistic": "p90"}),
            comparison: "GreaterThanOrEqualToThreshold",
            threshold: 20.0,
            treat_missing: None,
            breaching: 20.0,
            clearing: 3.0,
            before_breach: "INSUFFICIENT_DATA",
        },
    ];

    // The rows share nothing — each drives its own emulator — and every one of them is mostly
    // waiting for a transition the clock owes, so they wait at the same time.
    tokio::join!(run(&rows[0]), run(&rows[1]), run(&rows[2]));
}

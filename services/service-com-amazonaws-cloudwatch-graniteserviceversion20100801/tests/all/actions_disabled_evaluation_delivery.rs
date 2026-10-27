// SPDX-License-Identifier: CC0-1.0

//! The `ActionsEnabled` switch against the alarm EVALUATION, end to end over the wire through
//! the real `MultiServiceRouter` with CloudWatch, SNS and SQS on one engine — so what a client
//! sees is what is asserted: the state each alarm reports, what its history records, and what
//! arrives on the queue subscribed to the action's topic.
//!
//! The RULE asserted here: **a metric alarm's state is computed from the datapoints published
//! for the series it watches, and the transition — with its `StateUpdate` history item — happens
//! whatever `ActionsEnabled` says; the switch governs only the EXECUTION of the actions that
//! transition fires ("Indicates whether actions should be executed during any changes to the
//! alarm state", `MetricAlarm$ActionsEnabled`, written by `DisableAlarmActions` /
//! `EnableAlarmActions`), so an alarm whose actions are off transitions while publishing nothing
//! and recording no `Action` item, and turning them back on restores publication at that alarm's
//! NEXT real transition — never replaying the one it was muted for.**
//!
//! Two alarms on ONE series, differing only in that switch, are what makes the difference
//! observable: the alarm that kept its actions is the positive control, so a queue that stays
//! empty for the muted one cannot be a system that delivers nothing at all.
//!
//! The rows differ along what the rule generalises over — the statistic the windows are
//! aggregated with (a basic one and a percentile), the period and its resolution, the direction
//! of the comparison, the missing-data treatment, and the namespace the series lives in —
//! because the switch is about the DELIVERY of a transition, which every one of those reaches
//! the same way. The recorded high-resolution `Maximum` case is one row.
//!
//! Every state this suite asserts is one the evaluation computes from the datapoints a drive
//! published, so each drive places its datapoint where the clock cannot take it away before
//! the evaluation it makes due has been read: no drive sits at the oldest window the evaluation
//! range still holds, each one measures from the window it is made in — so a later drive is
//! never graded before an earlier one — and the drive that re-breaches after the clearing one
//! breaches whether or not a new window has begun in between.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, grant_sns_delivery_on, now_secs, received_messages, rpc, sns,
    subscribe_queue, wait_for_period_room, Emulator, PAST_EVALUATION_DELAY,
};

/// The alarm whose actions stay on: the positive control.
const ENABLED: &str = "actions-enabled-twin";

/// The alarm whose actions `DisableAlarmActions` turns off.
const MUTED: &str = "actions-disabled-twin";

/// How long a wait for a state the evaluation owes, or for a delivery that follows one, keeps
/// looking. Generous only so a failure is a failure rather than a hang: an evaluation is owed
/// inside the framework's sub-second window and the delivery follows it.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// How often such a wait looks again.
const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(100);

/// How long the absence of a delivery is measured over: several evaluation spans, so a
/// notification the muted alarm would have published has had every chance to arrive.
const SILENCE: std::time::Duration = std::time::Duration::from_millis(1_500);

/// How much of the current period window ONE drive needs left to it, in seconds.
///
/// A drive stamps its datapoint a whole number of windows before NOW, and the window an
/// evaluation counts those back from is the one the evaluation itself runs in — so the publish
/// and the read that materialises the evaluation it makes due are asked to land in the same
/// window the drive measured against, rather than one the clock moved on to, where the
/// datapoint is a window older than the drive placed it. Several evaluation spans, so the
/// drive and the first read that grades it fit with room to spare ([`wait_for_period_room`]),
/// and smaller than the shortest period a row uses.
const WINDOW_ROOM_SECS: f64 = 4.0;

/// One CloudWatch call, requiring a 200.
async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        operation,
        body,
    )
    .await
}

/// One row of the table: a series, how its windows are graded, and the two values that drive
/// the alarm across its threshold in each direction.
struct Row {
    /// What this row is, for every assertion message.
    case: &'static str,
    /// The namespace the row's series lives in.
    namespace: &'static str,
    /// The metric the row's alarm watches.
    metric: &'static str,
    /// The statistic members the alarm carries: `Statistic` or `ExtendedStatistic`.
    statistic: Value,
    /// The alarm's `Period`, in seconds, and the span one window covers.
    period: i64,
    /// The `StorageResolution` the datapoints are published with, or `None` for the
    /// documented default of 60.
    storage_resolution: Option<i64>,
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The alarm's `Threshold`.
    threshold: f64,
    /// The alarm's `TreatMissingData`, or `None` to leave the member out.
    treat_missing: Option<&'static str>,
    /// A value the comparison puts in `ALARM`.
    breaching: f64,
    /// A value the comparison puts in `OK`.
    clearing: f64,
    /// The value the LAST drive re-breaches with.
    ///
    /// That drive is made after the clearing one, and whether the clock has started a new
    /// period window in between is not something a test can pin — so this value is one the
    /// row's statistic breaches with BOTH on its own and aggregated with `clearing`, and the
    /// transition is the same either way. The lone-datapoint window is what `breaching`
    /// covers, in the drive that is alone in its own window.
    re_breaching: f64,
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
            "Period": self.period,
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

    /// Publish `value` into the row's series, stamped `windows_back` whole periods before now.
    ///
    /// The drive waits for enough of the current window to be left for the evaluation it makes
    /// due to be read inside that same window ([`WINDOW_ROOM_SECS`]): the windows are anchored
    /// at multiples of the period, so a drive made as one window ends is graded from the next
    /// one, where its datapoint is a window older than it was placed.
    async fn publish(&self, emulator: &Emulator, value: f64, windows_back: i64) {
        #[allow(clippy::cast_precision_loss)]
        let (period, windows) = (self.period as f64, windows_back as f64);
        wait_for_period_room(period, WINDOW_ROOM_SECS).await;
        let age = period * windows;
        let mut datum = json!({
            "MetricName": self.metric,
            "Dimensions": self.dimensions(),
            "Value": value,
            "Timestamp": now_secs() - age,
        });
        if let Some(resolution) = self.storage_resolution {
            datum["StorageResolution"] = json!(resolution);
        }
        cw(
            emulator,
            "PutMetricData",
            &json!({"Namespace": self.namespace, "MetricData": [datum]}),
        )
        .await;
    }
}

/// The metric alarm `DescribeAlarms` reports under `name`.
async fn described(emulator: &Emulator, name: &str) -> Value {
    let reported = cw(emulator, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    reported["MetricAlarms"][0].clone()
}

/// The state value `DescribeAlarms` reports for `name`.
async fn state_of(emulator: &Emulator, name: &str) -> String {
    described(emulator, name)
        .await
        .get("StateValue")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The state both twins report, as a client reads them.
async fn both_states(emulator: &Emulator) -> (String, String) {
    (
        state_of(emulator, ENABLED).await,
        state_of(emulator, MUTED).await,
    )
}

/// The state each twin reports once the evaluations the WRITE made due have settled.
///
/// A new alarm is stored in `INSUFFICIENT_DATA` and evaluated a span later over a window
/// holding no datapoint at all, and what that evaluation makes of an empty window is the row's
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

/// Read both twins until each reports `state`, and answer what they last reported.
///
/// A transition is owed a span after the datapoint that makes it due, so a test waits for the
/// state rather than assuming it is already there — and it waits by AWAITING, so the service's
/// own alarm clock gets to run while it does.
async fn await_both(emulator: &Emulator, state: &str) -> (String, String) {
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        let reported = both_states(emulator).await;
        if (reported.0 == state && reported.1 == state) || std::time::Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The alarm history items of `item_type` recorded for `name`.
async fn history(emulator: &Emulator, name: &str, item_type: &str) -> Vec<Value> {
    let reported = cw(
        emulator,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": name,
            "AlarmTypes": ["MetricAlarm"],
            "HistoryItemType": item_type,
        }),
    )
    .await;
    reported["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every notification that has arrived on one queue, accumulated across reads: a receive hides
/// what it handed over, so a test that reads the queue several times has to keep what it saw.
struct Mailbox {
    /// The queue every read below is made against.
    url: String,
    /// The alarm notifications read so far, in arrival order.
    arrived: Vec<Value>,
}

impl Mailbox {
    /// A mailbox over the queue at `url`.
    fn over(url: &str) -> Self {
        Self {
            url: url.to_string(),
            arrived: Vec::new(),
        }
    }

    /// Read the queue once, keeping every notification it handed over — and deleting each
    /// message it keeps.
    ///
    /// A received message a client does not delete comes back when its visibility timeout
    /// expires, so a mailbox that accumulates its reads would count one delivery as two the
    /// moment a row takes longer than that timeout to run. Deleting what it has kept is both
    /// what a client does and what makes "exactly one notification" mean the same thing however
    /// long the reads take.
    async fn read(&mut self, emulator: &Emulator) {
        for message in received_messages(emulator, &self.url).await {
            let body = message["Body"].as_str().unwrap_or_default();
            let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("a delivery is not the SNS envelope ({error}): {body}")
            });
            let inner = envelope["Message"].as_str().unwrap_or_default();
            let notification: Value = serde_json::from_str(inner).unwrap_or_else(|error| {
                panic!("a delivery is not an alarm notification ({error}): {inner}")
            });
            self.arrived.push(notification);
            let receipt = message["ReceiptHandle"]
                .as_str()
                .unwrap_or_else(|| panic!("a received message carries a receipt handle: {message}"));
            rpc(
                emulator,
                "sqs",
                "AmazonSQS",
                "DeleteMessage",
                &json!({"QueueUrl": self.url, "ReceiptHandle": receipt}),
            )
            .await;
        }
    }

    /// Read the queue until it has delivered `wanted` notifications for `alarm`, or the
    /// deadline passes.
    async fn await_for(&mut self, emulator: &Emulator, alarm: &str, wanted: usize) -> Vec<Value> {
        let deadline = std::time::Instant::now() + DEADLINE;
        loop {
            self.read(emulator).await;
            let found = self.of(alarm);
            if found.len() >= wanted || std::time::Instant::now() >= deadline {
                return found;
            }
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }

    /// Read the queue for `span`, which is how a client establishes that a notification does
    /// NOT come.
    async fn read_over(&mut self, emulator: &Emulator, span: std::time::Duration) {
        let deadline = std::time::Instant::now() + span;
        while std::time::Instant::now() < deadline {
            self.read(emulator).await;
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }

    /// Every notification that named `alarm`.
    fn of(&self, alarm: &str) -> Vec<Value> {
        self.arrived
            .iter()
            .filter(|notification| notification["AlarmName"].as_str() == Some(alarm))
            .cloned()
            .collect()
    }
}

/// Turn the named alarm's actions off or on through the operation that writes the switch, and
/// read the switch back the way a client does.
async fn switch_actions(emulator: &Emulator, name: &str, on: bool) {
    let operation = if on {
        "EnableAlarmActions"
    } else {
        "DisableAlarmActions"
    };
    cw(emulator, operation, &json!({"AlarmNames": [name]})).await;
    let reported = described(emulator, name).await;
    assert_eq!(
        reported.get("ActionsEnabled").and_then(Value::as_bool),
        Some(on),
        "{operation} must write ActionsEnabled {on} on {name}, got {reported}"
    );
}

/// Assert that `notification` reports the transition `old` -> `new` of `alarm` over the row's
/// series.
fn assert_reports_transition(
    case: &str,
    notification: &Value,
    alarm: &str,
    old: &str,
    new: &str,
    row: &Row,
) {
    assert_eq!(
        notification["AlarmName"].as_str(),
        Some(alarm),
        "case {case:?}: the notification must name the alarm that fired it, got {notification}"
    );
    assert_eq!(
        notification["NewStateValue"].as_str(),
        Some(new),
        "case {case:?}: expected the state moved to {new:?}, got {notification}"
    );
    assert_eq!(
        notification["OldStateValue"].as_str(),
        Some(old),
        "case {case:?}: expected the state moved from {old:?}, got {notification}"
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
    let dimensions = json!([{"name": "Run", "value": row.case}]);
    assert_eq!(
        trigger["Dimensions"], dimensions,
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

/// The state value one `StateUpdate` history item records the alarm as having moved to.
fn recorded_state(item: &Value) -> String {
    let data = item["HistoryData"].as_str().unwrap_or_default();
    let document: Value = serde_json::from_str(data)
        .unwrap_or_else(|error| panic!("HistoryData is not JSON ({error}): {data}"));
    document
        .pointer("/newState/stateValue")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The runtime flavour the booted emulator serves on, so the evaluation pass and the clock run
/// the way they do in production rather than inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_muted_alarm_transitions_on_published_data_and_delivers_only_once_re_enabled() {
    let rows = vec![
        Row {
            case: "a high-resolution Maximum above its threshold",
            namespace: "Custom/Latency",
            metric: "latency",
            statistic: json!({"Statistic": "Maximum"}),
            period: 10,
            // "data points with a period of less than 60 seconds ... are stored by a
            // PutMetricData call with a StorageResolution of 1"
            // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`).
            storage_resolution: Some(1),
            comparison: "GreaterThanThreshold",
            threshold: 50.0,
            treat_missing: Some("ignore"),
            breaching: 211.0,
            clearing: 2.0,
            // Maximum{clearing, re_breaching} is re_breaching, so one value serves both.
            re_breaching: 211.0,
        },
        Row {
            case: "a standard-resolution Average at its threshold",
            namespace: "Custom/Database",
            metric: "DatabaseConnections",
            statistic: json!({"Statistic": "Average"}),
            period: 60,
            storage_resolution: None,
            comparison: "GreaterThanOrEqualToThreshold",
            threshold: 100.0,
            treat_missing: None,
            breaching: 100.0,
            clearing: 99.0,
            // The mean of the clearing value and this one is still at or above the threshold,
            // which the breaching value sitting exactly ON it would not be.
            re_breaching: 150.0,
        },
        Row {
            case: "a Minimum below its threshold",
            namespace: "Custom/Capacity",
            metric: "FreeSlots",
            statistic: json!({"Statistic": "Minimum"}),
            period: 10,
            storage_resolution: Some(1),
            comparison: "LessThanThreshold",
            threshold: 5.0,
            treat_missing: Some("notBreaching"),
            breaching: 1.0,
            clearing: 42.0,
            // Minimum{clearing, re_breaching} is re_breaching, so one value serves both.
            re_breaching: 1.0,
        },
        Row {
            case: "a percentile over a customer namespace",
            namespace: "Custom/Checkout",
            metric: "ResponseTime",
            statistic: json!({"ExtendedStatistic": "p90"}),
            period: 30,
            storage_resolution: Some(1),
            comparison: "GreaterThanThreshold",
            threshold: 20.0,
            treat_missing: Some("ignore"),
            breaching: 99.0,
            clearing: 3.0,
            // p90 of the two values, by nearest rank, is the larger one.
            re_breaching: 99.0,
        },
    ];

    for row in &rows {
        let case = row.case;
        // A fresh emulator per row, so what the queue holds can only be this row's.
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "alarm-notifications")]).await,
            "TopicArn",
        );
        let (url, queue_arn) = given_queue(&emulator, "subscriber").await;
        grant_sns_delivery_on(&emulator, &url, &queue_arn).await;
        subscribe_queue(&emulator, &topic, &queue_arn).await;
        let mut mailbox = Mailbox::over(&url);

        for name in [ENABLED, MUTED] {
            cw(&emulator, "PutMetricAlarm", &row.alarm(name, &topic)).await;
        }
        switch_actions(&emulator, MUTED, false).await;
        let enabled = described(&emulator, ENABLED).await;
        assert_eq!(
            enabled.get("ActionsEnabled").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: the control alarm must keep its actions on — \"The default is \
             TRUE\" — got {enabled}"
        );

        // The state each twin reports before anything breaches: what the transition moves
        // FROM, which the missing-data treatment of the row decides, once the evaluation the
        // write made due has settled on it.
        let before = settled_states(&emulator).await;

        // One breaching datapoint, in a window of its own: both twins are evaluated into ALARM.
        // A drive stamped ONE window back is the oldest this suite places: the evaluation range
        // reaches further than that, so the datapoint is still graded after the clock has moved
        // the window it counts back from, and every later drive measures from the window it is
        // made in, which is never older than this one.
        row.publish(&emulator, row.breaching, 1).await;
        let reached = await_both(&emulator, "ALARM").await;
        assert_eq!(
            reached,
            ("ALARM".to_string(), "ALARM".to_string()),
            "case {case:?}: published data must move BOTH twins to ALARM whatever \
             ActionsEnabled says, got {reached:?}"
        );
        for name in [ENABLED, MUTED] {
            let alarm = described(&emulator, name).await;
            let reason = alarm["StateReason"].as_str().unwrap_or_default().to_string();
            assert!(
                reason.contains(&format!("[{:.1}", row.breaching)),
                "case {case:?}: {name} must explain its state by the datapoint it graded, got \
                 {reason:?}"
            );
        }

        // The control alarm delivered its notification, so the queue is not simply silent.
        let delivered = mailbox.await_for(&emulator, ENABLED, 1).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the alarm that kept its actions must deliver exactly one \
             notification, got {delivered:?}"
        );
        assert_reports_transition(case, &delivered[0], ENABLED, &before.0, "ALARM", row);
        let executed = history(&emulator, ENABLED, "Action").await;
        assert_eq!(
            executed.len(),
            1,
            "case {case:?}: the executed action must be recorded once, got {executed:?}"
        );

        // The muted alarm transitioned — its history says so — and published nothing.
        let updates = history(&emulator, MUTED, "StateUpdate").await;
        assert!(
            updates.iter().any(|item| recorded_state(item) == "ALARM"),
            "case {case:?}: the muted alarm's history must record the state update into ALARM, \
             got {updates:?}"
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
            "case {case:?}: a withheld notification must record no Action item, got \
             {muted_actions:?}"
        );

        // Actions back on: nothing is replayed for the transition they were off for …
        switch_actions(&emulator, MUTED, true).await;
        mailbox.read_over(&emulator, SILENCE).await;
        let replayed = mailbox.of(MUTED);
        assert!(
            replayed.is_empty(),
            "case {case:?}: turning the actions on must not notify for a transition that \
             already happened, got {replayed:?}"
        );

        // … and the NEXT real transition of that alarm is delivered. Driven by data the
        // evaluation grades, so it is the evaluation that moves both twins, not a forced state:
        // the clearing drive lands in a window newer than the breaching one above, and the
        // re-breaching drive in the window it is made in, which is that one or a newer still.
        row.publish(&emulator, row.clearing, 0).await;
        let cleared = await_both(&emulator, "OK").await;
        assert_eq!(
            cleared,
            ("OK".to_string(), "OK".to_string()),
            "case {case:?}: a clearing datapoint must move both twins to OK, got {cleared:?}"
        );
        row.publish(&emulator, row.re_breaching, 0).await;
        let again = await_both(&emulator, "ALARM").await;
        assert_eq!(
            again,
            ("ALARM".to_string(), "ALARM".to_string()),
            "case {case:?}: a breaching datapoint must move both twins back to ALARM, got \
             {again:?}"
        );

        let delivered = mailbox.await_for(&emulator, MUTED, 1).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the re-enabled alarm must deliver its next transition, got \
             {delivered:?}"
        );
        assert_reports_transition(case, &delivered[0], MUTED, "OK", "ALARM", row);
        let muted_actions = history(&emulator, MUTED, "Action").await;
        assert_eq!(
            muted_actions.len(),
            1,
            "case {case:?}: the re-enabled alarm's executed action must be recorded, got \
             {muted_actions:?}"
        );
        // The OK transition of either twin fires nothing: only AlarmActions are configured,
        // so the control alarm has exactly one more delivery than it started with.
        let control = mailbox.of(ENABLED);
        assert_eq!(
            control.len(),
            2,
            "case {case:?}: the control alarm must deliver one notification per ALARM \
             transition and nothing for its OK transition, got {control:?}"
        );
    }
}

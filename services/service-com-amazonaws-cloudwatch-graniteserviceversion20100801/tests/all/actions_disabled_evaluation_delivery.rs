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
//! A notification is published after the transition that owes it, and each twin's transition is
//! materialised by the evaluation that twin's OWN due instant makes due — so one twin's delivery
//! says nothing about whether the other's has been made yet, and the record accounting for a
//! delivery is written as that delivery completes, after the target already holds it. Every
//! count this suite asserts about a delivery, or about the `Action` item recording one, is
//! therefore AWAITED up to that count and then asserted to be exactly it: waiting is what makes
//! the count a count of what arrived rather than of what had arrived first.
//!
//! Every state this suite asserts is one the evaluation computes from the datapoints a drive
//! published, so no wait here can outlive the data it is waiting on. An alarm grades the data
//! inside its evaluation range at the instant it evaluates, and that range is a span of whole
//! period windows: a datapoint leaves it once the clock has moved on far enough, and from then
//! on the evaluation computes a state the drive no longer implies. A drive therefore places its
//! datapoint where the clock cannot take it away before the evaluation it makes due has been
//! read — no drive sits at the oldest window the range still holds, and each measures from the
//! window it is made in, so a later drive is never graded before an earlier one — and a wait
//! that has run longer than that stay publishes the same value again ([`REFRESH`]), the way the
//! series an alarm watches is really published, rather than waiting on for a state nothing asks
//! for any more.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, graded_window_stamp, grant_sns_delivery_on, received_messages,
    rpc, sns, subscribe_queue, wait_for_period_room, Emulator, PAST_EVALUATION_DELAY,
};

/// The alarm whose actions stay on: the positive control.
const ENABLED: &str = "actions-enabled-twin";

/// The alarm whose actions `DisableAlarmActions` turns off.
const MUTED: &str = "actions-disabled-twin";

/// How long a wait for a state the evaluation owes, or for a delivery that follows one, keeps
/// looking. Generous only so a failure is a failure rather than a hang: an evaluation is owed
/// inside the framework's sub-second window and the delivery follows it.
///
/// It is longer than a drive's datapoint stays inside the evaluation range for the shortest
/// period a row uses, which is why a wait that long keeps the series published ([`REFRESH`]):
/// otherwise a host slow enough to delay the first evaluation past that stay would turn a
/// transition the data owed into one that can never arrive.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// How often such a wait looks again.
const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(100);

/// How long a wait for a state the published data implies keeps looking before it publishes
/// that data AGAIN ([`await_both_driven`]).
///
/// A datapoint decides an alarm only while it is inside that alarm's evaluation range, which is
/// a span of period windows the clock keeps moving: the stay is a few windows, so it is shorter
/// than [`DEADLINE`] for the shortest period a row uses. Shorter than the shortest of those
/// stays, so the series an alarm grades always holds the value the wait is about.
const REFRESH: std::time::Duration = std::time::Duration::from_secs(5);

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
pub async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
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

    /// Publish `value` into the row's series, stamped `windows_back` whole periods before the
    /// newest window an evaluation GRADES, and answer the window it published into as a count of
    /// whole periods since the epoch.
    ///
    /// An alarm grades the periods that have CLOSED, so `windows_back` of zero is the newest
    /// closed window ([`graded_window_stamp`]) rather than the period the clock is still in.
    ///
    /// The drive waits for enough of the current window to be left for the evaluation it makes
    /// due to be read before the windows shift ([`WINDOW_ROOM_SECS`]): the windows are anchored
    /// at multiples of the period, so a drive made as one window ends is graded from the next
    /// one, where its datapoint is a window older than it was placed.
    ///
    /// `after`, where given, is a window this drive must not share: the alarms here evaluate
    /// ONE period, so what decides them is the newest datapoint of the series, and a drive
    /// landing in a window another value was published into would be graded against the two
    /// aggregated rather than against its own. Waiting for the clock to leave that window costs
    /// at most one period, and only a drive that really would collide waits at all.
    async fn drive(
        &self,
        emulator: &Emulator,
        value: f64,
        windows_back: i64,
        after: Option<f64>,
    ) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let (period, windows) = (self.period as f64, windows_back as f64);
        // The stamp decides the window, so the window is read FROM the stamp: a boundary the
        // clock crosses between the two would otherwise leave the drive reporting a window its
        // datapoint is not in.
        let stamp = loop {
            wait_for_period_room(period, WINDOW_ROOM_SECS).await;
            let stamp = graded_window_stamp(period) - period * windows;
            if after.is_none_or(|collides| (stamp / period).floor() > collides) {
                break stamp;
            }
            tokio::time::sleep(LOOK_AGAIN).await;
        };
        let mut datum = json!({
            "MetricName": self.metric,
            "Dimensions": self.dimensions(),
            "Value": value,
            "Timestamp": stamp,
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
        (stamp / period).floor()
    }
}

/// The metric alarm `DescribeAlarms` reports under `name`.
pub async fn described(emulator: &Emulator, name: &str) -> Value {
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

/// Read both twins until each reports `state`, publishing `value` into the row's series again
/// whenever the wait has outlived the stay of the datapoint it is waiting on ([`REFRESH`]).
/// Answers what the twins last reported, beside the window the LAST publish of the wait went
/// into — what the drive after it must not share.
///
/// A transition is owed a span after the datapoint that makes it due, so a test waits for the
/// state rather than assuming it is already there — and it waits by AWAITING, so the service's
/// own alarm clock gets to run while it does.
///
/// It re-drives because an alarm grades the data inside its evaluation range at the instant it
/// evaluates, and that range is a span of period windows the clock keeps moving: a datapoint
/// leaves it a few windows after it was stamped, and from then on the evaluation computes a
/// state from a series with nothing in range rather than the state the drive implied. A wait
/// that outlived its datapoint would be waiting for a state no datapoint still asks for — so
/// it publishes the value again, which is also what the series an alarm watches really looks
/// like: something keeps publishing it. Every republish carries the SAME value, so what the
/// windows aggregate is the same state and no extra transition can come of it.
async fn await_both_driven(
    emulator: &Emulator,
    row: &Row,
    value: f64,
    state: &str,
    published: f64,
) -> ((String, String), f64) {
    let deadline = std::time::Instant::now() + DEADLINE;
    let mut refresh_at = std::time::Instant::now() + REFRESH;
    let mut window = published;
    loop {
        let reported = both_states(emulator).await;
        if (reported.0 == state && reported.1 == state) || std::time::Instant::now() >= deadline {
            return (reported, window);
        }
        if std::time::Instant::now() >= refresh_at {
            window = row.drive(emulator, value, 0, None).await;
            refresh_at = std::time::Instant::now() + REFRESH;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The alarm history items of `item_type` recorded for `name`.
pub async fn history(emulator: &Emulator, name: &str, item_type: &str) -> Vec<Value> {
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

/// The alarm history items of `item_type` recorded for `name`, read until the alarm has at
/// least `wanted` of them or the deadline passes.
///
/// An `Action` item accounts for ONE delivery and is written as that delivery completes, which
/// is after the notification itself has reached the target: a client that has just read the
/// notification off the queue can still read the history before the item accounting for it is
/// written. So a count of them is read the way the queue is — until the item being asserted
/// about is there — while the assertion that follows keeps the upper bound the rule states.
pub async fn await_history(
    emulator: &Emulator,
    name: &str,
    item_type: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        let items = history(emulator, name, item_type).await;
        if items.len() >= wanted || std::time::Instant::now() >= deadline {
            return items;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every notification that has arrived on one queue, accumulated across reads: a receive hides
/// what it handed over, so a test that reads the queue several times has to keep what it saw.
pub struct Mailbox {
    /// The queue every read below is made against.
    url: String,
    /// What arrived so far, in arrival order: each delivery as the SNS envelope it came in and
    /// the alarm notification that envelope carried.
    arrived: Vec<(Value, Value)>,
}

impl Mailbox {
    /// A mailbox over the queue at `url`.
    pub fn over(url: &str) -> Self {
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
            self.arrived.push((envelope, notification));
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
    pub async fn await_for(
        &mut self,
        emulator: &Emulator,
        alarm: &str,
        wanted: usize,
    ) -> Vec<Value> {
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
    pub async fn read_over(&mut self, emulator: &Emulator, span: std::time::Duration) {
        let deadline = std::time::Instant::now() + span;
        while std::time::Instant::now() < deadline {
            self.read(emulator).await;
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }

    /// Every notification that named `alarm`.
    pub fn of(&self, alarm: &str) -> Vec<Value> {
        self.delivered_to(alarm)
            .map(|(_, notification)| notification.clone())
            .collect()
    }

    /// The SNS envelope each of those notifications was delivered in.
    pub fn envelopes_of(&self, alarm: &str) -> Vec<Value> {
        self.delivered_to(alarm)
            .map(|(envelope, _)| envelope.clone())
            .collect()
    }

    /// Every delivery whose notification named `alarm`.
    fn delivered_to<'mailbox>(
        &'mailbox self,
        alarm: &'mailbox str,
    ) -> impl Iterator<Item = &'mailbox (Value, Value)> {
        self.arrived
            .iter()
            .filter(move |(_, notification)| notification["AlarmName"].as_str() == Some(alarm))
    }
}

/// Turn the named alarm's actions off or on through the operation that writes the switch, and
/// read the switch back the way a client does.
pub async fn switch_actions(emulator: &Emulator, name: &str, on: bool) {
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
pub fn recorded_state(item: &Value) -> String {
    recorded_state_at(item, "/newState/stateValue")
}

/// The state value the same item records the alarm as having moved FROM.
pub fn recorded_previous_state(item: &Value) -> String {
    recorded_state_at(item, "/oldState/stateValue")
}

/// One state value out of a `StateUpdate` item's `HistoryData` document: the ONE reading of
/// that document, so the two sides of a recorded transition are not parsed two ways.
fn recorded_state_at(item: &Value, pointer: &str) -> String {
    let data = item["HistoryData"].as_str().unwrap_or_default();
    let document: Value = serde_json::from_str(data)
        .unwrap_or_else(|error| panic!("HistoryData is not JSON ({error}): {data}"));
    document
        .pointer(pointer)
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
        // made in, which is never older than this one. The wait keeps that value published for
        // as long as it looks, so the state it waits for is one the data still implies.
        let breaching_window = row.drive(&emulator, row.breaching, 1, None).await;
        let (reached, breaching_window) =
            await_both_driven(&emulator, row, row.breaching, "ALARM", breaching_window).await;
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
        // the clearing drive lands in a window newer than the breaching one, including any
        // window the wait above kept that value published into, and the re-breaching drive in
        // the window it is made in, which is that one or a newer still.
        let clearing_window = row
            .drive(&emulator, row.clearing, 0, Some(breaching_window))
            .await;
        let (cleared, _) =
            await_both_driven(&emulator, row, row.clearing, "OK", clearing_window).await;
        assert_eq!(
            cleared,
            ("OK".to_string(), "OK".to_string()),
            "case {case:?}: a clearing datapoint must move both twins to OK, got {cleared:?}"
        );
        let re_breaching_window = row.drive(&emulator, row.re_breaching, 0, None).await;
        let (again, _) = await_both_driven(
            &emulator,
            row,
            row.re_breaching,
            "ALARM",
            re_breaching_window,
        )
        .await;
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
        let muted_actions = await_history(&emulator, MUTED, "Action", 1).await;
        assert_eq!(
            muted_actions.len(),
            1,
            "case {case:?}: the re-enabled alarm's executed action must be recorded, got \
             {muted_actions:?}"
        );
        // The OK transition of either twin fires nothing: only AlarmActions are configured,
        // so the control alarm has exactly one more delivery than it started with — AWAITED,
        // like every other delivery here, because the twins' transitions are materialised by
        // the evaluations each alarm's own due instant makes due and the notification each owes
        // is published after it: one twin's delivery says nothing about whether the other's has
        // been made yet. The assertion is still that there are exactly two, so nothing the OK
        // transition might have fired can pass unnoticed.
        let control = mailbox.await_for(&emulator, ENABLED, 2).await;
        assert_eq!(
            control.len(),
            2,
            "case {case:?}: the control alarm must deliver one notification per ALARM \
             transition and nothing for its OK transition, got {control:?}"
        );
        assert_reports_transition(case, &control[1], ENABLED, "OK", "ALARM", row);
    }
}

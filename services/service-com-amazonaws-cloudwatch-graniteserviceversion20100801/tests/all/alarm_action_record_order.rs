// SPDX-License-Identifier: CC0-1.0

//! WHEN an executed alarm action reaches the alarm's history, end to end over the wire through
//! the real `MultiServiceRouter` with CloudWatch, SNS and SQS on one engine — so what is
//! asserted is what a client sees: the notifications that arrive on the queue subscribed to the
//! action's topics, beside the `Action` items `DescribeAlarmHistory` reports for the alarms that
//! fired them.
//!
//! The RULE asserted here: **an executed action is recorded in the history of the alarm that
//! fired it as that execution completes — one `Action` item per action ARN, carrying the
//! outcome of that one execution — so the record of a delivery is never withheld behind the
//! other actions the same delivery is batched with: when a client has seen `k` notifications
//! arrive, every one of them but the last is already accounted for in the history of the alarm
//! that published it.**
//!
//! One transition's notification fires every action ARN configured for the state it moved to,
//! and one delivery pass settles the notifications of EVERY alarm whose transition came due
//! together — each action an in-process publish that fans out again inside SNS, which a client
//! can read off the subscribed queue while the rest of the pass is still being delivered. The
//! rows therefore differ along the FAN-OUT of one pass — how many alarms transition together
//! and how many action ARNs each of their notifications fires — because that product is what
//! the records could be deferred behind.
//!
//! Nothing is asked of CloudWatch between the write and the queue: the transitions here are the
//! ones the clock makes due, so the pass runs in the service's own task while this suite reads
//! the queue, which is the only arrangement in which the ORDER of a delivery and its record is
//! observable at all.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, graded_window_stamp, grant_sns_delivery_on, received_messages,
    rpc, sns, subscribe_queue, Emulator,
};

/// The customer namespace every alarm in this suite watches.
const NAMESPACE: &str = "Custom/ActionRecords";

/// The metric every alarm in this suite watches.
const METRIC: &str = "Backlog";

/// The threshold every alarm in this suite compares against.
const THRESHOLD: f64 = 50.0;

/// A value above [`THRESHOLD`]: the datapoint that takes every alarm of a row into `ALARM`.
const BREACHING: f64 = 137.0;

/// The period every alarm in this suite evaluates, in seconds.
///
/// A regular-resolution period, and long enough that the one datapoint published stays inside
/// every alarm's evaluation range for far longer than a row takes to run: the rows assert ONE
/// transition per alarm, so the series must keep implying the state it drove them to.
const PERIOD_SECS: f64 = 60.0;

/// How long this suite waits for the deliveries and the records a transition owes.
///
/// Generous only so a failure is a failure rather than a hang: the evaluation is owed inside
/// the framework's sub-second window and the delivery follows it.
const DEADLINE: Duration = Duration::from_secs(20);

/// How often the queue and the history are looked at again.
///
/// Short, because what this suite is about is what a client can see WHILE a pass is still
/// delivering: a slow poll would only ever see a pass that has finished.
const LOOK_AGAIN: Duration = Duration::from_millis(20);

/// The CloudWatch signing name and target prefix, for `rpc`.
const CLOUDWATCH: (&str, &str) = ("monitoring", "GraniteServiceVersion20100801");

/// Call one CloudWatch operation, requiring a 200.
async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, CLOUDWATCH.0, CLOUDWATCH.1, operation, body).await
}

/// One row of the table: the fan-out of the single delivery pass it drives.
struct Row {
    /// What this row is, for every assertion message.
    case: &'static str,
    /// How many alarms transition in that pass, each owing a notification of its own.
    alarms: usize,
    /// How many action ARNs each of those notifications fires.
    actions: usize,
}

impl Row {
    /// The dimension set this row's series is identified by.
    fn dimensions(&self) -> Value {
        json!([{
            "Name": "Fanout",
            "Value": format!("{}-alarms-{}-actions", self.alarms, self.actions),
        }])
    }

    /// The names of the alarms this row writes.
    fn alarm_names(&self) -> Vec<String> {
        (0..self.alarms)
            .map(|index| format!("recorded-action-{index}"))
            .collect()
    }

    /// The `PutMetricAlarm` request for one of them: every one of the row's topics on the list
    /// the `ALARM` state selects, so one transition executes them all.
    fn alarm(&self, name: &str, topics: &[String]) -> Value {
        json!({
            "AlarmName": name,
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": self.dimensions(),
            "Statistic": "Maximum",
            "Period": PERIOD_SECS,
            "EvaluationPeriods": 1,
            "Threshold": THRESHOLD,
            "ComparisonOperator": "GreaterThanThreshold",
            // "the current alarm state is maintained" where a window holds nothing to grade
            // (`PutMetricAlarmInput$TreatMissingData`): the state this row's datapoint drives
            // every alarm to is then the only transition any of them makes.
            "TreatMissingData": "ignore",
            "AlarmActions": topics,
        })
    }
}

/// Every `Action` item `DescribeAlarmHistory` reports for `name`.
async fn action_items(emulator: &Emulator, name: &str) -> Vec<Value> {
    let reported = cw(
        emulator,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": name,
            "AlarmTypes": ["MetricAlarm"],
            "HistoryItemType": "Action",
        }),
    )
    .await;
    reported["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// How many action executions the whole store has recorded for `names`.
async fn recorded_actions(emulator: &Emulator, names: &[String]) -> usize {
    let mut recorded = 0;
    for name in names {
        recorded += action_items(emulator, name).await.len();
    }
    recorded
}

/// The alarm-notification document one delivery carries, out of its SNS envelope.
fn notification_of(message: &Value) -> Value {
    let body = message["Body"].as_str().unwrap_or_default();
    let envelope: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("a delivery is not the SNS envelope ({error}): {body}"));
    let inner = envelope["Message"].as_str().unwrap_or_default();
    serde_json::from_str(inner)
        .unwrap_or_else(|error| panic!("a delivery is not an alarm notification ({error}): {inner}"))
}

/// Read the queue once, answering the notifications it handed over and deleting each of them so
/// one delivery is never counted twice when its visibility timeout expires.
async fn drain(emulator: &Emulator, url: &str) -> Vec<Value> {
    let mut arrived = Vec::new();
    for message in received_messages(emulator, url).await {
        arrived.push(notification_of(&message));
        let receipt = message["ReceiptHandle"]
            .as_str()
            .unwrap_or_else(|| panic!("a received message carries a receipt handle: {message}"));
        rpc(
            emulator,
            "sqs",
            "AmazonSQS",
            "DeleteMessage",
            &json!({"QueueUrl": url, "ReceiptHandle": receipt}),
        )
        .await;
    }
    arrived
}

/// The JSON document one history item records, out of its `HistoryData`.
fn history_document(item: &Value) -> Value {
    let data = item["HistoryData"].as_str().unwrap_or_default();
    serde_json::from_str(data)
        .unwrap_or_else(|error| panic!("HistoryData is not JSON ({error}): {data}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_executed_action_is_recorded_as_the_delivery_pass_executes_it() {
    // The rows differ along the fan-out of ONE pass: the number of notifications it delivers
    // and the number of action ARNs each of them fires — the product the records of the
    // earlier deliveries could be deferred behind.
    let rows = vec![
        Row {
            case: "one alarm firing one action",
            alarms: 1,
            actions: 1,
        },
        Row {
            case: "three alarms firing one action each",
            alarms: 3,
            actions: 1,
        },
        Row {
            // "AlarmActions (ResourceList, len 0..5)"
            // (`smithy_query(query="operation",operation="PutMetricAlarm")`) — the most action
            // ARNs one state may select.
            case: "one alarm firing the five actions a state may carry",
            alarms: 1,
            actions: 5,
        },
        Row {
            case: "three alarms firing two actions each",
            alarms: 3,
            actions: 2,
        },
    ];

    for row in &rows {
        let case = row.case;
        // A fresh emulator per row, so what the queue holds can only be this row's.
        let emulator = emulator();
        let (url, queue_arn) = given_queue(&emulator, "action-records").await;
        grant_sns_delivery_on(&emulator, &url, &queue_arn).await;
        let mut topics = Vec::new();
        for index in 0..row.actions {
            let topic = element(
                &sns(
                    &emulator,
                    "CreateTopic",
                    &[("Name", &format!("alarm-actions-{index}"))],
                )
                .await,
                "TopicArn",
            );
            subscribe_queue(&emulator, &topic, &queue_arn).await;
            topics.push(topic);
        }

        // The breaching datapoint is published BEFORE the alarms, so the first evaluation of
        // each of them — the one its own write makes due, a span later — is its transition
        // into ALARM, and the pass that settles them delivers every notification they owe.
        cw(
            &emulator,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "MetricData": [{
                    "MetricName": METRIC,
                    "Dimensions": row.dimensions(),
                    "Value": BREACHING,
                    // Inside the newest window an evaluation GRADES: an alarm grades the periods
                    // that have closed, not the one the clock is in.
                    "Timestamp": graded_window_stamp(PERIOD_SECS),
                }],
            }),
        )
        .await;
        let names = row.alarm_names();
        for name in &names {
            cw(&emulator, "PutMetricAlarm", &row.alarm(name, &topics)).await;
        }

        // One notification per transition per action ARN: what the pass has to deliver, and
        // what it has to record.
        let owed = row.alarms * row.actions;
        let deadline = Instant::now() + DEADLINE;
        let mut delivered: Vec<Value> = Vec::new();
        loop {
            delivered.extend(drain(&emulator, &url).await);
            let recorded = recorded_actions(&emulator, &names).await;
            // THE ORDER: the record of one execution is written before the next execution of
            // the pass is made, so every delivery a client can see but the newest is already
            // accounted for. Read while the pass is still delivering, which is the only time
            // the difference is observable.
            assert!(
                recorded >= delivered.len().saturating_sub(1),
                "case {case:?}: {} notifications have arrived, so every one of them but the \
                 last must already be recorded in the history of the alarm that published it; \
                 got {recorded} Action items",
                delivered.len()
            );
            if delivered.len() >= owed && recorded >= owed {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the pass owes {owed} deliveries and {owed} records; got {} \
                 deliveries and {recorded} records",
                delivered.len()
            );
            tokio::time::sleep(LOOK_AGAIN).await;
        }
        assert_eq!(
            delivered.len(),
            owed,
            "case {case:?}: each transition must publish its notification to each action ARN \
             once, got {delivered:?}"
        );
        for notification in &delivered {
            assert_eq!(
                notification["NewStateValue"].as_str(),
                Some("ALARM"),
                "case {case:?}: the published datapoint must be what every delivery reports, \
                 got {notification}"
            );
        }

        // What each alarm's own history records about it: one item per action ARN it fired,
        // each naming that ARN, the state that selected it and the outcome of that one
        // execution.
        for name in &names {
            let items = action_items(&emulator, name).await;
            assert_eq!(
                items.len(),
                row.actions,
                "case {case:?}: {name} fired {} actions, so its history must record that many \
                 executions, got {items:?}",
                row.actions
            );
            let mut executed: Vec<String> = Vec::new();
            for item in &items {
                let document = history_document(item);
                assert_eq!(
                    (
                        document["actionState"].as_str(),
                        document["stateValue"].as_str(),
                        document["error"].as_null(),
                    ),
                    (Some("Succeeded"), Some("ALARM"), Some(())),
                    "case {case:?}: {name} delivered this action, so the record must report it \
                     as succeeded for the state that selected it, got {document}"
                );
                let resource = document["notificationResource"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    item["HistorySummary"].as_str(),
                    Some(format!("Successfully executed action {resource}").as_str()),
                    "case {case:?}: {name} must summarise the execution by the action it \
                     executed, got {item}"
                );
                executed.push(resource);
            }
            executed.sort();
            let mut configured = topics.clone();
            configured.sort();
            assert_eq!(
                executed, configured,
                "case {case:?}: {name} must record one execution per action ARN it carries, \
                 got {executed:?}"
            );
        }
    }
}

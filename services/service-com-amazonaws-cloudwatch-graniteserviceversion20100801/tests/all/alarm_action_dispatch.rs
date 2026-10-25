// SPDX-License-Identifier: CC0-1.0

//! The ALARM-ACTION DISPATCH slice, end to end over the wire: which transitions execute an
//! alarm's actions, and what the alarm's history records about each execution.
//!
//! Three rules are asserted here, each over a TABLE whose rows differ along what the rule
//! generalises over rather than along a name:
//!
//! 1. `EnableAlarmActions` and `DisableAlarmActions` write `ActionsEnabled` on every alarm
//!    they name, whatever kind of alarm it is, and a state transition executes the actions
//!    configured for the state it moved to only while that member is true. Rows differ in the
//!    kind of alarm, in the setting the alarm was written with and in the setting the call
//!    asks for.
//! 2. An action execution is recorded in the alarm's history as one item per action ARN,
//!    carrying whether it succeeded, and for a failure the error the invoked service answered
//!    with — and for a failure NO published message, because nothing received one. Rows differ
//!    in whether the action's target exists.
//! 3. The notification an alarm's transition publishes reports the transition: the state the
//!    alarm moved to, the state it moved from and the instant of the change, which is the
//!    instant `DescribeAlarms` reports as `StateTransitionedTimestamp`.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, grant_sns_delivery, now_secs, rpc, sns, wait_for_evaluation, Emulator,
};

/// The customer namespace every alarm in this suite watches.
const NAMESPACE: &str = "Custom/Dispatch";

/// The metric every alarm in this suite watches.
const METRIC: &str = "QueueDepth";

/// The threshold every alarm in this suite compares against.
const THRESHOLD: f64 = 50.0;

/// A value above [`THRESHOLD`]: a datapoint that breaches.
const BREACHING: f64 = 137.0;

/// A value below [`THRESHOLD`]: a datapoint that does not breach.
const CLEAR: f64 = 3.0;

/// The length of the period every alarm in this suite evaluates, in seconds.
const PERIOD_SECS: f64 = 60.0;

/// The CloudWatch target prefix, for `rpc`.
const CLOUDWATCH: (&str, &str) = ("monitoring", "GraniteServiceVersion20100801");

/// The SQS target prefix, for `rpc`.
const SQS: (&str, &str) = ("sqs", "AmazonSQS");

/// Call one CloudWatch operation, requiring a 200.
async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, CLOUDWATCH.0, CLOUDWATCH.1, operation, body).await
}

/// Call one SQS operation, requiring a 200.
async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, SQS.0, SQS.1, operation, body).await
}

/// A queue subscribed to a fresh topic: the path an alarm notification travels. Answers the
/// topic ARN and the queue URL.
async fn topic_with_subscribed_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let queue = sqs(emulator, "CreateQueue", &json!({"QueueName": name})).await;
    let queue_url = queue["QueueUrl"]
        .as_str()
        .expect("CreateQueue answers a QueueUrl")
        .to_string();
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": queue_url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .expect("GetQueueAttributes answers the QueueArn")
        .to_string();
    // The queue policy an SNS subscription needs to deliver to it: the service principal
    // `sns.amazonaws.com`, allowed `sqs:SendMessage` on this queue.
    sqs(
        emulator,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": queue_url,
            "Attributes": {"Policy": grant_sns_delivery(&queue_arn)},
        }),
    )
    .await;
    let topic_arn = element(&sns(emulator, "CreateTopic", &[("Name", name)]).await, "TopicArn");
    sns(
        emulator,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
        ],
    )
    .await;
    (topic_arn, queue_url)
}

/// The metric alarm this suite configures: one period, one evaluation period, and the same
/// topic on all three of its action lists, so a transition to ANY state has an action to
/// execute.
fn alarm(name: &str, topic_arn: &str, actions_enabled: bool) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Queue", "Value": name}],
        "Statistic": "Maximum",
        "Period": PERIOD_SECS,
        "EvaluationPeriods": 1,
        "Threshold": THRESHOLD,
        "ComparisonOperator": "GreaterThanThreshold",
        "ActionsEnabled": actions_enabled,
        "AlarmActions": [topic_arn],
        "OKActions": [topic_arn],
        "InsufficientDataActions": [topic_arn],
    })
}

/// Publish `value` into the series `queue` names, stamped `age_secs` before now, so successive
/// publishes land in successive evaluation periods.
async fn publish(emulator: &Emulator, queue: &str, value: f64, age_secs: f64) {
    cw(
        emulator,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": METRIC,
                "Dimensions": [{"Name": "Queue", "Value": queue}],
                "Value": value,
                "Timestamp": now_secs() - age_secs,
            }],
        }),
    )
    .await;
}

/// Every message the queue holds, as the notification documents they carry.
async fn notifications(emulator: &Emulator, queue_url: &str) -> Vec<Value> {
    let received = sqs(
        emulator,
        "ReceiveMessage",
        &json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let envelope: Value = serde_json::from_str(
                message.get("Body").and_then(Value::as_str).unwrap_or_default(),
            )
            .expect("the delivered body is the SNS notification envelope");
            serde_json::from_str(envelope["Message"].as_str().unwrap_or_default())
                .expect("the alarm notification is JSON")
        })
        .collect()
}

/// Read the queue until it has delivered at least `want` notifications, and answer every one
/// it delivered.
///
/// Delivery is asynchronous: the call or the clock that materialises a transition publishes
/// what it owes, so a test waits for the notification instead of assuming the read that saw the
/// new state also saw the delivery.
async fn await_notifications(emulator: &Emulator, queue_url: &str, want: usize) -> Vec<Value> {
    let mut delivered: Vec<Value> = Vec::new();
    for _ in 0..STATE_READS {
        delivered.extend(notifications(emulator, queue_url).await);
        if delivered.len() >= want {
            break;
        }
        ::tokio::time::sleep(::std::time::Duration::from_millis(100)).await;
    }
    delivered
}

/// Read the alarm's history until it holds at least `want` items of `item_type`, and answer
/// them: an item recording an action is written when the action has been executed, which is
/// after the call that materialised the transition answered.
async fn await_history(
    emulator: &Emulator,
    name: &str,
    alarm_type: &str,
    item_type: &str,
    want: usize,
) -> Vec<Value> {
    let mut items = Vec::new();
    for _ in 0..STATE_READS {
        items = history(emulator, name, alarm_type, item_type).await;
        if items.len() >= want {
            break;
        }
        ::tokio::time::sleep(::std::time::Duration::from_millis(100)).await;
    }
    items
}

/// The alarm's history items of `item_type`, for an alarm of `alarm_type`.
///
/// "If you omit this parameter, only metric alarm history is returned."
/// (`DescribeAlarmHistoryInput$AlarmTypes`) — so the kind is always asked for.
async fn history(
    emulator: &Emulator,
    name: &str,
    alarm_type: &str,
    item_type: &str,
) -> Vec<Value> {
    let reported = cw(
        emulator,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": name,
            "AlarmTypes": [alarm_type],
            "HistoryItemType": item_type,
        }),
    )
    .await;
    reported["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The metric alarm `DescribeAlarms` reports under `name`.
async fn described(emulator: &Emulator, name: &str) -> Value {
    let reported = cw(emulator, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    reported["MetricAlarms"][0].clone()
}

/// How many times [`await_state`] reads the alarm before it gives up: the transitions it waits
/// for are each owed a span of their own (an alarm's own evaluation, then its parent's), so a
/// chain of them needs several spans.
const STATE_READS: usize = 40;

/// Read the alarm of `alarm_type` that `DescribeAlarms` reports in `list` until it reports
/// `state`, and answer it; panic with what it last reported.
///
/// A transition is owed a span after the write that makes it due, so a test waits for the state
/// rather than assuming it is there — and it waits by AWAITING, so the service's own alarm
/// clock gets to run while it does.
async fn await_state(
    emulator: &Emulator,
    name: &str,
    alarm_type: &str,
    list: &str,
    state: &str,
) -> Value {
    let mut last = Value::Null;
    for _ in 0..STATE_READS {
        let reported = cw(
            emulator,
            "DescribeAlarms",
            &json!({"AlarmNames": [name], "AlarmTypes": [alarm_type]}),
        )
        .await;
        last = reported[list][0].clone();
        if last.get("StateValue").and_then(Value::as_str) == Some(state) {
            return last;
        }
        ::tokio::time::sleep(::std::time::Duration::from_millis(100)).await;
    }
    panic!("{name} must reach {state}; it last reported {last}")
}

#[tokio::test]
async fn an_alarm_whose_actions_are_disabled_transitions_without_notifying_until_they_are_on() {
    let emulator = emulator();
    let name = "suppressed";
    let (topic_arn, queue_url) = topic_with_subscribed_queue(&emulator, name).await;
    cw(
        &emulator,
        "PutMetricAlarm",
        &alarm(name, &topic_arn, false),
    )
    .await;
    wait_for_evaluation();

    // The datapoint the alarm watches breaches, so the alarm transitions — with its actions
    // off, the transition is all that happens.
    publish(&emulator, name, BREACHING, 0.0).await;
    let reported = await_state(&emulator, name, "MetricAlarm", "MetricAlarms", "ALARM").await;
    assert_eq!(
        reported.get("StateValue").and_then(Value::as_str),
        Some("ALARM"),
        "with actions disabled the alarm must still transition, got {reported}"
    );
    assert_eq!(
        reported.get("ActionsEnabled").and_then(Value::as_bool),
        Some(false),
        "the alarm must report the ActionsEnabled it was written with, got {reported}"
    );
    let withheld = notifications(&emulator, &queue_url).await;
    assert!(
        withheld.is_empty(),
        "an alarm whose actions are disabled must notify nothing, got {withheld:?}"
    );
    let executed = history(&emulator, name, "MetricAlarm", "Action").await;
    assert!(
        executed.is_empty(),
        "a withheld notification must record no Action item, got {executed:?}"
    );

    // Actions on: the NEXT transition executes them, and nothing is replayed for the
    // transition that happened while they were off.
    cw(
        &emulator,
        "EnableAlarmActions",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    let reported = described(&emulator, name).await;
    assert_eq!(
        reported.get("ActionsEnabled").and_then(Value::as_bool),
        Some(true),
        "EnableAlarmActions must write ActionsEnabled, got {reported}"
    );
    assert!(
        notifications(&emulator, &queue_url).await.is_empty(),
        "turning the actions on must not notify for a transition that already happened"
    );

    // Two more transitions, each driven the way `SetAlarmState` documents: "When the updated
    // state differs from the previous value, the action configured for the appropriate state
    // is invoked." Each one executes the list its own state selects, so the queue holds one
    // notification per transition, in the order the transitions happened.
    for state in ["OK", "ALARM"] {
        cw(
            &emulator,
            "SetAlarmState",
            &json!({
                "AlarmName": name,
                "StateValue": state,
                "StateReason": "driving the transition this suite asserts",
            }),
        )
        .await;
    }
    let delivered = await_notifications(&emulator, &queue_url, 2).await;
    let states: Vec<(&str, &str)> = delivered
        .iter()
        .map(|notification| {
            (
                notification["OldStateValue"].as_str().unwrap_or_default(),
                notification["NewStateValue"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        states,
        vec![("ALARM", "OK"), ("OK", "ALARM")],
        "each transition after the actions were turned on must publish one notification \
         naming the states it moved between, got {delivered:?}"
    );
    let executed = await_history(&emulator, name, "MetricAlarm", "Action", 2).await;
    assert_eq!(
        executed.len(),
        2,
        "each executed action must be recorded in the alarm's history, got {executed:?}"
    );
}

#[tokio::test]
async fn a_transition_the_clock_makes_due_notifies_without_another_cloudwatch_call() {
    let emulator = emulator();
    let name = "clock-driven";
    let (topic_arn, queue_url) = topic_with_subscribed_queue(&emulator, name).await;
    // An alarm over a series nothing has published into, whose missing datapoints are treated
    // as breaching: its first evaluation — the one the write makes due a span later — takes it
    // from the INSUFFICIENT_DATA its creation set to ALARM.
    let mut configured = alarm(name, &topic_arn, true);
    configured["TreatMissingData"] = json!("breaching");
    cw(&emulator, "PutMetricAlarm", &configured).await;

    // From here on NOTHING is asked of CloudWatch: only the queue is read. The notification
    // arrives because the transition the clock made due was materialised without a call.
    for _ in 0..40 {
        if !notifications(&emulator, &queue_url).await.is_empty() {
            return;
        }
        ::tokio::time::sleep(::std::time::Duration::from_millis(100)).await;
    }
    panic!(
        "the transition the clock made due must notify the alarm's action without another \
         CloudWatch call"
    );
}

/// One row of the enable/disable table.
struct ActionsRow {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The operation that writes the alarm, and the request that writes it.
    write: (&'static str, Value),
    /// The modelled `AlarmType` of the alarm, which is what `DescribeAlarms` reports it
    /// under and what a request has to ask for to see it.
    alarm_type: &'static str,
    /// Which list of `DescribeAlarms` the alarm comes back in.
    reported_in: &'static str,
    /// The operation this row calls on the alarm.
    call: &'static str,
    /// The `ActionsEnabled` the alarm must report afterwards.
    expected: bool,
}

#[tokio::test]
async fn enabling_and_disabling_actions_writes_actions_enabled_on_every_kind_of_alarm() {
    let emulator = emulator();
    let (topic_arn, _) = topic_with_subscribed_queue(&emulator, "actions-table").await;

    // Rows differ in the KIND of alarm (the three kinds `DescribeAlarms` reports), in the
    // setting the alarm was written with, and in which of the two operations is called — the
    // things the rule generalises over.
    let rows: Vec<ActionsRow> = vec![
        ActionsRow {
            case: "a metric alarm written with its actions off, enabled",
            write: ("PutMetricAlarm", alarm("metric-off", &topic_arn, false)),
            alarm_type: "MetricAlarm",
            reported_in: "MetricAlarms",
            call: "EnableAlarmActions",
            expected: true,
        },
        ActionsRow {
            case: "a metric alarm written with its actions on, disabled",
            write: ("PutMetricAlarm", alarm("metric-on", &topic_arn, true)),
            alarm_type: "MetricAlarm",
            reported_in: "MetricAlarms",
            call: "DisableAlarmActions",
            expected: false,
        },
        ActionsRow {
            case: "a metric alarm already enabled, enabled again",
            write: ("PutMetricAlarm", alarm("metric-again", &topic_arn, true)),
            alarm_type: "MetricAlarm",
            reported_in: "MetricAlarms",
            call: "EnableAlarmActions",
            expected: true,
        },
        ActionsRow {
            case: "a composite alarm written with its actions off, enabled",
            write: (
                "PutCompositeAlarm",
                json!({
                    "AlarmName": "composite-off",
                    "AlarmRule": "ALARM(metric-on)",
                    "ActionsEnabled": false,
                    "AlarmActions": [topic_arn],
                }),
            ),
            alarm_type: "CompositeAlarm",
            reported_in: "CompositeAlarms",
            call: "EnableAlarmActions",
            expected: true,
        },
        ActionsRow {
            case: "a composite alarm written with its actions on, disabled",
            write: (
                "PutCompositeAlarm",
                json!({
                    "AlarmName": "composite-on",
                    "AlarmRule": "ALARM(metric-on)",
                    "ActionsEnabled": true,
                    "AlarmActions": [topic_arn],
                }),
            ),
            alarm_type: "CompositeAlarm",
            reported_in: "CompositeAlarms",
            call: "DisableAlarmActions",
            expected: false,
        },
        ActionsRow {
            case: "a log alarm written with its actions on, disabled",
            write: (
                "PutLogAlarm",
                json!({
                    "AlarmName": "log-on",
                    "ScheduledQueryConfiguration": {
                        "QueryString": "fields @message | limit 1",
                        "ScheduledQueryRoleARN":
                            "arn:aws:iam::000000000000:role/cwl-scheduled-query",
                        "AggregationExpression": "count(*)",
                        "ScheduleConfiguration": {
                            "ScheduleExpression": "rate(5 minutes)",
                            "StartTimeOffset": 360,
                        },
                    },
                    "QueryResultsToEvaluate": 1,
                    "QueryResultsToAlarm": 1,
                    "Threshold": THRESHOLD,
                    "ComparisonOperator": "GreaterThanThreshold",
                    "ActionsEnabled": true,
                    "AlarmActions": [topic_arn],
                }),
            ),
            alarm_type: "LogAlarm",
            reported_in: "LogAlarms",
            call: "DisableAlarmActions",
            expected: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let (operation, request) = &row.write;
        cw(&emulator, operation, request).await;
        let name = request["AlarmName"]
            .as_str()
            .expect("every row names its alarm")
            .to_string();
        cw(&emulator, row.call, &json!({"AlarmNames": [name.clone()]})).await;

        // "If you omit this parameter, only metric alarms are returned."
        // (`DescribeAlarmsInput$AlarmTypes`) — so a row asks for its own kind.
        let reported = cw(
            &emulator,
            "DescribeAlarms",
            &json!({"AlarmNames": [name.clone()], "AlarmTypes": [row.alarm_type]}),
        )
        .await;
        let alarm = reported[row.reported_in][0].clone();
        assert_eq!(
            alarm.get("ActionsEnabled").and_then(Value::as_bool),
            Some(row.expected),
            "case {case:?}: expected ActionsEnabled {}, got {alarm}",
            row.expected
        );

        // A call that CHANGED the member is a configuration update, and one that asked for
        // the setting the alarm already had changed nothing to record.
        let changed = row.expected != request["ActionsEnabled"].as_bool().unwrap_or(true);
        let updates = history(&emulator, &name, row.alarm_type, "ConfigurationUpdate").await;
        let expected_updates = 1 + usize::from(changed);
        assert_eq!(
            updates.len(),
            expected_updates,
            "case {case:?}: expected {expected_updates} ConfigurationUpdate items, got \
             {updates:?}"
        );
    }

    // A name no alarm answers to is not refused: this operation models no error at all.
    cw(
        &emulator,
        "EnableAlarmActions",
        &json!({"AlarmNames": ["no-such-alarm"]}),
    )
    .await;
}

#[tokio::test]
async fn an_executed_action_is_recorded_with_the_targets_own_answer() {
    // (case, whether the action's topic still exists when the alarm fires, the action state
    //  the history must record)
    let rows = [
        ("the action's topic exists", true, "Succeeded"),
        ("the action's topic has been deleted", false, "Failed"),
    ];

    for (case, exists, action_state) in rows {
        let emulator = emulator();
        let name = if exists { "reachable" } else { "doomed" };
        let (topic_arn, queue_url) = topic_with_subscribed_queue(&emulator, name).await;
        if !exists {
            sns(&emulator, "DeleteTopic", &[("TopicArn", &topic_arn)]).await;
        }
        cw(&emulator, "PutMetricAlarm", &alarm(name, &topic_arn, true)).await;
        wait_for_evaluation();
        publish(&emulator, name, BREACHING, PERIOD_SECS).await;

        let reported =
            await_state(&emulator, name, "MetricAlarm", "MetricAlarms", "ALARM").await;
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some("ALARM"),
            "case {case:?}: the alarm must transition whether or not its action can run, got \
             {reported}"
        );
        let executed = await_history(&emulator, name, "MetricAlarm", "Action", 1).await;
        assert_eq!(
            executed.len(),
            1,
            "case {case:?}: expected one Action item for the one action ARN, got {executed:?}"
        );
        let data: Value = serde_json::from_str(
            executed[0]
                .get("HistoryData")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .expect("an Action item's HistoryData is JSON");
        assert_eq!(
            data["actionState"].as_str(),
            Some(action_state),
            "case {case:?}: expected actionState {action_state}, got {data}"
        );
        assert_eq!(
            data["notificationResource"].as_str(),
            Some(topic_arn.as_str()),
            "case {case:?}: the item must name the action ARN it executed, got {data}"
        );
        if exists {
            // The action ran: the record carries what was published and no error, and the
            // subscriber holds the same document.
            assert!(
                data["error"].is_null(),
                "case {case:?}: an action that ran must record no error, got {data}"
            );
            let published = data["publishedMessage"].as_str().unwrap_or_default();
            assert!(
                published.contains(name),
                "case {case:?}: the record must carry the message that was published, got \
                 {data}"
            );
            let delivered = await_notifications(&emulator, &queue_url, 1).await;
            assert_eq!(
                delivered.len(),
                1,
                "case {case:?}: expected one delivered notification, got {delivered:?}"
            );
        } else {
            // Nothing was handed over, so there is no published message to record — and the
            // error recorded is the one the invoked service answered with.
            assert!(
                data.get("publishedMessage").is_none(),
                "case {case:?}: a failed action must record no published message, got {data}"
            );
            assert_eq!(
                data["error"].as_str(),
                Some("Topic does not exist"),
                "case {case:?}: the item must record the target's own error, got {data}"
            );
        }
    }
}

#[tokio::test]
async fn a_notification_reports_the_transition_the_alarm_made() {
    let emulator = emulator();
    let name = "transition";
    let (topic_arn, queue_url) = topic_with_subscribed_queue(&emulator, name).await;
    cw(&emulator, "PutMetricAlarm", &alarm(name, &topic_arn, true)).await;
    wait_for_evaluation();
    publish(&emulator, name, BREACHING, PERIOD_SECS).await;

    let reported = await_state(&emulator, name, "MetricAlarm", "MetricAlarms", "ALARM").await;
    let delivered = await_notifications(&emulator, &queue_url, 1).await;
    assert_eq!(
        delivered.len(),
        1,
        "expected one notification for the one transition, got {delivered:?}"
    );
    let notification = &delivered[0];
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
        notification["AlarmArn"].as_str(),
        reported.get("AlarmArn").and_then(Value::as_str),
        "the notification must name the alarm by the ARN DescribeAlarms reports, got \
         {notification}"
    );
    // One instant, read two ways: the text the notification carries and the number
    // `DescribeAlarms` reports must name the same moment.
    let change_time = notification["StateChangeTime"].as_str().unwrap_or_default();
    let transitioned = reported
        .get("StateTransitionedTimestamp")
        .and_then(Value::as_f64)
        .expect("DescribeAlarms reports when the state last changed");
    let parsed = ::aws_smithy_types::DateTime::from_str(
        change_time,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .expect("the notification's StateChangeTime is an RFC 3339 instant")
    .as_secs_f64();
    assert!(
        (parsed - transitioned).abs() < 0.001,
        "the notification's StateChangeTime must be the instant DescribeAlarms reports as \
         StateTransitionedTimestamp ({transitioned}), got {notification}"
    );
    // The reason the notification carries is the alarm's own, and it names the datapoint the
    // evaluation graded.
    let reason = notification["NewStateReason"].as_str().unwrap_or_default();
    assert_eq!(
        Some(reason),
        reported.get("StateReason").and_then(Value::as_str),
        "the notification's reason must be the alarm's own, got {notification}"
    );
    assert!(
        reason.contains(&format!("[{BREACHING:.1} (")),
        "the reason must cite the datapoint that was graded, got {reason:?}"
    );
}

#[tokio::test]
async fn a_composite_alarm_notifies_once_its_rule_holds_and_names_the_children_it_read() {
    let emulator = emulator();
    let (topic_arn, queue_url) = topic_with_subscribed_queue(&emulator, "composite").await;
    // Two metric alarms whose own actions are off, so every notification on the queue is the
    // composite's, and one composite over both of them: "ALARM(a) AND ALARM(b)".
    for child in ["child-a", "child-b"] {
        cw(&emulator, "PutMetricAlarm", &alarm(child, &topic_arn, false)).await;
    }
    cw(
        &emulator,
        "PutCompositeAlarm",
        &json!({
            "AlarmName": "composite",
            "AlarmRule": "ALARM(child-a) AND ALARM(child-b)",
            "AlarmActions": [topic_arn],
        }),
    )
    .await;

    // One child breaching and the other clear: the rule is false, so the composite is in OK
    // and has nothing to notify (its OK actions are not configured).
    publish(&emulator, "child-a", BREACHING, 0.0).await;
    publish(&emulator, "child-b", CLEAR, 0.0).await;
    let first = await_state(&emulator, "child-a", "MetricAlarm", "MetricAlarms", "ALARM").await;
    await_state(&emulator, "child-b", "MetricAlarm", "MetricAlarms", "OK").await;
    let half = await_state(
        &emulator,
        "composite",
        "CompositeAlarm",
        "CompositeAlarms",
        "OK",
    )
    .await;
    assert!(
        notifications(&emulator, &queue_url).await.is_empty(),
        "an AND rule with one child in ALARM must notify nothing, got {half}"
    );

    // The second child reaches ALARM, so the rule holds and the composite notifies ONCE.
    publish(&emulator, "child-b", BREACHING, 0.0).await;
    let second = await_state(&emulator, "child-b", "MetricAlarm", "MetricAlarms", "ALARM").await;
    let fired = await_state(
        &emulator,
        "composite",
        "CompositeAlarm",
        "CompositeAlarms",
        "ALARM",
    )
    .await;
    let delivered = await_notifications(&emulator, &queue_url, 1).await;
    assert_eq!(
        delivered.len(),
        1,
        "the composite must notify exactly once, when the second child arrives, got \
         {delivered:?}"
    );
    assert!(
        notifications(&emulator, &queue_url).await.is_empty(),
        "the composite's one transition must notify once, not again"
    );
    let notification = &delivered[0];
    assert_eq!(
        notification["AlarmRule"].as_str(),
        Some("ALARM(child-a) AND ALARM(child-b)"),
        "the notification must report the rule that was evaluated, got {notification}"
    );
    // The children the rule read, each by the ARN `DescribeAlarms` reports for it and with the
    // state it was in, in the order the rule names them.
    let arn_of = |child: &Value| {
        child
            .get("AlarmArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let children: Vec<(String, String)> = notification["TriggeringChildren"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|child| {
            (
                child["Arn"].as_str().unwrap_or_default().to_string(),
                child["State"]["Value"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        children,
        vec![
            (arn_of(&first), "ALARM".to_string()),
            (arn_of(&second), "ALARM".to_string()),
        ],
        "the notification must name the children the rule read and the state each was in, got \
         {notification}"
    );
    // The composite's own explanation attributes the change to the child that completed the
    // rule, and its reason data reports the same children.
    let reason = fired
        .get("StateReason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        reason.starts_with(&format!("{} transitioned to ALARM", arn_of(&second))),
        "the reason must name the child whose transition completed the rule, got {reason:?}"
    );
    let data: Value = serde_json::from_str(
        fired
            .get("StateReasonData")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .expect("a composite alarm's StateReasonData is JSON");
    let triggering: Vec<(String, String)> = data["triggeringAlarms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|child| {
            (
                child["arn"].as_str().unwrap_or_default().to_string(),
                child["state"]["value"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        triggering, children,
        "the reason data must report the same children the notification does, got {data}"
    );
}

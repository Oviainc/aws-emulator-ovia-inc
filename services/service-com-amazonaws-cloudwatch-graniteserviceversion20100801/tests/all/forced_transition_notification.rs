// SPDX-License-Identifier: CC0-1.0

//! WHAT the notification a forced transition publishes carries, and WHOSE dispatch the
//! `ActionsEnabled` switch gates — end to end over the wire with CloudWatch, SNS and SQS on one
//! engine, so each document is read where a client reads it: off the queue subscribed to the
//! alarm's action topic.
//!
//! "Temporarily sets the state of an alarm for testing purposes. When the updated state differs
//! from the previous value, the action configured for the appropriate state is invoked."
//! (`smithy_query(query="operation",operation="SetAlarmState")`) — so a force that moves the alarm
//! publishes to the ARNs configured for the state it moved TO, and every variable part of the
//! document it publishes is taken from THAT request and from the alarm's own stored configuration:
//! the alarm it names, the state it moved to, the state it moved from and the reason the request
//! sent, beside the metric configuration the alarm was written with. Nothing in it is decided by
//! anything else.
//!
//! Two rules are asserted here, each over a table whose rows differ along what the rule
//! generalises over rather than along a name:
//!
//! 1. The document's contents. Rows differ in the state forced (so in which of the three action
//!    lists is the one configured), in the state the alarm is moved out of, in the reason's text
//!    and length — up to the longest `StateReason` the model allows
//!    (`smithy_query(query="shape",shape="StateReason")`, `@length 0..1023`) — and in the shape of
//!    the alarm's own configuration, from a plain statistic to an extended statistic over a
//!    dimensioned metric.
//! 2. The switch is PER ALARM: `DisableAlarmActions` withholds the dispatch of the alarms it
//!    names and of no others, even where another alarm's actions point at the same topic, and
//!    `EnableAlarmActions` restores it for the next differing transition. A target that never
//!    dispatches at all would pass neither leg.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, grant_sns_delivery_from, rpc, sns, subscribe_queue, Emulator,
    TARGET_PREFIX,
};

/// The signing name CloudWatch's own requests are scoped by.
const CLOUDWATCH: &str = "monitoring";

/// The SQS signing name and target prefix, for `rpc`.
const SQS: (&str, &str) = ("sqs", "AmazonSQS");

/// Call one CloudWatch operation, requiring a 200.
async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, CLOUDWATCH, TARGET_PREFIX, operation, body).await
}

/// Call one SQS operation, requiring a 200.
async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, SQS.0, SQS.1, operation, body).await
}

/// A topic with a queue of this emulator subscribed to it, the queue's policy granting the SNS
/// service principal the send and scoping that grant to this topic. Answers the topic's ARN and
/// the queue's URL.
async fn given_topic_with_subscriber(emulator: &Emulator, name: &str) -> (String, String) {
    let (url, queue_arn) = given_queue(emulator, name).await;
    let topic = element(
        &sns(emulator, "CreateTopic", &[("Name", name)]).await,
        "TopicArn",
    );
    grant_sns_delivery_from(emulator, &url, &queue_arn, &[&topic]).await;
    subscribe_queue(emulator, &topic, &queue_arn).await;
    (topic, url)
}

/// Force `name` into `state` with `reason`, which is the call whose dispatch this suite reads.
async fn force(emulator: &Emulator, name: &str, state: &str, reason: &str) {
    cw(
        emulator,
        "SetAlarmState",
        &json!({"AlarmName": name, "StateValue": state, "StateReason": reason}),
    )
    .await;
}

/// Turn the actions of the alarms `names` on or off, through the operation that writes the switch.
async fn switch_actions(emulator: &Emulator, names: &[&str], enabled: bool) {
    let operation = if enabled {
        "EnableAlarmActions"
    } else {
        "DisableAlarmActions"
    };
    cw(emulator, operation, &json!({"AlarmNames": names})).await;
}

/// Everything waiting on the queue at `url`, as the alarm notification each message carries,
/// paired with the `TopicArn` of the SNS envelope it arrived in.
///
/// What it receives it also DELETES: a received message a consumer does not delete becomes
/// visible again when its visibility timeout runs out, so a later drain of the same queue would
/// count a notification this one already accounted for.
async fn notifications(emulator: &Emulator, url: &str) -> Vec<(String, Value)> {
    let received = sqs(
        emulator,
        "ReceiveMessage",
        // The long poll a client waiting for an asynchronous notification uses: "The duration (in
        // seconds) for which the call waits for a message to arrive in the queue before
        // returning." (`ReceiveMessageRequest$WaitTimeSeconds`)
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 1}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    for message in &messages {
        if let Some(receipt) = message["ReceiptHandle"].as_str() {
            sqs(
                emulator,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": receipt}),
            )
            .await;
        }
    }
    messages
        .iter()
        .map(|message| {
            let body = message["Body"].as_str().unwrap_or_default();
            let envelope: Value = serde_json::from_str(body)
                .unwrap_or_else(|error| panic!("the body is not an SNS envelope ({error}): {body}"));
            let inner = envelope["Message"].as_str().unwrap_or_default();
            let notification: Value = serde_json::from_str(inner).unwrap_or_else(|error| {
                panic!("the message is not an alarm notification ({error}): {inner}")
            });
            (
                envelope["TopicArn"].as_str().unwrap_or_default().to_string(),
                notification,
            )
        })
        .collect()
}

/// The members of a metric alarm's configuration the notification's `Trigger` reports under the
/// SAME names the model gives them (`MetricAlarm$Namespace`, `$MetricName`, `$Statistic`,
/// `$ExtendedStatistic`, `$Unit`, `$Period`, `$EvaluationPeriods`, `$DatapointsToAlarm`,
/// `$ComparisonOperator`, `$Threshold`, `$TreatMissingData`,
/// `smithy_query(query="shape",shape="MetricAlarm")`). Each is asserted on the rows that carry it,
/// which is what makes the trigger the alarm's OWN configuration rather than a fixed document.
const TRIGGER_MEMBERS: &[&str] = &[
    "Namespace",
    "MetricName",
    "Statistic",
    "ExtendedStatistic",
    "Unit",
    "Period",
    "EvaluationPeriods",
    "DatapointsToAlarm",
    "ComparisonOperator",
    "Threshold",
    "TreatMissingData",
];

/// The longest `StateReason` a caller may send
/// (`smithy_query(query="shape",shape="StateReason")`, `@length 0..1023`).
const MAX_STATE_REASON_CHARS: usize = 1023;

/// Which action list a state's ARNs live on: "The actions to execute when this alarm transitions
/// to the ALARM state from any other state" (`PutMetricAlarmInput$AlarmActions`), and the same for
/// `OKActions` and `InsufficientDataActions`.
fn action_member(state: &str) -> &'static str {
    match state {
        "ALARM" => "AlarmActions",
        "OK" => "OKActions",
        _ => "InsufficientDataActions",
    }
}

/// One row of the document table.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The state the row forces LAST: the one whose actions must be invoked.
    state: &'static str,
    /// The state the row forces FIRST, which the alarm carries no action for — so that force
    /// moves the alarm without publishing anything, and the state it leaves behind is what the
    /// delivered notification must report as the state moved from.
    from: &'static str,
    /// The reason the last force sends, which the notification must carry back.
    reason: String,
    /// The alarm's own metric configuration, as `PutMetricAlarm` is asked for it.
    configuration: Value,
}

/// The table: one row per shape of configuration, state and reason this rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "ALARM out of OK, a plain statistic",
            state: "ALARM",
            from: "OK",
            reason: "behavioral baseline".to_string(),
            configuration: json!({
                "Namespace": "Custom/Dispatch",
                "MetricName": "Errors",
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "TreatMissingData": "missing",
            }),
        },
        Row {
            case: "OK out of ALARM, an extended statistic over a dimensioned metric",
            state: "OK",
            from: "ALARM",
            // Punctuation and a non-ASCII character: the reason travels as a JSON string inside
            // another JSON string, so what the request sent must come back byte for byte.
            reason: "latency recovered: 12.5 ms ≤ 250.5 ms — \"within budget\"".to_string(),
            configuration: json!({
                "Namespace": "Custom/Latency",
                "MetricName": "RequestLatency",
                "ExtendedStatistic": "p99",
                "Dimensions": [{"Name": "Queue", "Value": "orders"}],
                "Unit": "Milliseconds",
                "Period": 300,
                "EvaluationPeriods": 2,
                "DatapointsToAlarm": 2,
                "Threshold": 250.5,
                "ComparisonOperator": "LessThanOrEqualToThreshold",
            }),
        },
        Row {
            case: "INSUFFICIENT_DATA out of OK, with the longest reason the model allows",
            state: "INSUFFICIENT_DATA",
            from: "OK",
            // `StateReason` is `@length 0..1023`
            // (`smithy_query(query="shape",shape="StateReason")`): the bound itself, so the
            // document carries a reason of the greatest length a caller may send.
            reason: {
                let clause = "no heartbeat arrived ";
                let mut reason = clause.repeat(MAX_STATE_REASON_CHARS / clause.len());
                reason.push_str(&"x".repeat(MAX_STATE_REASON_CHARS - reason.len()));
                reason
            },
            configuration: json!({
                "Namespace": "Custom/Gaps",
                "MetricName": "Heartbeats",
                "Statistic": "Sum",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 0.0,
                "ComparisonOperator": "LessThanThreshold",
                "TreatMissingData": "breaching",
            }),
        },
    ]
}

#[tokio::test]
async fn a_forced_transition_publishes_the_request_and_the_alarms_own_configuration() {
    for row in rows() {
        let Row {
            case,
            state,
            from,
            reason,
            configuration,
        } = row;
        assert!(
            reason.chars().count() <= MAX_STATE_REASON_CHARS,
            "case {case:?}: the row's reason must stay inside the modelled length, got {}",
            reason.chars().count()
        );
        let emulator = emulator();
        let (topic, url) = given_topic_with_subscriber(&emulator, "state-triggered").await;
        let alarm = "errors-high";
        let mut request = configuration.clone();
        request["AlarmName"] = json!(alarm);
        // Only the list of the state the row forces carries the topic, so the first force below
        // has nothing to invoke and the second has exactly this one ARN.
        request[action_member(state)] = json!([&topic]);
        cw(&emulator, "PutMetricAlarm", &request).await;

        // The alarm is moved OUT of the state the row forces into, by a force whose own state has
        // no action configured: the transition happens and nothing is published for it.
        force(&emulator, alarm, from, "moving the alarm into place").await;
        let quiet = notifications(&emulator, &url).await;
        assert!(
            quiet.is_empty(),
            "case {case:?}: a transition to a state the alarm carries no action for must publish \
             nothing, got {quiet:?}"
        );

        force(&emulator, alarm, state, &reason).await;
        let delivered = notifications(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the forced transition must publish exactly one notification, got \
             {delivered:?}"
        );
        let (published_to, notification) = &delivered[0];
        assert_eq!(
            published_to, &topic,
            "case {case:?}: the notification must arrive from the alarm's action topic, got \
             {published_to:?}"
        );
        assert_eq!(
            notification["AlarmName"].as_str(),
            Some(alarm),
            "case {case:?}: the notification must name the alarm the request named, got \
             {notification}"
        );
        assert_eq!(
            notification["NewStateValue"].as_str(),
            Some(state),
            "case {case:?}: the notification must carry the state the request forced, got \
             {notification}"
        );
        assert_eq!(
            notification["OldStateValue"].as_str(),
            Some(from),
            "case {case:?}: the notification must carry the state the alarm left, got \
             {notification}"
        );
        assert_eq!(
            notification["NewStateReason"].as_str(),
            Some(reason.as_str()),
            "case {case:?}: the notification must carry the reason the request sent, got {}",
            notification["NewStateReason"]
        );

        // The configuration the alarm itself was written with, reported as what was evaluated.
        let trigger = &notification["Trigger"];
        for member in TRIGGER_MEMBERS {
            let Some(asked) = configuration.get(member) else {
                continue;
            };
            assert_eq!(
                trigger.get(member),
                Some(asked),
                "case {case:?}: the trigger must report the alarm's own {member}, expected \
                 {asked}, got {trigger}"
            );
        }
        // A dimension is spelled in the trigger the way a notification spells it — lower case —
        // where the alarm's own configuration spells it the way the model does.
        if let Some(dimensions) = configuration["Dimensions"].as_array() {
            let expected: Vec<Value> = dimensions
                .iter()
                .map(|dimension| json!({"name": dimension["Name"], "value": dimension["Value"]}))
                .collect();
            assert_eq!(
                trigger["Dimensions"],
                Value::Array(expected.clone()),
                "case {case:?}: the trigger must report the alarm's own dimensions, expected \
                 {expected:?}, got {trigger}"
            );
        }
        // The lists themselves travel with the document, so a consumer sees which ARNs the
        // transition fired at.
        assert_eq!(
            notification[action_member(state)],
            json!([&topic]),
            "case {case:?}: the notification must report the action list of the state moved to, \
             got {notification}"
        );
    }
}

#[tokio::test]
async fn the_actions_switch_withholds_only_the_alarms_it_names() {
    // Two alarms whose ALARM actions point at the SAME topic, so the only thing that can tell
    // their dispatches apart is the switch each one carries.
    let emulator = emulator();
    let (topic, url) = given_topic_with_subscriber(&emulator, "shared").await;
    let muted = "muted-alarm";
    let watched = "watched-alarm";
    for name in [muted, watched] {
        cw(
            &emulator,
            "PutMetricAlarm",
            &json!({
                "AlarmName": name,
                "Namespace": "Custom/Dispatch",
                "MetricName": "Errors",
                "Dimensions": [{"Name": "Alarm", "Value": name}],
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "TreatMissingData": "missing",
                "AlarmActions": [topic.as_str()],
            }),
        )
        .await;
    }
    switch_actions(&emulator, &[muted], false).await;

    // Both alarms are forced into ALARM, which is a state change for both.
    for name in [muted, watched] {
        force(&emulator, name, "ALARM", "measuring the suppression").await;
    }
    let delivered = notifications(&emulator, &url).await;
    assert_eq!(
        delivered.len(),
        1,
        "exactly the unmuted alarm's transition must publish, got {delivered:?}"
    );
    assert_eq!(
        delivered[0].1["AlarmName"].as_str(),
        Some(watched),
        "the delivered notification must be the unmuted alarm's, got {}",
        delivered[0].1
    );

    // The switch gates the dispatch, not the transition: both alarms moved.
    let described = cw(
        &emulator,
        "DescribeAlarms",
        &json!({"AlarmNames": [muted, watched]}),
    )
    .await;
    let alarms = described["MetricAlarms"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (name, enabled) in [(muted, false), (watched, true)] {
        let alarm = alarms
            .iter()
            .find(|alarm| alarm["AlarmName"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("DescribeAlarms must report {name}, got {described}"));
        assert_eq!(
            alarm["StateValue"].as_str(),
            Some("ALARM"),
            "{name} must report the state it was forced into, got {alarm}"
        );
        assert_eq!(
            alarm["ActionsEnabled"].as_bool(),
            Some(enabled),
            "{name} must report ActionsEnabled {enabled}, got {alarm}"
        );
    }

    // With the switch back on, the NEXT differing transition of the once-muted alarm delivers —
    // and the one withheld while it was off is not delivered late, so only this one arrives.
    switch_actions(&emulator, &[muted], true).await;
    force(&emulator, muted, "OK", "clearing before the retry").await;
    force(&emulator, muted, "ALARM", "the retry itself").await;
    let restored = notifications(&emulator, &url).await;
    assert_eq!(
        restored.len(),
        1,
        "the re-enabled alarm's next transition must publish exactly one notification, got \
         {restored:?}"
    );
    assert_eq!(
        (
            restored[0].1["AlarmName"].as_str(),
            restored[0].1["NewStateValue"].as_str(),
            restored[0].1["OldStateValue"].as_str(),
            restored[0].1["NewStateReason"].as_str(),
        ),
        (
            Some(muted),
            Some("ALARM"),
            Some("OK"),
            Some("the retry itself")
        ),
        "the restored notification must report the once-muted alarm's own transition, got {}",
        restored[0].1
    );
}

// SPDX-License-Identifier: CC0-1.0

//! A state a caller FORCES with `SetAlarmState` is TEMPORARY, and every force into a state the
//! alarm is not already in invokes the actions configured for that state — end to end over the
//! wire with CloudWatch, SNS and SQS on one engine, so each notification is asserted where a
//! client sees it: on the queue subscribed to the alarm's topic.
//!
//! "Temporarily sets the state of an alarm for testing purposes. When the updated state differs
//! from the previous value, the action configured for the appropriate state is invoked. […]
//! Metric alarms returns to their actual state quickly, often within seconds."
//! (`smithy_query(query="operation",operation="SetAlarmState")`) — so forcing a state twice over
//! is two state changes, because the evaluation takes the alarm back in between, and each of them
//! publishes to the alarm's `AlarmActions` topic while `ActionsEnabled` is true.
//!
//! The rows differ along what the rule generalises over: the Region the alarm and its topic live
//! in, how the subscribed queue's policy scopes the grant, and which state is forced; and the
//! cycle each row drives differs along whether the alarm's actions are enabled, so the same
//! force delivers or is withheld.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, rpc_in_region, sns_in_region, Emulator, PAST_EVALUATION_DELAY,
};

/// The CloudWatch target prefix, for `rpc_in_region`.
const CLOUDWATCH: (&str, &str) = ("monitoring", "GraniteServiceVersion20100801");

/// The SQS target prefix, for `rpc_in_region`.
const SQS: (&str, &str) = ("sqs", "AmazonSQS");

/// Call one CloudWatch operation in `region`, requiring a 200.
async fn cw(emulator: &Emulator, region: &str, operation: &str, body: &Value) -> Value {
    rpc_in_region(emulator, region, CLOUDWATCH.0, CLOUDWATCH.1, operation, body).await
}

/// Call one SQS operation in `region`, requiring a 200.
async fn sqs(emulator: &Emulator, region: &str, operation: &str, body: &Value) -> Value {
    rpc_in_region(emulator, region, SQS.0, SQS.1, operation, body).await
}

/// How the subscribed queue's policy grants SNS the send: the shape of the documented grant
/// ("you must ensure that the Amazon SQS queue policy allows the Amazon SNS topic to send
/// messages to the queue"), and whether it is scoped to the publishing topic.
#[derive(Debug, Clone, Copy)]
enum Grant {
    /// The service principal, allowed the send on this queue, unscoped.
    Unscoped,
    /// The same grant, scoped by condition to the topic that publishes.
    ScopedToTopic,
}

impl Grant {
    /// The policy document this grant is written as.
    fn policy(self, queue_arn: &str, topic_arn: &str) -> String {
        let mut statement = json!({
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": "sns.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        });
        if matches!(self, Self::ScopedToTopic) {
            statement["Condition"] = json!({"ArnEquals": {"aws:SourceArn": topic_arn}});
        }
        json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
    }
}

/// A topic in `region` with a queue of this emulator subscribed to it, the queue granting SNS
/// the send the way `grant` writes it. Answers the topic ARN and the queue's URL.
async fn given_topic_with_subscriber(
    emulator: &Emulator,
    region: &str,
    name: &str,
    grant: Grant,
) -> (String, String) {
    let created = sqs(emulator, region, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        region,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {name}: no QueueArn in {attributes}"))
        .to_string();
    let topic = element(
        &sns_in_region(emulator, region, "CreateTopic", &[("Name", name)]).await,
        "TopicArn",
    );
    sqs(
        emulator,
        region,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": {"Policy": grant.policy(&queue_arn, &topic)},
        }),
    )
    .await;
    sns_in_region(
        emulator,
        region,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
        ],
    )
    .await;
    (topic, url)
}

/// The alarm every row configures: one period, one evaluation period, the row's topic on the
/// action list of the state the row forces, and missing data treated the way the recorded
/// scenario treats it — so the alarm has no datapoint of its own and its evaluated state is
/// `INSUFFICIENT_DATA`.
fn alarm_request(name: &str, state: &str, topic: &str) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "Namespace": "Custom/ForcedState",
        "MetricName": "Errors",
        "Statistic": "Average",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 1.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "TreatMissingData": "missing",
    });
    // The action list is the one selected by the state the row forces: "The actions to execute
    // when this alarm transitions to the ALARM state" / "to the OK state" / "to the
    // INSUFFICIENT_DATA state" (`PutMetricAlarmInput$AlarmActions` / `$OKActions` /
    // `$InsufficientDataActions`).
    let member = match state {
        "ALARM" => "AlarmActions",
        "OK" => "OKActions",
        _ => "InsufficientDataActions",
    };
    request[member] = json!([topic]);
    request
}

/// Force `name` into `state` in `region`.
async fn force(emulator: &Emulator, region: &str, name: &str, state: &str) {
    cw(
        emulator,
        region,
        "SetAlarmState",
        &json!({
            "AlarmName": name,
            "StateValue": state,
            "StateReason": "driven by the test",
        }),
    )
    .await;
}

/// Turn the alarm's actions on or off, through the operation that writes that switch.
async fn switch_actions(emulator: &Emulator, region: &str, name: &str, enabled: bool) {
    let operation = if enabled {
        "EnableAlarmActions"
    } else {
        "DisableAlarmActions"
    };
    cw(emulator, region, operation, &json!({"AlarmNames": [name]})).await;
}

/// Every notification waiting on the queue at `url`, as the alarm documents SNS enveloped,
/// with the envelope's `TopicArn` beside each one.
async fn notifications(emulator: &Emulator, region: &str, url: &str) -> Vec<(String, Value)> {
    let received = sqs(
        emulator,
        region,
        "ReceiveMessage",
        // The long poll a client waiting for an asynchronous notification uses: "The duration
        // (in seconds) for which the call waits for a message to arrive in the queue before
        // returning." (`ReceiveMessageRequest$WaitTimeSeconds`)
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 1}),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let body = message["Body"].as_str().unwrap_or_default();
            let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("the body is not the SNS envelope ({error}): {body}")
            });
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

/// The alarm's state as `DescribeAlarms` reports it.
async fn reported_state(emulator: &Emulator, region: &str, name: &str) -> String {
    let described = cw(
        emulator,
        region,
        "DescribeAlarms",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Wait until every evaluation started before now is due, so the evaluation has taken back
/// whatever a caller forced.
///
/// A thread sleep rather than `tokio::time::sleep`: this workspace enables tokio's `macros`,
/// `rt` and `rt-multi-thread` features only, so there is no timer driver to await on.
fn wait_past_the_forced_span() {
    std::thread::sleep(PAST_EVALUATION_DELAY);
}

/// The runtime flavour the emulator boots on, so this row runs with the service's own alarm
/// clock really waking up between the legs rather than only when the test awaits.
///
/// What it adds over the rows above: the clock settles this store every time an evaluation
/// comes due, so a transition the clock materialises delivers what it owes — and NOTHING
/// else: the queue must not keep filling after the one notification each forced transition
/// publishes, because a client that drains the queue between its own legs has to see it run
/// dry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_clock_delivers_each_forced_transition_once_and_nothing_further() {
    let region = "us-east-1";
    let emulator = emulator();
    let (topic, url) =
        given_topic_with_subscriber(&emulator, region, "clock-driven", Grant::ScopedToTopic).await;
    let alarm = "errors-high";
    cw(
        &emulator,
        region,
        "PutMetricAlarm",
        &alarm_request(alarm, "ALARM", &topic),
    )
    .await;

    for leg in 1..=3 {
        force(&emulator, region, alarm, "ALARM").await;
        let delivered = notifications(&emulator, region, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "leg {leg}: the forced transition must publish exactly one notification, got \
             {delivered:?}"
        );
        // Long enough for several of the clock's wake-ups, each of which settles this store:
        // the alarm's evaluated state is reached once and nothing further is published for it.
        ::tokio::time::sleep(PAST_EVALUATION_DELAY * 3).await;
        let since = notifications(&emulator, region, &url).await;
        assert!(
            since.is_empty(),
            "leg {leg}: the clock must publish nothing beyond the transition's own \
             notification, got {since:?}"
        );
        let reclaimed = reported_state(&emulator, region, alarm).await;
        assert_eq!(
            reclaimed, "INSUFFICIENT_DATA",
            "leg {leg}: the clock's own evaluation must take the forced state back, got \
             {reclaimed:?}"
        );
    }
}

#[tokio::test]
async fn each_forced_transition_invokes_the_actions_of_the_state_it_moves_to() {
    // (case, Region, how the subscribed queue grants the send, the state forced)
    let rows: &[(&str, &str, Grant, &str)] = &[
        (
            "ALARM, with the grant scoped to the topic",
            "us-east-1",
            Grant::ScopedToTopic,
            "ALARM",
        ),
        (
            "ALARM in another Region, with an unscoped grant",
            "eu-west-2",
            Grant::Unscoped,
            "ALARM",
        ),
        (
            "OK, which fires the alarm's OK actions",
            "us-west-2",
            Grant::ScopedToTopic,
            "OK",
        ),
    ];

    for (case, region, grant, state) in rows {
        let emulator = emulator();
        let (topic, url) =
            given_topic_with_subscriber(&emulator, region, "state-triggered", *grant).await;
        let alarm = "errors-high";
        cw(
            &emulator,
            region,
            "PutMetricAlarm",
            &alarm_request(alarm, state, &topic),
        )
        .await;

        // The force itself: the state differs from the state the alarm was created in, so the
        // actions of the state it moved to are invoked.
        force(&emulator, region, alarm, state).await;
        let reported = reported_state(&emulator, region, alarm).await;
        assert_eq!(
            reported, *state,
            "case {case:?}: the alarm must report the state it was forced into, got {reported:?}"
        );
        let delivered = notifications(&emulator, region, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the forced transition must publish one notification, got \
             {delivered:?}"
        );
        let (published_to, notification) = &delivered[0];
        assert_eq!(
            published_to, &topic,
            "case {case:?}: the notification must come from the alarm's action topic, got \
             {published_to:?}"
        );
        assert_eq!(
            notification["AlarmName"].as_str(),
            Some(alarm),
            "case {case:?}: the notification must name the alarm, got {notification}"
        );
        assert_eq!(
            notification["NewStateValue"].as_str(),
            Some(*state),
            "case {case:?}: the notification must carry the state moved to, got {notification}"
        );
        assert_eq!(
            notification["Trigger"]["MetricName"].as_str(),
            Some("Errors"),
            "case {case:?}: the notification must name the metric the alarm watches, got \
             {notification}"
        );

        // "Metric alarms returns to their actual state quickly": the evaluation takes the alarm
        // back, and with no datapoint of this series and missing data treated as `missing` the
        // state it computes is INSUFFICIENT_DATA.
        wait_past_the_forced_span();
        let reclaimed = reported_state(&emulator, region, alarm).await;
        assert_eq!(
            reclaimed, "INSUFFICIENT_DATA",
            "case {case:?}: the evaluation must take the forced state back, got {reclaimed:?}"
        );

        // So forcing the SAME state again is another state change, and it invokes the same
        // actions again.
        force(&emulator, region, alarm, state).await;
        let again = notifications(&emulator, region, &url).await;
        assert_eq!(
            again.len(),
            1,
            "case {case:?}: forcing the state the evaluation took back must publish the \
             notification again, got {again:?}"
        );
        assert_eq!(
            again[0].1["OldStateValue"].as_str(),
            Some("INSUFFICIENT_DATA"),
            "case {case:?}: the second notification must report the state the alarm left, got \
             {}",
            again[0].1
        );
    }
}

#[tokio::test]
async fn disabling_the_actions_withholds_the_forced_transitions_notification() {
    // The same cycle as the suppression differential a client measures with: a positive control,
    // the same force with the actions off, and the same force again once they are back on. Rows
    // differ in the Region and in how the queue's policy scopes the grant.
    let rows: &[(&str, &str, Grant)] = &[
        ("the grant scoped to the topic", "us-east-1", Grant::ScopedToTopic),
        ("an unscoped grant in another Region", "ap-south-1", Grant::Unscoped),
    ];

    for (case, region, grant) in rows {
        let emulator = emulator();
        let (topic, url) =
            given_topic_with_subscriber(&emulator, region, "suppression", *grant).await;
        let alarm = "errors-high";
        cw(
            &emulator,
            region,
            "PutMetricAlarm",
            &alarm_request(alarm, "ALARM", &topic),
        )
        .await;

        // Leg 1: the positive control. The actions are enabled by default ("Indicates whether
        // actions should be executed during any changes to the alarm state. The default is
        // TRUE."), so the forced transition delivers.
        force(&emulator, region, alarm, "ALARM").await;
        let delivered = notifications(&emulator, region, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: leg 1 must deliver the forced transition's notification, got \
             {delivered:?}"
        );

        // Leg 2: the same force with the actions off. The state still changes — the switch gates
        // the dispatch, not the transition — and nothing is delivered.
        wait_past_the_forced_span();
        switch_actions(&emulator, region, alarm, false).await;
        force(&emulator, region, alarm, "ALARM").await;
        let reported = reported_state(&emulator, region, alarm).await;
        assert_eq!(
            reported, "ALARM",
            "case {case:?}: leg 2 must still move the alarm's state, got {reported:?}"
        );
        let withheld = notifications(&emulator, region, &url).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: leg 2 must deliver nothing while the actions are off, got \
             {withheld:?}"
        );

        // Leg 3: the actions back on, the same force once more. The notification withheld in
        // leg 2 is not delivered late: only this transition's own is.
        wait_past_the_forced_span();
        switch_actions(&emulator, region, alarm, true).await;
        force(&emulator, region, alarm, "ALARM").await;
        let restored = notifications(&emulator, region, &url).await;
        assert_eq!(
            restored.len(),
            1,
            "case {case:?}: leg 3 must deliver exactly the notification of the transition made \
             with the actions back on, got {restored:?}"
        );
        assert_eq!(
            restored[0].1["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: leg 3's notification must carry the state moved to, got {}",
            restored[0].1
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The `ActionsEnabled` suppression DIFFERENTIAL a client measures, over an alarm that has
//! been STANDING long enough for its own evaluation clock to have run between the legs — the
//! way a client drives it, one request at a time, with the service's clock ticking in
//! between.
//!
//! The rule asserted here: a forced transition into a state invokes that state's action list
//! whenever the state VALUE changes, however long the alarm has been standing and however
//! many evaluations the clock has made due since it was created, and `ActionsEnabled` is the
//! only thing that suppresses the invocation — so the same force delivers before
//! `DisableAlarmActions`, delivers nothing while it is in force, and delivers again after
//! `EnableAlarmActions`.
//!
//! What this adds over `forced_state_actions.rs`, which asserts the same differential: there,
//! every force follows its `PutMetricAlarm` within the span before the alarm's FIRST
//! evaluation comes due, so the suppression is measured on an alarm whose clock has never
//! run. Here each leg waits past that span first, on the runtime flavour the emulator boots
//! on, so the evaluations the clock owes have been materialised — and the state the force
//! moves the alarm OUT OF is the one the clock left behind rather than the one the create
//! wrote. A dispatch that only fires for the first transition of a freshly created alarm
//! passes there and fails here.
//!
//! The rows differ along what the rule generalises over: how missing data is treated (which
//! is what decides the state the clock leaves the alarm in, and so the state each force moves
//! out of) and how many evaluation spans pass before the force.

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, grant_sns_delivery_on, rpc, sns, subscribe_queue, Emulator,
    PAST_EVALUATION_DELAY,
};

/// The CloudWatch target prefix, for `rpc`.
const CLOUDWATCH: (&str, &str) = ("monitoring", "GraniteServiceVersion20100801");

/// The SQS target prefix, for `rpc`.
const SQS: (&str, &str) = ("sqs", "AmazonSQS");

/// How long a leg waits for the notification it expects before calling it undelivered.
///
/// Far above the span a delivery takes (the publish happens within the forcing call), and
/// driven off what has ARRIVED rather than off a fixed sleep, so a slow machine delays a leg
/// instead of breaking it.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How long the SUPPRESSED leg leaves the queue alone before reading it empty: the
/// differential a client measures is calibrated from the delivery it just timed, and the
/// delivery each leg here times happens inside the forcing call, so this is far above it
/// while keeping the suite quick.
const QUIET_SPAN: Duration = Duration::from_secs(2);

/// Call one CloudWatch operation, requiring a 200.
async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, CLOUDWATCH.0, CLOUDWATCH.1, operation, body).await
}

/// Call one SQS operation, requiring a 200.
async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, SQS.0, SQS.1, operation, body).await
}

/// Take every notification waiting on the queue at `url`, as the alarm documents SNS
/// enveloped — the long poll a client waiting for an asynchronous notification uses
/// ("The duration (in seconds) for which the call waits for a message to arrive in the queue
/// before returning", `ReceiveMessageRequest$WaitTimeSeconds`), with the messages it hands
/// over deleted so a later leg reads only what arrived since.
async fn drain(emulator: &Emulator, url: &str) -> Vec<Value> {
    let received = sqs(
        emulator,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 1}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut notifications = Vec::with_capacity(messages.len());
    for message in &messages {
        let body = message["Body"].as_str().unwrap_or_default();
        let envelope: Value = serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("the body is not the SNS envelope ({error}): {body}"));
        let inner = envelope["Message"].as_str().unwrap_or_default();
        notifications.push(
            serde_json::from_str(inner).unwrap_or_else(|error| {
                panic!("the message is not an alarm notification ({error}): {inner}")
            }),
        );
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                emulator,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
    }
    notifications
}

/// Drain the queue until `wanted` notifications have arrived or `deadline` runs out,
/// answering everything that arrived.
async fn drain_within(
    emulator: &Emulator,
    url: &str,
    wanted: usize,
    deadline: Duration,
) -> Vec<Value> {
    let until = std::time::Instant::now() + deadline;
    let mut arrived: Vec<Value> = Vec::new();
    while arrived.len() < wanted && std::time::Instant::now() < until {
        arrived.extend(drain(emulator, url).await);
    }
    arrived
}

/// Drain until `wanted` notifications have arrived or [`DELIVERY_DEADLINE`] runs out.
async fn drain_until(emulator: &Emulator, url: &str, wanted: usize) -> Vec<Value> {
    drain_within(emulator, url, wanted, DELIVERY_DEADLINE).await
}

/// Force `name` into `state`.
async fn force(emulator: &Emulator, name: &str, state: &str) {
    cw(
        emulator,
        "SetAlarmState",
        &json!({
            "AlarmName": name,
            "StateValue": state,
            "StateReason": "driven by the test",
        }),
    )
    .await;
}

/// Turn the alarm's actions on or off, through the operation that writes that switch, and
/// answer what `DescribeAlarms` then reports for the member.
async fn switch_actions(emulator: &Emulator, name: &str, enabled: bool) -> Option<bool> {
    let operation = if enabled {
        "EnableAlarmActions"
    } else {
        "DisableAlarmActions"
    };
    cw(emulator, operation, &json!({"AlarmNames": [name]})).await;
    let described = cw(emulator, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    described["MetricAlarms"][0]["ActionsEnabled"].as_bool()
}

/// The state `DescribeAlarms` reports for `name`.
async fn reported_state(emulator: &Emulator, name: &str) -> String {
    let described = cw(emulator, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The runtime flavour the emulator boots on, so the service's own alarm clock really wakes up
/// between the legs rather than only when the test awaits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_standing_alarms_forced_transition_delivers_unless_its_actions_are_off() {
    // (case, how missing data is treated, how many evaluation spans pass before each force)
    let rows: &[(&str, &str, u32)] = &[
        ("missing data treated as missing", "missing", 1),
        ("missing data ignored", "ignore", 1),
        ("several evaluation spans before each force", "notBreaching", 4),
    ];

    for (case, treat_missing_data, spans) in rows {
        let emulator = emulator();
        let (url, queue_arn) = given_queue(&emulator, "suppression").await;
        grant_sns_delivery_on(&emulator, &url, &queue_arn).await;
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "suppression")]).await,
            "TopicArn",
        );
        subscribe_queue(&emulator, &topic, &queue_arn).await;

        let alarm = "errors-high";
        cw(
            &emulator,
            "PutMetricAlarm",
            &json!({
                "AlarmName": alarm,
                "Namespace": "Custom/Suppression",
                "MetricName": "Errors",
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "TreatMissingData": treat_missing_data,
                "AlarmActions": [topic.clone()],
            }),
        )
        .await;

        // The one thing this suite adds: the alarm STANDS for the spans the row asks for, so
        // every evaluation the clock owes has been materialised before the first force.
        let standing = PAST_EVALUATION_DELAY * *spans;
        ::tokio::time::sleep(standing).await;

        // Leg 1, the positive control: the actions are enabled by default ("Indicates whether
        // actions should be executed during any changes to the alarm state. The default is
        // TRUE."), so the force into ALARM invokes the alarm's `AlarmActions`. The force into
        // OK before it is what makes the next one a state CHANGE whatever the clock left
        // behind, and it fires nothing itself: this alarm carries no `OKActions`.
        force(&emulator, alarm, "OK").await;
        force(&emulator, alarm, "ALARM").await;
        let delivered = drain_until(&emulator, &url, 1).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: leg 1 must deliver the forced transition into ALARM, got \
             {delivered:?}"
        );
        assert_eq!(
            delivered[0]["AlarmName"].as_str(),
            Some(alarm),
            "case {case:?}: the notification must name the alarm, got {}",
            delivered[0]
        );
        assert_eq!(
            delivered[0]["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the notification must carry the state moved to, got {}",
            delivered[0]
        );

        // Leg 2: the same differential with the actions off. The state still moves — the
        // switch gates the dispatch, not the transition — and nothing is delivered, however
        // long the alarm has been standing.
        let reported = switch_actions(&emulator, alarm, false).await;
        assert_eq!(
            reported,
            Some(false),
            "case {case:?}: DisableAlarmActions must turn the switch off, got {reported:?}"
        );
        ::tokio::time::sleep(standing).await;
        force(&emulator, alarm, "OK").await;
        force(&emulator, alarm, "ALARM").await;
        let moved = reported_state(&emulator, alarm).await;
        assert_eq!(
            moved, "ALARM",
            "case {case:?}: leg 2 must still move the alarm's state, got {moved:?}"
        );
        let withheld = drain_within(&emulator, &url, 1, QUIET_SPAN).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: leg 2 must deliver nothing while the actions are off, got \
             {withheld:?}"
        );

        // Leg 3: the actions back on, and the same force once more. The notification withheld
        // in leg 2 is not delivered late: only this transition's own is.
        let reported = switch_actions(&emulator, alarm, true).await;
        assert_eq!(
            reported,
            Some(true),
            "case {case:?}: EnableAlarmActions must turn the switch back on, got {reported:?}"
        );
        ::tokio::time::sleep(standing).await;
        force(&emulator, alarm, "OK").await;
        force(&emulator, alarm, "ALARM").await;
        let restored = drain_until(&emulator, &url, 1).await;
        assert_eq!(
            restored.len(),
            1,
            "case {case:?}: leg 3 must deliver the transition made with the actions back on, \
             got {restored:?}"
        );
        assert_eq!(
            restored[0]["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: leg 3's notification must carry the state moved to, got {}",
            restored[0]
        );
    }
}

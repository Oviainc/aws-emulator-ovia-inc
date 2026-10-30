// SPDX-License-Identifier: CC0-1.0

//! A COMPOSITE alarm over children whose states the metric evaluation of PUBLISHED DATAPOINTS
//! moves, end to end over the wire with CloudWatch, SNS and SQS on one engine — so what a
//! client sees is what is asserted: the state `DescribeAlarms` reports for the parent, the
//! explanation it reports with it, and the notification that reaches the queue subscribed to
//! the action's topic.
//!
//! The RULE asserted here: **a composite alarm's `StateValue` is "the rule that this alarm uses
//! to evaluate its alarm state" (`CompositeAlarm$AlarmRule`) applied to the CURRENT states of
//! the alarms that rule references — ALARM while the expression is true and OK while it is
//! false, `INSUFFICIENT_DATA` only before its first evaluation — and it is re-derived whenever
//! one of those alarms changes state, including a change the evaluation of published datapoints
//! makes. Each change of the parent's own `StateValue` writes its `StateReason` ("An explanation
//! for the alarm state, in text format"), which names the child whose transition completed the
//! rule and the state that child moved to, its `StateReasonData` ("An explanation for the alarm
//! state, in JSON format"), whose `triggeringAlarms` name every child the rule read a state
//! from with that state, and its `StateTransitionedTimestamp` ("The timestamp of the last change
//! to the alarm's StateValue"), and executes the action list of the state it moved to exactly
//! once — publishing a notification whose `Subject` is `<STATE>: "<AlarmName>" in <Region>` and
//! whose message reports the rule that was evaluated and the children it read (`AlarmRule`,
//! `TriggeringChildren`) and carries no metric `Trigger`, a composite alarm having no metric.**
//!
//! The rows differ along what that rule generalises over: which operators combine the state
//! functions and how they nest (a conjunction, a disjunction, a negation, a negation inside a
//! conjunction), whether the rule references a child by name or by ARN, which child's transition
//! completes the rule, in which direction that child moved, and the period and storage
//! resolution of the series its children are graded over. The recorded case — a conjunction of
//! two high-resolution children — is one row; the sibling suite
//! ([`crate::composite_rule_child_datapoints`]) drives that same row with datapoints published
//! as they happen rather than placed in a window.
//!
//! A drive places its datapoint in a period window the evaluation GRADES, and two drives of one
//! child are placed in different windows, so the value a drive means to be graded is the most
//! recent one its alarm finds: a window is aggregated with the alarm's `Statistic`, so two values
//! in ONE window would be one datapoint. Every state and every count here is AWAITED and then
//! asserted, because a transition is owed a span after the write that makes it due and the
//! delivery follows the transition.

use serde_json::{json, Value};
use std::time::Duration;

use crate::harness::{
    document_instant_secs, element, given_queue, graded_window_stamp, grant_sns_delivery_on, rpc,
    sns, subscribe_queue, wait_for_period_room, Emulator, REGION,
};

/// The namespace the children of every row watch.
pub const NAMESPACE: &str = "Custom/CompositeChildren";

/// The metric they watch.
pub const METRIC: &str = "Depth";

/// The threshold every child compares its windows against.
const THRESHOLD: f64 = 50.0;

/// A value above [`THRESHOLD`]: the window breaches.
pub const BREACHING: f64 = 137.0;

/// A value below it: the window does not breach.
pub const CLEARING: f64 = 3.0;

/// The two children every row creates.
const CHILD_NAMES: [&str; 2] = ["rule-child-a", "rule-child-b"];

/// The composite alarm every row creates.
pub const COMPOSITE: &str = "rule-parent";

/// How long a wait for a state the published data implies, or for a delivery that follows one,
/// keeps looking. Generous only so a failure is a failure rather than a hang: each transition is
/// owed a span inside the framework's sub-second window and the delivery follows it.
const DEADLINE: Duration = Duration::from_secs(20);

/// How often such a wait looks again.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// How long the queue is watched for a delivery that must NOT be made: several evaluation
/// spans, so an action executed again on a later evaluation of the same (unchanged) state
/// would show up.
const SETTLED_WATCH: Duration = Duration::from_millis(1_200);

/// The modelled alarm states this suite names.
pub const ALARM: &str = "ALARM";
pub const OK: &str = "OK";
pub const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

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

/// One Amazon SQS call, requiring a 200.
async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, "sqs", "AmazonSQS", operation, body).await
}

/// A topic in this emulator with a queue of this emulator subscribed to it, the queue granting
/// the SNS service principal the send it needs to receive anything. Answers the topic's ARN and
/// the queue's url.
pub async fn given_topic_with_subscribed_queue(
    emulator: &Emulator,
    name: &str,
) -> (String, String) {
    let (url, queue_arn) = given_queue(emulator, name).await;
    grant_sns_delivery_on(emulator, &url, &queue_arn).await;
    let topic = element(
        &sns(emulator, "CreateTopic", &[("Name", name)]).await,
        "TopicArn",
    );
    subscribe_queue(emulator, &topic, &queue_arn).await;
    (topic, url)
}

/// Which child a step drives, and which children a rule reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    /// The first child.
    A,
    /// The second child.
    B,
}

impl Child {
    /// The alarm name this child is created under.
    pub fn name(self) -> &'static str {
        match self {
            Self::A => CHILD_NAMES[0],
            Self::B => CHILD_NAMES[1],
        }
    }
}

/// One drive of a row: the datapoint it publishes, and what the parent must report once the
/// child it drives has been graded on it.
#[derive(Debug, Clone, Copy)]
struct Step {
    /// The child the datapoint belongs to.
    child: Child,
    /// Whether the value breaches the child's threshold.
    breaching: bool,
    /// How many period windows before the newest window the evaluation grades the datapoint is
    /// placed in, so two drives of one child are two datapoints rather than one window.
    windows_back: f64,
    /// The state the composite alarm must report once this step has been graded.
    composite: &'static str,
    /// Whether this step CHANGES the composite's state — and so executes the actions of the
    /// state it moved to exactly once. A step the rule's value is unchanged by executes none.
    transition: bool,
}

impl Step {
    /// The value this step publishes.
    fn value(self) -> f64 {
        if self.breaching {
            BREACHING
        } else {
            CLEARING
        }
    }

    /// The state the child this step drives must reach on it.
    fn child_state(self) -> &'static str {
        if self.breaching {
            ALARM
        } else {
            OK
        }
    }
}

/// One row of the table: a rule, the children it reads, the series its children are graded
/// over, and the drives that move them.
struct Row {
    /// What this row is, for every assertion message.
    case: &'static str,
    /// The rule, built from the two references — by name or by ARN, as `by_arn` decides.
    rule: fn(&str, &str) -> String,
    /// Whether the rule references its children by ARN rather than by name: "You can use either
    /// alarm names or ARNs to reference the other alarms that are to be evaluated."
    /// (`PutCompositeAlarmInput$AlarmRule`)
    by_arn: bool,
    /// The children the rule reads a state from.
    reads: &'static [Child],
    /// The children's `Period`, in seconds.
    period: f64,
    /// The `StorageResolution` their datapoints are published with, or `None` for the
    /// documented default of 60.
    storage_resolution: Option<i64>,
    /// The state the composite reports on its FIRST evaluation, before any datapoint: every
    /// child is then in `INSUFFICIENT_DATA`.
    initial: &'static str,
    /// The drives, in order.
    steps: &'static [Step],
}

/// The `PutMetricAlarm` request for one child graded over `period`-second windows.
pub fn child_alarm(period: f64, name: &str) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Child", "Value": name}],
        "Statistic": "Maximum",
        "Period": period,
        "EvaluationPeriods": 1,
        "Threshold": THRESHOLD,
        "ComparisonOperator": "GreaterThanThreshold",
        // "If you specify `ignore`, the current alarm state is maintained"
        // (`PutMetricAlarmInput$TreatMissingData`) — so a child graded on a drive keeps the
        // state that drive gave it once its datapoint has left the evaluation range, and the
        // parent's rule is read against the state its child really reached.
        "TreatMissingData": "ignore",
    })
}

/// How much of the current period window a drive waits for before it places its datapoint
/// relative to that window: a drive and the read of the state it decides then land in the
/// window the drive measured from, so the datapoint cannot be one window further back than the
/// row placed it by the time its alarm is graded.
const WINDOW_ROOM_SECS: f64 = 5.0;

/// Publish one step's datapoint into the series of the child it drives.
async fn drive(emulator: &Emulator, row: &Row, step: Step) {
    wait_for_period_room(row.period, WINDOW_ROOM_SECS).await;
    let mut datum = json!({
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Child", "Value": step.child.name()}],
        "Value": step.value(),
        "Timestamp": graded_window_stamp(row.period) - row.period * step.windows_back,
    });
    if let Some(resolution) = row.storage_resolution {
        datum["StorageResolution"] = json!(resolution);
    }
    cw(
        emulator,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": [datum]}),
    )
    .await;
}

/// The alarms `DescribeAlarms` reports for both kinds at once, the way a client polling for a
/// parent and its children reads them: "If you specify CompositeAlarms, the operation returns
/// only a list of composite alarms" (`DescribeAlarmsInput$AlarmTypes`), so both kinds are
/// asked for.
async fn described(emulator: &Emulator) -> Value {
    cw(
        emulator,
        "DescribeAlarms",
        &json!({"AlarmTypes": ["CompositeAlarm", "MetricAlarm"]}),
    )
    .await
}

/// One alarm out of that read, by the list it is reported under and its name.
pub async fn alarm_of(emulator: &Emulator, list: &str, name: &str) -> Value {
    described(emulator)
        .await
        .get(list)
        .and_then(Value::as_array)
        .and_then(|alarms| {
            alarms
                .iter()
                .find(|alarm| alarm.get("AlarmName").and_then(Value::as_str) == Some(name))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// Poll until the alarm `name` of `list` reports `state`, and answer it as it then stood.
pub async fn await_state(
    emulator: &Emulator,
    case: &str,
    list: &str,
    name: &str,
    state: &str,
) -> Value {
    let started = std::time::Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < DEADLINE {
        last = alarm_of(emulator, list, name).await;
        if last.get("StateValue").and_then(Value::as_str) == Some(state) {
            return last;
        }
        ::tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case {case:?}: {name} must reach {state}; it last reported {last}")
}

/// Take every notification waiting on the queue, as the subject and the alarm document SNS
/// enveloped, deleting what it hands over so a later step reads only what arrived since.
async fn take(emulator: &Emulator, case: &str, url: &str) -> Vec<(String, Value)> {
    let received = sqs(
        emulator,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut taken = Vec::with_capacity(messages.len());
    for message in &messages {
        let body = message["Body"].as_str().unwrap_or_default();
        let envelope: Value = serde_json::from_str(body).unwrap_or_else(|error| {
            panic!("case {case:?}: not the SNS envelope ({error}): {body}")
        });
        let inner = envelope["Message"].as_str().unwrap_or_default();
        let notification: Value = serde_json::from_str(inner).unwrap_or_else(|error| {
            panic!("case {case:?}: not an alarm notification ({error}): {inner}")
        });
        taken.push((
            envelope["Subject"].as_str().unwrap_or_default().to_string(),
            notification,
        ));
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                emulator,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
    }
    taken
}

/// Wait for `want` notifications, then keep watching for one more: a count asserted after that
/// watch is a count of what the transition really delivered rather than of what arrived first.
pub async fn awaited_notifications(
    emulator: &Emulator,
    case: &str,
    url: &str,
    want: usize,
) -> Vec<(String, Value)> {
    let started = std::time::Instant::now();
    let mut arrived: Vec<(String, Value)> = Vec::new();
    while started.elapsed() < DEADLINE && arrived.len() < want {
        arrived.extend(take(emulator, case, url).await);
        if arrived.len() < want {
            ::tokio::time::sleep(LOOK_AGAIN).await;
        }
    }
    let watch = std::time::Instant::now();
    while watch.elapsed() < SETTLED_WATCH {
        arrived.extend(take(emulator, case, url).await);
        ::tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert_eq!(
        arrived.len(),
        want,
        "case {case:?}: the transition must execute its actions exactly {want} time(s), got \
         {arrived:?}"
    );
    arrived
}

/// The `triggeringAlarms` of a composite alarm's `StateReasonData`, as (arn, state value) pairs.
fn triggering_alarms(case: &str, alarm: &Value) -> Vec<(String, String)> {
    let data = alarm["StateReasonData"].as_str().unwrap_or_default();
    let document: Value = serde_json::from_str(data).unwrap_or_else(|error| {
        panic!("case {case:?}: StateReasonData is not JSON ({error}): {data}")
    });
    document["triggeringAlarms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["arn"].as_str().unwrap_or_default().to_string(),
                entry["state"]["value"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Each child the rule reads, as its ARN and the state it is in.
async fn read_children(emulator: &Emulator, reads: &[Child]) -> Vec<(String, String)> {
    let mut states = Vec::with_capacity(reads.len());
    for child in reads {
        let alarm = alarm_of(emulator, "MetricAlarms", child.name()).await;
        states.push((
            alarm["AlarmArn"].as_str().unwrap_or_default().to_string(),
            alarm["StateValue"].as_str().unwrap_or_default().to_string(),
        ));
    }
    states.sort();
    states
}

/// Everything a composite alarm's transition owes: its explanation, the children that
/// explanation names, and the one notification it published.
#[allow(clippy::too_many_arguments)]
pub async fn assert_transition(
    emulator: &Emulator,
    case: &str,
    reads: &[Child],
    parent: &Value,
    trigger: Child,
    trigger_state: &str,
    previous: &str,
    rule: &str,
    url: &str,
) {
    let state = parent["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let trigger_arn = alarm_of(emulator, "MetricAlarms", trigger.name()).await["AlarmArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let reason = parent["StateReason"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let prefix = format!("{trigger_arn} transitioned to {trigger_state}");
    assert!(
        reason.starts_with(&prefix),
        "case {case:?}: the explanation must open with the child that completed the rule \
         ({prefix:?}), got {reason:?}"
    );

    let wanted = read_children(emulator, reads).await;
    let mut reported = triggering_alarms(case, parent);
    reported.sort();
    assert_eq!(
        reported, wanted,
        "case {case:?}: triggeringAlarms must name every child the rule read, with its state; \
         got {reported:?} for {wanted:?}"
    );

    let delivered = awaited_notifications(emulator, case, url, 1).await;
    let (subject, notification) = &delivered[0];
    assert_eq!(
        subject,
        &format!("{state}: \"{COMPOSITE}\" in {REGION}"),
        "case {case:?}: the subject must name the state, the alarm and its Region, got \
         {subject:?}"
    );
    for (member, value) in [
        ("AlarmName", Value::from(COMPOSITE)),
        ("AlarmArn", parent["AlarmArn"].clone()),
        ("NewStateValue", Value::from(state.clone())),
        ("OldStateValue", Value::from(previous)),
        ("NewStateReason", Value::from(reason.clone())),
        ("AlarmRule", Value::from(rule)),
        ("Region", Value::from(REGION)),
    ] {
        assert_eq!(
            notification.get(member).cloned().unwrap_or(Value::Null),
            value,
            "case {case:?}: the notification's {member} must report the transition, got \
             {notification}"
        );
    }
    // "The timestamp of the last change to the alarm's StateValue."
    // (`CompositeAlarm$StateTransitionedTimestamp`) — the instant the notification reports as
    // the change is that same instant.
    let change = document_instant_secs(notification["StateChangeTime"].as_str().unwrap_or_default())
        .unwrap_or_else(|| panic!("case {case:?}: no StateChangeTime in {notification}"));
    let transitioned = parent["StateTransitionedTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: no StateTransitionedTimestamp in {parent}"));
    assert!(
        (change - transitioned).abs() <= 0.001,
        "case {case:?}: StateChangeTime {change} must be the alarm's StateTransitionedTimestamp \
         {transitioned}"
    );
    // The children the rule read, as the envelope reports them — and no metric `Trigger`: a
    // composite alarm has no metric.
    let mut named: Vec<(String, String)> = notification["TriggeringChildren"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Arn"].as_str().unwrap_or_default().to_string(),
                entry["State"]["Value"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    named.sort();
    assert_eq!(
        named, wanted,
        "case {case:?}: TriggeringChildren must report every child the rule read, with its \
         state; got {named:?} for {wanted:?}"
    );
    assert!(
        notification.get("Trigger").is_none(),
        "case {case:?}: a composite alarm has no metric, so its notification carries no \
         Trigger: {notification}"
    );
}

/// The runtime flavour the booted emulator serves on, so the evaluation pass and the alarm
/// clock run the way they do in production rather than inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_composite_rule_follows_the_states_its_children_are_evaluated_into() {
    let rows: &[Row] = &[
        Row {
            // The recorded case: a conjunction of two high-resolution children.
            case: "a conjunction of two children, completed by the second one",
            rule: |a, b| format!("ALARM(\"{a}\") AND ALARM(\"{b}\")"),
            by_arn: false,
            reads: &[Child::A, Child::B],
            period: 10.0,
            // "data points with a period of less than 60 seconds ... are stored by a
            // PutMetricData call with a StorageResolution of 1"
            // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`).
            storage_resolution: Some(1),
            initial: OK,
            steps: &[
                Step {
                    child: Child::A,
                    breaching: true,
                    windows_back: 1.0,
                    composite: OK,
                    transition: false,
                },
                Step {
                    child: Child::B,
                    breaching: true,
                    windows_back: 1.0,
                    composite: ALARM,
                    transition: true,
                },
                Step {
                    child: Child::A,
                    breaching: false,
                    windows_back: 0.0,
                    composite: OK,
                    transition: true,
                },
            ],
        },
        Row {
            case: "a disjunction whose children are referenced by ARN",
            rule: |a, b| format!("ALARM(\"{a}\") OR ALARM(\"{b}\")"),
            by_arn: true,
            reads: &[Child::A, Child::B],
            period: 60.0,
            storage_resolution: None,
            initial: OK,
            steps: &[
                Step {
                    child: Child::A,
                    breaching: true,
                    windows_back: 1.0,
                    composite: ALARM,
                    transition: true,
                },
                Step {
                    child: Child::A,
                    breaching: false,
                    windows_back: 0.0,
                    composite: OK,
                    transition: true,
                },
            ],
        },
        Row {
            case: "a negation, which the child's ALARM makes false",
            rule: |a, _| format!("NOT ALARM(\"{a}\")"),
            by_arn: false,
            reads: &[Child::A],
            period: 30.0,
            storage_resolution: Some(1),
            initial: ALARM,
            steps: &[
                Step {
                    child: Child::A,
                    breaching: true,
                    windows_back: 1.0,
                    composite: OK,
                    transition: true,
                },
                Step {
                    child: Child::A,
                    breaching: false,
                    windows_back: 0.0,
                    composite: ALARM,
                    transition: true,
                },
            ],
        },
        Row {
            case: "a negation nested in a conjunction, completed by the negated child",
            rule: |a, b| format!("ALARM(\"{a}\") AND NOT OK(\"{b}\")"),
            by_arn: false,
            reads: &[Child::A, Child::B],
            period: 60.0,
            storage_resolution: None,
            initial: OK,
            steps: &[
                Step {
                    child: Child::A,
                    breaching: true,
                    windows_back: 1.0,
                    composite: ALARM,
                    transition: true,
                },
                Step {
                    child: Child::B,
                    breaching: true,
                    windows_back: 1.0,
                    composite: ALARM,
                    transition: false,
                },
                Step {
                    child: Child::B,
                    breaching: false,
                    windows_back: 0.0,
                    composite: OK,
                    transition: true,
                },
            ],
        },
    ];

    for row in rows {
        let case = row.case;
        // A fresh emulator per row, so what the queue holds can only be this row's.
        let emulator = crate::harness::emulator();
        let (topic, url) = given_topic_with_subscribed_queue(&emulator, "composite-actions").await;

        // The children, and how the rule references them.
        let mut references: Vec<String> = Vec::new();
        for child in [Child::A, Child::B] {
            cw(
                &emulator,
                "PutMetricAlarm",
                &child_alarm(row.period, child.name()),
            )
            .await;
            let created = await_state(
                &emulator,
                case,
                "MetricAlarms",
                child.name(),
                INSUFFICIENT_DATA,
            )
            .await;
            references.push(if row.by_arn {
                created["AlarmArn"].as_str().unwrap_or_default().to_string()
            } else {
                child.name().to_string()
            });
        }
        let rule = (row.rule)(&references[0], &references[1]);
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

        // "When this operation creates an alarm, the alarm state is immediately set to
        // INSUFFICIENT_DATA. The alarm is then evaluated and its state is set appropriately.
        // Any actions associated with the new state are then executed." (`PutCompositeAlarm`)
        let mut parent =
            await_state(&emulator, case, "CompositeAlarms", COMPOSITE, row.initial).await;
        let initial = awaited_notifications(&emulator, case, &url, 1).await;
        assert_eq!(
            initial[0].1["NewStateValue"].as_str(),
            Some(row.initial),
            "case {case:?}: the first evaluation must execute the actions of the state it \
             computed, got {}",
            initial[0].1
        );
        assert_eq!(
            initial[0].1["OldStateValue"].as_str(),
            Some(INSUFFICIENT_DATA),
            "case {case:?}: the first evaluation moves the alarm off the state its creation \
             left, got {}",
            initial[0].1
        );
        // The state each following transition moves FROM.
        let mut previous = row.initial.to_string();

        for (index, step) in row.steps.iter().enumerate() {
            drive(&emulator, row, *step).await;
            await_state(
                &emulator,
                case,
                "MetricAlarms",
                step.child.name(),
                step.child_state(),
            )
            .await;
            let reported =
                await_state(&emulator, case, "CompositeAlarms", COMPOSITE, step.composite).await;
            if step.transition {
                assert_transition(
                    &emulator,
                    case,
                    row.reads,
                    &reported,
                    step.child,
                    step.child_state(),
                    &previous,
                    &rule,
                    &url,
                )
                .await;
                previous = step.composite.to_string();
            } else {
                // The rule's value did not change, so nothing was executed and the explanation
                // the parent reports is still the one its last change wrote.
                awaited_notifications(&emulator, case, &url, 0).await;
                assert_eq!(
                    reported["StateReason"].as_str(),
                    parent["StateReason"].as_str(),
                    "case {case:?}: step {index} changes no state, so the explanation must \
                     stand, got {reported}"
                );
                assert_eq!(
                    reported["StateTransitionedTimestamp"], parent["StateTransitionedTimestamp"],
                    "case {case:?}: step {index} changes no state, so the transition instant \
                     must stand, got {reported}"
                );
            }
            parent = reported;
        }
    }
}

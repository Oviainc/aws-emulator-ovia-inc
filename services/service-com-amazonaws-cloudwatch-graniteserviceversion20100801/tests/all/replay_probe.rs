// SPDX-License-Identifier: CC0-1.0

//! The same rule as [`crate::sns_action_topic_policy`], for the transition the EVALUATION
//! CLOCK makes due rather than one a call forces — which is how the recorded scenario reaches
//! it, and the only way SEVERAL alarms' actions are executed in one pass.
//!
//! Two things are asserted here that a forced transition cannot reach:
//!
//! 1. an alarm whose missing data is treated as breaching evaluates itself into `ALARM`, and
//!    each action that transition fires is decided on its own topic's policy;
//! 2. where one pass settles the notifications of TWO alarms aimed at ONE topic, each publish
//!    is decided for the alarm it comes from — so a statement scoped to one alarm refuses that
//!    alarm's publish and leaves the other's alone.

use serde_json::{json, Value};

use crate::harness::{emulator, rpc, Emulator, PAST_EVALUATION_DELAY};
use crate::sns_action_topic_policy::{
    action_items, alarm_arn, allow_everyone, assert_action_item, delivered,
    given_topic_with_subscriber, poll_delivered, policy, set_topic_policy, statement,
    CLOUDWATCH_PRINCIPAL,
};

/// Poll the alarm's `Action` history the way a client waiting for an asynchronous action
/// does, until it holds `wanted` items or the attempts run out.
///
/// A thread sleep rather than `tokio::time::sleep`: this workspace enables tokio's `macros`,
/// `rt` and `rt-multi-thread` features only, so there is no timer driver to await on.
async fn poll_action_items(
    emulator: &Emulator,
    alarm: &str,
    wanted: usize,
) -> Vec<(String, Value)> {
    let mut items = Vec::new();
    for _ in 0..20 {
        std::thread::sleep(PAST_EVALUATION_DELAY / 3);
        items = action_items(emulator, alarm).await;
        if items.len() >= wanted {
            break;
        }
    }
    items
}

/// An alarm over a metric that receives no datapoint at all, whose missing data is breaching:
/// nothing forces its state, the evaluation clock moves it into `ALARM` and fires `topics`.
///
/// Each alarm watches its own dimension set, so two of them are two time series.
async fn given_breaching_alarm(emulator: &Emulator, name: &str, instance: &str, topics: &[&str]) {
    rpc(
        emulator,
        "monitoring",
        "GraniteServiceVersion20100801",
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "AWS/RDS",
            "MetricName": "DatabaseConnections",
            "Dimensions": [{"Name": "DBInstanceIdentifier", "Value": instance}],
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanOrEqualToThreshold",
            "TreatMissingData": "breaching",
            "AlarmActions": topics,
        }),
    )
    .await;
}

/// The runtime flavour is the one the booted emulator serves on, so the evaluation pass runs
/// the way it does in production rather than inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_evaluated_transition_decides_its_actions_the_same_way() {
    let emulator = emulator();
    let (allowed, _) = given_topic_with_subscriber(&emulator, "allowed").await;
    let (denied, denied_queue) = given_topic_with_subscriber(&emulator, "denied").await;
    set_topic_policy(
        &emulator,
        &denied,
        &policy(vec![
            allow_everyone(),
            statement(
                "Deny",
                json!({"Service": CLOUDWATCH_PRINCIPAL}),
                json!("sns:Publish"),
                json!({"Resource": denied.clone()}),
            ),
        ]),
    )
    .await;

    let alarm = "db-connections-breaching";
    given_breaching_alarm(&emulator, alarm, "no-such-instance", &[&allowed, &denied]).await;

    let items = poll_action_items(&emulator, alarm, 2).await;
    assert_eq!(
        items.len(),
        2,
        "the evaluated transition must execute and record both actions, got {items:?}"
    );
    for (case, topic, published) in [
        ("the topic that denies nothing", &allowed, true),
        ("the denying topic", &denied, false),
    ] {
        let (_, item) = items
            .iter()
            .find(|(resource, _)| resource == topic)
            .unwrap_or_else(|| panic!("no Action item for {topic}: {items:?}"));
        assert_action_item(case, item, topic, published);
    }
    let withheld = delivered(&emulator, &denied_queue).await;
    assert_eq!(
        withheld, 0,
        "the denying topic's subscriber must receive nothing, got {withheld}"
    );

    // With the deny gone, a fresh alarm's action to that same topic is published and
    // delivered — the deny was the only thing suppressing it.
    set_topic_policy(&emulator, &denied, &policy(vec![allow_everyone()])).await;
    let fresh = "db-connections-breaching-again";
    given_breaching_alarm(&emulator, fresh, "no-such-instance-either", &[&denied]).await;
    let items = poll_action_items(&emulator, fresh, 1).await;
    assert_eq!(
        items.len(),
        1,
        "the fresh alarm's action must be executed and recorded, got {items:?}"
    );
    assert_action_item("the deny removed", &items[0].1, &denied, true);
    let reached = poll_delivered(&emulator, &denied_queue, 1).await;
    assert_eq!(
        reached, 1,
        "the topic must deliver the fresh alarm's notification once nothing denies it, got \
         {reached}"
    );
}

/// One pass settles the notifications of SEVERAL alarms, so the decision is taken per publish:
/// a statement scoped by `aws:SourceArn` to one alarm refuses that alarm's publish to the
/// shared topic and says nothing about the other alarm's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_alarms_firing_one_topic_are_decided_for_the_alarm_each_publish_comes_from() {
    let emulator = emulator();
    let (topic, queue) = given_topic_with_subscriber(&emulator, "shared").await;

    // Both alarms are created BEFORE anything is due, so one later call settles both of them
    // in the same pass and the two publishes to this one topic are decided in it.
    let refused = "connections-refused";
    let served = "connections-served";
    given_breaching_alarm(&emulator, refused, "refused-instance", &[&topic]).await;
    given_breaching_alarm(&emulator, served, "served-instance", &[&topic]).await;
    let refused_arn = alarm_arn(&emulator, refused).await;
    let served_arn = alarm_arn(&emulator, served).await;
    assert_ne!(
        refused_arn, served_arn,
        "the two alarms must be two resources, got {refused_arn}"
    );
    set_topic_policy(
        &emulator,
        &topic,
        &policy(vec![
            allow_everyone(),
            statement(
                "Deny",
                json!({"Service": CLOUDWATCH_PRINCIPAL}),
                json!("sns:Publish"),
                json!({"Condition": {"ArnEquals": {"aws:SourceArn": refused_arn}}}),
            ),
        ]),
    )
    .await;

    let refused_items = poll_action_items(&emulator, refused, 1).await;
    assert_eq!(
        refused_items.len(),
        1,
        "the alarm the deny names must record its refused action, got {refused_items:?}"
    );
    assert_action_item("the alarm the deny names", &refused_items[0].1, &topic, false);
    let served_items = poll_action_items(&emulator, served, 1).await;
    assert_eq!(
        served_items.len(),
        1,
        "the other alarm must record its action too, got {served_items:?}"
    );
    assert_action_item("the alarm the deny does not name", &served_items[0].1, &topic, true);

    let waiting = poll_delivered(&emulator, &queue, 1).await;
    assert_eq!(
        waiting, 1,
        "only the notification of the alarm the deny does not name may reach the subscriber, \
         got {waiting}"
    );
}

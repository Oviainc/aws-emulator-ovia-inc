// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteAnomalyMonitor`: what a delete removes from the monitor
//! registry, what it leaves of the subscriptions that alerted on the deleted monitor, and
//! how an ARN the account holds no monitor under is refused.
//!
//! Every test drives the service over its real wire protocol through the shared
//! [`crate::harness`], so nothing here reaches past the HTTP surface a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_costexplorer_awsinsightsindexservice::state::MONITOR_TYPE_DIMENSIONAL;

use crate::harness::{error_code, Emulator};

/// An email subscriber address, in the form `SubscriberAddress`'s modelled pattern admits
/// for the email channel.
const EMAIL_ADDRESS: &str = "cost.ops@example.com";

/// Create one managed cost anomaly monitor and answer with its ARN.
async fn create_monitor(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateAnomalyMonitor",
            &json!({"AnomalyMonitor": {
                "MonitorName": name,
                "MonitorType": MONITOR_TYPE_DIMENSIONAL,
                "MonitorDimension": "SERVICE",
            }}),
        )
        .await;
    created["MonitorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("MonitorArn must be present, got {created}"))
        .to_string()
}

/// Create one subscription alerting on `monitors`, and answer with its ARN.
///
/// `DAILY` is one of the `AnomalySubscriptionFrequency` values
/// (`smithy_query(query="shape", shape="AnomalySubscriptionFrequency")`), and the
/// deprecated scalar `Threshold` is the shorter of the two threshold forms the member
/// documents — which of them the subscription carries is not what this suite is about.
async fn create_subscription(emulator: &Emulator, name: &str, monitors: &[String]) -> String {
    let created = emulator
        .ok(
            "CreateAnomalySubscription",
            &json!({"AnomalySubscription": {
                "SubscriptionName": name,
                "MonitorArnList": monitors,
                "Subscribers": [{"Type": "EMAIL", "Address": EMAIL_ADDRESS}],
                "Frequency": "DAILY",
                "Threshold": 100.0,
            }}),
        )
        .await;
    created["SubscriptionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("SubscriptionArn must be present, got {created}"))
        .to_string()
}

/// Every monitor ARN the account holds, in the order `GetAnomalyMonitors` reports them.
async fn monitor_arns(emulator: &Emulator) -> Vec<String> {
    emulator.ok("GetAnomalyMonitors", &json!({})).await["AnomalyMonitors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|monitor| {
            monitor["MonitorArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every subscription the account holds, keyed by ARN, as `GetAnomalySubscriptions`
/// reports them.
async fn subscriptions_by_arn(emulator: &Emulator) -> Vec<(String, Value)> {
    emulator
        .ok("GetAnomalySubscriptions", &json!({}))
        .await["AnomalySubscriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|subscription| {
            (
                subscription["SubscriptionArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                subscription,
            )
        })
        .collect()
}

/// The `MonitorArnList` a reported subscription carries.
fn monitor_arn_list(subscription: &Value) -> Vec<String> {
    subscription["MonitorArnList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect()
}

/// One association shape to delete a monitor out of: how many monitors the account holds,
/// which of them each subscription alerts on (by index), and which monitor is deleted.
struct Case {
    label: &'static str,
    monitors: usize,
    subscriptions: &'static [&'static [usize]],
    delete: usize,
}

/// The association shapes a monitor can be deleted out of: one no subscription alerts on,
/// a subscription's only monitor, the first and a middle one of several a subscription
/// alerts on, and one several subscriptions alert on at once.
const CASES: &[Case] = &[
    Case {
        label: "a monitor no subscription alerts on",
        monitors: 2,
        subscriptions: &[&[1]],
        delete: 0,
    },
    Case {
        label: "the only monitor a subscription alerts on",
        monitors: 1,
        subscriptions: &[&[0]],
        delete: 0,
    },
    Case {
        label: "the first of the monitors a subscription alerts on",
        monitors: 3,
        subscriptions: &[&[0, 1, 2]],
        delete: 0,
    },
    Case {
        label: "a middle one of the monitors a subscription alerts on",
        monitors: 3,
        subscriptions: &[&[0, 1, 2]],
        delete: 1,
    },
    Case {
        label: "a monitor several subscriptions alert on",
        monitors: 2,
        subscriptions: &[&[0, 1], &[0], &[1]],
        delete: 0,
    },
    Case {
        label: "a monitor no subscription at all exists for",
        monitors: 1,
        subscriptions: &[],
        delete: 0,
    },
];

/// RULE: `DeleteAnomalyMonitor` removes the monitor its `MonitorArn` names from the
/// account's monitor registry and dissociates it from every cost anomaly subscription that
/// alerted on it — the ARN is dropped from each subscription's `MonitorArnList` while the
/// subscription itself stays, so one whose only monitor was deleted survives alerting on
/// no monitor — and every other monitor and subscription is left exactly as it was.
///
/// The table spans the association shapes a deleted monitor can stand in: alerted on by no
/// subscription, by one that names it alone, by one that names it first or in the middle of
/// several, and by several subscriptions at once.
#[tokio::test]
async fn a_deleted_monitor_is_dropped_from_the_subscriptions_that_alerted_on_it() {
    for case in CASES {
        let label = case.label;
        let emulator = Emulator::start();

        let mut monitors: Vec<String> = Vec::new();
        for index in 0..case.monitors {
            monitors.push(create_monitor(&emulator, &format!("monitor-{index}")).await);
        }
        let mut subscriptions: Vec<(String, Vec<String>)> = Vec::new();
        for (index, indices) in case.subscriptions.iter().enumerate() {
            let named: Vec<String> = indices.iter().map(|at| monitors[*at].clone()).collect();
            let arn =
                create_subscription(&emulator, &format!("subscription-{index}"), &named).await;
            subscriptions.push((arn, named));
        }

        let deleted = monitors[case.delete].clone();
        let answered = emulator
            .ok("DeleteAnomalyMonitor", &json!({"MonitorArn": deleted}))
            .await;
        assert!(
            answered.is_null() || answered.as_object().is_some_and(serde_json::Map::is_empty),
            "case {label:?}: the response carries no members, got {answered}"
        );

        let mut expected_monitors: Vec<String> = monitors
            .iter()
            .filter(|arn| **arn != deleted)
            .cloned()
            .collect();
        // The registry is keyed on the ARN, so that is the order a read reports.
        expected_monitors.sort();
        let reported = monitor_arns(&emulator).await;
        assert_eq!(
            reported, expected_monitors,
            "case {label:?}: exactly the monitors other than the deleted one must be \
             reported, got {reported:?}"
        );

        let reported_subscriptions = subscriptions_by_arn(&emulator).await;
        assert_eq!(
            reported_subscriptions.len(),
            subscriptions.len(),
            "case {label:?}: no subscription is deleted with the monitor, got \
             {reported_subscriptions:?}"
        );
        for (arn, named) in &subscriptions {
            let (_, subscription) = reported_subscriptions
                .iter()
                .find(|(reported_arn, _)| reported_arn == arn)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: expected the subscription {arn:?} to be reported")
                });
            let expected: Vec<String> = named
                .iter()
                .filter(|monitor| **monitor != deleted)
                .cloned()
                .collect();
            assert_eq!(
                monitor_arn_list(subscription),
                expected,
                "case {label:?}: the subscription {arn:?} must alert on exactly the monitors \
                 it named other than the deleted one, got {subscription}"
            );
        }

        // The monitor filter reads the same association from the other side: nothing alerts
        // on the deleted monitor any more.
        let by_deleted_monitor = emulator
            .ok("GetAnomalySubscriptions", &json!({"MonitorArn": deleted}))
            .await;
        assert_eq!(
            by_deleted_monitor["AnomalySubscriptions"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            0,
            "case {label:?}: no subscription alerts on the deleted monitor, got \
             {by_deleted_monitor}"
        );
    }
}

/// RULE: an ARN the account holds no cost anomaly monitor under is refused 404
/// `UnknownMonitorException` — including one an earlier delete already removed, so the
/// delete is not idempotent — and the refusal leaves the registry untouched.
///
/// The table spans the ways a `MonitorArn` the input's modelled constraints admit (length
/// 0..1024, pattern `^[\S\s]*$`, from `smithy_query(query="operation",
/// operation="DeleteAnomalyMonitor")`) can name no monitor: a well-formed monitor ARN of
/// this account, one of another account, the ARN of a different resource kind, a value that
/// is no ARN at all, and the empty string.
#[tokio::test]
async fn a_delete_of_a_monitor_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::start();
    let kept = create_monitor(&emulator, "the-monitor-that-stays").await;
    let subscription =
        create_subscription(&emulator, "untouched", std::slice::from_ref(&kept)).await;
    let removed = create_monitor(&emulator, "deleted-once").await;
    emulator
        .ok("DeleteAnomalyMonitor", &json!({"MonitorArn": removed}))
        .await;

    let unknown = vec![
        (
            "a well-formed monitor ARN of this account naming no monitor",
            format!(
                "arn:aws:ce::{}:anomalymonitor/00000000-0000-8000-8000-000000000000",
                engine::emulator_account_id()
            ),
        ),
        (
            "a monitor ARN of another account",
            "arn:aws:ce::999999999999:anomalymonitor/11111111-1111-8111-8111-111111111111"
                .to_string(),
        ),
        (
            "the ARN of a different resource kind",
            engine::emulator_arn("ce", "", "costcategory/not-a-monitor"),
        ),
        ("a value that is no ARN at all", "monitor-1".to_string()),
        ("the empty string", String::new()),
        ("a monitor an earlier delete already removed", removed.clone()),
    ];

    for (label, arn) in &unknown {
        let (status, answered) = emulator
            .call("DeleteAnomalyMonitor", &json!({"MonitorArn": arn}))
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (404, "UnknownMonitorException"),
            "case {label:?}: expected a 404 UnknownMonitorException, got {status}: {answered}"
        );
    }

    assert_eq!(
        monitor_arns(&emulator).await,
        vec![kept.clone()],
        "a refused delete must remove nothing"
    );
    let reported = subscriptions_by_arn(&emulator).await;
    assert_eq!(
        reported.len(),
        1,
        "a refused delete must leave every subscription in place, got {reported:?}"
    );
    assert_eq!(
        (reported[0].0.as_str(), monitor_arn_list(&reported[0].1)),
        (subscription.as_str(), vec![kept]),
        "a refused delete must dissociate nothing, got {}",
        reported[0].1
    );
}

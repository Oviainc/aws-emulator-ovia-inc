// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the cost anomaly plane answers a request that names a monitor
//! (or a subscription) the account does not hold.
//!
//! Two rules, each asserted over a table of inputs:
//!
//! * an ARN-list member documented as a FILTER on a paginated read SELECTS the resources
//!   the account holds that it names — it asserts the existence of none of them — so an
//!   ARN naming nothing contributes no element and the read still answers HTTP 200 with
//!   its required list;
//! * an operation that resolves a monitor ARN and finds no monitor refuses with
//!   `UnknownMonitorException` at HTTP 400, carrying Cost Explorer's fixed "no monitor
//!   present" sentence over the bare monitor id of the ARN the request named.
//!
//! Every test drives the service over its real wire protocol through the shared
//! [`crate::harness`], so nothing here reaches past the HTTP surface a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_costexplorer_awsinsightsindexservice::state::{
    MONITOR_ID_AS_MONITOR, MONITOR_ID_AS_RESOURCE, MONITOR_TYPE_DIMENSIONAL,
};

use crate::harness::{error_code, error_message, Emulator};

/// An email subscriber address, in the form `SubscriberAddress`'s modelled pattern admits
/// for the email channel.
const EMAIL_ADDRESS: &str = "cost.ops@example.com";

/// The status Cost Explorer serves an unknown-monitor refusal at. The model defaults
/// `UnknownMonitorException` to 404 (`smithy_query(query="shape",
/// shape="UnknownMonitorException")`); the service answers the dangling monitor reference
/// as a client-input refusal instead.
const UNKNOWN_MONITOR_STATUS: u16 = 400;

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

/// A `CreateAnomalySubscription` request body alerting on `monitors`.
///
/// `DAILY` is one of the `AnomalySubscriptionFrequency` values
/// (`smithy_query(query="shape", shape="AnomalySubscriptionFrequency")`).
fn subscription_body(name: &str, monitors: &[String]) -> Value {
    json!({"AnomalySubscription": {
        "SubscriptionName": name,
        "MonitorArnList": monitors,
        "Subscribers": [{"Type": "EMAIL", "Address": EMAIL_ADDRESS}],
        "Frequency": "DAILY",
        "Threshold": 100.0,
    }})
}

/// Create one subscription alerting on `monitors` and answer with its ARN.
async fn create_subscription(emulator: &Emulator, name: &str, monitors: &[String]) -> String {
    let body = subscription_body(name, monitors);
    let created = emulator.ok("CreateAnomalySubscription", &body).await;
    created["SubscriptionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("SubscriptionArn must be present, got {created}"))
        .to_string()
}

/// The member of each element of a reported list, in the order the read reported them.
fn reported(body: &Value, list: &str, member: &str) -> Vec<String> {
    body[list]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|element| element[member].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The bare id an ARN carries: the last segment of its resource portion. Computed here
/// from the ARN the test sent, so the assertion does not borrow the service's own helper.
fn id_of(arn: &str) -> &str {
    match arn.rsplit_once('/') {
        Some((_, id)) => id,
        None => arn,
    }
}

/// One value that names no monitor, and the label that tells which kind it is.
struct UnknownArn {
    label: &'static str,
    arn: String,
}

/// The ways a well-formed ARN can name no monitor the account holds.
///
/// Every row satisfies the `Arn` shape `AnomalySubscription$MonitorArnList`'s member
/// targets (`smithy_query(query="shape", shape="Arn")`: length 20..2048, pattern
/// `^arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`), so these reach
/// every operation that takes a monitor reference however tightly it constrains it.
fn well_formed_unknown_arns() -> Vec<UnknownArn> {
    let account = engine::emulator_account_id();
    vec![
        UnknownArn {
            label: "a well-formed monitor ARN of this account naming no monitor",
            arn: format!(
                "arn:aws:ce::{account}:anomalymonitor/00000000-0000-8000-8000-000000000000"
            ),
        },
        UnknownArn {
            label: "a monitor ARN of another account",
            arn: "arn:aws:ce::999999999999:anomalymonitor/11111111-1111-8111-8111-111111111111"
                .to_string(),
        },
        UnknownArn {
            label: "the ARN of another of this service's resource kinds",
            arn: engine::emulator_arn("ce", "", "costcategory/not-a-monitor"),
        },
        UnknownArn {
            label: "the ARN of another service's resource",
            arn: format!("arn:aws:sns:us-east-1:{account}:not-a-monitor"),
        },
    ]
}

/// The ways a value that is NOT a well-formed ARN can name no monitor the account holds.
///
/// Every row is admitted by the looser `GenericString` / `Value` shapes the monitor-ARN
/// member of `DeleteAnomalyMonitor` and the ARN-list filters target
/// (`smithy_query(query="shape", shape="Value")`: length 0..1024, pattern `^[\S\s]*$`), and
/// refused at the boundary by the members that target `Arn` — so these rows drive only the
/// operations whose own constraints admit them.
fn loosely_formed_unknown_values() -> Vec<UnknownArn> {
    vec![
        UnknownArn {
            label: "a value that is no ARN at all",
            arn: "monitor-1".to_string(),
        },
        UnknownArn {
            label: "an id with no ARN around it",
            arn: "00000000-0000-8000-8000-000000000000".to_string(),
        },
        UnknownArn {
            label: "the empty string",
            arn: String::new(),
        },
    ]
}

/// Every unknown value a member targeting the loose string shapes admits.
fn unknown_arns() -> Vec<UnknownArn> {
    let mut rows = well_formed_unknown_arns();
    rows.extend(loosely_formed_unknown_values());
    rows
}

/// RULE: `GetAnomalyMonitors`'s `MonitorArnList` and `GetAnomalySubscriptions`'s
/// `SubscriptionArnList` are FILTERS — each selects the resources the account holds that
/// the list names and asserts the existence of none of them — so a member naming nothing
/// contributes no element and the read answers HTTP 200 with the required list it selected,
/// empty when nothing matched.
///
/// The table spans the filter shapes that can name something the account does not hold:
/// one unknown ARN alone, one beside a known ARN, several unknown ARNs at once, every kind
/// of unknown value from [`unknown_arns`], and — the teardown-then-confirm loop — the ARN
/// of a resource an earlier delete removed.
#[tokio::test]
async fn an_arn_filter_selects_what_the_account_holds_and_asserts_no_existence() {
    let emulator = Emulator::start();
    let kept_monitor = create_monitor(&emulator, "the-monitor-that-stays").await;
    let kept_subscription = create_subscription(
        &emulator,
        "the-subscription-that-stays",
        std::slice::from_ref(&kept_monitor),
    )
    .await;
    let deleted_monitor = create_monitor(&emulator, "deleted-monitor").await;
    emulator
        .ok(
            "DeleteAnomalyMonitor",
            &json!({"MonitorArn": deleted_monitor}),
        )
        .await;

    // Each read is driven with its own filter member, its own output list and the member
    // that identifies an element — the rule is the same for both.
    let reads: [(&str, &str, &str, &str, &str); 2] = [
        (
            "GetAnomalyMonitors",
            "MonitorArnList",
            "AnomalyMonitors",
            "MonitorArn",
            kept_monitor.as_str(),
        ),
        (
            "GetAnomalySubscriptions",
            "SubscriptionArnList",
            "AnomalySubscriptions",
            "SubscriptionArn",
            kept_subscription.as_str(),
        ),
    ];

    for (operation, filter_member, list, id_member, known) in reads {
        let mut filters: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
        for unknown in unknown_arns() {
            filters.push((
                format!("{} on its own", unknown.label),
                vec![unknown.arn.clone()],
                Vec::new(),
            ));
            filters.push((
                format!("{} beside an ARN the account holds", unknown.label),
                vec![known.to_string(), unknown.arn.clone()],
                vec![known.to_string()],
            ));
        }
        let every_unknown: Vec<String> = unknown_arns().into_iter().map(|row| row.arn).collect();
        filters.push((
            "every unknown ARN at once".to_string(),
            every_unknown,
            Vec::new(),
        ));
        filters.push((
            "the ARN of a monitor an earlier delete removed".to_string(),
            vec![deleted_monitor.clone()],
            Vec::new(),
        ));
        filters.push((
            "an ARN the account holds, alone".to_string(),
            vec![known.to_string()],
            vec![known.to_string()],
        ));

        for (case, filter, expected) in filters {
            let (status, answered) = emulator
                .call(operation, &json!({ filter_member: filter }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} ({operation}): expected HTTP 200, got {status}: {answered}"
            );
            assert!(
                answered.get(list).is_some_and(Value::is_array),
                "case {case:?} ({operation}): the required {list} member must be present, got \
                 {answered}"
            );
            let selected = reported(&answered, list, id_member);
            assert_eq!(
                selected, expected,
                "case {case:?} ({operation}): expected exactly the named resources the account \
                 holds, got {selected:?}"
            );
            assert!(
                error_code(&answered).is_empty() && error_message(&answered).is_empty(),
                "case {case:?} ({operation}): a selecting filter raises no error, got {answered}"
            );
        }
    }
}

/// RULE: an operation that resolves a monitor ARN against the account's monitor registry
/// and finds no monitor under it refuses with `UnknownMonitorException` at HTTP 400 —
/// not the 404 the shape's `@httpError` defaults to — carrying the fixed sentence "No
/// monitor present with `<member>`: `<monitorId>`", where `<monitorId>` is the bare id
/// component of the ARN the request named (never the whole ARN) and `<member>` is the word
/// the refusing operation uses: `resourceId` where the monitor is the call's own resource,
/// `monitorId` where the request only references it.
///
/// The table spans both refusing operations and every kind of value that names no monitor,
/// including one an earlier delete removed.
#[tokio::test]
async fn a_dangling_monitor_reference_is_refused_with_the_services_own_sentence() {
    let emulator = Emulator::start();
    let kept = create_monitor(&emulator, "the-monitor-that-stays").await;
    let removed = create_monitor(&emulator, "deleted-once").await;
    emulator
        .ok("DeleteAnomalyMonitor", &json!({"MonitorArn": removed}))
        .await;

    let mut delete_rows = unknown_arns();
    delete_rows.push(UnknownArn {
        label: "a monitor an earlier delete already removed",
        arn: removed.clone(),
    });
    // `AnomalySubscription$MonitorArnList`'s member targets the tighter `Arn` shape, so
    // only a well-formed ARN gets past the boundary validator and reaches this gate;
    // `DeleteAnomalyMonitor`'s `MonitorArn` is a loose `GenericString`, so every row above
    // reaches its handler.
    let mut create_rows = well_formed_unknown_arns();
    create_rows.push(UnknownArn {
        label: "a monitor an earlier delete already removed",
        arn: removed.clone(),
    });

    // The delete's own SUBJECT is the monitor; the subscription create only REFERENCES it,
    // driven once with the unknown monitor alone and once beside a monitor the account
    // does hold.
    let mut requests: Vec<(&str, &str, &str, String, Value)> = Vec::new();
    for unknown in &delete_rows {
        requests.push((
            "DeleteAnomalyMonitor",
            "the monitor named alone",
            MONITOR_ID_AS_RESOURCE,
            unknown.arn.clone(),
            json!({"MonitorArn": unknown.arn}),
        ));
    }
    for unknown in &create_rows {
        requests.push((
            "CreateAnomalySubscription",
            "the monitor named alone",
            MONITOR_ID_AS_MONITOR,
            unknown.arn.clone(),
            subscription_body("dangling-reference", std::slice::from_ref(&unknown.arn)),
        ));
        requests.push((
            "CreateAnomalySubscription",
            "the monitor named beside one the account holds",
            MONITOR_ID_AS_MONITOR,
            unknown.arn.clone(),
            subscription_body(
                "dangling-reference-beside-a-known-one",
                &[kept.clone(), unknown.arn.clone()],
            ),
        ));
    }

    for (operation, shape, member, arn, body) in &requests {
        let case = format!("{operation}: {shape} {arn:?}");
        let (status, answered) = emulator.call(operation, body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (UNKNOWN_MONITOR_STATUS, "UnknownMonitorException"),
            "case {case:?}: expected {UNKNOWN_MONITOR_STATUS} UnknownMonitorException, got \
             {status}: {answered}"
        );
        assert_eq!(
            error_message(&answered),
            format!("No monitor present with {member}: {}", id_of(arn)),
            "case {case:?}: expected the service's own sentence over the bare monitor id, got \
             {answered}"
        );
    }

    // The refusals wrote nothing: the monitor that was never deleted is still the only one,
    // and no subscription was persisted.
    let monitors = emulator.ok("GetAnomalyMonitors", &json!({})).await;
    assert_eq!(
        reported(&monitors, "AnomalyMonitors", "MonitorArn"),
        vec![kept],
        "a refused request must leave the monitor registry as it was, got {monitors}"
    );
    let subscriptions = emulator.ok("GetAnomalySubscriptions", &json!({})).await;
    assert_eq!(
        reported(&subscriptions, "AnomalySubscriptions", "SubscriptionArn"),
        Vec::<String>::new(),
        "a refused create must persist no subscription, got {subscriptions}"
    );
}

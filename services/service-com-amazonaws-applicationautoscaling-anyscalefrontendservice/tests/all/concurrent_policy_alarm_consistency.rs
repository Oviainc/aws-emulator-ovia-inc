// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch alarm set a target-tracking policy owns, when several callers upsert one
//! policy name at once.
//!
//! The rule under test: **the alarms CloudWatch holds for a target-tracking scaling policy
//! are the ones computed from the policy configuration this service holds — so however many
//! writes of ONE policy name overlap, the account is left with one policy and with the alarm
//! set that policy's surviving configuration asks for, never with an alarm set belonging to a
//! configuration that has been overwritten.**
//!
//! A policy's alarms ARE the policy: they are "the CloudWatch alarms associated with the
//! scaling policy" (`ScalingPolicy$Alarms`), and a target-tracking policy is driven by them
//! rather than by its document, so an alarm set left over from a losing write would scale the
//! resource at a target nothing asked for, and a client reading the policy back could not tell.
//!
//! Each row is checked against a REFERENCE emulator given the surviving configuration in ONE
//! call: that is what "the alarm set the stored configuration asks for" means, and it leaves
//! the thresholds, periods and evaluation counts to the service's own rules rather than to a
//! second copy of them in this file.
//!
//! The rows differ along what the rule generalises over: how many writes overlap, the target
//! values they race with (distinct, and all equal), whether the tracked metric counts the
//! capacity consumed over the alarm period — which is what makes the thresholds depend on the
//! resource's provisioned capacity and adds the provisioned-capacity alarm pair — and which
//! service's resource is scaled. Every row races against a target that already holds other
//! policies, since this service materialises the alarms of all of a target's policies on
//! every write.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};

use super::emulator::{alarm_kinds, Emulator};

/// One row: the target to scale, the metric its policy tracks, and the target values the
/// overlapping writes carry.
struct RaceRow {
    case: &'static str,
    /// A value of the model's `ScalableDimension` enum.
    dimension: &'static str,
    /// A value of the model's `MetricType` enum, published for that dimension.
    metric_type: &'static str,
    /// The `TargetValue` each overlapping write asks for, one per racer.
    ///
    /// Every value lies inside the window the owning service publishes for its metric: the
    /// provisioned-throughput utilisation metrics take a percentage of provisioned capacity,
    /// and the ECS CPU utilisation metric a percentage of the service's CPU.
    targets: &'static [f64],
    /// Which alarms the surviving policy must own, by the kind read out of each alarm's name.
    kinds: &'static [&'static str],
}

/// The table. The `dynamodb` rows track a metric that counts consumed units, so their
/// thresholds are a function of the target AND the table's provisioned capacity and the
/// policy owns the provisioned-capacity pair beside the tracked metric's; the `ecs` rows
/// track a percentage, whose threshold IS the target value and which owns no capacity pair.
const RACE_TABLE: &[RaceRow] = &[
    RaceRow {
        case: "two writes of one policy on a DynamoDB read-capacity target",
        dimension: "dynamodb:table:ReadCapacityUnits",
        metric_type: "DynamoDBReadCapacityUtilization",
        targets: &[80.0, 30.0],
        kinds: &[
            "AlarmHigh",
            "AlarmLow",
            "ProvisionedCapacityHigh",
            "ProvisionedCapacityLow",
        ],
    },
    RaceRow {
        case: "four writes of one policy on a DynamoDB write-capacity target",
        dimension: "dynamodb:table:WriteCapacityUnits",
        metric_type: "DynamoDBWriteCapacityUtilization",
        targets: &[40.0, 55.0, 70.0, 25.0],
        kinds: &[
            "AlarmHigh",
            "AlarmLow",
            "ProvisionedCapacityHigh",
            "ProvisionedCapacityLow",
        ],
    },
    RaceRow {
        case: "eight writes of one policy on an ECS service target",
        dimension: "ecs:service:DesiredCount",
        metric_type: "ECSServiceAverageCPUUtilization",
        targets: &[15.0, 25.0, 35.0, 45.0, 55.0, 65.0, 75.0, 85.0],
        kinds: &["AlarmHigh", "AlarmLow"],
    },
    RaceRow {
        case: "three writes of one policy that all ask for the same target value",
        dimension: "ecs:service:DesiredCount",
        metric_type: "ECSServiceAverageCPUUtilization",
        targets: &[60.0, 60.0, 60.0],
        kinds: &["AlarmHigh", "AlarmLow"],
    },
];

/// The policy name every racer of a row writes.
const POLICY_NAME: &str = "one-name";

/// The capacity window every row's target is registered with.
const MIN_CAPACITY: i64 = 1;
const MAX_CAPACITY: i64 = 20;

/// How many other target-tracking policies the row's target already holds while the writes of
/// one policy name overlap.
///
/// A scalable target may hold several policies and this service materialises the alarms of
/// ALL of them on every write, so a target holding others is the case where one write's alarm
/// pass genuinely overlaps another's.
const OTHER_POLICIES: usize = 40;

/// How many times each row is raced.
///
/// An interleaving is not a thing a test can schedule, so each row is raced repeatedly: the
/// rule has to hold for every one of them, and a handful of rounds is enough to reach the
/// orders in which two alarm passes overlap while keeping the suite quick.
const ROUNDS: usize = 8;

/// The `PutScalingPolicy` request for the raced policy at `target_value`.
fn put_request(target: &Value, metric_type: &str, target_value: f64) -> Value {
    let mut request = target.clone();
    request["PolicyName"] = json!(POLICY_NAME);
    request["PolicyType"] = json!("TargetTrackingScaling");
    request["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": target_value,
        "PredefinedMetricSpecification": {"PredefinedMetricType": metric_type},
    });
    request
}

/// Give `target` the [`OTHER_POLICIES`] policies it holds beside the one the writes race on.
///
/// Each is a STEP scaling policy: "you can have one or more target tracking scaling policies"
/// for a target but not two on one metric specification, and not every scalable dimension
/// supports a customised one, so the policies a target holds beside the raced one are of the
/// type a target may hold many of. Every write of any policy of the target passes over all of
/// them, which is what makes one write's alarm pass overlap another's.
async fn given_other_policies(emulator: &Emulator, target: &Value, row: &RaceRow) {
    for other in 0..OTHER_POLICIES {
        let mut request = target.clone();
        request["PolicyName"] = json!(format!("other-{other}"));
        request["PolicyType"] = json!("StepScaling");
        request["StepScalingPolicyConfiguration"] = json!({
            "AdjustmentType": "ChangeInCapacity",
            "MetricAggregationType": "Average",
            "Cooldown": 60,
            "StepAdjustments": [{
                "MetricIntervalLowerBound": 0.0,
                "ScalingAdjustment": 1 + other as i64,
            }],
        });
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected PutScalingPolicy for other-{other} to answer 200, got \
             {status} {body:?}",
            row.case
        );
    }
}

/// What one alarm IS, for the comparison against the reference: everything about it that the
/// policy's configuration decides.
///
/// The alarm's own state is left out — that is CloudWatch's, moved by evaluation rather than
/// by this service — and so is its ARN, which is a function of the name beside it.
fn configuration_of(alarm: &Value) -> Value {
    let members = [
        "AlarmName",
        "Namespace",
        "MetricName",
        "Statistic",
        "Unit",
        "Dimensions",
        "Period",
        "EvaluationPeriods",
        "Threshold",
        "ComparisonOperator",
        "AlarmActions",
        "AlarmDescription",
        "ActionsEnabled",
    ];
    let mut configuration = serde_json::Map::new();
    for member in members {
        configuration.insert(
            member.to_string(),
            alarm.get(member).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(configuration)
}

/// The alarm set an emulator holds, by alarm name, as the configuration of each.
fn alarm_set(alarms: &[Value]) -> BTreeMap<String, Value> {
    alarms
        .iter()
        .map(|alarm| {
            (
                alarm["AlarmName"].as_str().unwrap_or_default().to_string(),
                configuration_of(alarm),
            )
        })
        .collect()
}

/// The scaling policies an emulator holds for `target`.
async fn policies_of(emulator: &Emulator, target: &Value, case: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call("DescribeScalingPolicies", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingPolicies to answer 200, got {status} {body:?}"
    );
    body["ScalingPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The alarm names a policy of `policies` reports as its own.
fn alarm_names_of(policies: &[Value], policy_name: &str) -> BTreeSet<String> {
    policies
        .iter()
        .filter(|policy| policy["PolicyName"] == json!(policy_name))
        .flat_map(|policy| policy["Alarms"].as_array().cloned().unwrap_or_default())
        .map(|alarm| alarm["AlarmName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The whole alarm set, and the raced policy's own alarm names, of an emulator given `row`'s
/// target, its other policies and ONE write of the raced policy at `target_value`.
///
/// This is the reference every raced emulator is compared against: the state the account is
/// in when the surviving configuration is the only one that was ever written.
async fn reference_state(
    row: &RaceRow,
    target_value: f64,
) -> (BTreeMap<String, Value>, BTreeSet<String>) {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target(row.dimension, MIN_CAPACITY, MAX_CAPACITY)
        .await;
    given_other_policies(&emulator, &target, row).await;
    let (status, body) = emulator
        .call(
            "PutScalingPolicy",
            put_request(&target, row.metric_type, target_value),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the reference PutScalingPolicy to answer 200, got {status} {body:?}",
        row.case
    );
    let policies = policies_of(&emulator, &target, row.case).await;
    (
        alarm_set(&emulator.target_tracking_alarms().await),
        alarm_names_of(&policies, POLICY_NAME),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_writes_of_one_policy_leave_the_stored_configurations_alarm_set() {
    for row in RACE_TABLE {
        for round in 0..ROUNDS {
            let case = format!("{} (round {round})", row.case);
            let case = case.as_str();
            let emulator = Arc::new(Emulator::empty());
            let target = emulator
                .given_target(row.dimension, MIN_CAPACITY, MAX_CAPACITY)
                .await;
            given_other_policies(&emulator, &target, row).await;

            // Every racer writes the SAME policy name, so these are writes of one policy.
            let mut tasks = Vec::new();
            for (racer, target_value) in row.targets.iter().enumerate() {
                let emulator = Arc::clone(&emulator);
                let request = put_request(&target, row.metric_type, *target_value);
                tasks.push(tokio::spawn(async move {
                    let (status, body) = emulator.call("PutScalingPolicy", request).await;
                    (racer, status, body)
                }));
            }
            for task in tasks {
                let (racer, status, body) = task.await.expect("the racing task completed");
                assert_eq!(
                    status, 200,
                    "case {case:?}: racer {racer} expected HTTP 200 from PutScalingPolicy, got \
                     {status} {body:?}"
                );
            }

            // One policy name is one policy, however many writes of it overlapped, and the
            // target's other policies are untouched.
            let policies = policies_of(&emulator, &target, case).await;
            let raced: Vec<&Value> = policies
                .iter()
                .filter(|policy| policy["PolicyName"] == json!(POLICY_NAME))
                .collect();
            assert_eq!(
                raced.len(),
                1,
                "case {case:?}: one policy name leaves one policy, got {raced:?}"
            );
            assert_eq!(
                policies.len(),
                OTHER_POLICIES + 1,
                "case {case:?}: the target's other policies survive untouched, got {policies:?}"
            );
            let stored = raced[0]["TargetTrackingScalingPolicyConfiguration"]["TargetValue"]
                .as_f64()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the stored policy must report its TargetValue, got {:?}",
                        raced[0]
                    )
                });
            assert!(
                row.targets.contains(&stored),
                "case {case:?}: the surviving TargetValue is one a write asked for, got \
                 {stored} against {:?}",
                row.targets
            );

            // The alarm set CloudWatch holds is the one the SURVIVING configuration asks
            // for — compared member by member against an emulator given that configuration
            // as its only write of this policy.
            let held = alarm_set(&emulator.target_tracking_alarms().await);
            let (expected, expected_own) = reference_state(row, stored).await;
            assert_eq!(
                held.keys().collect::<Vec<_>>(),
                expected.keys().collect::<Vec<_>>(),
                "case {case:?}: the target's policies own the alarms their stored \
                 configurations name, got {:?} against {:?}",
                held.keys().collect::<Vec<_>>(),
                expected.keys().collect::<Vec<_>>()
            );
            for (name, configuration) in &expected {
                assert_eq!(
                    held.get(name),
                    Some(configuration),
                    "case {case:?}: alarm {name} must carry the configuration the stored \
                     TargetValue {stored} asks for, got {:?}",
                    held.get(name)
                );
            }

            // And the policy record names exactly its own alarms, so a client reading the
            // policy back is pointed at the alarms that exist.
            assert_eq!(
                alarm_names_of(&policies, POLICY_NAME),
                expected_own,
                "case {case:?}: the policy's Alarms are the alarms CloudWatch holds for it, \
                 got {:?}",
                raced[0]["Alarms"]
            );

            // The kinds of the raced policy's own alarms are the ones that metric's policy
            // owns: the provisioned-capacity pair only where the thresholds depend on the
            // resource's provisioned capacity.
            let own: Vec<Value> = emulator
                .target_tracking_alarms()
                .await
                .into_iter()
                .filter(|alarm| {
                    expected_own.contains(alarm["AlarmName"].as_str().unwrap_or_default())
                })
                .collect();
            let kinds = alarm_kinds(&own);
            assert_eq!(
                kinds,
                row.kinds
                    .iter()
                    .map(|kind| (*kind).to_string())
                    .collect::<Vec<_>>(),
                "case {case:?}: expected the alarm kinds {:?}, got {kinds:?}",
                row.kinds
            );
        }
    }
}

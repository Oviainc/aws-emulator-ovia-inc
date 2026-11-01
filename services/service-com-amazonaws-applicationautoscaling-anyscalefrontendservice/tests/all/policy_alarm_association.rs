// SPDX-License-Identifier: CC0-1.0

//! How a scaling policy and the CloudWatch alarms that invoke it are tied together, end to end
//! over the wire: what a policy reports as its `Alarms`, what deleting a policy leaves in the
//! alarms that named it, and what happens when two policies of one scalable target are
//! triggered at the same time.
//!
//! Three rules, each asserted over a span of inputs rather than over one request:
//!
//! * a policy's `Alarms` are "the CloudWatch alarms associated with the scaling policy"
//!   (`ScalingPolicy$Alarms`), and what associates an alarm with a policy is the alarm's own
//!   action list — "the actions to execute when this alarm transitions to the ALARM state […]
//!   specified as an Amazon Resource Name (ARN)" (`PutMetricAlarmInput$AlarmActions`). So a read
//!   of a policy reports the alarms CloudWatch holds that name it, whoever created them, and a
//!   policy nothing names reports none;
//! * deleting a scaling policy — on its own, or with the scalable target it belongs to — removes
//!   that policy's ARN from the action lists that name it and leaves the alarms themselves in
//!   place: "deleting a step scaling policy deletes the underlying alarm action, but does not
//!   delete the CloudWatch alarm";
//! * of the policies of ONE scalable target triggered at the same time, exactly one is
//!   performed: "Application Auto Scaling gives precedence to the policy that provides the
//!   largest capacity for both scale out and scale in", so the capacity the target lands on is
//!   the largest any of them asked for, and the activity log records that one movement only.

use std::time::Duration;

use serde_json::{json, Value};

use super::emulator::{Emulator, ACCOUNT, REGION};

/// The namespace and dimension every row scales.
const NAMESPACE: &str = "ecs";
/// See [`NAMESPACE`].
const DIMENSION: &str = "ecs:service:DesiredCount";

/// The metric namespace the suites' own alarms watch — a customer namespace, so an alarm for it
/// is created exactly as a client creates one.
const METRIC_NAMESPACE: &str = "Emulator/PolicyAlarms";

/// The metric those alarms watch inside it.
const METRIC_NAME: &str = "Load";

/// The threshold every alarm compares against.
const THRESHOLD: f64 = 50.0;

/// How long a poll waits for the scaling clock, and how long it sleeps between looks.
///
/// Generous: a target whose policies are arbitrated against each other collects the breaches of
/// one window before performing any of them, and nothing in the emulator sleeps for that.
const CLOCK_WAIT: Duration = Duration::from_secs(20);
/// See [`CLOCK_WAIT`].
const CLOCK_POLL: Duration = Duration::from_millis(100);

/// How long the suite watches a settled target for a movement that must never arrive: one whole
/// arbitration window after the winning movement landed, in which a suppressed breach performed
/// late would have shown up.
const QUIET_WAIT: Duration = Duration::from_secs(3);

/// The three identity members of the `ecs` scalable target on `resource_id`.
fn target(resource_id: &str) -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": resource_id,
        "ScalableDimension": DIMENSION,
    })
}

/// A metric alarm's ARN, as CloudWatch documents it:
/// `arn:aws:cloudwatch:<region>:<account>:alarm:<AlarmName>`.
fn alarm_arn(name: &str) -> String {
    format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:alarm:{name}")
}

/// Register the target `resource_id` names with the window `min`..`max`.
async fn register(emulator: &Emulator, resource_id: &str, min: i64, max: i64, case: &str) {
    let mut request = target(resource_id);
    request["MinCapacity"] = json!(min);
    request["MaxCapacity"] = json!(max);
    let (status, body) = emulator.call("RegisterScalableTarget", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
    );
}

/// Put a step scaling policy carrying `configuration` and answer its `PolicyARN`, which is what
/// an alarm names to invoke it.
async fn put_step_policy(
    emulator: &Emulator,
    resource_id: &str,
    policy_name: &str,
    configuration: Value,
    case: &str,
) -> String {
    let mut request = target(resource_id);
    request["PolicyName"] = json!(policy_name);
    request["PolicyType"] = json!("StepScaling");
    request["StepScalingPolicyConfiguration"] = configuration;
    let (status, body) = emulator.call("PutScalingPolicy", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
    );
    // A step policy mints no alarms of its own, so the put reports the alarms it CREATED as
    // none at all.
    assert_eq!(
        body["Alarms"],
        json!([]),
        "case {case:?}: expected a step policy to create no alarms, got {body:?}"
    );
    body["PolicyARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a PolicyARN, got {body:?}"))
        .to_string()
}

/// Create a metric alarm whose `AlarmActions` are `actions`.
async fn put_alarm(emulator: &Emulator, name: &str, actions: &[&str], case: &str) {
    let (status, body) = emulator
        .call_cloudwatch(
            "PutMetricAlarm",
            json!({
                "AlarmName": name,
                "Namespace": METRIC_NAMESPACE,
                "MetricName": METRIC_NAME,
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": THRESHOLD,
                "ComparisonOperator": "GreaterThanThreshold",
                "AlarmActions": actions,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutMetricAlarm for {name:?} to answer 200, got {status} {body:?}"
    );
}

/// The `Alarms` one policy reports, as `DescribeScalingPolicies` answers them.
async fn policy_alarms(emulator: &Emulator, resource_id: &str, name: &str, case: &str) -> Value {
    let mut request = target(resource_id);
    request["PolicyNames"] = json!([name]);
    let (status, body) = emulator.call("DescribeScalingPolicies", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingPolicies to answer 200, got {status} {body:?}"
    );
    let described = body["ScalingPolicies"]
        .as_array()
        .and_then(|policies| policies.first().cloned())
        .unwrap_or_else(|| panic!("case {case:?}: expected the policy {name:?}, got {body:?}"));
    described["Alarms"].clone()
}

/// The `Alarms` list a wiring must report: one entry per alarm name, ordered by name.
fn expected_alarms(names: &[&str]) -> Value {
    let mut names = names.to_vec();
    names.sort_unstable();
    Value::Array(
        names
            .iter()
            .map(|name| json!({"AlarmName": name, "AlarmARN": alarm_arn(name)}))
            .collect(),
    )
}

/// One wiring of customer alarms onto one policy: which alarms name it, and the alarms the
/// policy must then report.
struct WiringRow {
    case: &'static str,
    /// The alarms that name the policy under test.
    wired: &'static [&'static str],
    /// Alarms created in the same CloudWatch store that name something else, so the read
    /// reports the alarms of THIS policy rather than every alarm there is.
    unwired: &'static [&'static str],
}

/// The table. The rows differ along how many alarms invoke the policy (none, one, several) and
/// along how many alarms beside them exist — including an alarm wired to another scaling policy,
/// which is the one way a foreign alarm looks most like a match.
const WIRING_TABLE: &[WiringRow] = &[
    WiringRow {
        case: "no alarm names the policy",
        wired: &[],
        unwired: &[],
    },
    WiringRow {
        case: "one customer alarm names the policy",
        wired: &["breach-high"],
        unwired: &[],
    },
    WiringRow {
        case: "one alarm names the policy and one names a topic",
        wired: &["breach-high"],
        unwired: &["unrelated"],
    },
    WiringRow {
        case: "three alarms name the policy",
        wired: &["zzz-last", "breach-high", "mid-load"],
        unwired: &[],
    },
    WiringRow {
        case: "an alarm names the OTHER policy of the same target",
        wired: &["breach-high"],
        unwired: &["other-policys-alarm"],
    },
];

#[tokio::test]
async fn a_policys_alarms_are_the_cloudwatch_alarms_that_invoke_it() {
    /// The ARN an unwired alarm names instead of the policy under test: a topic, which is the
    /// other kind of action an alarm executes.
    const TOPIC_ARN: &str = "arn:aws:sns:us-east-1:000000000000:capacity-alerts";

    for row in WIRING_TABLE {
        let case = row.case;
        let emulator = Emulator::empty();
        let resource_id = emulator.given_ecs_service("assoc", "web", 2).await;
        register(&emulator, &resource_id, 1, 10, case).await;
        let configuration = json!({
            "AdjustmentType": "ChangeInCapacity",
            "StepAdjustments": [{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}],
            "Cooldown": 0,
        });
        let policy_arn =
            put_step_policy(&emulator, &resource_id, "stepper", configuration.clone(), case).await;
        // A second policy of the same target, so an alarm can name a policy that is not the one
        // read back.
        let other_arn =
            put_step_policy(&emulator, &resource_id, "other", configuration, case).await;

        // Before any alarm names it, the policy is invoked by nothing.
        let listed = policy_alarms(&emulator, &resource_id, "stepper", case).await;
        assert_eq!(
            listed,
            json!([]),
            "case {case:?}: expected a policy no alarm names to report no alarms, got {listed:?}"
        );

        for name in row.wired {
            put_alarm(&emulator, name, &[&policy_arn], case).await;
        }
        for name in row.unwired {
            let action = if *name == "other-policys-alarm" {
                other_arn.as_str()
            } else {
                TOPIC_ARN
            };
            put_alarm(&emulator, name, &[action], case).await;
        }

        let listed = policy_alarms(&emulator, &resource_id, "stepper", case).await;
        let expected = expected_alarms(row.wired);
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the policy to report {expected:?}, got {listed:?}"
        );

        // The association is CloudWatch's state, not a value this service stored: deleting the
        // alarms takes them off the policy again.
        if !row.wired.is_empty() {
            let (status, body) = emulator
                .call_cloudwatch("DeleteAlarms", json!({"AlarmNames": row.wired}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteAlarms to answer 200, got {status} {body:?}"
            );
            let listed = policy_alarms(&emulator, &resource_id, "stepper", case).await;
            assert_eq!(
                listed,
                json!([]),
                "case {case:?}: expected the deleted alarms to leave the policy, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_target_tracking_policy_reports_the_alarms_it_minted_for_itself() {
    let case = "a target-tracking policy";
    let emulator = Emulator::empty();
    let resource_id = emulator.given_ecs_service("tracked", "web", 2).await;
    register(&emulator, &resource_id, 1, 10, case).await;

    let mut request = target(&resource_id);
    request["PolicyName"] = json!("tracker");
    request["PolicyType"] = json!("TargetTrackingScaling");
    request["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": 40.0,
        "PredefinedMetricSpecification": {
            "PredefinedMetricType": "ECSServiceAverageCPUUtilization",
        },
    });
    let (status, body) = emulator.call("PutScalingPolicy", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
    );
    // The alarms the put reports are the ones it CREATED, and the same alarms invoke the policy,
    // so the read reports exactly that set — whichever order each names them in.
    let mut created: Vec<Value> = body["Alarms"].as_array().cloned().unwrap_or_default();
    assert!(
        !created.is_empty(),
        "case {case:?}: expected a target-tracking policy to create alarms, got {body:?}"
    );
    created.sort_by_key(|alarm| alarm["AlarmName"].as_str().unwrap_or_default().to_string());

    let listed = policy_alarms(&emulator, &resource_id, "tracker", case).await;
    assert_eq!(
        listed,
        Value::Array(created.clone()),
        "case {case:?}: expected the policy to report the alarms it created {created:?}, got \
         {listed:?}"
    );
}

/// One way a scaling policy goes away, as the request that does it.
struct DeletionRow {
    case: &'static str,
    operation: &'static str,
    /// Whether the call takes the whole scalable target with it, which deletes every policy of
    /// that target.
    deregisters: bool,
}

/// The table: both operations that delete a scaling policy.
const DELETION_TABLE: &[DeletionRow] = &[
    DeletionRow {
        case: "the policy deleted on its own",
        operation: "DeleteScalingPolicy",
        deregisters: false,
    },
    DeletionRow {
        case: "the scalable target deregistered",
        operation: "DeregisterScalableTarget",
        deregisters: true,
    },
];

#[tokio::test]
async fn deleting_a_policy_strips_its_arn_from_the_alarms_and_keeps_the_alarms() {
    /// A topic the customer alarm also notifies, which the deletion must leave alone.
    const TOPIC_ARN: &str = "arn:aws:sns:us-east-1:000000000000:capacity-alerts";
    /// The customer's own alarm, wired to the policy that is deleted.
    const WIRED_ALARM: &str = "customer-breach";
    /// A second alarm, wired to a policy of ANOTHER scalable target, which the deletion must
    /// leave wired.
    const OTHER_ALARM: &str = "another-targets-breach";

    for row in DELETION_TABLE {
        let case = row.case;
        let emulator = Emulator::empty();
        let resource_id = emulator.given_ecs_service("dropped", "web", 2).await;
        register(&emulator, &resource_id, 1, 10, case).await;
        let other_resource_id = emulator.given_ecs_service("kept", "web", 2).await;
        register(&emulator, &other_resource_id, 1, 10, case).await;
        let configuration = json!({
            "AdjustmentType": "ChangeInCapacity",
            "StepAdjustments": [{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}],
            "Cooldown": 0,
        });
        let policy_arn =
            put_step_policy(&emulator, &resource_id, "stepper", configuration.clone(), case).await;
        let kept_arn = put_step_policy(
            &emulator,
            &other_resource_id,
            "stepper",
            configuration,
            case,
        )
        .await;

        put_alarm(&emulator, WIRED_ALARM, &[&policy_arn, TOPIC_ARN], case).await;
        put_alarm(&emulator, OTHER_ALARM, &[&kept_arn], case).await;
        let before = described_alarm(&emulator, WIRED_ALARM, case).await;
        assert_eq!(
            before["AlarmActions"],
            json!([policy_arn, TOPIC_ARN]),
            "case {case:?}: expected the alarm to start out naming the policy, got {before:?}"
        );

        let mut request = target(&resource_id);
        if !row.deregisters {
            request["PolicyName"] = json!("stepper");
        }
        let (status, body) = emulator.call(row.operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {} to answer 200, got {status} {body:?}",
            row.operation
        );

        // The alarm itself survives, with everything but the deleted policy's action: the action
        // that is not a scaling policy stays, and so does every member of its configuration.
        let after = described_alarm(&emulator, WIRED_ALARM, case).await;
        assert_eq!(
            after["AlarmActions"],
            json!([TOPIC_ARN]),
            "case {case:?}: expected the deleted policy to leave the alarm's actions, got \
             {after:?}"
        );
        for member in [
            "AlarmName",
            "AlarmARN",
            "Namespace",
            "MetricName",
            "Statistic",
            "Period",
            "EvaluationPeriods",
            "Threshold",
            "ComparisonOperator",
        ] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: expected the alarm's {member} to be unchanged, got {after:?}"
            );
        }

        // And the alarm wired to a policy that was NOT deleted keeps its action.
        let other = described_alarm(&emulator, OTHER_ALARM, case).await;
        assert_eq!(
            other["AlarmActions"],
            json!([kept_arn]),
            "case {case:?}: expected the surviving policy to stay wired, got {other:?}"
        );
    }
}

/// One alarm as `DescribeAlarms` reports it.
async fn described_alarm(emulator: &Emulator, name: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call_cloudwatch("DescribeAlarms", json!({"AlarmNames": [name]}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeAlarms to answer 200, got {status} {body:?}"
    );
    body["MetricAlarms"]
        .as_array()
        .and_then(|alarms| alarms.first().cloned())
        .unwrap_or_else(|| panic!("case {case:?}: expected the alarm {name:?}, got {body:?}"))
}

/// One of the two policies a row triggers together.
struct ArbitratedPolicy {
    name: &'static str,
    adjustment_type: &'static str,
    adjustment: i64,
    /// Whether its alarm breaches ABOVE the threshold, which names both the comparison the
    /// alarm makes and the end of its step's interval that is inclusive.
    breaches_above: bool,
}

/// One pair of policies triggered at the same time on one scalable target.
struct ArbitrationRow {
    case: &'static str,
    /// The `desiredCount` the ECS service starts at, which both adjustments are computed from.
    starting: i64,
    min: i64,
    max: i64,
    policies: [ArbitratedPolicy; 2],
    /// The index in `policies` of the one whose movement must be performed.
    winner: usize,
    /// The capacity the scalable target must land on.
    capacity: i64,
    /// Whether the SECOND policy's alarm is driven into `ALARM` first, so the outcome cannot
    /// depend on the order the triggers arrive in.
    second_alarm_first: bool,
}

/// The table. The rows differ along the direction each policy moves in (two scale-outs, a
/// scale-out against a scale-in, two scale-ins), the adjustment type each states its movement
/// with, which of the two asks for the larger capacity, and the order the two alarms transition
/// in — and the winner is not always the first policy, so nothing passes on list order.
const ARBITRATION_TABLE: &[ArbitrationRow] = &[
    ArbitrationRow {
        case: "two scale-outs, the larger one wins",
        starting: 4,
        min: 1,
        max: 20,
        policies: [
            ArbitratedPolicy {
                name: "plus-three",
                adjustment_type: "ChangeInCapacity",
                adjustment: 3,
                breaches_above: true,
            },
            ArbitratedPolicy {
                name: "plus-one",
                adjustment_type: "ChangeInCapacity",
                adjustment: 1,
                breaches_above: true,
            },
        ],
        winner: 0,
        capacity: 7,
        second_alarm_first: false,
    },
    ArbitrationRow {
        case: "the larger one wins when its alarm transitions last",
        starting: 4,
        min: 1,
        max: 20,
        policies: [
            ArbitratedPolicy {
                name: "plus-one",
                adjustment_type: "ChangeInCapacity",
                adjustment: 1,
                breaches_above: true,
            },
            ArbitratedPolicy {
                name: "plus-five",
                adjustment_type: "ChangeInCapacity",
                adjustment: 5,
                breaches_above: true,
            },
        ],
        winner: 1,
        capacity: 9,
        second_alarm_first: false,
    },
    ArbitrationRow {
        case: "a scale-out against a scale-in, the largest capacity wins",
        starting: 4,
        min: 1,
        max: 20,
        policies: [
            ArbitratedPolicy {
                name: "out-to-nine",
                adjustment_type: "ExactCapacity",
                adjustment: 9,
                breaches_above: true,
            },
            ArbitratedPolicy {
                name: "in-to-two",
                adjustment_type: "ExactCapacity",
                adjustment: 2,
                breaches_above: false,
            },
        ],
        winner: 0,
        capacity: 9,
        second_alarm_first: true,
    },
    ArbitrationRow {
        case: "two scale-ins, the largest resulting capacity wins",
        starting: 8,
        min: 1,
        max: 20,
        policies: [
            ArbitratedPolicy {
                name: "in-to-one",
                adjustment_type: "ExactCapacity",
                adjustment: 1,
                breaches_above: false,
            },
            ArbitratedPolicy {
                name: "in-to-six",
                adjustment_type: "ExactCapacity",
                adjustment: 6,
                breaches_above: false,
            },
        ],
        winner: 1,
        capacity: 6,
        second_alarm_first: true,
    },
    ArbitrationRow {
        case: "a percentage scale-out against a fixed one",
        starting: 10,
        min: 1,
        max: 30,
        policies: [
            ArbitratedPolicy {
                name: "plus-half",
                adjustment_type: "PercentChangeInCapacity",
                adjustment: 50,
                breaches_above: true,
            },
            ArbitratedPolicy {
                name: "plus-two",
                adjustment_type: "ChangeInCapacity",
                adjustment: 2,
                breaches_above: true,
            },
        ],
        winner: 0,
        capacity: 15,
        second_alarm_first: false,
    },
    ArbitrationRow {
        case: "the larger movement is clamped to the registered ceiling and still wins",
        starting: 4,
        min: 1,
        max: 6,
        policies: [
            ArbitratedPolicy {
                name: "plus-twenty",
                adjustment_type: "ChangeInCapacity",
                adjustment: 20,
                breaches_above: true,
            },
            ArbitratedPolicy {
                name: "plus-one",
                adjustment_type: "ChangeInCapacity",
                adjustment: 1,
                breaches_above: true,
            },
        ],
        winner: 0,
        capacity: 6,
        second_alarm_first: false,
    },
];

#[tokio::test]
async fn two_policies_triggered_together_move_the_target_once_by_the_largest_capacity() {
    for row in ARBITRATION_TABLE {
        let case = row.case;
        let emulator = Emulator::empty();
        let cluster = "arbitrated";
        let service = "web";
        let resource_id = emulator
            .given_ecs_service(cluster, service, row.starting)
            .await;
        register(&emulator, &resource_id, row.min, row.max, case).await;

        let mut alarms: Vec<String> = Vec::new();
        for policy in &row.policies {
            let bound = if policy.breaches_above {
                "MetricIntervalLowerBound"
            } else {
                "MetricIntervalUpperBound"
            };
            let configuration = json!({
                "AdjustmentType": policy.adjustment_type,
                "StepAdjustments": [{bound: 0.0, "ScalingAdjustment": policy.adjustment}],
                // Nothing in a row breaches twice, so the wait a previous activity is given
                // does not come into it; each row states it rather than defaulting it.
                "Cooldown": 0,
            });
            let policy_arn =
                put_step_policy(&emulator, &resource_id, policy.name, configuration, case).await;
            let alarm_name = format!("{}-alarm", policy.name);
            let (status, body) = emulator
                .call_cloudwatch(
                    "PutMetricAlarm",
                    json!({
                        "AlarmName": alarm_name,
                        "Namespace": METRIC_NAMESPACE,
                        "MetricName": METRIC_NAME,
                        "Statistic": "Average",
                        "Period": 60,
                        "EvaluationPeriods": 1,
                        "Threshold": THRESHOLD,
                        "ComparisonOperator": if policy.breaches_above {
                            "GreaterThanThreshold"
                        } else {
                            "LessThanThreshold"
                        },
                        "AlarmActions": [policy_arn],
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutMetricAlarm for {alarm_name:?} to answer 200, got \
                 {status} {body:?}"
            );
            alarms.push(alarm_name);
        }

        // Both alarms go into ALARM one after the other, which is what triggers both policies in
        // one window.
        let order: Vec<&String> = if row.second_alarm_first {
            alarms.iter().rev().collect()
        } else {
            alarms.iter().collect()
        };
        for alarm_name in order {
            emulator.set_alarm_state(alarm_name, "ALARM").await;
        }

        let moved = await_desired_count(&emulator, cluster, service, row.capacity).await;
        assert_eq!(
            moved, row.capacity,
            "case {case:?}: expected the ECS desiredCount to reach {}, got {moved:?}",
            row.capacity
        );

        // The loser is fully suppressed: it is not performed late either, so the capacity and the
        // log are still the winner's after a window in which a second movement would have shown
        // up.
        tokio::time::sleep(QUIET_WAIT).await;
        let settled = emulator.ecs_desired_count(cluster, service).await;
        assert_eq!(
            settled, row.capacity,
            "case {case:?}: expected the desiredCount to stay at {}, got {settled:?}",
            row.capacity
        );

        let listed = activities(&emulator, &resource_id, case).await;
        let winner = row.policies[row.winner].name;
        let loser = row.policies[1 - row.winner].name;
        let attributable = |policy: &str| -> Vec<Value> {
            listed
                .iter()
                .filter(|activity| {
                    activity["Cause"]
                        .as_str()
                        .is_some_and(|cause| cause.ends_with(&format!("triggered policy {policy}")))
                })
                .cloned()
                .collect()
        };
        let by_loser = attributable(loser);
        assert!(
            by_loser.is_empty(),
            "case {case:?}: expected no activity attributable to {loser:?}, got {by_loser:?}"
        );
        let by_winner = attributable(winner);
        assert_eq!(
            by_winner.len(),
            1,
            "case {case:?}: expected exactly one activity attributable to {winner:?}, got \
             {by_winner:?}"
        );
        let activity = &by_winner[0];
        let expected_description = format!("Setting desired count to {}.", row.capacity);
        assert_eq!(
            activity["Description"],
            json!(expected_description),
            "case {case:?}: expected Description {expected_description:?}, got {activity:?}"
        );
        let expected_cause = format!(
            "monitor alarm {}-alarm in state ALARM triggered policy {winner}",
            winner
        );
        assert_eq!(
            activity["Cause"],
            json!(expected_cause),
            "case {case:?}: expected Cause {expected_cause:?}, got {activity:?}"
        );
    }
}

/// The scaling activities recorded for one target, newest first.
async fn activities(emulator: &Emulator, resource_id: &str, case: &str) -> Vec<Value> {
    let mut request = target(resource_id);
    request["IncludeNotScaledActivities"] = json!(true);
    let (status, body) = emulator
        .call("DescribeScalingActivities", request)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingActivities to answer 200, got {status} {body:?}"
    );
    body["ScalingActivities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ScalingActivities list, got {body:?}"))
        .clone()
}

/// Wait for the ECS service's `desiredCount` to reach `expected`, answering what it last read.
///
/// Nothing in the emulator sleeps for a movement: the poll is the TEST waiting for the clock
/// that executes a breach, so a row that never moves fails on the value it last saw.
async fn await_desired_count(
    emulator: &Emulator,
    cluster: &str,
    service: &str,
    expected: i64,
) -> i64 {
    let deadline = std::time::Instant::now() + CLOCK_WAIT;
    let mut last = emulator.ecs_desired_count(cluster, service).await;
    while last != expected && std::time::Instant::now() < deadline {
        tokio::time::sleep(CLOCK_POLL).await;
        last = emulator.ecs_desired_count(cluster, service).await;
    }
    last
}

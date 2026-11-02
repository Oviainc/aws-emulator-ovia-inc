// SPDX-License-Identifier: CC0-1.0

//! ARBITRATION between the scaling policies of ONE scalable target, end to end over the wire:
//! what the capacity of the scaled resource does when two policies of one target are triggered,
//! whatever the span between the two triggers.
//!
//! The rule under test: "Multiple scaling policies can be in force at the same time for the same
//! scalable target […] there is a chance that multiple policies could conflict, instructing the
//! scalable target to scale out or in at the same time. Application Auto Scaling gives precedence
//! to the policy that provides the largest capacity for both scale out and scale in." So ONE
//! scaling decision is taken for the policies of one target that are triggered together: the
//! capacity each of them asks for is computed from the capacity the target held as of that
//! decision, the target is moved to the largest of them, and every other policy is suppressed —
//! it moves no capacity and records no activity. The capacity a target lands on is therefore the
//! largest capacity one of its policies asked for and never the accumulation of two of them,
//! however far apart the alarms that trigger them transition.
//!
//! The rows differ along the SPAN between the two triggers (back to back, a span inside the
//! window the breaches of one target are collected over, and a span past it), along which of the
//! two policies is triggered first, along the direction each moves in and along the adjustment
//! type each states its movement with — so nothing passes by the order the triggers happen to
//! arrive in or by one policy's arithmetic.
//!
//! Every case goes through the front door a client uses: the target, the policies, the alarms,
//! the breaches, the activity log and the read-back of the ECS service are all requests.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::emulator::Emulator;

/// The namespace and dimension every row scales: "ecs:service:DesiredCount - The task count of
/// an ECS service".
const NAMESPACE: &str = "ecs";
/// See [`NAMESPACE`].
const DIMENSION: &str = "ecs:service:DesiredCount";

/// The metric namespace the rows' own alarms watch — a customer namespace, so each alarm is
/// created exactly as a client creates one and a datapoint for it is published over the wire.
const METRIC_NAMESPACE: &str = "Emulator/Arbitration";

/// The metric one policy's alarm watches inside it: one series per policy, so a row decides
/// WHEN each of its two policies is triggered by publishing into that policy's own series.
fn metric_name(policy: &str) -> String {
    format!("Load-{policy}")
}

/// How far past its threshold a published datapoint puts the metric, which is the difference a
/// step adjustment's interval is an interval OF.
const BREACH_MARGIN: f64 = 25.0;

/// The `Period` every row's alarms are evaluated on, in seconds.
const ALARM_PERIOD_SECS: f64 = 60.0;

/// A timestamp inside the newest `period`-second window a CloudWatch alarm's evaluation grades:
/// the middle of the newest window that has already CLOSED.
///
/// A metric is aggregated into periods anchored at multiples of the period and an alarm is
/// evaluated over the periods that are complete, so a datapoint meant to decide an alarm now is
/// stamped here rather than in the period the clock is still in.
fn graded_window_stamp(period: f64) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    ((now / period).floor() - 1.0) * period + period / 2.0
}

/// How long a poll waits for an alarm to report the state its datapoint puts it in.
const ALARM_WAIT: Duration = Duration::from_secs(20);

/// The threshold every alarm compares against, so a step's interval is a difference from this
/// value.
const THRESHOLD: f64 = 50.0;

/// How long a poll waits for the capacity a row expects, and how long it sleeps between looks.
///
/// Generous: the breaches of a target more than one of whose policies an alarm can fire are
/// collected over a window before any of them is performed, and nothing in the emulator sleeps
/// for that window.
const CAPACITY_WAIT: Duration = Duration::from_secs(50);
/// See [`CAPACITY_WAIT`].
const CAPACITY_POLL: Duration = Duration::from_millis(100);

/// How long a settled target is watched for a movement that must never arrive.
const QUIET_WAIT: Duration = Duration::from_secs(4);

/// The three identity members of the `ecs` scalable target on `resource_id`.
fn target(resource_id: &str) -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": resource_id,
        "ScalableDimension": DIMENSION,
    })
}

/// One of the two policies a row triggers on one target.
struct TriggeredPolicy {
    name: &'static str,
    /// "Specifies how the ScalingAdjustment value in a StepAdjustment is interpreted."
    adjustment_type: &'static str,
    adjustment: i64,
    /// Whether its alarm breaches ABOVE the threshold, which names both the comparison the
    /// alarm makes and the end of its step's interval that is inclusive: "If the metric value is
    /// above the breach threshold, the lower bound is inclusive […] Otherwise, it's exclusive."
    breaches_above: bool,
}

/// One pair of policies triggered on one scalable target, and what the target's capacity must do.
struct ArbitrationRow {
    case: &'static str,
    /// The `desiredCount` the ECS service starts at, which is the capacity as of the decision
    /// both policies are computed from.
    starting: i64,
    min: i64,
    max: i64,
    /// The two policies, in the order their alarms are driven into `ALARM`.
    policies: [TriggeredPolicy; 2],
    /// How long after the first alarm the second one is driven into `ALARM`.
    skew: Duration,
    /// The capacity the target must end on: the largest capacity either policy asked for, each
    /// computed from `starting`.
    capacity: i64,
    /// The index in `policies` of the one whose movement must be the last recorded activity.
    winner: usize,
    /// Whether exactly ONE activity may be recorded for the pair — true for the rows whose two
    /// triggers are collected into one decision, where the loser is suppressed before anything
    /// moves.
    one_activity: bool,
    /// Capacities the resource must never be reported at — above all, the ACCUMULATION of both
    /// adjustments.
    never: &'static [i64],
}

/// The rows whose two triggers arrive inside the window the breaches of one target are collected
/// over, so the pair settles ONE decision and the target moves exactly once.
const TOGETHER_TABLE: &[ArbitrationRow] = &[
    ArbitrationRow {
        case: "two scale-outs triggered back to back, the larger one second",
        starting: 5,
        min: 1,
        max: 60,
        policies: [
            TriggeredPolicy {
                name: "plus-two",
                adjustment_type: "ChangeInCapacity",
                adjustment: 2,
                breaches_above: true,
            },
            TriggeredPolicy {
                name: "plus-nine",
                adjustment_type: "ChangeInCapacity",
                adjustment: 9,
                breaches_above: true,
            },
        ],
        skew: Duration::ZERO,
        capacity: 14,
        winner: 1,
        one_activity: true,
        // 16 is 5 + 2 + 9: the two adjustments applied one after the other.
        never: &[7, 16],
    },
    ArbitrationRow {
        case: "two scale-outs triggered back to back, the larger one first",
        starting: 5,
        min: 1,
        max: 60,
        policies: [
            TriggeredPolicy {
                name: "plus-nine",
                adjustment_type: "ChangeInCapacity",
                adjustment: 9,
                breaches_above: true,
            },
            TriggeredPolicy {
                name: "plus-two",
                adjustment_type: "ChangeInCapacity",
                adjustment: 2,
                breaches_above: true,
            },
        ],
        skew: Duration::ZERO,
        capacity: 14,
        winner: 0,
        one_activity: true,
        never: &[7, 16],
    },
    ArbitrationRow {
        case: "the second trigger arrives seconds after the first, the larger one second",
        starting: 5,
        min: 1,
        max: 60,
        policies: [
            TriggeredPolicy {
                name: "plus-two",
                adjustment_type: "ChangeInCapacity",
                adjustment: 2,
                breaches_above: true,
            },
            TriggeredPolicy {
                name: "plus-nine",
                adjustment_type: "ChangeInCapacity",
                adjustment: 9,
                breaches_above: true,
            },
        ],
        skew: Duration::from_secs(4),
        capacity: 14,
        winner: 1,
        one_activity: true,
        never: &[7, 16],
    },
    ArbitrationRow {
        case: "a scale-out against a scale-in, the largest capacity wins",
        starting: 8,
        min: 1,
        max: 60,
        policies: [
            TriggeredPolicy {
                name: "in-to-two",
                adjustment_type: "ExactCapacity",
                adjustment: 2,
                breaches_above: false,
            },
            TriggeredPolicy {
                name: "out-to-eleven",
                adjustment_type: "ExactCapacity",
                adjustment: 11,
                breaches_above: true,
            },
        ],
        skew: Duration::from_secs(2),
        capacity: 11,
        winner: 1,
        one_activity: true,
        never: &[2],
    },
    ArbitrationRow {
        case: "a percentage scale-out against a fixed one, clamped to the ceiling",
        starting: 10,
        min: 1,
        max: 13,
        policies: [
            TriggeredPolicy {
                name: "plus-two",
                adjustment_type: "ChangeInCapacity",
                adjustment: 2,
                breaches_above: true,
            },
            TriggeredPolicy {
                name: "plus-half",
                adjustment_type: "PercentChangeInCapacity",
                adjustment: 50,
                breaches_above: true,
            },
        ],
        skew: Duration::from_secs(1),
        // 10 + 50% is 15, which the registered ceiling places at 13; the fixed +2 asks for 12.
        capacity: 13,
        winner: 1,
        one_activity: true,
        never: &[12, 15, 18],
    },
];

/// The rows whose second trigger arrives PAST that window, so the first policy has already
/// settled a decision when the second is read: the second is then compared against that decision
/// instead of adding to what it left behind.
const AFTERWARDS_TABLE: &[ArbitrationRow] = &[
    ArbitrationRow {
        case: "a larger policy triggered after the decision moves the target to its own capacity",
        starting: 5,
        min: 1,
        max: 60,
        policies: [
            TriggeredPolicy {
                name: "plus-two",
                adjustment_type: "ChangeInCapacity",
                adjustment: 2,
                breaches_above: true,
            },
            TriggeredPolicy {
                name: "plus-nine",
                adjustment_type: "ChangeInCapacity",
                adjustment: 9,
                breaches_above: true,
            },
        ],
        skew: Duration::from_secs(12),
        capacity: 14,
        winner: 1,
        one_activity: false,
        never: &[16],
    },
    ArbitrationRow {
        case: "a smaller policy triggered after the decision is suppressed entirely",
        starting: 5,
        min: 1,
        max: 60,
        policies: [
            TriggeredPolicy {
                name: "plus-nine",
                adjustment_type: "ChangeInCapacity",
                adjustment: 9,
                breaches_above: true,
            },
            TriggeredPolicy {
                name: "plus-two",
                adjustment_type: "ChangeInCapacity",
                adjustment: 2,
                breaches_above: true,
            },
        ],
        skew: Duration::from_secs(12),
        capacity: 14,
        winner: 0,
        one_activity: true,
        never: &[16, 7],
    },
];

#[tokio::test]
async fn policies_triggered_together_move_the_target_once_to_the_largest_capacity() {
    for row in TOGETHER_TABLE {
        drive(row).await;
    }
}

#[tokio::test]
async fn a_policy_triggered_after_the_decision_never_adds_to_the_capacity_it_settled() {
    for row in AFTERWARDS_TABLE {
        drive(row).await;
    }
}

/// Register the target, put both policies with an alarm each, drive the two alarms into `ALARM`
/// `row.skew` apart, and hold the row's assertions about what the capacity and the activity log
/// then say.
async fn drive(row: &ArbitrationRow) {
    let case = row.case;
    let emulator = Emulator::empty();
    let cluster = "arbitration";
    let service = "web";
    let resource_id = emulator
        .given_ecs_service(cluster, service, row.starting)
        .await;

    let mut registration = target(&resource_id);
    registration["MinCapacity"] = json!(row.min);
    registration["MaxCapacity"] = json!(row.max);
    let (status, body) = emulator
        .call("RegisterScalableTarget", registration)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
    );

    for policy in &row.policies {
        let policy_arn = put_step_policy(&emulator, &resource_id, policy, case).await;
        put_alarm(&emulator, policy, &policy_arn, case).await;
    }

    // The two triggers, `row.skew` apart: a datapoint past each alarm's threshold is published
    // into that alarm's own series, and the alarm CloudWatch evaluates from it is what triggers
    // the policy — so each trigger arrives on its own, which is how the alarms of one target
    // reach this service, and it keeps breaching rather than lapsing after it.
    breach(&emulator, &row.policies[0], case).await;
    if !row.skew.is_zero() {
        tokio::time::sleep(row.skew).await;
    }
    breach(&emulator, &row.policies[1], case).await;

    // Every capacity the resource was reported at while it settled, in the order they were read.
    let readings = read_until(&emulator, cluster, service, row.capacity).await;
    let settled = readings.last().copied().unwrap_or_default();
    assert_eq!(
        settled, row.capacity,
        "case {case:?}: expected the ECS desiredCount to settle at {}, got {settled:?} after \
         reading {readings:?}",
        row.capacity
    );
    for forbidden in row.never {
        assert!(
            !readings.contains(forbidden),
            "case {case:?}: expected the desiredCount never to be reported at {forbidden}, got \
             {readings:?}"
        );
    }

    // Nothing moves afterwards: a policy that lost the decision is suppressed rather than
    // performed late.
    tokio::time::sleep(QUIET_WAIT).await;
    let after = emulator.ecs_desired_count(cluster, service).await;
    assert_eq!(
        after, row.capacity,
        "case {case:?}: expected the desiredCount to stay at {}, got {after:?}",
        row.capacity
    );

    let listed = activities(&emulator, &resource_id, case).await;
    let causes: Vec<String> = listed
        .iter()
        .map(|activity| activity["Cause"].as_str().unwrap_or_default().to_string())
        .collect();
    if row.one_activity {
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one scaling activity for the pair, got {causes:?}"
        );
    }
    // The movement the target ended on is the winner's, whatever else the log holds: it names
    // that policy as its cause and the capacity the target settled at as its description.
    let winner = row.policies[row.winner].name;
    let newest = listed
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected a scaling activity, got none"));
    let expected_cause = format!(
        "monitor alarm {} in state ALARM triggered policy {winner}",
        alarm_of(winner)
    );
    assert_eq!(
        newest["Cause"],
        json!(expected_cause),
        "case {case:?}: expected the newest activity's Cause to be {expected_cause:?}, got \
         {newest:?}"
    );
    let expected_description = format!("Setting desired count to {}.", row.capacity);
    assert_eq!(
        newest["Description"],
        json!(expected_description),
        "case {case:?}: expected the newest activity's Description to be \
         {expected_description:?}, got {newest:?}"
    );
    // The loser of a settled decision records nothing at all: no movement of its own reached the
    // log, so the only activity attributable to it is the one the winner recorded (none).
    if row.one_activity {
        let loser = row.policies[1 - row.winner].name;
        let by_loser: Vec<&String> = causes
            .iter()
            .filter(|cause| cause.ends_with(&format!("triggered policy {loser}")))
            .collect();
        assert!(
            by_loser.is_empty(),
            "case {case:?}: expected no activity attributable to {loser:?}, got {causes:?}"
        );
    }
}

/// Put one step scaling policy of a row and answer its `PolicyARN`, which is what an alarm names
/// to invoke it.
async fn put_step_policy(
    emulator: &Emulator,
    resource_id: &str,
    policy: &TriggeredPolicy,
    case: &str,
) -> String {
    let bound = if policy.breaches_above {
        "MetricIntervalLowerBound"
    } else {
        "MetricIntervalUpperBound"
    };
    let mut request = target(resource_id);
    request["PolicyName"] = json!(policy.name);
    request["PolicyType"] = json!("StepScaling");
    // No `Cooldown`: the rows state the policies the way a client that leaves the wait to the
    // service states them, so the documented default of 300 seconds is what is in force.
    request["StepScalingPolicyConfiguration"] = json!({
        "AdjustmentType": policy.adjustment_type,
        "StepAdjustments": [{bound: 0.0, "ScalingAdjustment": policy.adjustment}],
    });
    let (status, body) = emulator.call("PutScalingPolicy", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutScalingPolicy for {:?} to answer 200, got {status} {body:?}",
        policy.name
    );
    body["PolicyARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a PolicyARN, got {body:?}"))
        .to_string()
}

/// Create the CloudWatch alarm that invokes one policy: its `AlarmActions` name that policy, which
/// is how a client attaches a step scaling policy to an alarm.
async fn put_alarm(
    emulator: &Emulator,
    policy: &TriggeredPolicy,
    policy_arn: &str,
    case: &str,
) {
    let alarm_name = alarm_of(policy.name);
    let (status, body) = emulator
        .call_cloudwatch(
            "PutMetricAlarm",
            json!({
                "AlarmName": alarm_name,
                "Namespace": METRIC_NAMESPACE,
                "MetricName": metric_name(policy.name),
                "Statistic": "Average",
                "Period": ALARM_PERIOD_SECS as i64,
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
        "case {case:?}: expected PutMetricAlarm for {alarm_name:?} to answer 200, got {status} \
         {body:?}"
    );
}

/// The name of the alarm that invokes one policy.
fn alarm_of(policy: &str) -> String {
    format!("{policy}-alarm")
}

/// Trigger one policy: publish a datapoint past its alarm's threshold, on the side that alarm
/// compares for, and wait for the alarm to report `ALARM` — which is the instant the policy was
/// triggered at.
async fn breach(emulator: &Emulator, policy: &TriggeredPolicy, case: &str) {
    let value = if policy.breaches_above {
        THRESHOLD + BREACH_MARGIN
    } else {
        THRESHOLD - BREACH_MARGIN
    };
    let (status, body) = emulator
        .call_cloudwatch(
            "PutMetricData",
            json!({
                "Namespace": METRIC_NAMESPACE,
                "MetricData": [{
                    "MetricName": metric_name(policy.name),
                    "Value": value,
                    "Timestamp": graded_window_stamp(ALARM_PERIOD_SECS),
                }],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutMetricData for {:?} to answer 200, got {status} {body:?}",
        policy.name
    );

    let alarm_name = alarm_of(policy.name);
    let deadline = Instant::now() + ALARM_WAIT;
    loop {
        let (status, body) = emulator
            .call_cloudwatch("DescribeAlarms", json!({"AlarmNames": [alarm_name.clone()]}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeAlarms to answer 200, got {status} {body:?}"
        );
        let state = body["MetricAlarms"]
            .as_array()
            .and_then(|alarms| alarms.first())
            .and_then(|alarm| alarm["StateValue"].as_str())
            .unwrap_or_default()
            .to_string();
        if state == "ALARM" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the alarm {alarm_name:?} to reach ALARM, it reports \
             {state:?}"
        );
        tokio::time::sleep(CAPACITY_POLL).await;
    }
}

/// Read the ECS service's `desiredCount` until it reaches `wanted` or the wait runs out, and
/// answer every DISTINCT capacity it was reported at in the order they were read — so a row can
/// assert what the capacity did on its way as well as where it ended.
async fn read_until(
    emulator: &Emulator,
    cluster: &str,
    service: &str,
    wanted: i64,
) -> Vec<i64> {
    let deadline = Instant::now() + CAPACITY_WAIT;
    let mut readings: Vec<i64> = Vec::new();
    loop {
        let count = emulator.ecs_desired_count(cluster, service).await;
        if readings.last() != Some(&count) {
            readings.push(count);
        }
        if count == wanted || Instant::now() >= deadline {
            return readings;
        }
        tokio::time::sleep(CAPACITY_POLL).await;
    }
}

/// The scaling activities recorded for one target, newest first, including the ones that were
/// not scaled.
async fn activities(emulator: &Emulator, resource_id: &str, case: &str) -> Vec<Value> {
    let mut request = target(resource_id);
    request["IncludeNotScaledActivities"] = json!(true);
    let (status, body) = emulator.call("DescribeScalingActivities", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingActivities to answer 200, got {status} {body:?}"
    );
    body["ScalingActivities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

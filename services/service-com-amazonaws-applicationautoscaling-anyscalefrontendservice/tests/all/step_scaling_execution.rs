// SPDX-License-Identifier: CC0-1.0

//! EXECUTING a step scaling policy: what a CloudWatch alarm breach does to the resource a
//! scalable target scales, end to end over the wire.
//!
//! The rule under test: when an alarm that executes a scaling policy as one of its actions is
//! in `ALARM`, Application Auto Scaling executes that policy — it picks the step adjustment
//! whose threshold-relative interval the breach falls in, computes the capacity that
//! adjustment asks for as its `AdjustmentType` interprets it, CLAMPS that capacity into the
//! `MinCapacity`/`MaxCapacity` the scalable target carries at that instant (an adjustment that
//! overshoots a bound is performed AT the bound, not refused), sets the capacity of the
//! resource in the service that owns it, and records the movement as one scaling activity.
//!
//! The docs the rows are read off:
//!
//! * `StepAdjustment$MetricIntervalLowerBound` / `$MetricIntervalUpperBound`: "The lower bound
//!   for the difference between the alarm threshold and the CloudWatch metric. If the metric
//!   value is above the breach threshold, the lower bound is inclusive […] Otherwise, it's
//!   exclusive", and the mirror of that for the upper bound; "A null value indicates negative
//!   infinity" / "positive infinity".
//! * `StepScalingPolicyConfiguration$AdjustmentType`: "Specifies how the ScalingAdjustment
//!   value in a StepAdjustment is interpreted […] ChangeInCapacity, ExactCapacity, and
//!   PercentChangeInCapacity" (`ADJUSTMENT_TYPE_VALUES`).
//! * `StepScalingPolicyConfiguration$MinAdjustmentMagnitude`: "The minimum value to scale by
//!   when the adjustment type is PercentChangeInCapacity."
//! * `StepScalingPolicyConfiguration$Cooldown`: "The amount of time, in seconds, to wait for a
//!   previous scaling activity to take effect. If not specified, the default value is 300."
//! * `RegisterScalableTarget$ScalableDimension`: "ecs:service:DesiredCount - The task count of
//!   an ECS service", which is the capacity an `ecs` target moves.
//!
//! Every case goes through the same front door a client uses — the policy, the alarm, the
//! breach and the read-back of the ECS service are all requests — so nothing reaches into a
//! state struct and a capacity that is not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};

use crate::emulator::{Emulator, PolicyAlarm};

/// The namespace an `ecs:service:DesiredCount` target is registered in.
const NAMESPACE: &str = "ecs";

/// The dimension every row scales.
const DIMENSION: &str = "ecs:service:DesiredCount";

/// The metric namespace the suite's own alarms watch. A customer namespace, so a datapoint for
/// it can be published over the wire the way a client publishes one.
const METRIC_NAMESPACE: &str = "Emulator/StepScaling";

/// The metric those alarms watch inside it.
const METRIC_NAME: &str = "Load";

/// The threshold every alarm in the suite breaches, so a step's interval is a difference from
/// this value.
const THRESHOLD: f64 = 50.0;

/// How long a poll that waits for the scaling clock keeps looking, and how long it sleeps
/// between looks. Generous: the clock wakes up several times inside this window, and nothing
/// in the emulator sleeps for it.
const CLOCK_WAIT: Duration = Duration::from_secs(6);
/// See [`CLOCK_WAIT`].
const CLOCK_POLL: Duration = Duration::from_millis(100);

/// One step adjustment of a row's policy: the bounds of the metric-to-threshold difference it
/// covers, and the adjustment it asks for.
struct Step {
    lower: Option<f64>,
    upper: Option<f64>,
    adjustment: i64,
}

/// One execution to pin: the window the target is registered with, the capacity the ECS
/// service starts at, the policy that fires, the direction its alarm breaches in, and the
/// capacity the movement must land on.
struct ExecutionRow {
    case: &'static str,
    /// The `desiredCount` the ECS service is created with, which is the capacity the
    /// adjustment is computed from.
    starting: i64,
    min: i64,
    max: i64,
    adjustment_type: &'static str,
    steps: &'static [Step],
    min_adjustment_magnitude: Option<i64>,
    /// The alarm's `ComparisonOperator`, which names the side of the threshold the breach is
    /// on.
    comparison_operator: &'static str,
    /// Where the capacity lands once the adjustment is clamped into the window.
    capacity: i64,
}

/// The rows differ in the adjustment type, in the direction and magnitude of the adjustment,
/// in whether the result falls inside the window or past either bound, and in how many steps
/// the policy carries.
const EXECUTION_CATALOG: &[ExecutionRow] = &[
    // The adjustment overshoots the ceiling: the movement happens AT the ceiling.
    ExecutionRow {
        case: "change in capacity past the ceiling",
        starting: 4,
        min: 4,
        max: 6,
        adjustment_type: "ChangeInCapacity",
        steps: &[Step { lower: Some(0.0), upper: None, adjustment: 10 }],
        min_adjustment_magnitude: None,
        comparison_operator: "GreaterThanThreshold",
        capacity: 6,
    },
    // The mirror of it: an adjustment that undershoots the floor moves to the floor.
    ExecutionRow {
        case: "change in capacity past the floor",
        starting: 4,
        min: 2,
        max: 6,
        adjustment_type: "ChangeInCapacity",
        steps: &[Step { lower: None, upper: Some(0.0), adjustment: -10 }],
        min_adjustment_magnitude: None,
        comparison_operator: "LessThanThreshold",
        capacity: 2,
    },
    // An adjustment inside the window is performed as it stands.
    ExecutionRow {
        case: "change in capacity inside the window",
        starting: 4,
        min: 1,
        max: 10,
        adjustment_type: "ChangeInCapacity",
        steps: &[Step { lower: Some(0.0), upper: None, adjustment: 3 }],
        min_adjustment_magnitude: None,
        comparison_operator: "GreaterThanThreshold",
        capacity: 7,
    },
    // `ExactCapacity`: the adjustment IS the capacity, and it is clamped like any other.
    ExecutionRow {
        case: "exact capacity past the ceiling",
        starting: 4,
        min: 1,
        max: 6,
        adjustment_type: "ExactCapacity",
        steps: &[Step { lower: Some(0.0), upper: None, adjustment: 9 }],
        min_adjustment_magnitude: None,
        comparison_operator: "GreaterThanThreshold",
        capacity: 6,
    },
    ExecutionRow {
        case: "exact capacity inside the window",
        starting: 4,
        min: 1,
        max: 10,
        adjustment_type: "ExactCapacity",
        steps: &[Step { lower: Some(0.0), upper: None, adjustment: 2 }],
        min_adjustment_magnitude: None,
        comparison_operator: "GreaterThanThreshold",
        capacity: 2,
    },
    // A percentage of the capacity: 30% of 7 is 2.1 units, taken toward zero.
    ExecutionRow {
        case: "percent change toward zero",
        starting: 7,
        min: 1,
        max: 20,
        adjustment_type: "PercentChangeInCapacity",
        steps: &[Step { lower: Some(0.0), upper: None, adjustment: 30 }],
        min_adjustment_magnitude: None,
        comparison_operator: "GreaterThanThreshold",
        capacity: 9,
    },
    // 10% of 4 is 0.4 units, which is not nothing: a non-zero percentage moves by a unit.
    ExecutionRow {
        case: "percent change under one unit",
        starting: 4,
        min: 1,
        max: 20,
        adjustment_type: "PercentChangeInCapacity",
        steps: &[Step { lower: Some(0.0), upper: None, adjustment: 10 }],
        min_adjustment_magnitude: None,
        comparison_operator: "GreaterThanThreshold",
        capacity: 5,
    },
    // The same percentage with the documented floor on its magnitude.
    ExecutionRow {
        case: "percent change under the minimum magnitude",
        starting: 4,
        min: 1,
        max: 20,
        adjustment_type: "PercentChangeInCapacity",
        steps: &[Step { lower: Some(0.0), upper: None, adjustment: 10 }],
        min_adjustment_magnitude: Some(3),
        comparison_operator: "GreaterThanThreshold",
        capacity: 7,
    },
    // A percentage scale-in: 25% of 10 is 2.5 units, taken toward zero, removed.
    ExecutionRow {
        case: "percent change scaling in",
        starting: 10,
        min: 1,
        max: 20,
        adjustment_type: "PercentChangeInCapacity",
        steps: &[Step { lower: None, upper: Some(0.0), adjustment: -25 }],
        min_adjustment_magnitude: None,
        comparison_operator: "LessThanThreshold",
        capacity: 8,
    },
    // Several steps: a breach whose size the alarm's state carries no datapoint for is the
    // SMALLEST one in its direction, the threshold itself, so the step at the threshold is the
    // one that fires.
    ExecutionRow {
        case: "the step at the threshold of a multi-step policy",
        starting: 4,
        min: 1,
        max: 20,
        adjustment_type: "ChangeInCapacity",
        steps: &[
            Step { lower: Some(0.0), upper: Some(10.0), adjustment: 1 },
            Step { lower: Some(10.0), upper: None, adjustment: 5 },
        ],
        min_adjustment_magnitude: None,
        comparison_operator: "GreaterThanThreshold",
        capacity: 5,
    },
];

/// `StepAdjustments` as the request carries them.
fn step_adjustments(steps: &[Step]) -> Value {
    Value::Array(
        steps
            .iter()
            .map(|step| {
                let mut adjustment = json!({"ScalingAdjustment": step.adjustment});
                if let Some(lower) = step.lower {
                    adjustment["MetricIntervalLowerBound"] = json!(lower);
                }
                if let Some(upper) = step.upper {
                    adjustment["MetricIntervalUpperBound"] = json!(upper);
                }
                adjustment
            })
            .collect(),
    )
}

/// The three identity members of an `ecs` scalable target on `resource_id`.
fn target(resource_id: &str) -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": resource_id,
        "ScalableDimension": DIMENSION,
    })
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

/// Put a step scaling policy and answer the `PolicyARN` it reports, which is the action an
/// alarm executes.
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
    body["PolicyARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a PolicyARN, got {body:?}"))
        .to_string()
}

/// The scaling activities recorded for one target, newest first.
async fn activities(emulator: &Emulator, resource_id: &str, case: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call("DescribeScalingActivities", target(resource_id))
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

#[tokio::test]
async fn an_alarm_breach_executes_the_policy_clamped_into_the_registered_window() {
    for row in EXECUTION_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        let cluster = "scaled";
        let service = "web";
        let resource_id = emulator
            .given_ecs_service(cluster, service, row.starting)
            .await;
        register(&emulator, &resource_id, row.min, row.max, case).await;

        let mut configuration = json!({
            "AdjustmentType": row.adjustment_type,
            "StepAdjustments": step_adjustments(row.steps),
            // Nothing in a row breaches twice, so the wait a previous activity is given does
            // not come into it; it is named rather than defaulted so each row states it.
            "Cooldown": 0,
        });
        if let Some(magnitude) = row.min_adjustment_magnitude {
            configuration["MinAdjustmentMagnitude"] = json!(magnitude);
        }
        let policy_arn =
            put_step_policy(&emulator, &resource_id, "step-policy", configuration, case).await;

        // Before the breach nothing has moved: the policy on its own changes no capacity.
        assert_eq!(
            emulator.ecs_desired_count(cluster, service).await,
            row.starting,
            "case {case:?}: expected putting the policy to move no capacity"
        );

        let alarm_name = format!("{case}-alarm");
        emulator
            .given_policy_alarm(&PolicyAlarm {
                name: &alarm_name,
                policy_arn: &policy_arn,
                metric_namespace: METRIC_NAMESPACE,
                metric_name: METRIC_NAME,
                threshold: THRESHOLD,
                comparison_operator: row.comparison_operator,
            })
            .await;
        emulator.set_alarm_state(&alarm_name, "ALARM").await;

        let moved = await_desired_count(&emulator, cluster, service, row.capacity).await;
        assert_eq!(
            moved, row.capacity,
            "case {case:?}: expected the ECS desiredCount to reach {}, got {moved:?}",
            row.capacity
        );

        // The movement is reported as one scaling activity, which names the capacity it set
        // and the alarm and policy that caused it.
        let listed = activities(&emulator, &resource_id, case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one scaling activity, got {listed:?}"
        );
        let activity = &listed[0];
        let expected_description = format!("Setting desired count to {}.", row.capacity);
        assert_eq!(
            activity["Description"],
            json!(expected_description),
            "case {case:?}: expected Description {expected_description:?}, got {activity:?}"
        );
        let expected_cause =
            format!("monitor alarm {alarm_name} in state ALARM triggered policy step-policy");
        assert_eq!(
            activity["Cause"],
            json!(expected_cause),
            "case {case:?}: expected Cause {expected_cause:?}, got {activity:?}"
        );
        assert_eq!(
            activity["NotScaledReasons"],
            Value::Null,
            "case {case:?}: expected a scaled activity to carry no NotScaledReasons, \
             got {activity:?}"
        );

        // And the window is what held the movement: the target reports the same bounds it was
        // registered with, whatever the adjustment asked for.
        let (status, body) = emulator
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": NAMESPACE, "ResourceIds": [resource_id]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {status} {body:?}"
        );
        let reported = &body["ScalableTargets"][0];
        assert_eq!(
            (reported["MinCapacity"].as_i64(), reported["MaxCapacity"].as_i64()),
            (Some(row.min), Some(row.max)),
            "case {case:?}: expected the registered window to be unchanged, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_breach_whose_size_the_alarm_measured_picks_the_step_that_size_falls_in() {
    // The same two-step policy as the catalogue's multi-step row, breached by a DATAPOINT 25
    // past the threshold instead of by a forced state: the difference falls in the second
    // step's interval, so that step is the one performed.
    let case = "measured breach past the first step";
    let emulator = Emulator::empty();
    let cluster = "measured";
    let service = "web";
    let resource_id = emulator.given_ecs_service(cluster, service, 4).await;
    register(&emulator, &resource_id, 1, 20, case).await;
    let policy_arn = put_step_policy(
        &emulator,
        &resource_id,
        "step-policy",
        json!({
            "AdjustmentType": "ChangeInCapacity",
            "Cooldown": 0,
            "StepAdjustments": [
                {"MetricIntervalLowerBound": 0, "MetricIntervalUpperBound": 10,
                 "ScalingAdjustment": 1},
                {"MetricIntervalLowerBound": 10, "ScalingAdjustment": 5},
            ],
        }),
        case,
    )
    .await;
    let alarm_name = "measured-alarm";
    emulator
        .given_policy_alarm(&PolicyAlarm {
            name: alarm_name,
            policy_arn: &policy_arn,
            metric_namespace: METRIC_NAMESPACE,
            metric_name: METRIC_NAME,
            threshold: THRESHOLD,
            comparison_operator: "GreaterThanThreshold",
        })
        .await;

    // One datapoint 25 past the threshold, published the way a client publishes one. The alarm
    // CloudWatch evaluates from it is what fires the policy.
    let (status, body) = emulator
        .call_cloudwatch(
            "PutMetricData",
            json!({
                "Namespace": METRIC_NAMESPACE,
                "MetricData": [{"MetricName": METRIC_NAME, "Value": THRESHOLD + 25.0}],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutMetricData to answer 200, got {status} {body:?}"
    );

    let moved = await_desired_count(&emulator, cluster, service, 9).await;
    assert_eq!(
        moved, 9,
        "case {case:?}: expected the second step (+5) to be the one performed, \
         so a desiredCount of 9, got {moved:?}"
    );
}

#[tokio::test]
async fn one_transition_is_executed_once_and_the_next_waits_for_the_cooldown() {
    let case = "one transition, one execution";
    let emulator = Emulator::empty();
    let cluster = "cooled";
    let service = "web";
    let resource_id = emulator.given_ecs_service(cluster, service, 4).await;
    register(&emulator, &resource_id, 1, 20, case).await;
    let policy_arn = put_step_policy(
        &emulator,
        &resource_id,
        "step-policy",
        json!({
            "AdjustmentType": "ChangeInCapacity",
            "StepAdjustments": [{"MetricIntervalLowerBound": 0, "ScalingAdjustment": 2}],
        }),
        case,
    )
    .await;
    let alarm_name = "cooled-alarm";
    emulator
        .given_policy_alarm(&PolicyAlarm {
            name: alarm_name,
            policy_arn: &policy_arn,
            metric_namespace: METRIC_NAMESPACE,
            metric_name: METRIC_NAME,
            threshold: THRESHOLD,
            comparison_operator: "GreaterThanThreshold",
        })
        .await;
    emulator.set_alarm_state(alarm_name, "ALARM").await;

    let moved = await_desired_count(&emulator, cluster, service, 6).await;
    assert_eq!(
        moved, 6,
        "case {case:?}: expected the first breach to add 2 tasks, got {moved:?}"
    );

    // The alarm is STILL in ALARM: that is the same transition, which has already been
    // performed, so no further pass performs it again.
    for _ in 0..3 {
        let listed = activities(&emulator, &resource_id, case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected the one transition to be executed once, got {listed:?}"
        );
        tokio::time::sleep(CLOCK_POLL).await;
    }
    assert_eq!(
        emulator.ecs_desired_count(cluster, service).await,
        6,
        "case {case:?}: expected the capacity to stay where the one execution left it"
    );

    // A NEW transition into ALARM is a new breach — but this policy named no `Cooldown`, so
    // the documented default wait of 300 seconds is still in force: the breach waits, and
    // records no activity and no movement inside it.
    emulator.set_alarm_state(alarm_name, "OK").await;
    emulator.set_alarm_state(alarm_name, "ALARM").await;
    let listed = activities(&emulator, &resource_id, case).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected a breach inside the cooldown to record no activity, \
         got {listed:?}"
    );
    assert_eq!(
        emulator.ecs_desired_count(cluster, service).await,
        6,
        "case {case:?}: expected a breach inside the cooldown to move no capacity"
    );
}

#[tokio::test]
async fn a_suspended_direction_performs_no_movement() {
    let case = "scale out suspended";
    let emulator = Emulator::empty();
    let cluster = "suspended";
    let service = "web";
    let resource_id = emulator.given_ecs_service(cluster, service, 4).await;
    let mut registration = target(&resource_id);
    registration["MinCapacity"] = json!(1);
    registration["MaxCapacity"] = json!(20);
    // "DynamicScalingOutSuspended — Whether scale out by a target tracking scaling policy or a
    // step scaling policy is suspended."
    registration["SuspendedState"] = json!({"DynamicScalingOutSuspended": true});
    let (status, body) = emulator
        .call("RegisterScalableTarget", registration)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
    );
    let policy_arn = put_step_policy(
        &emulator,
        &resource_id,
        "step-policy",
        json!({
            "AdjustmentType": "ChangeInCapacity",
            "Cooldown": 0,
            "StepAdjustments": [{"MetricIntervalLowerBound": 0, "ScalingAdjustment": 2}],
        }),
        case,
    )
    .await;
    let alarm_name = "suspended-alarm";
    emulator
        .given_policy_alarm(&PolicyAlarm {
            name: alarm_name,
            policy_arn: &policy_arn,
            metric_namespace: METRIC_NAMESPACE,
            metric_name: METRIC_NAME,
            threshold: THRESHOLD,
            comparison_operator: "GreaterThanThreshold",
        })
        .await;
    emulator.set_alarm_state(alarm_name, "ALARM").await;

    // Give the clock the same window a row that DOES move is given, so this asserts the
    // movement never happens rather than that it had not happened yet.
    let settled = await_desired_count(&emulator, cluster, service, 6).await;
    assert_eq!(
        settled, 4,
        "case {case:?}: expected a suspended scale-out direction to move no capacity, \
         got {settled:?}"
    );
    let listed = activities(&emulator, &resource_id, case).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: expected no scaling activity at all, got {listed:?}"
    );
}

#[tokio::test]
async fn an_alarm_wired_to_another_policy_executes_nothing() {
    let case = "an alarm that names no policy of this target";
    let emulator = Emulator::empty();
    let cluster = "unwired";
    let service = "web";
    let resource_id = emulator.given_ecs_service(cluster, service, 4).await;
    register(&emulator, &resource_id, 1, 20, case).await;
    put_step_policy(
        &emulator,
        &resource_id,
        "step-policy",
        json!({
            "AdjustmentType": "ChangeInCapacity",
            "Cooldown": 0,
            "StepAdjustments": [{"MetricIntervalLowerBound": 0, "ScalingAdjustment": 2}],
        }),
        case,
    )
    .await;
    // An alarm whose action is an SNS topic rather than this policy: the wiring is what makes
    // a policy alarm-driven, so a breach of this one executes nothing.
    let alarm_name = "unwired-alarm";
    emulator
        .given_policy_alarm(&PolicyAlarm {
            name: alarm_name,
            policy_arn: &format!(
                "arn:aws:sns:{}:{}:ops",
                crate::emulator::REGION,
                crate::emulator::ACCOUNT
            ),
            metric_namespace: METRIC_NAMESPACE,
            metric_name: METRIC_NAME,
            threshold: THRESHOLD,
            comparison_operator: "GreaterThanThreshold",
        })
        .await;
    emulator.set_alarm_state(alarm_name, "ALARM").await;

    let settled = await_desired_count(&emulator, cluster, service, 6).await;
    assert_eq!(
        settled, 4,
        "case {case:?}: expected no execution at all, got a desiredCount of {settled:?}"
    );
    let listed = activities(&emulator, &resource_id, case).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: expected no scaling activity at all, got {listed:?}"
    );
}

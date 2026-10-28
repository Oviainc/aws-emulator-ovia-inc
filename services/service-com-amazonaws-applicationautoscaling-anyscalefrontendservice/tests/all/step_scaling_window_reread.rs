// SPDX-License-Identifier: CC0-1.0

//! WHICH capacity window a step scaling policy's adjustment is clamped into: the one the
//! scalable target carries when the breach is dispatched, not the one that was in force when
//! the policy was put.
//!
//! The rule under test: executing a scaling policy places the capacity it computed "inside of
//! this range" — the `MinCapacity`/`MaxCapacity` the scalable target carries AT THAT INSTANT —
//! so a registration that moves either bound between the policy being put and the breach being
//! dispatched is the registration the movement is held by, in either direction and whether it
//! widens or narrows the window.
//!
//! The docs the rows are read off:
//!
//! * `RegisterScalableTarget$MinCapacity` / `$MaxCapacity`: the bounds a target is registered
//!   with, and "To update a scalable target, specify the parameters that you want to change",
//!   which is what makes a later registration an UPDATE of the same identity rather than a
//!   second target.
//! * `StepScalingPolicyConfiguration$AdjustmentType`: "Specifies how the ScalingAdjustment
//!   value in a StepAdjustment is interpreted […] ChangeInCapacity, ExactCapacity, and
//!   PercentChangeInCapacity".
//! * `StepAdjustment$MetricIntervalLowerBound` / `$MetricIntervalUpperBound`: the interval of
//!   "the difference between the alarm threshold and the CloudWatch metric", whose null end is
//!   an infinity.
//! * `RegisterScalableTarget$ScalableDimension`: "ecs:service:DesiredCount - The task count of
//!   an ECS service", the capacity these rows move.
//!
//! Each row states the capacity the window IN FORCE AT THE BREACH asks for and the one the
//! window the policy was put under would have asked for, and asserts the movement lands on the
//! first and not the second — so a policy that captured its window when it was created cannot
//! pass. Every call goes through the front door a client uses, and the capacity is read back
//! out of ECS.

use std::time::Duration;

use serde_json::{json, Value};

use crate::emulator::{Emulator, PolicyAlarm};

/// The namespace an `ecs:service:DesiredCount` target is registered in.
const NAMESPACE: &str = "ecs";

/// The dimension every row scales.
const DIMENSION: &str = "ecs:service:DesiredCount";

/// The metric namespace this suite's alarms watch. A customer namespace, so the alarm is one a
/// client could create itself.
const METRIC_NAMESPACE: &str = "Emulator/StepScalingWindow";

/// The metric those alarms watch inside it.
const METRIC_NAME: &str = "Load";

/// The threshold every alarm in the suite breaches.
const THRESHOLD: f64 = 50.0;

/// How long a poll that waits for the scaling clock keeps looking, and how long it sleeps
/// between looks. Nothing in the emulator sleeps for a movement, so the waiting is the test's.
const CLOCK_WAIT: Duration = Duration::from_secs(6);
/// See [`CLOCK_WAIT`].
const CLOCK_POLL: Duration = Duration::from_millis(100);

/// One re-registration to pin: the window the policy is put under, the window registered
/// before the breach is dispatched, and the two capacities that tell the first apart from the
/// second.
struct WindowRow {
    case: &'static str,
    /// The `desiredCount` the ECS service is created with. Every row creates it at the floor of
    /// the window it is first registered with, so the registration itself moves nothing and the
    /// only movement in the row is the breach's.
    starting: i64,
    /// The window in force when `PutScalingPolicy` is called.
    put_window: (i64, i64),
    /// The window registered after that, in force when the breach is dispatched. It contains
    /// `starting`, so re-registering moves no capacity either.
    breach_window: (i64, i64),
    adjustment_type: &'static str,
    /// The adjustment of the single step every row's policy carries.
    adjustment: i64,
    /// The alarm's `ComparisonOperator`, which names the side of the threshold the breach is on
    /// and so the direction the step is read in.
    comparison_operator: &'static str,
    /// Where the capacity lands, clamped into `breach_window`.
    capacity: i64,
    /// Where it would have landed had the policy kept `put_window`. Different from
    /// [`WindowRow::capacity`] in every row, and asserted against.
    stale_capacity: i64,
}

/// The rows differ in which bound moved, in whether the move widened or narrowed the window,
/// in the adjustment type, and in whether the adjustment is held by the new bound or admitted
/// in full.
const WINDOW_CATALOG: &[WindowRow] = &[
    // The ceiling is lowered under the policy: the scale out is held by the NEW ceiling, which
    // is below the capacity the old window would have allowed.
    WindowRow {
        case: "the ceiling was lowered after the policy was put",
        starting: 2,
        put_window: (2, 20),
        breach_window: (2, 4),
        adjustment_type: "ChangeInCapacity",
        adjustment: 10,
        comparison_operator: "GreaterThanThreshold",
        capacity: 4,
        stale_capacity: 12,
    },
    // The ceiling is raised under the policy: the scale out reaches the new ceiling, past where
    // the window the policy was put under would have stopped it.
    WindowRow {
        case: "the ceiling was raised after the policy was put",
        starting: 2,
        put_window: (2, 4),
        breach_window: (2, 9),
        adjustment_type: "ChangeInCapacity",
        adjustment: 10,
        comparison_operator: "GreaterThanThreshold",
        capacity: 9,
        stale_capacity: 4,
    },
    // The same widening with an adjustment the new window admits in FULL: the clamp is a
    // window, not a jump to the nearest bound.
    WindowRow {
        case: "the widened window admits the adjustment in full",
        starting: 2,
        put_window: (2, 4),
        breach_window: (2, 9),
        adjustment_type: "ChangeInCapacity",
        adjustment: 3,
        comparison_operator: "GreaterThanThreshold",
        capacity: 5,
        stale_capacity: 4,
    },
    // `ExactCapacity` names the capacity outright, and is clamped into the window of the
    // instant like any other adjustment.
    WindowRow {
        case: "an exact capacity against a lowered ceiling",
        starting: 2,
        put_window: (2, 20),
        breach_window: (2, 5),
        adjustment_type: "ExactCapacity",
        adjustment: 9,
        comparison_operator: "GreaterThanThreshold",
        capacity: 5,
        stale_capacity: 9,
    },
    // The FLOOR is the other bound, and it is re-read too: lowering it lets a scale in go
    // below where the floor the policy was put under would have held it.
    WindowRow {
        case: "the floor was lowered after the policy was put",
        starting: 8,
        put_window: (8, 20),
        breach_window: (3, 20),
        adjustment_type: "ChangeInCapacity",
        adjustment: -10,
        comparison_operator: "LessThanThreshold",
        capacity: 3,
        stale_capacity: 8,
    },
    // A percentage scale in against a lowered floor: 40% of 10 is 4 units, taken toward zero,
    // which the new floor holds at 7 and the old floor would have held at 10 — no movement at
    // all.
    WindowRow {
        case: "a percentage scale in against a lowered floor",
        starting: 10,
        put_window: (10, 20),
        breach_window: (7, 20),
        adjustment_type: "PercentChangeInCapacity",
        adjustment: -40,
        comparison_operator: "LessThanThreshold",
        capacity: 7,
        stale_capacity: 10,
    },
];

/// The three identity members of an `ecs` scalable target on `resource_id`.
fn target(resource_id: &str) -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": resource_id,
        "ScalableDimension": DIMENSION,
    })
}

/// Register the target `resource_id` names with the window `min`..`max`, which is a first
/// registration or an update of the same identity depending on what is stored.
async fn register(emulator: &Emulator, resource_id: &str, (min, max): (i64, i64), case: &str) {
    let mut request = target(resource_id);
    request["MinCapacity"] = json!(min);
    request["MaxCapacity"] = json!(max);
    let (status, body) = emulator.call("RegisterScalableTarget", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget {min}..{max} to answer 200, \
         got {status} {body:?}"
    );
}

/// Put a one-step `StepScaling` policy and answer the `PolicyARN` an alarm executes.
///
/// `Cooldown` is 0 in every row: no row breaches twice inside a wait, so the wait a previous
/// activity is given is named rather than left to its default.
async fn put_step_policy(
    emulator: &Emulator,
    resource_id: &str,
    adjustment_type: &str,
    adjustment: i64,
    scale_out: bool,
    case: &str,
) -> String {
    let step = if scale_out {
        json!({"MetricIntervalLowerBound": 0, "ScalingAdjustment": adjustment})
    } else {
        json!({"MetricIntervalUpperBound": 0, "ScalingAdjustment": adjustment})
    };
    let mut request = target(resource_id);
    request["PolicyName"] = json!("step-policy");
    request["PolicyType"] = json!("StepScaling");
    request["StepScalingPolicyConfiguration"] = json!({
        "AdjustmentType": adjustment_type,
        "Cooldown": 0,
        "StepAdjustments": [step],
    });
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
async fn a_breach_is_clamped_into_the_window_registered_when_it_is_dispatched() {
    for row in WINDOW_CATALOG {
        let case = row.case;
        assert_ne!(
            row.capacity, row.stale_capacity,
            "case {case:?}: the row must tell the two windows apart"
        );
        let emulator = Emulator::empty();
        let cluster = "scaled";
        let service = "web";
        let resource_id = emulator
            .given_ecs_service(cluster, service, row.starting)
            .await;

        // The window the POLICY is put under.
        register(&emulator, &resource_id, row.put_window, case).await;
        let scale_out = row.comparison_operator.starts_with("GreaterThan");
        let policy_arn = put_step_policy(
            &emulator,
            &resource_id,
            row.adjustment_type,
            row.adjustment,
            scale_out,
            case,
        )
        .await;

        // The window registered AFTER it, which is the one in force when the breach is
        // dispatched. It contains the capacity the service sits at, so registering it moves
        // nothing by itself.
        register(&emulator, &resource_id, row.breach_window, case).await;
        assert_eq!(
            emulator.ecs_desired_count(cluster, service).await,
            row.starting,
            "case {case:?}: expected re-registering the window to move no capacity"
        );

        let alarm_name = "window-alarm";
        emulator
            .given_policy_alarm(&PolicyAlarm {
                name: alarm_name,
                policy_arn: &policy_arn,
                metric_namespace: METRIC_NAMESPACE,
                metric_name: METRIC_NAME,
                threshold: THRESHOLD,
                comparison_operator: row.comparison_operator,
            })
            .await;
        emulator.set_alarm_state(alarm_name, "ALARM").await;

        let moved = await_desired_count(&emulator, cluster, service, row.capacity).await;
        assert_eq!(
            moved, row.capacity,
            "case {case:?}: expected the breach to be clamped into the window registered at \
             dispatch, so a desiredCount of {}, got {moved:?}",
            row.capacity
        );
        assert_ne!(
            moved, row.stale_capacity,
            "case {case:?}: expected the window the policy was put under not to be the one the \
             movement was clamped into"
        );

        // The movement is the one activity of the target, and it names the capacity it set.
        let listed = activities(&emulator, &resource_id, case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one scaling activity, got {listed:?}"
        );
        let expected_description = format!("Setting desired count to {}.", row.capacity);
        assert_eq!(
            listed[0]["Description"],
            json!(expected_description),
            "case {case:?}: expected Description {expected_description:?}, got {:?}",
            listed[0]
        );

        // And the window the movement was held by is the one the target reports.
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
            (
                reported["MinCapacity"].as_i64(),
                reported["MaxCapacity"].as_i64()
            ),
            (Some(row.breach_window.0), Some(row.breach_window.1)),
            "case {case:?}: expected the window registered last to be the one reported, \
             got {reported:?}"
        );
    }
}

#[tokio::test]
async fn each_breach_of_one_policy_is_clamped_into_the_window_of_its_own_instant() {
    // One policy, one alarm, and a ceiling that moves between breaches: each breach is held by
    // the window of ITS instant, so the capacity the same +10 lands on changes with the
    // window — which a policy that remembered a window could not do.
    let case = "one policy, a ceiling that moves between breaches";
    let emulator = Emulator::empty();
    let cluster = "walked";
    let service = "web";
    let resource_id = emulator.given_ecs_service(cluster, service, 2).await;
    register(&emulator, &resource_id, (2, 20), case).await;
    let policy_arn =
        put_step_policy(&emulator, &resource_id, "ChangeInCapacity", 10, true, case).await;
    let alarm_name = "walked-alarm";
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

    // The ceiling in force at each breach, and where the +10 lands under it: 2 + 10 held at 4,
    // then 4 + 10 held at 9, then 9 + 10 held at 12.
    for (step, (ceiling, expected)) in [(4_i64, 4_i64), (9, 9), (12, 12)].into_iter().enumerate() {
        register(&emulator, &resource_id, (2, ceiling), case).await;
        // A NEW transition into ALARM is a new breach; the policy's `Cooldown` is 0, so no wait
        // defers it. The pause is what makes the transition a new one to read: a transition is
        // identified by the instant the alarm moved, which CloudWatch reports to the
        // millisecond.
        tokio::time::sleep(CLOCK_POLL).await;
        emulator.set_alarm_state(alarm_name, "OK").await;
        emulator.set_alarm_state(alarm_name, "ALARM").await;
        let moved = await_desired_count(&emulator, cluster, service, expected).await;
        assert_eq!(
            moved, expected,
            "case {case:?}: breach {step} under a ceiling of {ceiling} expected a desiredCount \
             of {expected}, got {moved:?}"
        );
    }

    let listed = activities(&emulator, &resource_id, case).await;
    assert_eq!(
        listed.len(),
        3,
        "case {case:?}: expected one scaling activity per breach, got {listed:?}"
    );
}

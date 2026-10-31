// SPDX-License-Identifier: CC0-1.0

//! The `StatusMessage` of a scaling activity, end to end over the wire.
//!
//! The rule under test: every activity this service records has already started, so each one
//! reports a `StatusMessage` ("a simple message about the current status of the scaling
//! activity") that names the capacity the movement SET on the target — the value after the
//! registered window held it — and how far the fulfilment of that capacity got: awaited while
//! the change is on its way to the service that owns the scaled resource, and settled from the
//! activity's fulfilment instant on. `Description` states the intent; the message states the
//! outcome, so a client that reads it back learns which capacity the movement settled on.
//!
//! The docs the rows are read off:
//!
//! * `ScalingActivity$StatusMessage`: "A simple message about the current status of the scaling
//!   activity"
//!   (`smithy_query(query="doc",shape="com.amazonaws.applicationautoscaling#ScalingActivity",member="StatusMessage")`).
//! * `ScalingActivity$Description`: "A simple description of what action the scaling activity
//!   intends to accomplish", whose wording the model's own `DescribeScalingActivities` example
//!   carries ("Setting desired count to 1.").
//! * `RegisterScalableTarget$ScalableDimension`: "This string consists of the service
//!   namespace, resource type, and scaling property", which is where each row's noun comes
//!   from — the scaling property read as words.
//! * `RegisterScalableTarget$MinCapacity` / `$MaxCapacity`: the window a movement is held by,
//!   which is what makes the capacity a message names the CLAMPED one rather than the one the
//!   caller's adjustment asked for.
//!
//! Both suites below drive the production router through the shared harness in
//! `tests/all/emulator.rs`, over a TABLE of namespaces, dimensions, directions and
//! magnitudes: one reaches the movements through registrations, the other through a step
//! scaling policy whose alarm breaches, so the rule is pinned on both paths that record an
//! activity.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::state::ACTIVITY_FULFILMENT;

use crate::emulator::{resource_id_for, Emulator, PolicyAlarm};

/// How long a case waits before expecting a movement to have reached its terminal status: the
/// span the service gives a capacity movement, plus room for the request that reads it.
/// Nothing in the emulator sleeps for it.
fn fulfilment_wait() -> Duration {
    ACTIVITY_FULFILMENT + Duration::from_millis(400)
}

/// How long a poll that waits for the scaling clock keeps looking, and how long it sleeps
/// between looks.
const CLOCK_WAIT: Duration = Duration::from_secs(6);
/// See [`CLOCK_WAIT`].
const CLOCK_POLL: Duration = Duration::from_millis(100);

/// The status a movement carries until its change is confirmed, and the one it carries after.
const IN_PROGRESS: &str = "InProgress";
/// See [`IN_PROGRESS`].
const SUCCESSFUL: &str = "Successful";

/// The message an activity whose change is still on its way reports: the capacity it set and
/// the service the change is waiting on.
fn awaiting_message(noun: &str, capacity: i64, namespace: &str) -> String {
    format!("Successfully set {noun} to {capacity}. Waiting for change to be fulfilled by {namespace}.")
}

/// The message the same activity reports once its change has taken effect.
fn fulfilled_message(noun: &str, capacity: i64, namespace: &str) -> String {
    format!("Successfully set {noun} to {capacity}. Change successfully fulfilled by {namespace}.")
}

/// The message an activity in `status` must report for a movement to `capacity`, or `None`
/// when the status is one no movement of this service reports.
fn expected_message(
    status: &str,
    noun: &str,
    capacity: i64,
    namespace: &str,
) -> Option<String> {
    match status {
        IN_PROGRESS => Some(awaiting_message(noun, capacity, namespace)),
        SUCCESSFUL => Some(fulfilled_message(noun, capacity, namespace)),
        _ => None,
    }
}

/// Assert that `activity` reports the message its status owes for a movement to `capacity`,
/// and that a client reading the message learns that capacity.
fn assert_reports_capacity(
    activity: &Value,
    noun: &str,
    capacity: i64,
    namespace: &str,
    case: &str,
) -> String {
    let status = activity["StatusCode"].as_str().unwrap_or_default().to_string();
    let expected = expected_message(&status, noun, capacity, namespace).unwrap_or_else(|| {
        panic!("case {case:?}: expected a started movement to be {IN_PROGRESS} or {SUCCESSFUL}, got {status:?} in {activity:?}")
    });
    assert_eq!(
        activity["StatusMessage"],
        json!(expected),
        "case {case:?}: expected StatusMessage {expected:?} for a {status} movement, \
         got {activity:?}"
    );
    let reported = activity["StatusMessage"].as_str().unwrap_or_default();
    assert!(
        reported.contains(&capacity.to_string()),
        "case {case:?}: expected the status message to name the capacity {capacity}, \
         got {reported:?}"
    );
    status
}

/// One registration-driven movement: the window the target is first registered with, the
/// window registered next, and where the capacity lands once that window is enforced.
///
/// The namespace/dimension pairs are entries of the model's own `ScalableDimension` value set,
/// and each `noun` is that dimension's scaling property read as words. The rows differ in
/// namespace, in the direction the capacity moves, in the magnitude it moves by, in whether
/// the resource lives in a service this emulator holds a store for, and in whether the
/// property is one word or three.
struct MovementRow {
    dimension: &'static str,
    /// The `MinCapacity`/`MaxCapacity` the target is first registered with; the resource is
    /// created at the floor, so this registration moves nothing.
    registered: (i64, i64),
    /// The window registered next, which moves the capacity.
    moved_to: (i64, i64),
    /// Where the capacity lands under that window.
    capacity: i64,
    noun: &'static str,
}

const MOVEMENT_CATALOG: &[MovementRow] = &[
    // A raised floor scales a DynamoDB table's read capacity out by four units.
    MovementRow {
        dimension: "dynamodb:table:ReadCapacityUnits",
        registered: (1, 20),
        moved_to: (5, 20),
        capacity: 5,
        noun: "read capacity units",
    },
    // A lowered ceiling scales an ECS service in, the dimension the message's noun is three
    // words for.
    MovementRow {
        dimension: "ecs:service:DesiredCount",
        registered: (2, 12),
        moved_to: (1, 1),
        capacity: 1,
        noun: "desired count",
    },
    // A namespace whose owning service this emulator holds no store for: the activity is
    // recorded and reported all the same.
    MovementRow {
        dimension: "lambda:function:ProvisionedConcurrency",
        registered: (2, 8),
        moved_to: (6, 8),
        capacity: 6,
        noun: "provisioned concurrency",
    },
    // A large capacity moved by 150 units.
    MovementRow {
        dimension: "kafka:broker-storage:VolumeSize",
        registered: (100, 1000),
        moved_to: (250, 1000),
        capacity: 250,
        noun: "volume size",
    },
    // A one-word property, scaled in to a single unit.
    MovementRow {
        dimension: "elasticache:replication-group:Replicas",
        registered: (3, 6),
        moved_to: (1, 1),
        capacity: 1,
        noun: "replicas",
    },
];

/// The namespace a dimension belongs to: its first segment.
fn namespace_of(dimension: &str) -> &str {
    dimension.split(':').next().unwrap_or_default()
}

/// The three identity members of a row's target.
fn identity(dimension: &str) -> Value {
    json!({
        "ServiceNamespace": namespace_of(dimension),
        "ResourceId": resource_id_for(dimension),
        "ScalableDimension": dimension,
    })
}

/// Register `dimension`'s target with the window `min`..`max`, creating the resource it names
/// where this emulator owns the service that resource lives in.
async fn register(emulator: &Emulator, dimension: &str, (min, max): (i64, i64), case: &str) {
    emulator
        .given_resource(namespace_of(dimension), &resource_id_for(dimension), min)
        .await;
    let mut request = identity(dimension);
    request["MinCapacity"] = json!(min);
    request["MaxCapacity"] = json!(max);
    let (status, body) = emulator.call("RegisterScalableTarget", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget {min}..{max} to answer 200, \
         got {status} {body:?}"
    );
}

/// The activity log of one target, newest first, as `DescribeScalingActivities` reports it.
async fn activities(emulator: &Emulator, target: Value, case: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call("DescribeScalingActivities", target)
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

#[tokio::test]
async fn an_activity_reports_the_capacity_it_set_while_the_change_is_awaited_and_once_it_settles() {
    // How many rows were really read back before their fulfilment instant. Two requests one
    // after another are far inside the fulfilment span, so this is every row on an unloaded
    // machine; it is counted rather than assumed, because a row that turned out fulfilled
    // would have proved nothing about the awaited half of the rule.
    let mut awaited = 0_usize;
    for row in MOVEMENT_CATALOG {
        let case = row.dimension;
        let namespace = namespace_of(row.dimension);
        let emulator = Emulator::empty();
        register(&emulator, row.dimension, row.registered, case).await;

        // The first registration adopts the window it declares, so the only movement of the
        // row is the one the second registration makes.
        let none_yet = activities(&emulator, identity(row.dimension), case).await;
        assert!(
            none_yet.is_empty(),
            "case {case:?}: expected a first registration to report no activity, got {none_yet:?}"
        );

        register(&emulator, row.dimension, row.moved_to, case).await;

        let listed = activities(&emulator, identity(row.dimension), case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one scaling activity, got {listed:?}"
        );
        let started = &listed[0];
        let status = assert_reports_capacity(started, row.noun, row.capacity, namespace, case);
        if status == IN_PROGRESS {
            awaited += 1;
            assert_eq!(
                started["EndTime"],
                Value::Null,
                "case {case:?}: expected a movement still awaiting fulfilment to carry no \
                 EndTime, got {started:?}"
            );
        }
        // The message is the outcome, not the intent: the description of the same activity
        // states what it set out to do.
        let expected_description = format!("Setting {} to {}.", row.noun, row.capacity);
        assert_eq!(
            started["Description"],
            json!(expected_description),
            "case {case:?}: expected Description {expected_description:?}, got {started:?}"
        );

        // A read does not change what the next reader sees.
        let again = activities(&emulator, identity(row.dimension), case).await;
        assert_eq!(
            again, listed,
            "case {case:?}: expected a second read to answer identically, got {again:?}"
        );

        // The clock alone settles it, and the settled message names the same capacity.
        tokio::time::sleep(fulfilment_wait()).await;
        let settled = activities(&emulator, identity(row.dimension), case).await;
        assert_eq!(
            settled.len(),
            1,
            "case {case:?}: expected the log to still hold one activity, got {settled:?}"
        );
        assert_eq!(
            settled[0]["StatusCode"],
            json!(SUCCESSFUL),
            "case {case:?}: expected the movement to be {SUCCESSFUL} once its fulfilment \
             instant has passed, got {:?}",
            settled[0]
        );
        assert_reports_capacity(&settled[0], row.noun, row.capacity, namespace, case);
        assert_eq!(
            settled[0]["ActivityId"], started["ActivityId"],
            "case {case:?}: expected the same activity, not a second one, got {:?}",
            settled[0]
        );
    }
    assert!(
        awaited > 0,
        "expected at least one of the {} rows to be read back before its fulfilment instant, \
         got {awaited}",
        MOVEMENT_CATALOG.len()
    );
}

/// The metric namespace this suite's alarms watch. A customer namespace, so the alarm is one a
/// client could create itself.
const METRIC_NAMESPACE: &str = "Emulator/ActivityStatusMessage";

/// The metric those alarms watch inside it.
const METRIC_NAME: &str = "Load";

/// The threshold every alarm in the suite breaches.
const THRESHOLD: f64 = 50.0;

/// One step-scaling breach: where the ECS service starts, the window registered when the
/// breach is dispatched, the adjustment the single step asks for, and the capacity the
/// movement lands on once that window holds it.
struct BreachRow {
    case: &'static str,
    starting: i64,
    window: (i64, i64),
    adjustment_type: &'static str,
    adjustment: i64,
    comparison_operator: &'static str,
    /// Where the capacity lands: the adjustment clamped into the window.
    capacity: i64,
}

/// The rows differ in the direction of the adjustment, in which bound holds it (or in neither
/// holding it), and in the adjustment type.
const BREACH_CATALOG: &[BreachRow] = &[
    // A scale out that overshoots the ceiling: the movement happens AT the ceiling, and the
    // message names that bound rather than the capacity the adjustment asked for.
    BreachRow {
        case: "a scale out clamped at the ceiling",
        starting: 4,
        window: (4, 6),
        adjustment_type: "ChangeInCapacity",
        adjustment: 10,
        comparison_operator: "GreaterThanThreshold",
        capacity: 6,
    },
    // The symmetric leg: a scale in that undershoots the floor is performed at the floor.
    BreachRow {
        case: "a scale in clamped at the floor",
        starting: 6,
        window: (4, 6),
        adjustment_type: "ChangeInCapacity",
        adjustment: -10,
        comparison_operator: "LessThanThreshold",
        capacity: 4,
    },
    // An adjustment the window admits in full: the message names the capacity the adjustment
    // itself asked for.
    BreachRow {
        case: "a scale out the window admits in full",
        starting: 2,
        window: (1, 20),
        adjustment_type: "ChangeInCapacity",
        adjustment: 3,
        comparison_operator: "GreaterThanThreshold",
        capacity: 5,
    },
    // An exact capacity above the ceiling, held at the ceiling.
    BreachRow {
        case: "an exact capacity clamped at the ceiling",
        starting: 2,
        window: (2, 5),
        adjustment_type: "ExactCapacity",
        adjustment: 9,
        comparison_operator: "GreaterThanThreshold",
        capacity: 5,
    },
];

/// The dimension every breach row scales, and the noun its messages carry.
const ECS_DIMENSION: &str = "ecs:service:DesiredCount";
/// See [`ECS_DIMENSION`].
const ECS_NOUN: &str = "desired count";

/// Register the ECS target `resource_id` names with the window `min`..`max`.
async fn register_ecs(emulator: &Emulator, resource_id: &str, (min, max): (i64, i64), case: &str) {
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": "ecs",
                "ResourceId": resource_id,
                "ScalableDimension": ECS_DIMENSION,
                "MinCapacity": min,
                "MaxCapacity": max,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget {min}..{max} to answer 200, \
         got {status} {body:?}"
    );
}

/// Put a one-step `StepScaling` policy on `resource_id` and answer the `PolicyARN` an alarm
/// executes.
async fn put_step_policy(
    emulator: &Emulator,
    resource_id: &str,
    row: &BreachRow,
    case: &str,
) -> String {
    let scale_out = row.comparison_operator.starts_with("GreaterThan");
    let step = if scale_out {
        json!({"MetricIntervalLowerBound": 0, "ScalingAdjustment": row.adjustment})
    } else {
        json!({"MetricIntervalUpperBound": 0, "ScalingAdjustment": row.adjustment})
    };
    let (status, body) = emulator
        .call(
            "PutScalingPolicy",
            json!({
                "ServiceNamespace": "ecs",
                "ResourceId": resource_id,
                "ScalableDimension": ECS_DIMENSION,
                "PolicyName": "step-policy",
                "PolicyType": "StepScaling",
                "StepScalingPolicyConfiguration": {
                    "AdjustmentType": row.adjustment_type,
                    // No row breaches twice, so the wait a previous activity is given is named
                    // rather than left to its default.
                    "Cooldown": 0,
                    "StepAdjustments": [step],
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
    );
    body["PolicyARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a PolicyARN, got {body:?}"))
        .to_string()
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
async fn a_clamped_step_scaling_breach_reports_the_capacity_it_landed_on() {
    for row in BREACH_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        let cluster = "scaled";
        let service = "web";
        let resource_id = emulator
            .given_ecs_service(cluster, service, row.starting)
            .await;
        register_ecs(&emulator, &resource_id, row.window, case).await;
        let policy_arn = put_step_policy(&emulator, &resource_id, row, case).await;

        let alarm_name = "breach-alarm";
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
            "case {case:?}: expected the breach to move the ECS desiredCount to {}, \
             got {moved:?}",
            row.capacity
        );

        // The activity the breach recorded reports the capacity it landed on — the clamped
        // one — whichever side of its fulfilment instant it is read on.
        let target = json!({
            "ServiceNamespace": "ecs",
            "ResourceId": resource_id,
            "ScalableDimension": ECS_DIMENSION,
            "IncludeNotScaledActivities": true,
        });
        let listed = activities(&emulator, target.clone(), case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one scaling activity, got {listed:?}"
        );
        assert_reports_capacity(&listed[0], ECS_NOUN, row.capacity, "ecs", case);
        assert_eq!(
            listed[0]["NotScaledReasons"],
            Value::Null,
            "case {case:?}: expected a performed movement to carry no NotScaledReasons, \
             got {:?}",
            listed[0]
        );

        tokio::time::sleep(fulfilment_wait()).await;
        let settled = activities(&emulator, target, case).await;
        assert_eq!(
            settled[0]["StatusCode"],
            json!(SUCCESSFUL),
            "case {case:?}: expected the movement to be {SUCCESSFUL} once its fulfilment \
             instant has passed, got {:?}",
            settled[0]
        );
        assert_reports_capacity(&settled[0], ECS_NOUN, row.capacity, "ecs", case);
    }
}

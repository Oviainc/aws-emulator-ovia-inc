// SPDX-License-Identifier: CC0-1.0

//! EXECUTING a target-tracking scaling policy: what a breach of the alarms it minted does to
//! the resource the scalable target scales, end to end over the wire.
//!
//! The rules under test:
//!
//! * a target-tracking policy carries no adjustment, so when one of the two alarms it minted
//!   on the metric it tracks is in `ALARM` the policy moves the capacity to the LEAST capacity
//!   at which that metric would sit at the target value — "Application Auto Scaling […] scales
//!   the capacity to keep the metric at, or close to, the specified target value". For a
//!   metric that counts the capacity units CONSUMED over the alarm's period that capacity is
//!   `measured / (TargetValue% * period)`, and for a metric that already is the quantity the
//!   target names, which varies inversely with the capacity serving it, it is
//!   `current * measured / TargetValue`; the result is clamped into the `MinCapacity` /
//!   `MaxCapacity` the target carries at that instant, and recorded as one scaling activity;
//! * `SuspendedState.DynamicScalingOutSuspended` — "Whether scale out by a target tracking
//!   scaling policy or a step scaling policy is suspended" — WITHHOLDS that movement: the
//!   suspended target keeps the same alarms as an unsuspended control, records no scaling
//!   activity at all (including in the `IncludeNotScaledActivities` view) while its own
//!   scale-out alarm sits in `ALARM`, and stays at the capacity it started at. Lifting the
//!   suspension delivers the withheld scale-out with no new metric movement, as a `Successful`
//!   activity whose `Cause` names the alarm and the policy and whose `Description` names the
//!   capacity that was set.
//!
//! Every case goes through the front door a client uses — the table, the target, the policy,
//! the alarm state and the read-back of the provisioned capacity are all requests. The only
//! thing not published over the wire is the load itself: `PutMetricData` refuses an `AWS/`
//! namespace, so the consumed-capacity and CPU series the alarms watch are seeded into the
//! metric store the way the service that owns each of them writes it
//! ([`crate::emulator::Emulator::seed_metric_data`]).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::emulator::{alarm_kind, alarm_kinds, Emulator};

/// How long a poll that waits for a clock keeps looking, and how long it sleeps between
/// looks. Generous: both the alarm clock and the scaling clock wake up several times inside
/// this window, and nothing in the emulator sleeps for either.
const CLOCK_WAIT: Duration = Duration::from_secs(10);
/// See [`CLOCK_WAIT`].
const CLOCK_POLL: Duration = Duration::from_millis(100);

/// How many closed period windows a row seeds its series into.
///
/// The scale-in alarm of a tracked metric is evaluated over fifteen periods, so a breach of it
/// has to be fifteen datapoints wide; the scale-out alarms need two or three. Seeding this
/// many covers every one of them.
const SEEDED_WINDOWS: i64 = 18;

/// Which alarm of the pair on the tracked metric a row breaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// `AlarmHigh`: the metric is above the target.
    Out,
    /// `AlarmLow`: the metric is below the buffered target.
    In,
}

impl Direction {
    /// The kind segment the alarm of this direction carries in its name.
    fn alarm_kind(self) -> &'static str {
        match self {
            Self::Out => "AlarmHigh",
            Self::In => "AlarmLow",
        }
    }
}

/// The resource a row scales, and the series its policy tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tracked {
    /// A PROVISIONED DynamoDB table's read capacity, tracked by
    /// `DynamoDBReadCapacityUtilization`: a Sum of the units consumed over each 60-second
    /// period.
    DynamodbRead,
    /// An ECS service's task count, tracked by `ECSServiceAverageCPUUtilization`: an Average
    /// percentage, which is already the quantity the target value is expressed in.
    EcsCpu,
}

/// One execution to pin: the capacity the resource starts at, the window it is registered
/// with, the policy's target value, the value the seeded series takes in each graded window,
/// and the capacity the movement must land on.
struct ExecutionRow {
    case: &'static str,
    tracked: Tracked,
    starting: i64,
    min: i64,
    max: i64,
    target_value: f64,
    /// The value of the tracked series in each seeded window: a Sum of consumed units for a
    /// consumed-units metric, an Average percentage for the ECS row.
    measured: f64,
    direction: Direction,
    capacity: i64,
}

/// The rows differ along what the rule generalises over: which service owns the resource,
/// whether the tracked metric counts consumed units or is already a percentage, the direction
/// the alarm breaches in, the target value, and whether the capacity the control loop asks for
/// falls inside the registered window or past either bound.
const EXECUTION_CATALOG: &[ExecutionRow] = &[
    // 300 consumed units a minute at a 50% target is 10 units of provisioned capacity:
    // 300 / (0.5 * 60).
    ExecutionRow {
        case: "a DynamoDB read breach inside the window",
        tracked: Tracked::DynamodbRead,
        starting: 2,
        min: 1,
        max: 50,
        target_value: 50.0,
        measured: 300.0,
        direction: Direction::Out,
        capacity: 10,
    },
    // The same arithmetic at a different target: 300 / (0.25 * 60) is 20 units.
    ExecutionRow {
        case: "a DynamoDB read breach at a 25% target",
        tracked: Tracked::DynamodbRead,
        starting: 2,
        min: 1,
        max: 50,
        target_value: 25.0,
        measured: 300.0,
        direction: Direction::Out,
        capacity: 20,
    },
    // A fraction of a unit short of the target is still short of it: 310 / 30 is 10.33 units,
    // taken up.
    ExecutionRow {
        case: "a DynamoDB read breach that lands between two units",
        tracked: Tracked::DynamodbRead,
        starting: 2,
        min: 1,
        max: 50,
        target_value: 50.0,
        measured: 310.0,
        direction: Direction::Out,
        capacity: 11,
    },
    // 3000 / 30 is 100 units, which is past the ceiling: the movement happens AT the ceiling.
    ExecutionRow {
        case: "a DynamoDB read breach past the ceiling",
        tracked: Tracked::DynamodbRead,
        starting: 2,
        min: 1,
        max: 12,
        target_value: 50.0,
        measured: 3000.0,
        direction: Direction::Out,
        capacity: 12,
    },
    // Scaling IN: 60 consumed units a minute is below the scale-in alarm's threshold
    // ((50 - 20)% of 10 units over 60 s = 180), and the capacity that puts the load at the
    // target is 60 / 30 = 2 units.
    ExecutionRow {
        case: "a DynamoDB read scale-in inside the window",
        tracked: Tracked::DynamodbRead,
        starting: 10,
        min: 1,
        max: 50,
        target_value: 50.0,
        measured: 60.0,
        direction: Direction::In,
        capacity: 2,
    },
    // The same scale-in against a floor it may not go under: the movement happens AT the floor.
    ExecutionRow {
        case: "a DynamoDB read scale-in past the floor",
        tracked: Tracked::DynamodbRead,
        starting: 10,
        min: 6,
        max: 50,
        target_value: 50.0,
        measured: 60.0,
        direction: Direction::In,
        capacity: 6,
    },
    // A metric that already IS the quantity the target names: 4 tasks averaging 75% CPU
    // against a 50% target need 4 * 75 / 50 = 6 tasks.
    ExecutionRow {
        case: "an ECS CPU breach on a percentage metric",
        tracked: Tracked::EcsCpu,
        starting: 4,
        min: 1,
        max: 20,
        target_value: 50.0,
        measured: 75.0,
        direction: Direction::Out,
        capacity: 6,
    },
    // The mirror of it: 6 tasks averaging 20% CPU are below the scale-in alarm's threshold
    // (a tenth under the target, 45%), and 6 * 20 / 50 = 2.4 tasks, taken up to 3.
    ExecutionRow {
        case: "an ECS CPU scale-in on a percentage metric",
        tracked: Tracked::EcsCpu,
        starting: 6,
        min: 1,
        max: 20,
        target_value: 50.0,
        measured: 20.0,
        direction: Direction::In,
        capacity: 3,
    },
];

/// The middle of the `index`-th most recent period window that has already CLOSED.
///
/// A metric is aggregated into periods anchored at multiples of the period and an alarm grades
/// the periods that are complete, so a datapoint meant to decide an alarm now is stamped in a
/// closed window rather than in the one the clock is still in.
fn graded_window_stamp(period: f64, index: i64) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    ((now / period).floor() - index as f64) * period + period / 2.0
}

/// The three identity members of the scalable target a row registers.
fn target_of(row: &ExecutionRow, resource_id: &str) -> Value {
    json!({
        "ServiceNamespace": match row.tracked {
            Tracked::DynamodbRead => "dynamodb",
            Tracked::EcsCpu => "ecs",
        },
        "ResourceId": resource_id,
        "ScalableDimension": match row.tracked {
            Tracked::DynamodbRead => "dynamodb:table:ReadCapacityUnits",
            Tracked::EcsCpu => "ecs:service:DesiredCount",
        },
    })
}

/// The predefined metric type a row's policy tracks.
fn metric_type(tracked: Tracked) -> &'static str {
    match tracked {
        Tracked::DynamodbRead => "DynamoDBReadCapacityUtilization",
        Tracked::EcsCpu => "ECSServiceAverageCPUUtilization",
    }
}

/// Seed the series a row's policy tracks, in every window its alarms grade.
fn seed_load(emulator: &Emulator, row: &ExecutionRow, resource: &str) {
    let (namespace, metric, dimensions) = match row.tracked {
        Tracked::DynamodbRead => (
            "AWS/DynamoDB",
            "ConsumedReadCapacityUnits",
            vec![("TableName", resource)],
        ),
        Tracked::EcsCpu => (
            "AWS/ECS",
            "CPUUtilization",
            vec![("ClusterName", "tracked"), ("ServiceName", resource)],
        ),
    };
    for index in 1..=SEEDED_WINDOWS {
        emulator.seed_metric_data(
            namespace,
            metric,
            &dimensions,
            graded_window_stamp(60.0, index),
            &[row.measured],
        );
    }
}

/// Put the target-tracking policy a row describes, with both cooldowns at zero so no row waits
/// for a previous activity, and answer the `PolicyARN`.
async fn put_policy(emulator: &Emulator, row: &ExecutionRow, target: &Value, name: &str) -> String {
    let mut request = target.clone();
    request["PolicyName"] = json!(name);
    request["PolicyType"] = json!("TargetTrackingScaling");
    request["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": row.target_value,
        "ScaleOutCooldown": 0,
        "ScaleInCooldown": 0,
        "PredefinedMetricSpecification": {"PredefinedMetricType": metric_type(row.tracked)},
    });
    let (status, body) = emulator.call("PutScalingPolicy", request).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}",
        row.case
    );
    body["PolicyARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: expected a PolicyARN, got {body:?}", row.case))
        .to_string()
}

/// Every alarm CloudWatch holds under the `TargetTracking` prefix, each as
/// `(kind, StateValue)`.
async fn alarm_states(emulator: &Emulator) -> Vec<(String, String)> {
    emulator
        .target_tracking_alarms()
        .await
        .iter()
        .map(|alarm| {
            (
                alarm_kind(alarm),
                alarm["StateValue"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Wait for the alarm of `kind` to be evaluated into `ALARM`, answering its `AlarmName`.
///
/// The poll is the TEST waiting for CloudWatch's own alarm clock to grade the seeded series;
/// a `DescribeAlarms` is also what leaves that clock running.
async fn await_alarm(emulator: &Emulator, kind: &str, case: &str) -> String {
    let deadline = Instant::now() + CLOCK_WAIT;
    loop {
        let alarms = emulator.target_tracking_alarms().await;
        let breaching = alarms.iter().find(|alarm| {
            alarm_kind(alarm) == kind && alarm["StateValue"].as_str() == Some("ALARM")
        });
        if let Some(alarm) = breaching {
            return alarm["AlarmName"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the {kind} alarm to be evaluated into ALARM, got {:?}",
            alarm_states(emulator).await
        );
        tokio::time::sleep(CLOCK_POLL).await;
    }
}

/// The capacity the service that owns a row's resource holds for it right now.
async fn capacity_of(emulator: &Emulator, row: &ExecutionRow, resource: &str) -> i64 {
    match row.tracked {
        Tracked::DynamodbRead => {
            let (status, body) = emulator
                .call_dynamodb("DescribeTable", json!({"TableName": resource}))
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected DescribeTable to answer 200, got {status} {body:?}",
                row.case
            );
            body["Table"]["ProvisionedThroughput"]["ReadCapacityUnits"]
                .as_i64()
                .unwrap_or_default()
        }
        Tracked::EcsCpu => emulator.ecs_desired_count("tracked", resource).await,
    }
}

/// Wait for that capacity to reach `expected`, answering what it last read.
async fn await_capacity(
    emulator: &Emulator,
    row: &ExecutionRow,
    resource: &str,
    expected: i64,
) -> i64 {
    let deadline = Instant::now() + CLOCK_WAIT;
    let mut last = capacity_of(emulator, row, resource).await;
    while last != expected && Instant::now() < deadline {
        tokio::time::sleep(CLOCK_POLL).await;
        last = capacity_of(emulator, row, resource).await;
    }
    last
}

/// The scaling activities recorded for one target, newest first.
async fn activities(emulator: &Emulator, target: &Value, case: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call("DescribeScalingActivities", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingActivities to answer 200, got {status} {body:?}"
    );
    body["ScalingActivities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The noun a scaling activity's text names a row's capacity by.
fn capacity_noun(tracked: Tracked) -> &'static str {
    match tracked {
        Tracked::DynamodbRead => "read capacity units",
        Tracked::EcsCpu => "desired count",
    }
}

#[tokio::test]
async fn a_breach_moves_the_capacity_the_target_value_asks_for() {
    for row in EXECUTION_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        let resource = "tracked";
        let resource_id = match row.tracked {
            Tracked::DynamodbRead => {
                emulator
                    .create_provisioned_table(resource, row.starting, row.starting)
                    .await;
                format!("table/{resource}")
            }
            Tracked::EcsCpu => {
                emulator
                    .given_ecs_service("tracked", resource, row.starting)
                    .await
            }
        };
        let target = target_of(row, &resource_id);

        let mut registration = target.clone();
        registration["MinCapacity"] = json!(row.min);
        registration["MaxCapacity"] = json!(row.max);
        let (status, body) = emulator.call("RegisterScalableTarget", registration).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
        );
        put_policy(&emulator, row, &target, "tracker").await;

        // The policy on its own moves nothing: a target-tracking policy is driven by its
        // alarms.
        let before = capacity_of(&emulator, row, resource).await;
        assert_eq!(
            before, row.starting,
            "case {case:?}: expected putting the policy to move no capacity, got {before}"
        );

        seed_load(&emulator, row, resource);
        let alarm_name = await_alarm(&emulator, row.direction.alarm_kind(), case).await;

        let moved = await_capacity(&emulator, row, resource, row.capacity).await;
        assert_eq!(
            moved, row.capacity,
            "case {case:?}: expected the capacity to reach {}, got {moved}",
            row.capacity
        );

        // The movement is reported as one scaling activity, naming the capacity it set and
        // the alarm and policy that caused it.
        let listed = activities(&emulator, &target, case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one scaling activity, got {listed:?}"
        );
        let activity = &listed[0];
        let expected_description = format!(
            "Setting {} to {}.",
            capacity_noun(row.tracked),
            row.capacity
        );
        assert_eq!(
            activity["Description"],
            json!(expected_description),
            "case {case:?}: expected Description {expected_description:?}, got {activity:?}"
        );
        let expected_cause =
            format!("monitor alarm {alarm_name} in state ALARM triggered policy tracker");
        assert_eq!(
            activity["Cause"],
            json!(expected_cause),
            "case {case:?}: expected Cause {expected_cause:?}, got {activity:?}"
        );
    }
}

#[tokio::test]
async fn a_suspended_scale_out_is_withheld_and_delivered_when_it_is_resumed() {
    // The first catalogue row, driven on two identical tables at once: one SUSPENDED subject
    // and one unsuspended control. The control completing its scale-out under the same load is
    // the barrier the subject is judged at — exactly what a client watching both would wait
    // for.
    let row = &EXECUTION_CATALOG[0];
    let case = "scale out suspended";
    let emulator = Emulator::empty();
    let subject = "subject";
    let control = "control";
    for table in [subject, control] {
        emulator
            .create_provisioned_table(table, row.starting, row.starting)
            .await;
    }
    let subject_target = target_of(row, &format!("table/{subject}"));
    let control_target = target_of(row, &format!("table/{control}"));

    let mut suspended = subject_target.clone();
    suspended["MinCapacity"] = json!(row.min);
    suspended["MaxCapacity"] = json!(row.max);
    suspended["SuspendedState"] = json!({
        "DynamicScalingInSuspended": false,
        "DynamicScalingOutSuspended": true,
        "ScheduledScalingSuspended": false,
    });
    let (status, body) = emulator.call("RegisterScalableTarget", suspended).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the suspended registration to answer 200, got {status} {body:?}"
    );
    let mut running = control_target.clone();
    running["MinCapacity"] = json!(row.min);
    running["MaxCapacity"] = json!(row.max);
    let (status, body) = emulator.call("RegisterScalableTarget", running).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the control registration to answer 200, got {status} {body:?}"
    );
    put_policy(&emulator, row, &subject_target, "subject-tracker").await;
    put_policy(&emulator, row, &control_target, "control-tracker").await;

    seed_load(&emulator, row, subject);
    seed_load(&emulator, row, control);

    // The barrier: the control's own scale-out arrives.
    await_alarm(&emulator, row.direction.alarm_kind(), case).await;
    let control_moved = await_capacity(&emulator, row, control, row.capacity).await;
    assert_eq!(
        control_moved, row.capacity,
        "case {case:?}: expected the unsuspended control to reach {}, got {control_moved}",
        row.capacity
    );
    let control_activities = activities(&emulator, &control_target, case).await;
    assert_eq!(
        control_activities.len(),
        1,
        "case {case:?}: expected the control to record one activity, got {control_activities:?}"
    );

    // The suspended subject: its own scale-out alarm is in ALARM, it keeps the same alarm
    // kinds the control has, and it has moved nothing and recorded nothing.
    let subject_alarms: Vec<Value> = emulator
        .target_tracking_alarms()
        .await
        .into_iter()
        .filter(|alarm| {
            alarm["AlarmName"]
                .as_str()
                .is_some_and(|name| name.contains(subject))
        })
        .collect();
    let control_alarms: Vec<Value> = emulator
        .target_tracking_alarms()
        .await
        .into_iter()
        .filter(|alarm| {
            alarm["AlarmName"]
                .as_str()
                .is_some_and(|name| name.contains(control))
        })
        .collect();
    assert_eq!(
        alarm_kinds(&subject_alarms),
        alarm_kinds(&control_alarms),
        "case {case:?}: expected the suspended subject to keep the alarm kinds the control \
         has, got {subject_alarms:?}"
    );
    let subject_high = subject_alarms
        .iter()
        .find(|alarm| alarm_kind(alarm) == row.direction.alarm_kind())
        .unwrap_or_else(|| panic!("case {case:?}: expected the subject's scale-out alarm"));
    assert_eq!(
        subject_high["StateValue"],
        json!("ALARM"),
        "case {case:?}: expected the subject's own scale-out alarm to be in ALARM, got \
         {subject_high:?}"
    );
    let withheld = capacity_of(&emulator, row, subject).await;
    assert_eq!(
        withheld, row.starting,
        "case {case:?}: expected the suspended subject to stay at {}, got {withheld}",
        row.starting
    );
    assert!(
        activities(&emulator, &subject_target, case).await.is_empty(),
        "case {case:?}: expected the suspended subject to record no scaling activity, got {:?}",
        activities(&emulator, &subject_target, case).await
    );
    let mut including_not_scaled = subject_target.clone();
    including_not_scaled["IncludeNotScaledActivities"] = json!(true);
    let listed = activities(&emulator, &including_not_scaled, case).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: expected no activity in the IncludeNotScaledActivities view either, \
         got {listed:?}"
    );

    // Lifting the suspension delivers the withheld scale-out, with no new metric movement.
    let mut resume = subject_target.clone();
    resume["SuspendedState"] = json!({
        "DynamicScalingInSuspended": false,
        "DynamicScalingOutSuspended": false,
        "ScheduledScalingSuspended": false,
    });
    let (status, body) = emulator.call("RegisterScalableTarget", resume).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the resuming registration to answer 200, got {status} {body:?}"
    );
    let delivered = await_capacity(&emulator, row, subject, row.capacity).await;
    assert_eq!(
        delivered, row.capacity,
        "case {case:?}: expected the resumed subject to be delivered {}, got {delivered} \
         (alarms {:?}, activities {:?})",
        row.capacity,
        alarm_states(&emulator).await,
        activities(&emulator, &subject_target, case).await,
    );
    let listed = activities(&emulator, &subject_target, case).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected the delivery to be one scaling activity, got {listed:?}"
    );
    let activity = &listed[0];
    let expected_description = format!(
        "Setting {} to {}.",
        capacity_noun(row.tracked),
        row.capacity
    );
    assert_eq!(
        activity["Description"],
        json!(expected_description),
        "case {case:?}: expected Description {expected_description:?}, got {activity:?}"
    );
    let expected_cause = format!(
        "monitor alarm {} in state ALARM triggered policy subject-tracker",
        subject_high["AlarmName"].as_str().unwrap_or_default()
    );
    assert_eq!(
        activity["Cause"],
        json!(expected_cause),
        "case {case:?}: expected Cause {expected_cause:?}, got {activity:?}"
    );
    let status_code = activity["StatusCode"].as_str().unwrap_or_default();
    assert!(
        status_code == "Successful" || status_code == "InProgress",
        "case {case:?}: expected the delivered activity to be a performed movement, got \
         {activity:?}"
    );
}

#[tokio::test]
async fn the_alarms_that_watch_the_provisioned_capacity_move_nothing() {
    // The pair a consumed-units policy mints on the resource's own provisioned capacity
    // reports that the utilisation thresholds were computed from a capacity the resource no
    // longer has; it states no target for the tracked metric, so a breach of it is no
    // control-loop input and moves no capacity.
    let case = "a provisioned-capacity alarm in ALARM";
    let row = &EXECUTION_CATALOG[0];
    let emulator = Emulator::empty();
    let resource = "tracked";
    emulator
        .create_provisioned_table(resource, row.starting, row.starting)
        .await;
    let target = target_of(row, &format!("table/{resource}"));
    let mut registration = target.clone();
    registration["MinCapacity"] = json!(row.min);
    registration["MaxCapacity"] = json!(row.max);
    let (status, body) = emulator.call("RegisterScalableTarget", registration).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
    );
    put_policy(&emulator, row, &target, "tracker").await;

    let alarms = emulator.target_tracking_alarms().await;
    let capacity_alarm = alarms
        .iter()
        .find(|alarm| alarm_kind(alarm) == "ProvisionedCapacityHigh")
        .unwrap_or_else(|| panic!("case {case:?}: expected a ProvisionedCapacityHigh alarm"));
    let alarm_name = capacity_alarm["AlarmName"].as_str().unwrap_or_default();
    emulator.set_alarm_state(alarm_name, "ALARM").await;

    // Long enough for several scaling-clock wake-ups: nothing is owed, so nothing moves.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let held = capacity_of(&emulator, row, resource).await;
    assert_eq!(
        held, row.starting,
        "case {case:?}: expected the capacity to stay at {}, got {held}",
        row.starting
    );
    let listed = activities(&emulator, &target, case).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: expected no scaling activity, got {listed:?}"
    );
}

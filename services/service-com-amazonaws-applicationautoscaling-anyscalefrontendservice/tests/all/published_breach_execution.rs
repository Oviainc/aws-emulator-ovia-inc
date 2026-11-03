// SPDX-License-Identifier: CC0-1.0

//! A breaching CUSTOMER alarm over a series a client is publishing into, end to end: from the
//! `PutMetricData` call to the ECS service's `desiredCount`.
//!
//! ONE rule, asserted over a table: **a CloudWatch alarm that names a step scaling policy's
//! ARN among its `AlarmActions` executes that policy as soon as the datapoints the client has
//! published put the alarm in `ALARM`** — "The actions to execute when this alarm transitions
//! to the ALARM state from any other state. Each action is specified as an Amazon Resource
//! Name (ARN)" (`PutMetricAlarmInput$AlarmActions`) — and the execution moves the capacity of
//! the resource the scalable target scales, clamped into the `MinCapacity`/`MaxCapacity` the
//! target carries, recording the movement as one scaling activity. A series the alarm's own
//! comparison does NOT breach executes nothing: no capacity movement and no scaling activity
//! at all.
//!
//! What this suite covers that the sibling execution suites do not: the breach is driven by a
//! datapoint published the way a client publishes one — `PutMetricData` with no `Timestamp`,
//! so the datapoint is stamped at the instant of the call ("If you do not specify a time
//! stamp, the time stamp of the data point is the time that `PutMetricData` was called",
//! `smithy_query(query="doc",shape="MetricDatum",member="Timestamp")`) — rather than by
//! `SetAlarmState` or by a datapoint stamped into a window that has already closed. That is
//! the whole path a client that watches the scaled resource drives: publish, and read the
//! resource back in the service that owns it.
//!
//! The rows differ along what the rule generalises over: the direction of the comparison and
//! of the adjustment, whether the adjustment lands inside the registered window or past either
//! bound, the alarm's `Period` (a sub-minute cadence and the regular minute), the granularity
//! the published series is stored at, and — the negative row — whether the published value
//! breaches the comparison at all.

use std::time::Duration;

use serde_json::{json, Value};

use crate::emulator::Emulator;

/// The namespace an `ecs:service:DesiredCount` target is registered in.
const NAMESPACE: &str = "ecs";

/// The dimension every row scales: "ecs:service:DesiredCount - The task count of an ECS
/// service" (`RegisterScalableTarget$ScalableDimension`).
const DIMENSION: &str = "ecs:service:DesiredCount";

/// The metric namespace the rows publish into. A customer namespace, so every datapoint goes
/// over the wire exactly as a client's does.
const METRIC_NAMESPACE: &str = "Emulator/PublishedBreach";

/// The metric inside it that the rows' alarms watch.
const METRIC_NAME: &str = "Load";

/// The threshold every row's alarm carries.
const THRESHOLD: f64 = 90.0;

/// How long a row waits for the capacity its breach asks for, and how often it looks.
///
/// Nothing in the emulator sleeps for a movement: the poll is the TEST waiting for the
/// evaluation the clock owes the published datapoint and for the policy execution that follows
/// it, so a row whose capacity never moves fails on the value it last read. Generous: an
/// alarm's rolling range ends one evaluation cadence behind the instant it runs, which for
/// these periods is at most ten seconds.
const CLOCK_WAIT: Duration = Duration::from_secs(15);
/// See [`CLOCK_WAIT`].
const CLOCK_POLL: Duration = Duration::from_millis(100);

/// How long the row whose metric never breaches watches for a movement that must not happen.
///
/// Long enough to span many of the scaling clock's own passes and several alarm evaluations, so
/// an execution that should not happen would show up here.
const WITHHELD_WATCH: Duration = Duration::from_secs(3);

/// One row: what the target is registered with, what the ECS service starts at, the policy's
/// step, the alarm that fires it, the datapoint published and where the capacity must land.
struct Row {
    case: &'static str,
    /// The `desiredCount` the ECS service is created with.
    starting: i64,
    min: i64,
    max: i64,
    /// The one step adjustment of the row's policy, as `ScalingAdjustment` under
    /// `ChangeInCapacity`.
    adjustment: i64,
    /// Whether the step covers the interval ABOVE the threshold (its lower bound is zero)
    /// rather than the one below it.
    above: bool,
    comparison_operator: &'static str,
    /// The alarm's `Period`, in seconds.
    period: i64,
    /// The `StorageResolution` the datapoint is published with, or `None` for a publish that
    /// names none — the regular resolution ("This field is optional, if you do not specify it
    /// the default of 60 is used", `MetricDatum$StorageResolution`).
    storage_resolution: Option<i64>,
    /// The value published for the metric the alarm watches.
    value: f64,
    /// The arm a window with no datapoint of its own is counted under.
    ///
    /// A row whose datapoint BREACHES names `notBreaching`, under which nothing but a graded
    /// breaching datapoint can put the alarm in `ALARM` and so execute the policy; a row whose
    /// datapoint does not breach names `missing`, under which an evaluation that could decide
    /// nothing leaves the alarm in `INSUFFICIENT_DATA` — so the `OK` such a row asserts is
    /// reachable only by grading the datapoint that really was published, which is what makes
    /// "nothing executed" a statement about a metric the alarm SAW rather than about one it
    /// never read.
    treat_missing_data: &'static str,
    /// The capacity the ECS service must report once the execution has happened, or the
    /// starting capacity for a row whose metric never breaches.
    capacity: i64,
    /// Whether this row's datapoint breaches the alarm at all.
    breaches: bool,
}

/// The rows.
const ROWS: &[Row] = &[
    // The recorded configuration: a scale-in of three tasks, asked for by a value below the
    // alarm's threshold on a series published at the default resolution.
    Row {
        case: "a scale-in a breaching datapoint asks for",
        starting: 6,
        min: 1,
        max: 40,
        adjustment: -3,
        above: false,
        comparison_operator: "LessThanThreshold",
        period: 10,
        storage_resolution: None,
        value: 50.0,
        treat_missing_data: "notBreaching",
        capacity: 3,
        breaches: true,
    },
    // The same breach asking for more than the floor allows: the movement is performed AT the
    // floor rather than refused.
    Row {
        case: "a scale-in clamped at the registered floor",
        starting: 6,
        min: 4,
        max: 40,
        adjustment: -10,
        above: false,
        comparison_operator: "LessThanOrEqualToThreshold",
        period: 10,
        storage_resolution: None,
        value: 50.0,
        treat_missing_data: "notBreaching",
        capacity: 4,
        breaches: true,
    },
    // The mirror direction, clamped at the ceiling.
    Row {
        case: "a scale-out clamped at the registered ceiling",
        starting: 4,
        min: 1,
        max: 6,
        adjustment: 10,
        above: true,
        comparison_operator: "GreaterThanThreshold",
        period: 10,
        storage_resolution: None,
        value: THRESHOLD + 25.0,
        treat_missing_data: "notBreaching",
        capacity: 6,
        breaches: true,
    },
    // Another sub-minute cadence, and a scale-out inside the window.
    Row {
        case: "a thirty-second alarm period",
        starting: 2,
        min: 1,
        max: 20,
        adjustment: 5,
        above: true,
        comparison_operator: "GreaterThanOrEqualToThreshold",
        period: 30,
        storage_resolution: None,
        value: THRESHOLD,
        treat_missing_data: "notBreaching",
        capacity: 7,
        breaches: true,
    },
    // The regular cadence, over which the same rule holds.
    Row {
        case: "a one-minute alarm period",
        starting: 5,
        min: 1,
        max: 20,
        adjustment: -2,
        above: false,
        comparison_operator: "LessThanThreshold",
        period: 60,
        storage_resolution: None,
        value: 10.0,
        treat_missing_data: "notBreaching",
        capacity: 3,
        breaches: true,
    },
    // A series stored at one-second granularity, which a sub-minute period reads at that
    // granularity.
    Row {
        case: "a high-resolution series",
        starting: 8,
        min: 1,
        max: 20,
        adjustment: -4,
        above: false,
        comparison_operator: "LessThanThreshold",
        period: 10,
        storage_resolution: Some(1),
        value: 20.0,
        treat_missing_data: "notBreaching",
        capacity: 4,
        breaches: true,
    },
    // The negative property: a value the comparison does not breach executes nothing, so the
    // capacity stays where it was and the activity log stays empty.
    Row {
        case: "a metric that never breaches",
        starting: 6,
        min: 1,
        max: 40,
        adjustment: -3,
        above: false,
        comparison_operator: "LessThanThreshold",
        period: 10,
        storage_resolution: None,
        value: THRESHOLD + 60.0,
        // Nothing graded would leave this alarm in INSUFFICIENT_DATA, so the OK the row
        // asserts says the datapoint really was evaluated and still executed nothing.
        treat_missing_data: "missing",
        capacity: 6,
        breaches: false,
    },
    // And the same for a value on the other side of the other direction's comparison.
    Row {
        case: "a metric that never breaches a scale-out alarm",
        starting: 4,
        min: 1,
        max: 20,
        adjustment: 3,
        above: true,
        comparison_operator: "GreaterThanThreshold",
        period: 30,
        storage_resolution: None,
        value: THRESHOLD - 1.0,
        treat_missing_data: "missing",
        capacity: 4,
        breaches: false,
    },
];

/// The three identity members of the `ecs` scalable target on `resource_id`.
fn target(resource_id: &str) -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": resource_id,
        "ScalableDimension": DIMENSION,
    })
}

/// The scaling activities this target's log holds, newest first.
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

/// The state CloudWatch reports for one alarm.
async fn alarm_state(emulator: &Emulator, alarm_name: &str, case: &str) -> String {
    let (status, body) = emulator
        .call_cloudwatch("DescribeAlarms", json!({"AlarmNames": [alarm_name]}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeAlarms to answer 200, got {status} {body:?}"
    );
    body["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
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
async fn a_published_breach_executes_the_policy_the_alarm_names() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::empty();
        let cluster = "published";
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

        let mut step = json!({"ScalingAdjustment": row.adjustment});
        if row.above {
            step["MetricIntervalLowerBound"] = json!(0.0);
        } else {
            step["MetricIntervalUpperBound"] = json!(0.0);
        }
        let mut request = target(&resource_id);
        request["PolicyName"] = json!("published-breach");
        request["PolicyType"] = json!("StepScaling");
        request["StepScalingPolicyConfiguration"] = json!({
            "AdjustmentType": "ChangeInCapacity",
            "MetricAggregationType": "Maximum",
            // Nothing here breaches twice, so the wait a previous activity is given does not
            // come into it; it is named rather than defaulted so each row states it.
            "Cooldown": 0,
            "StepAdjustments": [step],
        });
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
        );
        let policy_arn = body["PolicyARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a PolicyARN, got {body:?}"))
            .to_string();

        // The CUSTOMER alarm: one a client creates itself, naming the policy's ARN as the
        // action its transition into ALARM executes.
        let alarm_name = format!("{}-alarm", case.replace(' ', "-"));
        let (status, body) = emulator
            .call_cloudwatch(
                "PutMetricAlarm",
                json!({
                    "AlarmName": alarm_name,
                    "Namespace": METRIC_NAMESPACE,
                    "MetricName": METRIC_NAME,
                    "Dimensions": [{"Name": "ServiceName", "Value": service}],
                    "Statistic": "Maximum",
                    "Period": row.period,
                    "EvaluationPeriods": 1,
                    "DatapointsToAlarm": 1,
                    "Threshold": THRESHOLD,
                    "ComparisonOperator": row.comparison_operator,
                    // The arm the row names, so what it asserts is a statement about a metric
                    // the alarm really graded (see `Row::treat_missing_data`).
                    "TreatMissingData": row.treat_missing_data,
                    "AlarmActions": [policy_arn],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutMetricAlarm to answer 200, got {status} {body:?}"
        );

        // One datapoint, published the way a client publishes one: no time stamp, so it is
        // stamped at the instant of this call.
        let mut datum = json!({
            "MetricName": METRIC_NAME,
            "Dimensions": [{"Name": "ServiceName", "Value": service}],
            "Value": row.value,
        });
        if let Some(resolution) = row.storage_resolution {
            datum["StorageResolution"] = json!(resolution);
        }
        let (status, body) = emulator
            .call_cloudwatch(
                "PutMetricData",
                json!({"Namespace": METRIC_NAMESPACE, "MetricData": [datum]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutMetricData to answer 200, got {status} {body:?}"
        );

        if !row.breaches {
            // Nothing is owed: the value is on the side of the threshold the comparison does
            // not breach, so the capacity stays where it was and the log stays empty.
            tokio::time::sleep(WITHHELD_WATCH).await;
            let held = emulator.ecs_desired_count(cluster, service).await;
            assert_eq!(
                held, row.capacity,
                "case {case:?}: expected the ECS desiredCount to stay at {}, got {held:?}",
                row.capacity
            );
            let listed = activities(&emulator, &resource_id, case).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: expected no scaling activity at all, got {listed:?}"
            );
            // And the alarm really did grade that datapoint: under the `missing` arm an
            // evaluation that decided nothing would report INSUFFICIENT_DATA instead.
            let state = alarm_state(&emulator, &alarm_name, case).await;
            assert_eq!(
                state, "OK",
                "case {case:?}: expected the alarm to grade the published datapoint into OK, \
                 got {state:?}"
            );
            continue;
        }

        let moved = await_desired_count(&emulator, cluster, service, row.capacity).await;
        assert_eq!(
            moved, row.capacity,
            "case {case:?}: expected the ECS desiredCount to reach {}, got {moved:?}",
            row.capacity
        );

        // The alarm the movement is attributable to reports the state that fired it.
        let state = alarm_state(&emulator, &alarm_name, case).await;
        assert_eq!(
            state, "ALARM",
            "case {case:?}: expected the alarm that executed the policy to report ALARM, \
             got {state:?}"
        );

        // And the movement is recorded as ONE activity, naming the capacity it set and the
        // alarm and policy that caused it.
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
        let expected_cause = format!(
            "monitor alarm {alarm_name} in state ALARM triggered policy published-breach"
        );
        assert_eq!(
            activity["Cause"],
            json!(expected_cause),
            "case {case:?}: expected Cause {expected_cause:?}, got {activity:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! What a target's `SuspendedState` does to a scheduled action whose instant has come.
//!
//! The rule: a scheduled action's occurrence is initiated only while SCHEDULED scaling is
//! running on the target it belongs to — "set the value to true if you don't want
//! Application Auto Scaling to add or remove capacity by initiating scheduled actions" —
//! and an occurrence that falls under such a suspension is dropped rather than held, so
//! resuming scheduled scaling does not replay it. The two flags that suspend POLICY-driven
//! scaling in and out say nothing about scheduled scaling, so an occurrence still fires
//! under either of them.
//!
//! Asserted over the table of suspended states below rather than over the one the issue
//! cited: every flag alone, the three together, and the flag set explicitly to `false`.
//! Each row also pins the second half of the upsert rule — a `SuspendedState` changes only
//! the flags it names — by reading the whole reported state back.

use std::time::Duration;

use serde_json::{json, Value};

use super::emulator::{at_schedule_in, Emulator};

/// The dimension every row scales: a DynamoDB table's provisioned write capacity, so the
/// capacity an initiated occurrence moves is readable in the table store too.
const DIMENSION: &str = "dynamodb:table:WriteCapacityUnits";

/// The namespace that dimension belongs to.
const NAMESPACE: &str = "dynamodb";

/// The window between putting the action and its occurrence. Nothing sleeps in the service:
/// the due instant is stored, and the first request after it passes materialises what the
/// clock owed.
const WINDOW: Duration = Duration::from_secs(4);

/// The capacity window the target is registered with.
const REGISTERED_MIN: i64 = 1;
const REGISTERED_MAX: i64 = 20;

/// The `MinCapacity` the scheduled action sets, above the registered floor so an initiated
/// occurrence is visible both as the target's bounds and as a capacity movement.
const SCHEDULED_MIN: i64 = 5;

/// One row: the `SuspendedState` the target carries when the occurrence comes due, and
/// whether the occurrence is initiated.
struct SuspensionRow {
    case: &'static str,
    /// `(member, value)` pairs of the `SuspendedState` the registration carries.
    suspended: &'static [(&'static str, bool)],
    /// Whether the occurrence is initiated.
    fires: bool,
}

/// The suspended states.
const SUSPENSION_ROWS: &[SuspensionRow] = &[
    SuspensionRow {
        case: "nothing suspended",
        suspended: &[],
        fires: true,
    },
    SuspensionRow {
        case: "scheduled scaling suspended",
        suspended: &[("ScheduledScalingSuspended", true)],
        fires: false,
    },
    SuspensionRow {
        case: "every activity suspended",
        suspended: &[
            ("ScheduledScalingSuspended", true),
            ("DynamicScalingInSuspended", true),
            ("DynamicScalingOutSuspended", true),
        ],
        fires: false,
    },
    SuspensionRow {
        case: "only policy-driven scale-in suspended",
        suspended: &[("DynamicScalingInSuspended", true)],
        fires: true,
    },
    SuspensionRow {
        case: "only policy-driven scale-out suspended",
        suspended: &[("DynamicScalingOutSuspended", true)],
        fires: true,
    },
    SuspensionRow {
        case: "scheduled scaling explicitly not suspended",
        suspended: &[("ScheduledScalingSuspended", false)],
        fires: true,
    },
];

#[tokio::test]
async fn a_scheduled_occurrence_is_initiated_only_while_scheduled_scaling_runs() {
    // Every row's action is put first, so one wait covers the whole table.
    let mut rows = Vec::new();
    for row in SUSPENSION_ROWS {
        let emulator = Emulator::empty();
        let target = emulator.given_target(DIMENSION, REGISTERED_MIN, REGISTERED_MAX).await;

        let mut suspend = target.clone();
        suspend["SuspendedState"] = suspended_state(row.suspended);
        let (status, body) = emulator.call("RegisterScalableTarget", suspend).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the suspended-state upsert to answer 200, got {status} {body:?}",
            row.case
        );

        let (schedule, _) = at_schedule_in(WINDOW);
        let mut action = target.clone();
        action["ScheduledActionName"] = json!("raise-the-floor");
        action["Schedule"] = json!(schedule);
        action["ScalableTargetAction"] = json!({"MinCapacity": SCHEDULED_MIN});
        let (status, body) = emulator.call("PutScheduledAction", action).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected PutScheduledAction to answer 200, got {status} {body:?}",
            row.case
        );
        rows.push((row, emulator, target));
    }

    // Past the instant every occurrence was due at.
    std::thread::sleep(WINDOW + Duration::from_millis(1_500));

    for (row, emulator, target) in rows {
        let case = row.case;
        if row.fires {
            let reported = scalable_target(&emulator, case, &target).await;
            assert_eq!(
                reported["MinCapacity"],
                json!(SCHEDULED_MIN),
                "case {case:?}: expected the occurrence to set the floor, got {reported:?}"
            );
            assert_eq!(
                write_capacity(&emulator, case).await,
                SCHEDULED_MIN,
                "case {case:?}: expected the new floor to move the table's write capacity"
            );
            assert!(
                caused_by_the_action(&emulator, case).await,
                "case {case:?}: expected a scaling activity naming the action that caused it"
            );
        } else {
            // The FIRST call after the instant passed is the one that resumes scheduled
            // scaling: the occurrence it finds due was owed while the suspension was still
            // in force, so it is dropped, and resuming must not replay it.
            let mut resume = target.clone();
            resume["SuspendedState"] = json!({"ScheduledScalingSuspended": false});
            let (status, body) = emulator.call("RegisterScalableTarget", resume).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the resume to answer 200, got {status} {body:?}"
            );
            let reported = scalable_target(&emulator, case, &target).await;
            assert_eq!(
                reported["MinCapacity"],
                json!(REGISTERED_MIN),
                "case {case:?}: expected the registered floor to stand after the resume, got \
                 {reported:?}"
            );
            assert_eq!(
                write_capacity(&emulator, case).await,
                REGISTERED_MIN,
                "case {case:?}: expected the table's write capacity to be left alone"
            );
            assert!(
                !caused_by_the_action(&emulator, case).await,
                "case {case:?}: expected the withheld occurrence to record no scaling activity"
            );
        }

        // The upsert changed only the flags it named: a flag the row set stays set, and one
        // it never named is reported as the `false` it is — except the one the resume above
        // deliberately cleared.
        let reported = scalable_target(&emulator, case, &target).await;
        for member in [
            "DynamicScalingInSuspended",
            "DynamicScalingOutSuspended",
            "ScheduledScalingSuspended",
        ] {
            let expected = if !row.fires && member == "ScheduledScalingSuspended" {
                false
            } else {
                row.suspended
                    .iter()
                    .find(|(name, _)| *name == member)
                    .is_some_and(|(_, value)| *value)
            };
            assert_eq!(
                reported["SuspendedState"][member],
                json!(expected),
                "case {case:?}: expected {member} to be reported as {expected}, got {:?}",
                reported["SuspendedState"]
            );
        }
    }
}

/// The `SuspendedState` member a row's flags make.
fn suspended_state(flags: &[(&str, bool)]) -> Value {
    let mut state = json!({});
    for (member, value) in flags {
        state[*member] = json!(value);
    }
    state
}

/// The one scalable target the suite registers, as `DescribeScalableTargets` reports it.
///
/// The report is narrowed by the target's own `ResourceId` beside its dimension: a dimension
/// names a capacity property of ONE resource, so this service refuses a dimension filter
/// sent with no resource id.
async fn scalable_target(emulator: &Emulator, case: &str, target: &Value) -> Value {
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({
                "ServiceNamespace": NAMESPACE,
                "ResourceIds": [target["ResourceId"].clone()],
                "ScalableDimension": DIMENSION,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalableTargets to answer 200, got {status} {body:?}"
    );
    body["ScalableTargets"][0].clone()
}

/// Whether the activity log holds a movement the scheduled action caused.
async fn caused_by_the_action(emulator: &Emulator, case: &str) -> bool {
    let (status, body) = emulator
        .call(
            "DescribeScalingActivities",
            json!({"ServiceNamespace": NAMESPACE}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingActivities to answer 200, got {status} {body:?}"
    );
    body["ScalingActivities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|activity| {
            activity["Cause"]
                .as_str()
                .unwrap_or_default()
                .contains("raise-the-floor")
        })
}

/// The provisioned write capacity the DynamoDB store holds for the table under test.
async fn write_capacity(emulator: &Emulator, case: &str) -> i64 {
    let (status, body) = emulator
        .call_dynamodb("DescribeTable", json!({"TableName": "catalog"}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeTable to answer 200, got {status} {body:?}"
    );
    body["Table"]["ProvisionedThroughput"]["WriteCapacityUnits"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a write capacity, got {body:?}"))
}

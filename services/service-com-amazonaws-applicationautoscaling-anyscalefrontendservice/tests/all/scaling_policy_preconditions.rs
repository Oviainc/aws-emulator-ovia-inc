// SPDX-License-Identifier: CC0-1.0

//! The preconditions `PutScalingPolicy` holds a policy to, end to end.
//!
//! Two rules, each asserted over a span of inputs:
//!
//! * A policy's TYPE is part of its identity. Re-putting an existing policy name with a
//!   different `PolicyType` is refused with HTTP 400 `ValidationException`, and the stored
//!   policy keeps the type, the configuration and the alarms it had; putting the SAME type
//!   again is an update and succeeds.
//! * A step policy's step adjustments must select exactly ONE adjustment for any breach:
//!   "the ranges of your step adjustments can't overlap or have a gap" (`StepAdjustment`).
//!   A range is `[MetricIntervalLowerBound, MetricIntervalUpperBound)` over the difference
//!   between the metric and the alarm threshold, with a null bound unbounded in that
//!   direction, so the ranges have to partition those differences: two that share a point
//!   name two adjustments for one breach, two that leave a gap name none for a breach
//!   inside it, and a range that is empty names no breach at all. Each is refused.

use serde_json::{json, Value};

use super::emulator::{error_code, Emulator};

/// The ECS service every row scales: the one namespace every policy type, predictive
/// included, is supported on.
const DIMENSION: &str = "ecs:service:DesiredCount";

/// A valid configuration of one policy type.
struct PolicyKind {
    policy_type: &'static str,
    member: &'static str,
    configuration: fn() -> Value,
}

/// A valid configuration of each policy type. One row per value of the model's
/// `PolicyType` enum (`POLICY_TYPE_VALUES`).
const POLICY_TYPES: &[PolicyKind] = &[
    PolicyKind {
        policy_type: "TargetTrackingScaling",
        member: "TargetTrackingScalingPolicyConfiguration",
        configuration: || {
            json!({"TargetValue": 40.0,
                   "PredefinedMetricSpecification": {"PredefinedMetricType": "ECSServiceAverageCPUUtilization"}})
        },
    },
    PolicyKind {
        policy_type: "StepScaling",
        member: "StepScalingPolicyConfiguration",
        configuration: || {
            json!({"AdjustmentType": "ChangeInCapacity",
                   "StepAdjustments": [{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}]})
        },
    },
    PolicyKind {
        policy_type: "PredictiveScaling",
        member: "PredictiveScalingPolicyConfiguration",
        configuration: || {
            json!({"MetricSpecifications": [{
                       "TargetValue": 40.0,
                       "PredefinedMetricPairSpecification": {"PredefinedMetricType": "ECSServiceCPUUtilization"}}]})
        },
    },
];

#[tokio::test]
async fn a_policy_cannot_change_its_type_under_the_same_name() {
    for first in POLICY_TYPES {
        for second in POLICY_TYPES {
            let (first_type, second_type) = (first.policy_type, second.policy_type);
            let case = format!("{first_type} then {second_type}");
            let emulator = Emulator::empty();
            let target = emulator.given_target(DIMENSION, 1, 20).await;

            let (status, body) = put(
                &emulator,
                &target,
                first_type,
                first.member,
                (first.configuration)(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the first put to answer 200, got {status} {body:?}"
            );
            let minted = body["Alarms"].as_array().cloned().unwrap_or_default();

            let (status, body) = put(
                &emulator,
                &target,
                second_type,
                second.member,
                (second.configuration)(),
            )
            .await;
            if second_type == first_type {
                // The same type is an update of the policy, not a change of what it is.
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected re-putting the same type to answer 200, got {status} \
                     {body:?}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {body:?}"
                );
            }

            // Whatever the second put answered, the stored policy is still a policy of the
            // type it was created with, carrying that type's configuration.
            let (status, body) = emulator
                .call("DescribeScalingPolicies", {
                    let mut request = target.clone();
                    request["PolicyNames"] = json!(["policy"]);
                    request
                })
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeScalingPolicies to answer 200, got {status} \
                 {body:?}"
            );
            let stored = body["ScalingPolicies"][0].clone();
            let expected_type = if second_type == first_type {
                second_type
            } else {
                first_type
            };
            assert_eq!(
                stored["PolicyType"].as_str().unwrap_or_default(),
                expected_type,
                "case {case:?}: expected the stored policy to have policy type {expected_type}, \
                 got {stored:?}"
            );
            let expected_member = if second_type == first_type {
                second.member
            } else {
                first.member
            };
            assert!(
                stored.get(expected_member).is_some(),
                "case {case:?}: expected the stored policy to carry {expected_member}, got \
                 {stored:?}"
            );

            // And it still owns the alarms it minted: a refused put changes nothing.
            if second_type != first_type {
                let alarms = emulator.target_tracking_alarms().await;
                assert_eq!(
                    alarms.len(),
                    minted.len(),
                    "case {case:?}: expected the policy to keep its {} alarm(s), got {alarms:?}",
                    minted.len()
                );
            }
        }
    }
}

/// One row of the step-range table: the step adjustments, and whether a policy carrying
/// them is a policy whose breach selects exactly one adjustment.
struct StepRow {
    case: &'static str,
    adjustments: fn() -> Value,
    accepted: bool,
}

/// The table. The accepted rows differ along the number of ranges and which ends are
/// unbounded; the refused ones along HOW the ranges fail to partition the metric
/// differences — two sharing an interior, a nesting, a duplicate, an overlap against an
/// unbounded end, an overlapping pair given out of order, a gap between two ranges and a
/// gap in the middle of three — plus a range that holds no point at all.
const STEP_TABLE: &[StepRow] = &[
    StepRow {
        case: "one range, unbounded above",
        adjustments: || json!([{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}]),
        accepted: true,
    },
    StepRow {
        case: "two contiguous ranges, both ends unbounded",
        adjustments: || {
            json!([{"MetricIntervalUpperBound": 0.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}])
        },
        accepted: true,
    },
    StepRow {
        case: "three contiguous ranges",
        adjustments: || {
            json!([{"MetricIntervalUpperBound": 0.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 10.0, "ScalingAdjustment": 3}])
        },
        accepted: true,
    },
    StepRow {
        case: "two ranges that touch where the upper bound is exclusive",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 10.0, "MetricIntervalUpperBound": 20.0, "ScalingAdjustment": 2}])
        },
        accepted: true,
    },
    StepRow {
        case: "two ranges with a gap between them",
        adjustments: || {
            json!([{"MetricIntervalUpperBound": -10.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 10.0, "ScalingAdjustment": 1}])
        },
        accepted: false,
    },
    StepRow {
        case: "three ranges with a gap in the middle",
        adjustments: || {
            json!([{"MetricIntervalUpperBound": 0.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 20.0, "ScalingAdjustment": 3}])
        },
        accepted: false,
    },
    StepRow {
        case: "two ranges overlapping in their interior",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 20.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 10.0, "MetricIntervalUpperBound": 30.0, "ScalingAdjustment": 2}])
        },
        accepted: false,
    },
    StepRow {
        case: "a range nested inside another",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 50.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 10.0, "MetricIntervalUpperBound": 20.0, "ScalingAdjustment": 2}])
        },
        accepted: false,
    },
    StepRow {
        case: "the same range twice",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 2}])
        },
        accepted: false,
    },
    StepRow {
        case: "a range unbounded below overlapping a bounded one",
        adjustments: || {
            json!([{"MetricIntervalUpperBound": 10.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 5.0, "MetricIntervalUpperBound": 20.0, "ScalingAdjustment": 1}])
        },
        accepted: false,
    },
    StepRow {
        case: "a range unbounded above overlapping a bounded one",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": -10.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": -1}])
        },
        accepted: false,
    },
    StepRow {
        case: "overlapping ranges listed out of order",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 30.0, "MetricIntervalUpperBound": 40.0, "ScalingAdjustment": 2},
                   {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 35.0, "ScalingAdjustment": 1}])
        },
        accepted: false,
    },
    StepRow {
        case: "a range that holds no point",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 10.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1}])
        },
        accepted: false,
    },
    StepRow {
        case: "a range whose bounds are the wrong way round",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 20.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1}])
        },
        accepted: false,
    },
];

#[tokio::test]
async fn step_adjustment_ranges_must_select_one_adjustment() {
    for row in STEP_TABLE {
        let case = row.case;
        let emulator = Emulator::empty();
        let target = emulator.given_target(DIMENSION, 1, 20).await;
        let adjustments = (row.adjustments)();
        let (status, body) = put(
            &emulator,
            &target,
            "StepScaling",
            "StepScalingPolicyConfiguration",
            json!({"AdjustmentType": "ChangeInCapacity", "StepAdjustments": adjustments}),
        )
        .await;

        if !row.accepted {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            // A refused policy is not stored at all.
            let stored = describe(&emulator, &target).await;
            assert_eq!(
                stored,
                Vec::<Value>::new(),
                "case {case:?}: expected a refused policy to store nothing, got {stored:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );
        // An accepted policy keeps every range it was put with, so a later breach is
        // evaluated against what the caller sent.
        let stored = describe(&emulator, &target).await;
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: expected one stored policy, got {stored:?}"
        );
        assert_eq!(
            stored[0]["StepScalingPolicyConfiguration"]["StepAdjustments"], adjustments,
            "case {case:?}: expected the stored step adjustments to be {adjustments:?}, got {:?}",
            stored[0]["StepScalingPolicyConfiguration"]
        );
    }
}

/// Put one policy named `policy` on `target`.
async fn put(
    emulator: &Emulator,
    target: &Value,
    policy_type: &str,
    member: &str,
    configuration: Value,
) -> (u16, Value) {
    let mut request = target.clone();
    request["PolicyName"] = json!("policy");
    request["PolicyType"] = json!(policy_type);
    request[member] = configuration;
    emulator.call("PutScalingPolicy", request).await
}

/// The policies the service holds for `target`.
async fn describe(emulator: &Emulator, target: &Value) -> Vec<Value> {
    let (status, body) = emulator
        .call("DescribeScalingPolicies", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "expected DescribeScalingPolicies to answer 200, got {status} {body:?}"
    );
    body["ScalingPolicies"].as_array().cloned().unwrap_or_default()
}

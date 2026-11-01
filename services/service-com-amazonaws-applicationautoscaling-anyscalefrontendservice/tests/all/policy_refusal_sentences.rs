// SPDX-License-Identifier: CC0-1.0

//! The two `PutScalingPolicy` refusals whose message is a FIXED sentence, asserted over the
//! span of requests each rule covers.
//!
//! The rule both parts assert: a refusal of this operation states the rule it enforces in the
//! service's own sentence for that error class and interpolates NOTHING from the request — not
//! the policy it is about, not the type that policy already carries, and not the bounds the
//! caller submitted. A client (or a deployment tool's error matcher) recognises a refusal by its
//! text, so a sentence that varies with the request is a different error contract per request.
//!
//! * Re-putting an existing policy name under another `PolicyType` is refused with one sentence,
//!   whichever two of the model's `PolicyType` values the change is between and whatever the
//!   policy is called.
//! * A step configuration whose `StepAdjustments` ranges do not partition the metric-to-threshold
//!   differences is refused with one sentence per way they fail: ranges that OVERLAP (two
//!   adjustments for one breach), ranges that leave a GAP (none for a breach inside it), and a
//!   range that holds no difference at all. Each sentence is the same for every shape of its
//!   own failure.

use serde_json::{json, Value};

use super::emulator::{error_code, error_message, Emulator};

/// The dimension every row scales: an ECS service is the one resource every `PolicyType` value,
/// `PredictiveScaling` included, is supported on.
const DIMENSION: &str = "ecs:service:DesiredCount";

/// The sentence a change of an existing policy's type is refused with.
const POLICY_TYPE_IS_NOT_CHANGEABLE: &str =
    "You cannot change a StepScaling policy to a non StepScaling policy, or vice versa.";

/// The sentence overlapping step-adjustment ranges are refused with.
const INTERVALS_OVERLAP: &str = "Step adjustment intervals cannot overlap";

/// The sentence step-adjustment ranges that leave a gap are refused with — the other half of
/// "the ranges of your step adjustments can't overlap or have a gap" (`StepAdjustment`).
const INTERVALS_HAVE_A_GAP: &str = "The ranges of your step adjustments can't have a gap";

/// The sentence one range that holds no metric-to-threshold difference is refused with.
const BOUNDS_OUT_OF_ORDER: &str =
    "A step adjustment's lower bound must be less than its upper bound";

/// A valid configuration of one policy type: the member that carries it, and its body.
struct PolicyKind {
    policy_type: &'static str,
    member: &'static str,
    configuration: fn() -> Value,
}

/// One valid configuration per value of the model's `PolicyType` enum
/// (`smithy_query(query="shape",shape="com.amazonaws.applicationautoscaling#PolicyType")`), so
/// the table below spans every change of type a caller can ask for.
const POLICY_TYPES: &[PolicyKind] = &[
    PolicyKind {
        policy_type: "StepScaling",
        member: "StepScalingPolicyConfiguration",
        configuration: || {
            json!({"AdjustmentType": "ChangeInCapacity",
                   "StepAdjustments": [{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}]})
        },
    },
    PolicyKind {
        policy_type: "TargetTrackingScaling",
        member: "TargetTrackingScalingPolicyConfiguration",
        configuration: || {
            json!({"TargetValue": 40.0,
                   "PredefinedMetricSpecification": {
                       "PredefinedMetricType": "ECSServiceAverageCPUUtilization"}})
        },
    },
    PolicyKind {
        policy_type: "PredictiveScaling",
        member: "PredictiveScalingPolicyConfiguration",
        configuration: || {
            json!({"MetricSpecifications": [{
                       "TargetValue": 40.0,
                       "PredefinedMetricPairSpecification": {
                           "PredefinedMetricType": "ECSServiceCPUUtilization"}}]})
        },
    },
];

/// Policy names the rows are run under: the sentence must not render any of them, so they differ
/// in length and in shape.
const POLICY_NAMES: &[&str] = &["scale-on-cpu", "p", "a-rather-long-policy-name.2_final"];

#[tokio::test]
async fn a_change_of_an_existing_policys_type_is_refused_with_one_fixed_sentence() {
    for first in POLICY_TYPES {
        for second in POLICY_TYPES {
            if first.policy_type == second.policy_type {
                continue;
            }
            for name in POLICY_NAMES {
                let case = format!("{} to {} as {name:?}", first.policy_type, second.policy_type);
                let emulator = Emulator::empty();
                let target = emulator.given_target(DIMENSION, 1, 20).await;

                let (status, body) = put(&emulator, &target, name, first, (first.configuration)())
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the first put to answer 200, got {status} {body:?}"
                );

                let (status, body) =
                    put(&emulator, &target, name, second, (second.configuration)()).await;
                assert_refused(&case, status, &body, POLICY_TYPE_IS_NOT_CHANGEABLE);
            }
        }
    }
}

/// One step configuration whose ranges do not partition the metric-to-threshold differences:
/// the adjustments, and the sentence the service refuses them with.
struct RangeRow {
    case: &'static str,
    adjustments: fn() -> Value,
    expected: &'static str,
}

/// The table. The overlapping rows differ along HOW the ranges overlap — in their interior, by
/// nesting, by being the same range twice, against an unbounded end, listed out of order, and
/// three ranges of which only the last two overlap — and the remaining rows are the OTHER two
/// ways the ranges fail, so each sentence is pinned as the one for its own failure rather than
/// as the only sentence this check has.
const RANGE_TABLE: &[RangeRow] = &[
    RangeRow {
        case: "a bounded range overlapping one unbounded above",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 5.0, "ScalingAdjustment": 2}])
        },
        expected: INTERVALS_OVERLAP,
    },
    RangeRow {
        case: "two ranges overlapping in their interior",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": -20.0, "MetricIntervalUpperBound": 5.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 30.0, "ScalingAdjustment": 2}])
        },
        expected: INTERVALS_OVERLAP,
    },
    RangeRow {
        case: "a range nested inside another",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 50.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 10.0, "MetricIntervalUpperBound": 20.0, "ScalingAdjustment": 2}])
        },
        expected: INTERVALS_OVERLAP,
    },
    RangeRow {
        case: "the same range twice",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 2}])
        },
        expected: INTERVALS_OVERLAP,
    },
    RangeRow {
        case: "a range unbounded below overlapping a bounded one",
        adjustments: || {
            json!([{"MetricIntervalUpperBound": 10.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 2.5, "MetricIntervalUpperBound": 20.0, "ScalingAdjustment": 1}])
        },
        expected: INTERVALS_OVERLAP,
    },
    RangeRow {
        case: "overlapping ranges listed out of order",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 30.0, "MetricIntervalUpperBound": 40.0, "ScalingAdjustment": 2},
                   {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 35.0, "ScalingAdjustment": 1}])
        },
        expected: INTERVALS_OVERLAP,
    },
    RangeRow {
        case: "three ranges of which the last two overlap",
        adjustments: || {
            json!([{"MetricIntervalUpperBound": 0.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 9.0, "MetricIntervalUpperBound": 20.0, "ScalingAdjustment": 3}])
        },
        expected: INTERVALS_OVERLAP,
    },
    RangeRow {
        case: "two ranges with a gap between them",
        adjustments: || {
            json!([{"MetricIntervalUpperBound": -10.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 10.0, "ScalingAdjustment": 1}])
        },
        expected: INTERVALS_HAVE_A_GAP,
    },
    RangeRow {
        case: "three ranges with a gap in the middle",
        adjustments: || {
            json!([{"MetricIntervalUpperBound": 0.0, "ScalingAdjustment": -1},
                   {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1},
                   {"MetricIntervalLowerBound": 20.0, "ScalingAdjustment": 3}])
        },
        expected: INTERVALS_HAVE_A_GAP,
    },
    RangeRow {
        case: "a range that holds no difference",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 10.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1}])
        },
        expected: BOUNDS_OUT_OF_ORDER,
    },
    RangeRow {
        case: "a range whose bounds are the wrong way round",
        adjustments: || {
            json!([{"MetricIntervalLowerBound": 20.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1}])
        },
        expected: BOUNDS_OUT_OF_ORDER,
    },
];

#[tokio::test]
async fn step_ranges_that_select_no_single_adjustment_carry_one_sentence_per_failure() {
    let step = &POLICY_TYPES[0];
    for row in RANGE_TABLE {
        let case = row.case;
        let emulator = Emulator::empty();
        let target = emulator.given_target(DIMENSION, 1, 20).await;
        let configuration = json!({
            "AdjustmentType": "ChangeInCapacity",
            "StepAdjustments": (row.adjustments)(),
        });
        let (status, body) = put(&emulator, &target, "stepper", step, configuration).await;
        assert_refused(case, status, &body, row.expected);
    }

    // And ranges that DO partition the differences are accepted, so what the sentences above
    // report is the failure and not the check itself.
    let emulator = Emulator::empty();
    let target = emulator.given_target(DIMENSION, 1, 20).await;
    let configuration = json!({
        "AdjustmentType": "ChangeInCapacity",
        "StepAdjustments": [
            {"MetricIntervalUpperBound": 0.0, "ScalingAdjustment": -1},
            {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1},
            {"MetricIntervalLowerBound": 10.0, "ScalingAdjustment": 3},
        ],
    });
    let (status, body) = put(&emulator, &target, "stepper", step, configuration).await;
    assert_eq!(
        status, 200,
        "case \"three contiguous ranges\": expected 200, got {status} {body:?}"
    );
}

/// Put `name` as a policy of `kind` carrying `configuration` on `target`.
async fn put(
    emulator: &Emulator,
    target: &Value,
    name: &str,
    kind: &PolicyKind,
    configuration: Value,
) -> (u16, Value) {
    let mut request = target.clone();
    request["PolicyName"] = json!(name);
    request["PolicyType"] = json!(kind.policy_type);
    request[kind.member] = configuration;
    emulator.call("PutScalingPolicy", request).await
}

/// Assert a 400 `ValidationException` carrying exactly `expected`.
fn assert_refused(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {body:?}"
    );
    let message = error_message(body);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

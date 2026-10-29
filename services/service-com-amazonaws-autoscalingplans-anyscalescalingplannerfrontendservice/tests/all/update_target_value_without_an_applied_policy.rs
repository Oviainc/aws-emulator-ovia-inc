// SPDX-License-Identifier: CC0-1.0

//! What an update does to the target value of a metric NO applied policy of the
//! resource tracks.
//!
//! The rule: an update keeps the target value in force only for a metric an applied
//! policy of the resource tracks to — that policy is the thing the update must not
//! re-point — so a metric the plan has applied no policy for takes the target value the
//! update supplies. `DisableDynamicScaling` "Controls whether dynamic scaling by AWS
//! Auto Scaling is disabled. When dynamic scaling is enabled, AWS Auto Scaling creates
//! target tracking scaling policies based on the specified target tracking
//! configurations" (`smithy_query(query="shape",shape="ScalingInstruction")`), so a
//! resource whose instruction in force turned dynamic scaling off has no such policy and
//! every target value it reports after an update is the update's.
//!
//! [`crate::applied_policies_and_status_narratives`] spans the other side of the same
//! rule — the value an applied policy tracks to is kept — so the rows here differ along
//! what decides WHICH side applies: whether the instruction in force applied a policy,
//! the form of the metric (predefined vs customized), the number of configurations on
//! the resource, the kind of scalable resource, and whether the update itself moves the
//! dynamic-scaling flag.
//!
//! The harness (router, dispatch, lifecycle clock) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use crate::applied_policies_and_status_narratives::{applied_plan, described, resources};
use crate::scaling_plan_lifecycle::Emulator;

/// A target tracking configuration on the one predefined metric the DynamoDB
/// read-capacity dimension supports
/// (`smithy_query(query="shape",shape="ScalingMetricType")`, mirrored in
/// `src/ops/types/shared_types.rs` as `SCALING_METRIC_TYPE_VALUES`).
fn predefined(target: f64) -> Value {
    json!({
        "PredefinedScalingMetricSpecification": {
            "PredefinedScalingMetricType": "DynamoDBReadCapacityUtilization"
        },
        "TargetValue": target
    })
}

/// A target tracking configuration on a customized metric, the other FORM a metric a
/// resource tracks can take.
fn customized(target: f64) -> Value {
    json!({
        "CustomizedScalingMetricSpecification": {
            "MetricName": "queue-depth",
            "Namespace": "MyApp",
            "Statistic": "Sum"
        },
        "TargetValue": target
    })
}

/// A DynamoDB table instruction tracking `configurations`, with `DisableDynamicScaling`
/// carried only when the case states it — the member's documented default is enabled
/// (`false`), so a case can leave it out, turn it off or state it on.
fn table_instruction(configurations: Vec<Value>, disable_dynamic_scaling: Option<bool>) -> Value {
    let mut instruction = json!({
        "ServiceNamespace": "dynamodb",
        "ResourceId": "table/my-table",
        "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
        "MinCapacity": 1,
        "MaxCapacity": 100,
        "TargetTrackingConfigurations": configurations
    });
    if let Some(disabled) = disable_dynamic_scaling {
        instruction["DisableDynamicScaling"] = json!(disabled);
    }
    instruction
}

/// An Aurora DB cluster instruction, so the rule is exercised on a second kind of
/// scalable resource too.
fn cluster_instruction(target: f64, disable_dynamic_scaling: bool) -> Value {
    json!({
        "ServiceNamespace": "rds",
        "ResourceId": "cluster:my-db-cluster",
        "ScalableDimension": "rds:cluster:ReadReplicaCount",
        "MinCapacity": 1,
        "MaxCapacity": 15,
        "TargetTrackingConfigurations": [customized(target)],
        "DisableDynamicScaling": disable_dynamic_scaling
    })
}

/// One row: the instructions in force, the instructions the update carries and the
/// instructions the plan reports once the update has settled.
struct Case {
    label: &'static str,
    in_force: Vec<Value>,
    requested: Vec<Value>,
    expected: Vec<Value>,
}

/// The table this suite spans.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no policy tracks the predefined metric, so the update's value takes effect",
            in_force: vec![table_instruction(vec![predefined(40.0)], Some(true))],
            requested: vec![table_instruction(vec![predefined(70.0)], Some(true))],
            expected: vec![table_instruction(vec![predefined(70.0)], Some(true))],
        },
        Case {
            label: "no policy tracks the customized metric either",
            in_force: vec![table_instruction(vec![customized(25.0)], Some(true))],
            requested: vec![table_instruction(vec![customized(60.0)], Some(true))],
            expected: vec![table_instruction(vec![customized(60.0)], Some(true))],
        },
        Case {
            label: "every configuration of a resource with no policy takes the update's value",
            in_force: vec![table_instruction(
                vec![predefined(40.0), customized(25.0)],
                Some(true),
            )],
            requested: vec![table_instruction(
                vec![predefined(70.0), customized(60.0)],
                Some(true),
            )],
            expected: vec![table_instruction(
                vec![predefined(70.0), customized(60.0)],
                Some(true),
            )],
        },
        Case {
            label: "an update that turns dynamic scaling back on sets the value its policy tracks",
            in_force: vec![table_instruction(vec![predefined(40.0)], Some(true))],
            // The member is omitted, which is its documented default (enabled).
            requested: vec![table_instruction(vec![predefined(70.0)], None)],
            expected: vec![table_instruction(vec![predefined(70.0)], None)],
        },
        Case {
            label: "the same, with dynamic scaling stated as enabled rather than omitted",
            in_force: vec![table_instruction(vec![predefined(40.0)], Some(true))],
            requested: vec![table_instruction(vec![predefined(70.0)], Some(false))],
            expected: vec![table_instruction(vec![predefined(70.0)], Some(false))],
        },
        Case {
            label: "an Aurora DB cluster with no policy takes the update's value too",
            in_force: vec![cluster_instruction(25.0, true)],
            requested: vec![cluster_instruction(60.0, true)],
            expected: vec![cluster_instruction(60.0, true)],
        },
        Case {
            // The other side of the rule: there IS a policy here, tracking to the value
            // in force, and the update does not re-point it.
            label: "a policy tracking the metric keeps the target value in force",
            in_force: vec![table_instruction(vec![predefined(40.0)], None)],
            requested: vec![table_instruction(vec![predefined(70.0)], None)],
            expected: vec![table_instruction(vec![predefined(40.0)], None)],
        },
        Case {
            // The policy in force is what the rule looks at: it exists when the update
            // arrives, so its target value is kept even though the same update goes on
            // to turn the dynamic scaling that minted it off.
            label: "a policy tracking the metric keeps it even as the update turns scaling off",
            in_force: vec![table_instruction(vec![predefined(40.0)], None)],
            requested: vec![table_instruction(vec![predefined(70.0)], Some(true))],
            expected: vec![table_instruction(vec![predefined(40.0)], Some(true))],
        },
    ]
}

/// The target tracking configurations the policies on `name`'s resources track, in the
/// order the resources and their policies are reported. A resource with no policy
/// contributes nothing, and the member is absent rather than an empty list.
async fn tracked_configurations(emulator: &Emulator, name: &str, label: &str) -> Vec<Value> {
    resources(emulator, name)
        .await
        .iter()
        .flat_map(|resource| {
            let policies = match resource.get("ScalingPolicies") {
                None => Vec::new(),
                Some(policies) => policies
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("case {label:?}: ScalingPolicies is not a list in {resource:?}")
                    })
                    .clone(),
            };
            policies
                .into_iter()
                .map(|policy| policy["TargetTrackingConfiguration"].clone())
                .collect::<Vec<Value>>()
        })
        .collect()
}

#[tokio::test]
async fn an_update_sets_the_target_value_of_a_metric_no_applied_policy_tracks() {
    for case in cases() {
        let emulator = Emulator::new();
        let label = case.label;
        applied_plan(&emulator, "my-plan", case.in_force.clone()).await;

        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ScalingPlanVersion": 1,
                    "ScalingInstructions": case.requested.clone(),
                }),
            )
            .await;
        emulator.make_all_due();

        let plan = described(&emulator, "my-plan").await;
        assert_eq!(
            plan.get("ScalingInstructions"),
            Some(&Value::Array(case.expected.clone())),
            "case {label:?}: wrong ScalingInstructions in force after the update, got {plan:?}"
        );

        // Whatever the instructions in force say, the policies on the resources track to
        // them: a resource whose instruction disabled dynamic scaling carries none, and
        // one whose instruction enables it carries a policy per configuration.
        let expected_configurations: Vec<Value> = case
            .expected
            .iter()
            .filter(|instruction| instruction["DisableDynamicScaling"] != json!(true))
            .flat_map(|instruction| {
                instruction["TargetTrackingConfigurations"]
                    .as_array()
                    .expect("the case's configurations")
                    .clone()
            })
            .collect();
        let tracked = tracked_configurations(&emulator, "my-plan", label).await;
        assert_eq!(
            tracked, expected_configurations,
            "case {label:?}: the applied policies must track the configurations in force"
        );
    }
}

#[tokio::test]
async fn a_resource_with_no_policy_takes_the_target_value_of_every_update() {
    let emulator = Emulator::new();
    applied_plan(
        &emulator,
        "my-plan",
        vec![table_instruction(vec![predefined(40.0)], Some(true))],
    )
    .await;

    // Nothing tracks this metric at any point, so each update in turn sets the value —
    // the rule is decided on the policies in force, not on how many updates have run.
    for target in [70.0, 90.0, 10.0] {
        let requested = table_instruction(vec![predefined(target)], Some(true));
        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ScalingPlanVersion": 1,
                    "ScalingInstructions": [requested.clone()],
                }),
            )
            .await;
        emulator.make_all_due();

        let plan = described(&emulator, "my-plan").await;
        assert_eq!(
            plan.get("ScalingInstructions"),
            Some(&Value::Array(vec![requested])),
            "case target {target:?}: an unpolicied metric takes the update's target value, \
             got {plan:?}"
        );
    }

    // Once an update re-enables dynamic scaling, a policy tracks the value it set, and
    // the next update no longer re-points it.
    emulator
        .ok(
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": "my-plan",
                "ScalingPlanVersion": 1,
                "ScalingInstructions": [table_instruction(vec![predefined(55.0)], Some(false))],
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
        .ok(
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": "my-plan",
                "ScalingPlanVersion": 1,
                "ScalingInstructions": [table_instruction(vec![predefined(80.0)], Some(false))],
            }),
        )
        .await;
    emulator.make_all_due();

    let plan = described(&emulator, "my-plan").await;
    assert_eq!(
        plan.get("ScalingInstructions"),
        Some(&Value::Array(vec![table_instruction(
            vec![predefined(55.0)],
            Some(false)
        )])),
        "case re-enabled: the value the applied policy tracks to is kept, got {plan:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! What the two describe envelopes report BESIDE the identity of a plan and its
//! resources — each over a TABLE of inputs rather than the one request that exposed it.
//!
//! Three rules are pinned here:
//!
//! * every status a describe reports comes with the narrative that explains it:
//!   `ScalingPlan$StatusMessage` is "A simple message about the current status of the
//!   scaling plan" and `ScalingPlanResource$ScalingStatusMessage` "A simple message
//!   about the current scaling status of the resource", and both status-code docs tell
//!   a caller to "Check the status message for more information" — so the message is a
//!   function of the status, on every described plan and resource, never absent;
//! * "AWS Auto Scaling creates target tracking scaling policies based on the scaling
//!   instructions" (`ScalingInstruction` doc), and `DisableDynamicScaling` "Controls
//!   whether dynamic scaling by AWS Auto Scaling is disabled. When dynamic scaling is
//!   enabled, AWS Auto Scaling creates target tracking scaling policies based on the
//!   specified target tracking configurations" — so an applied resource reports one
//!   `TargetTrackingScaling` policy per target tracking configuration of its own
//!   instruction, echoing that configuration, and an instruction that turned dynamic
//!   scaling off reports none;
//! * an update does not re-point a target tracking policy a resource already has: the
//!   target value in force for a metric the resource already tracks is kept — not even
//!   `ScalingPolicyUpdateBehavior` (`ReplaceExternalPolicies`, which deletes "any
//!   scaling policies that are external to AWS Auto Scaling") re-points one the plan
//!   itself applied — while everything else the update carries, including the rest of
//!   that metric's configuration, is applied.
//!
//! The harness (router, dispatch, lifecycle clock) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use crate::scaling_plan_lifecycle::{instruction, stack_source, Emulator};

/// The one `PolicyType` the model lists
/// (`smithy_query(query="shape",shape="PolicyType")`), mirrored in
/// `src/ops/types/shared_types.rs` as `POLICY_TYPE_VALUES`.
const TARGET_TRACKING_SCALING: &str = "TargetTrackingScaling";

/// The documented upper bound of `ScalingPolicy$PolicyName`
/// (`smithy_query(query="shape",shape="ScalingPolicy")`: len 1..256).
const MAX_POLICY_NAME: usize = 256;

/// A DynamoDB table instruction with `target` tracked on `metric`.
fn table_instruction(metric: &str, target: f64) -> Value {
    instruction(
        "dynamodb",
        "table/my-table",
        "dynamodb:table:ReadCapacityUnits",
        1,
        100,
        metric,
        target,
    )
}

/// Create `name` over a distinct application source and let the clock apply it, so the
/// plan is `Active` and its policies are on its resources.
///
/// `pub(crate)` so a suite that needs an APPLIED plan starts from this one rather than
/// building a second create-and-settle helper beside it.
pub(crate) async fn applied_plan(emulator: &Emulator, name: &str, instructions: Vec<Value>) {
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": name,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": instructions,
            }),
        )
        .await;
    emulator.make_all_due();
}

/// The resources of `name`, as `DescribeScalingPlanResources` reports them.
///
/// `pub(crate)` for the same reason: one reader of the resource envelope.
pub(crate) async fn resources(emulator: &Emulator, name: &str) -> Vec<Value> {
    let body = emulator
        .ok(
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": name, "ScalingPlanVersion": 1 }),
        )
        .await;
    body["ScalingPlanResources"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("no ScalingPlanResources in {body:?}"))
}

/// The one plan `name`, as `DescribeScalingPlans` reports it.
///
/// `pub(crate)` for the same reason: one reader of the plan envelope.
pub(crate) async fn described(emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ScalingPlanNames": [name] }),
        )
        .await;
    body["ScalingPlans"]
        .as_array()
        .and_then(|plans| plans.first().cloned())
        .unwrap_or_else(|| panic!("no plan named {name:?} in {body:?}"))
}

#[tokio::test]
async fn every_described_status_carries_the_narrative_that_explains_it() {
    let emulator = Emulator::new();
    applied_plan(&emulator, "my-plan", vec![table_instruction("DynamoDBReadCapacityUtilization", 50.0)])
        .await;

    // Each row: the call that leaves the plan in a transient status, then the status
    // pair (plan, resource) and the narrative pair reported while it is held open, and
    // the pair reported once the clock has applied the transition.
    for (label, operation, status, message, scaling_status, scaling_message) in [
        (
            "while being updated",
            "UpdateScalingPlan",
            "UpdateInProgress",
            "Scaling plan is being updated.",
            "PartiallyActive",
            "Target tracking scaling policies are being updated on the resource.",
        ),
        (
            "while being deleted",
            "DeleteScalingPlan",
            "DeletionInProgress",
            "Scaling plan is being deleted.",
            "PartiallyActive",
            "Target tracking scaling policies are being removed from the resource.",
        ),
    ] {
        emulator
            .ok(
                operation,
                json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 }),
            )
            .await;
        emulator.hold_pending("my-plan");

        let plan = described(&emulator, "my-plan").await;
        assert_eq!(
            (plan.get("StatusCode"), plan.get("StatusMessage")),
            (Some(&json!(status)), Some(&json!(message))),
            "case {label}: expected StatusCode {status} with its narrative, got {plan:?}"
        );
        let resource = resources(&emulator, "my-plan").await;
        let resource = &resource[0];
        assert_eq!(
            (
                resource.get("ScalingStatusCode"),
                resource.get("ScalingStatusMessage")
            ),
            (Some(&json!(scaling_status)), Some(&json!(scaling_message))),
            "case {label}: expected ScalingStatusCode {scaling_status} with its \
             narrative, got {resource:?}"
        );

        emulator.make_all_due();
        if operation == "UpdateScalingPlan" {
            // The settled plan reports the applied narrative on both envelopes.
            let plan = described(&emulator, "my-plan").await;
            assert_eq!(
                (plan.get("StatusCode"), plan.get("StatusMessage")),
                (
                    Some(&json!("Active")),
                    Some(&json!(
                        "Scaling plan has been created and applied to all resources."
                    ))
                ),
                "case {label}: an applied plan reports the applied narrative, got {plan:?}"
            );
            let resource = resources(&emulator, "my-plan").await;
            assert_eq!(
                (
                    resource[0].get("ScalingStatusCode"),
                    resource[0].get("ScalingStatusMessage")
                ),
                (
                    Some(&json!("Active")),
                    Some(&json!(
                        "Target tracking scaling policies have been applied to the resource."
                    ))
                ),
                "case {label}: an applied resource reports the applied narrative, got \
                 {resource:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_plan_still_being_created_reports_why_no_policy_is_applied_yet() {
    // The one status a create leaves behind: nothing has reached the resource yet, so
    // the resource reports `Inactive`, says so, and carries NO policy.
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "my-plan",
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": [table_instruction("DynamoDBReadCapacityUtilization", 50.0)],
            }),
        )
        .await;
    emulator.hold_pending("my-plan");

    let plan = described(&emulator, "my-plan").await;
    assert_eq!(
        (plan.get("StatusCode"), plan.get("StatusMessage")),
        (
            Some(&json!("CreationInProgress")),
            Some(&json!("Scaling plan is being created."))
        ),
        "a plan being created reports its own narrative, got {plan:?}"
    );
    let resource = resources(&emulator, "my-plan").await;
    let resource = &resource[0];
    assert_eq!(
        (
            resource.get("ScalingStatusCode"),
            resource.get("ScalingStatusMessage")
        ),
        (
            Some(&json!("Inactive")),
            Some(&json!(
                "Target tracking scaling policies have not been applied to the resource \
                 yet because the scaling plan is being created."
            ))
        ),
        "a resource of a plan being created reports why it is Inactive, got {resource:?}"
    );
    assert_eq!(
        resource.get("ScalingPolicies"),
        None,
        "no policy is applied before the plan is: got {resource:?}"
    );
}

/// A plan whose resources the policy table drives: the label, the instructions, and the
/// target tracking configurations each instruction's resource must report policies for.
struct PolicyCase {
    label: &'static str,
    instructions: Vec<Value>,
}

/// The table the policy test spans. Rows differ along what the rule generalises over:
/// the NUMBER of target tracking configurations on one instruction, the FORM of the
/// metric each tracks (predefined vs customized), the NUMBER of resources the plan
/// covers, and whether the instruction configures dynamic scaling at all.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            label: "one predefined configuration",
            instructions: vec![table_instruction("DynamoDBReadCapacityUtilization", 50.0)],
        },
        PolicyCase {
            label: "two configurations on one resource",
            instructions: vec![json!({
                "ServiceNamespace": "ecs",
                "ResourceId": "service/default/sample-webapp",
                "ScalableDimension": "ecs:service:DesiredCount",
                "MinCapacity": 2,
                "MaxCapacity": 20,
                "TargetTrackingConfigurations": [
                    {
                        "PredefinedScalingMetricSpecification": {
                            "PredefinedScalingMetricType": "ECSServiceAverageCPUUtilization"
                        },
                        "TargetValue": 55.5,
                        "DisableScaleIn": true,
                        "ScaleOutCooldown": 60,
                        "ScaleInCooldown": 300
                    },
                    {
                        "PredefinedScalingMetricSpecification": {
                            "PredefinedScalingMetricType": "ECSServiceAverageMemoryUtilization"
                        },
                        "TargetValue": 70.0
                    }
                ]
            })],
        },
        PolicyCase {
            label: "a customized metric configuration",
            instructions: vec![json!({
                "ServiceNamespace": "rds",
                "ResourceId": "cluster:my-db-cluster",
                "ScalableDimension": "rds:cluster:ReadReplicaCount",
                "MinCapacity": 1,
                "MaxCapacity": 15,
                "TargetTrackingConfigurations": [{
                    "CustomizedScalingMetricSpecification": {
                        "MetricName": "queue-depth",
                        "Namespace": "MyApp",
                        "Statistic": "Sum",
                        "Dimensions": [{ "Name": "Queue", "Value": "orders" }],
                        "Unit": "Count"
                    },
                    "TargetValue": 25.0,
                    "EstimatedInstanceWarmup": 120
                }]
            })],
        },
        PolicyCase {
            label: "two resources in one plan",
            instructions: vec![
                table_instruction("DynamoDBReadCapacityUtilization", 50.0),
                instruction(
                    "dynamodb",
                    "table/my-table/index/my-index",
                    "dynamodb:index:WriteCapacityUnits",
                    5,
                    500,
                    "DynamoDBWriteCapacityUtilization",
                    65.0,
                ),
            ],
        },
        PolicyCase {
            label: "dynamic scaling disabled",
            instructions: vec![json!({
                "ServiceNamespace": "autoscaling",
                "ResourceId": "autoScalingGroup/my-asg",
                "ScalableDimension": "autoscaling:autoScalingGroup:DesiredCapacity",
                "MinCapacity": 1,
                "MaxCapacity": 10,
                "TargetTrackingConfigurations": [{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "ASGAverageCPUUtilization"
                    },
                    "TargetValue": 40.0
                }],
                "DisableDynamicScaling": true
            })],
        },
    ]
}

#[tokio::test]
async fn an_applied_resource_reports_one_policy_per_target_tracking_configuration() {
    for case in policy_cases() {
        let emulator = Emulator::new();
        let label = case.label;
        applied_plan(&emulator, "my-plan", case.instructions.clone()).await;

        let reported = resources(&emulator, "my-plan").await;
        assert_eq!(
            reported.len(),
            case.instructions.len(),
            "case {label:?}: one resource per instruction, got {reported:?}"
        );

        let mut names: Vec<String> = Vec::new();
        for (resource, source) in reported.iter().zip(case.instructions.iter()) {
            let configurations = source["TargetTrackingConfigurations"]
                .as_array()
                .expect("the case's configurations");
            // An instruction that disabled dynamic scaling mints no policy at all,
            // so the member is absent rather than an empty list.
            let disabled = source["DisableDynamicScaling"] == json!(true);
            if disabled {
                assert_eq!(
                    resource.get("ScalingPolicies"),
                    None,
                    "case {label:?}: dynamic scaling is off, so no policy is applied, \
                     got {resource:?}"
                );
                continue;
            }
            let policies = resource["ScalingPolicies"]
                .as_array()
                .unwrap_or_else(|| panic!("case {label:?}: no ScalingPolicies in {resource:?}"));
            assert_eq!(
                policies.len(),
                configurations.len(),
                "case {label:?}: one policy per target tracking configuration, got \
                 {policies:?}"
            );
            for (policy, configuration) in policies.iter().zip(configurations.iter()) {
                assert_eq!(
                    policy.get("PolicyType"),
                    Some(&json!(TARGET_TRACKING_SCALING)),
                    "case {label:?}: a scaling plan mints target tracking policies, got \
                     {policy:?}"
                );
                assert_eq!(
                    policy.get("TargetTrackingConfiguration"),
                    Some(configuration),
                    "case {label:?}: the policy must echo its configuration, got {policy:?}"
                );
                let name = policy["PolicyName"].as_str().unwrap_or_default();
                assert!(
                    !name.is_empty()
                        && name.len() <= MAX_POLICY_NAME
                        && !name.chars().any(char::is_control),
                    "case {label:?}: PolicyName must be 1-{MAX_POLICY_NAME} printable \
                     characters, got {name:?}"
                );
                names.push(name.to_string());
            }
        }

        let unique: std::collections::BTreeSet<&String> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "case {label:?}: every policy of a plan needs its own name, got {names:?}"
        );

        // A read does not re-mint: the second read reports the same names.
        let again = resources(&emulator, "my-plan").await;
        assert_eq!(
            again, reported,
            "case {label:?}: two reads of the same state must report the same policies"
        );
    }
}

/// One row of the update-merge table: the instructions in force, the instructions the
/// update carries and what the plan reports afterwards.
struct MergeCase {
    label: &'static str,
    in_force: Vec<Value>,
    requested: Vec<Value>,
    expected: Vec<Value>,
}

/// The table the merge test spans. Rows differ along what the rule generalises over:
/// which metric the update's configuration names (one already tracked vs a new one),
/// which members of a tracked metric's configuration the update changes (the target
/// value vs the cooldowns and the scale-in flag), whether the instruction asks for the
/// resource's policies to be replaced, the FORM of the metric (predefined vs
/// customized), and whether the resource is one the plan already covers.
fn merge_cases() -> Vec<MergeCase> {
    let tracked = table_instruction("DynamoDBReadCapacityUtilization", 50.0);
    vec![
        MergeCase {
            label: "a new target value for a metric already tracked",
            in_force: vec![tracked.clone()],
            // The capacity bounds move; the target value of the tracked metric does not.
            requested: vec![instruction(
                "dynamodb",
                "table/my-table",
                "dynamodb:table:ReadCapacityUnits",
                4,
                400,
                "DynamoDBReadCapacityUtilization",
                75.0,
            )],
            expected: vec![instruction(
                "dynamodb",
                "table/my-table",
                "dynamodb:table:ReadCapacityUnits",
                4,
                400,
                "DynamoDBReadCapacityUtilization",
                50.0,
            )],
        },
        MergeCase {
            label: "the same new target value, asking for the policies to be replaced",
            in_force: vec![tracked.clone()],
            requested: vec![json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/my-table",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "MinCapacity": 1,
                "MaxCapacity": 100,
                "TargetTrackingConfigurations": [{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "DynamoDBReadCapacityUtilization"
                    },
                    "TargetValue": 75.0
                }],
                "ScalingPolicyUpdateBehavior": "ReplaceExternalPolicies"
            })],
            // `ReplaceExternalPolicies` deletes the policies that are EXTERNAL to AWS
            // Auto Scaling; the policy this plan applied itself is not one of them, so
            // the configuration in force is kept here too.
            expected: vec![json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/my-table",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "MinCapacity": 1,
                "MaxCapacity": 100,
                "TargetTrackingConfigurations": [{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "DynamoDBReadCapacityUtilization"
                    },
                    "TargetValue": 50.0
                }],
                "ScalingPolicyUpdateBehavior": "ReplaceExternalPolicies"
            })],
        },
        MergeCase {
            label: "cooldowns and the scale-in flag on a metric already tracked",
            in_force: vec![tracked.clone()],
            // Only the target value an applied policy tracks to is kept; every other
            // member of the configuration is the update's to set.
            requested: vec![json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/my-table",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "MinCapacity": 1,
                "MaxCapacity": 100,
                "TargetTrackingConfigurations": [{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "DynamoDBReadCapacityUtilization"
                    },
                    "TargetValue": 75.0,
                    "DisableScaleIn": true,
                    "ScaleOutCooldown": 90,
                    "ScaleInCooldown": 600
                }]
            })],
            expected: vec![json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/my-table",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "MinCapacity": 1,
                "MaxCapacity": 100,
                "TargetTrackingConfigurations": [{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "DynamoDBReadCapacityUtilization"
                    },
                    "TargetValue": 50.0,
                    "DisableScaleIn": true,
                    "ScaleOutCooldown": 90,
                    "ScaleInCooldown": 600
                }]
            })],
        },
        MergeCase {
            label: "a configuration for a metric the resource does not track yet",
            in_force: vec![tracked.clone()],
            requested: vec![json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/my-table",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "MinCapacity": 1,
                "MaxCapacity": 100,
                "TargetTrackingConfigurations": [
                    {
                        "PredefinedScalingMetricSpecification": {
                            "PredefinedScalingMetricType": "DynamoDBReadCapacityUtilization"
                        },
                        "TargetValue": 75.0
                    },
                    {
                        "CustomizedScalingMetricSpecification": {
                            "MetricName": "queue-depth",
                            "Namespace": "MyApp",
                            "Statistic": "Sum"
                        },
                        "TargetValue": 30.0
                    }
                ]
            })],
            expected: vec![json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/my-table",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "MinCapacity": 1,
                "MaxCapacity": 100,
                "TargetTrackingConfigurations": [
                    {
                        "PredefinedScalingMetricSpecification": {
                            "PredefinedScalingMetricType": "DynamoDBReadCapacityUtilization"
                        },
                        "TargetValue": 50.0
                    },
                    {
                        "CustomizedScalingMetricSpecification": {
                            "MetricName": "queue-depth",
                            "Namespace": "MyApp",
                            "Statistic": "Sum"
                        },
                        "TargetValue": 30.0
                    }
                ]
            })],
        },
        MergeCase {
            label: "an instruction for a resource the plan did not cover",
            in_force: vec![tracked.clone()],
            requested: vec![instruction(
                "dynamodb",
                "table/other-table",
                "dynamodb:table:ReadCapacityUnits",
                2,
                200,
                "DynamoDBReadCapacityUtilization",
                75.0,
            )],
            expected: vec![instruction(
                "dynamodb",
                "table/other-table",
                "dynamodb:table:ReadCapacityUnits",
                2,
                200,
                "DynamoDBReadCapacityUtilization",
                75.0,
            )],
        },
        MergeCase {
            label: "a new target value for a customized metric already tracked",
            in_force: vec![json!({
                "ServiceNamespace": "rds",
                "ResourceId": "cluster:my-db-cluster",
                "ScalableDimension": "rds:cluster:ReadReplicaCount",
                "MinCapacity": 1,
                "MaxCapacity": 15,
                "TargetTrackingConfigurations": [{
                    "CustomizedScalingMetricSpecification": {
                        "MetricName": "queue-depth",
                        "Namespace": "MyApp",
                        "Statistic": "Sum",
                        "Dimensions": [{ "Name": "Queue", "Value": "orders" }]
                    },
                    "TargetValue": 25.0
                }]
            })],
            requested: vec![json!({
                "ServiceNamespace": "rds",
                "ResourceId": "cluster:my-db-cluster",
                "ScalableDimension": "rds:cluster:ReadReplicaCount",
                "MinCapacity": 1,
                "MaxCapacity": 15,
                "TargetTrackingConfigurations": [{
                    "CustomizedScalingMetricSpecification": {
                        "MetricName": "queue-depth",
                        "Namespace": "MyApp",
                        "Statistic": "Sum",
                        "Dimensions": [{ "Name": "Queue", "Value": "orders" }]
                    },
                    "TargetValue": 60.0
                }]
            })],
            expected: vec![json!({
                "ServiceNamespace": "rds",
                "ResourceId": "cluster:my-db-cluster",
                "ScalableDimension": "rds:cluster:ReadReplicaCount",
                "MinCapacity": 1,
                "MaxCapacity": 15,
                "TargetTrackingConfigurations": [{
                    "CustomizedScalingMetricSpecification": {
                        "MetricName": "queue-depth",
                        "Namespace": "MyApp",
                        "Statistic": "Sum",
                        "Dimensions": [{ "Name": "Queue", "Value": "orders" }]
                    },
                    "TargetValue": 25.0
                }]
            })],
        },
    ]
}

#[tokio::test]
async fn an_update_keeps_the_configuration_in_force_for_a_metric_already_tracked() {
    for case in merge_cases() {
        let emulator = Emulator::new();
        let label = case.label;
        applied_plan(&emulator, "my-plan", case.in_force.clone()).await;

        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ScalingPlanVersion": 1,
                    "ScalingInstructions": case.requested,
                }),
            )
            .await;
        emulator.make_all_due();

        let plan = described(&emulator, "my-plan").await;
        assert_eq!(
            plan.get("ScalingInstructions"),
            Some(&Value::Array(case.expected.clone())),
            "case {label:?}: wrong ScalingInstructions in force after the update, got \
             {plan:?}"
        );

        // The policies on the resource follow the instructions in force, so the same
        // rule is observable through the resource envelope.
        let reported = resources(&emulator, "my-plan").await;
        let configurations: Vec<&Value> = case
            .expected
            .iter()
            .flat_map(|instruction| {
                instruction["TargetTrackingConfigurations"]
                    .as_array()
                    .expect("the case's configurations")
                    .iter()
            })
            .collect();
        let applied: Vec<&Value> = reported
            .iter()
            .flat_map(|resource| {
                resource["ScalingPolicies"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {label:?}: no policies in {resource:?}"))
                    .iter()
            })
            .map(|policy| &policy["TargetTrackingConfiguration"])
            .collect();
        assert_eq!(
            applied, configurations,
            "case {label:?}: the applied policies must echo the configurations in force"
        );
    }
}

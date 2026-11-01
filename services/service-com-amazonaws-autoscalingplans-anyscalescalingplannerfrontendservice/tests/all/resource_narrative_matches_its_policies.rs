// SPDX-License-Identifier: CC0-1.0

//! One resource's report has to agree with itself: the narrative a
//! `ScalingPlanResource` carries is the narrative of THAT resource.
//!
//! `ScalingPlanResource$ScalingStatusMessage` is "A simple message about the current
//! scaling status of the resource"
//! (`smithy_query(query="doc",shape="ScalingPlanResource",member="ScalingStatusMessage")`),
//! and `DisableDynamicScaling` "Controls whether dynamic scaling by AWS Auto Scaling is
//! disabled. When dynamic scaling is enabled, AWS Auto Scaling creates target tracking
//! scaling policies based on the specified target tracking configurations" — so the
//! narrative of a resource whose instruction turned dynamic scaling off never says that
//! target tracking scaling policies have been applied to, are being updated on, or are
//! being removed from it, because no such policy ever reaches it; what it reports is
//! what the plan did to that resource's scaling configuration, the subject the
//! `ScalingStatusCode` doc itself uses.
//!
//! The table below spans what the rule generalises over: whether the instruction
//! disables dynamic scaling (off, explicitly on, omitted), the NUMBER of target
//! tracking configurations it carries, the NUMBER of resources the plan covers
//! (including a plan whose resources disagree about dynamic scaling), the namespace and
//! scalable dimension each names — crossed with EVERY status a plan passes through, so
//! the transient narratives are pinned beside the settled one.
//!
//! The harness (router, dispatch, lifecycle clock) is reused from
//! [`crate::scaling_plan_lifecycle`], and the two describe readers from
//! [`crate::applied_policies_and_status_narratives`]; nothing here builds a second one.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::applied_policies_and_status_narratives::{described, resources};
use crate::scaling_plan_lifecycle::{
    instruction, resource_identity, stack_source, Emulator, ResourceIdentity,
};

/// The `ScalingStatusCode` of a resource nothing has been applied to yet
/// (`smithy_query(query="doc",shape="ScalingPlanResource",member="ScalingStatusCode")`:
/// "`Inactive` - The scaling configuration is not active because the scaling plan is
/// being created").
const INACTIVE: &str = "Inactive";

/// What a narrative claims about policies that are ON the resource. A resource reporting
/// no `ScalingPolicies` can make none of these claims, whatever the plan's status —
/// which is the whole rule this suite pins, asserted here independently of the exact
/// sentence each status reports.
const POLICY_CLAIMS: &[&str] = &[
    "Target tracking scaling policies have been applied to",
    "Target tracking scaling policies are being updated on",
    "Target tracking scaling policies are being removed from",
];

/// What a plan in one status reports, on both describe envelopes.
struct Reported {
    /// `ScalingPlan$StatusCode`.
    status_code: &'static str,
    /// `ScalingPlan$StatusMessage` — about the PLAN, so one per status.
    status_message: &'static str,
    /// The `ScalingStatusCode` every resource of such a plan reports.
    scaling_status_code: &'static str,
    /// `ScalingStatusMessage` of a resource whose instruction configures dynamic
    /// scaling, so target tracking scaling policies are what the plan works on.
    with_policies: &'static str,
    /// `ScalingStatusMessage` of a resource whose instruction turned dynamic scaling
    /// off, so the plan has no policy of that resource to work on.
    without_policies: &'static str,
}

/// Every status this service puts a plan in, in the order one plan passes through them.
fn reported_statuses() -> Vec<Reported> {
    vec![
        Reported {
            status_code: "CreationInProgress",
            status_message: "Scaling plan is being created.",
            scaling_status_code: INACTIVE,
            with_policies: "Target tracking scaling policies have not been applied to the \
                            resource yet because the scaling plan is being created.",
            without_policies: "The scaling configuration has not been applied to the resource \
                               yet because the scaling plan is being created.",
        },
        Reported {
            status_code: "Active",
            status_message: "Scaling plan has been created and applied to all resources.",
            scaling_status_code: "Active",
            with_policies: "Target tracking scaling policies have been applied to the resource.",
            without_policies: "The scaling configuration has been applied to the resource, which \
                               has no target tracking scaling policies because dynamic scaling \
                               is disabled.",
        },
        Reported {
            status_code: "UpdateInProgress",
            status_message: "Scaling plan is being updated.",
            scaling_status_code: "PartiallyActive",
            with_policies: "Target tracking scaling policies are being updated on the resource.",
            without_policies: "The scaling configuration is being updated on the resource.",
        },
        Reported {
            status_code: "DeletionInProgress",
            status_message: "Scaling plan is being deleted.",
            scaling_status_code: "PartiallyActive",
            with_policies: "Target tracking scaling policies are being removed from the resource.",
            without_policies: "The scaling configuration is being removed from the resource.",
        },
    ]
}

/// One plan the table drives: its label and the instructions it is created with.
struct Case {
    label: &'static str,
    instructions: Vec<Value>,
}

/// `source`, with `DisableDynamicScaling` set — the one member that decides whether
/// target tracking scaling policies reach the resource the instruction applies to.
fn dynamic_scaling_disabled(mut source: Value, disabled: bool) -> Value {
    source["DisableDynamicScaling"] = json!(disabled);
    source
}

/// A DynamoDB table instruction tracking one predefined metric.
fn table_instruction() -> Value {
    instruction(
        "dynamodb",
        "table/my-table",
        "dynamodb:table:ReadCapacityUnits",
        1,
        100,
        "DynamoDBReadCapacityUtilization",
        50.0,
    )
}

/// An Auto Scaling group instruction tracking one predefined metric.
fn group_instruction() -> Value {
    instruction(
        "autoscaling",
        "autoScalingGroup/my-asg",
        "autoscaling:autoScalingGroup:DesiredCapacity",
        1,
        10,
        "ASGAverageCPUUtilization",
        40.0,
    )
}

/// The table this suite spans.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // `DisableDynamicScaling` omitted: "The default is enabled (false)".
            label: "dynamic scaling omitted on one resource",
            instructions: vec![table_instruction()],
        },
        Case {
            label: "dynamic scaling off on one resource",
            instructions: vec![dynamic_scaling_disabled(group_instruction(), true)],
        },
        Case {
            label: "dynamic scaling explicitly on, two configurations",
            instructions: vec![dynamic_scaling_disabled(
                json!({
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
                            "TargetValue": 55.5
                        },
                        {
                            "CustomizedScalingMetricSpecification": {
                                "MetricName": "queue-depth",
                                "Namespace": "MyApp",
                                "Statistic": "Sum",
                                "Unit": "Count"
                            },
                            "TargetValue": 25.0
                        }
                    ]
                }),
                false,
            )],
        },
        Case {
            // The narrative is per RESOURCE, so one plan can report both of them.
            label: "one resource off and one on in the same plan",
            instructions: vec![
                table_instruction(),
                dynamic_scaling_disabled(group_instruction(), true),
            ],
        },
        Case {
            label: "dynamic scaling off on two resources",
            instructions: vec![
                dynamic_scaling_disabled(table_instruction(), true),
                dynamic_scaling_disabled(
                    instruction(
                        "rds",
                        "cluster:my-db-cluster",
                        "rds:cluster:ReadReplicaCount",
                        1,
                        15,
                        "RDSReaderAverageCPUUtilization",
                        60.0,
                    ),
                    true,
                ),
            ],
        },
    ]
}

/// Assert what `case`'s plan and each of its resources report while the plan is in
/// `reported`'s status.
async fn assert_report(emulator: &Emulator, name: &str, case: &Case, reported: &Reported) {
    let label = case.label;
    let status = reported.status_code;

    let plan = described(emulator, name).await;
    assert_eq!(
        (plan.get("StatusCode"), plan.get("StatusMessage")),
        (
            Some(&json!(reported.status_code)),
            Some(&json!(reported.status_message))
        ),
        "case {label:?} in {status}: the plan reports its status with the narrative that \
         explains it, got {plan:?}"
    );

    let by_identity: BTreeMap<ResourceIdentity, &Value> = case
        .instructions
        .iter()
        .map(|source| (resource_identity(source), source))
        .collect();
    let reported_resources = resources(emulator, name).await;
    assert_eq!(
        reported_resources.len(),
        case.instructions.len(),
        "case {label:?} in {status}: one resource per instruction, got \
         {reported_resources:?}"
    );

    for resource in &reported_resources {
        let identity = resource_identity(resource);
        let source = by_identity.get(&identity).unwrap_or_else(|| {
            panic!(
                "case {label:?} in {status}: {identity:?} is no resource of this plan, got \
                 {resource:?}"
            )
        });
        let disabled = source["DisableDynamicScaling"] == json!(true);
        let expected_message = if disabled {
            reported.without_policies
        } else {
            reported.with_policies
        };
        assert_eq!(
            resource.get("ScalingStatusCode"),
            Some(&json!(reported.scaling_status_code)),
            "case {label:?} / {identity:?} in {status}: expected ScalingStatusCode {}, got \
             {resource:?}",
            reported.scaling_status_code
        );
        assert_eq!(
            resource.get("ScalingStatusMessage"),
            Some(&json!(expected_message)),
            "case {label:?} / {identity:?} in {status}: expected the narrative \
             {expected_message:?}, got {resource:?}"
        );

        // The policies the same read reports: present once the plan's configuration has
        // reached the resource, and only for an instruction that configures dynamic
        // scaling at all.
        let policies = resource.get("ScalingPolicies").and_then(Value::as_array);
        let expect_policies = !disabled && reported.scaling_status_code != INACTIVE;
        assert_eq!(
            policies.is_some(),
            expect_policies,
            "case {label:?} / {identity:?} in {status}: expected ScalingPolicies present to \
             be {expect_policies}, got {resource:?}"
        );
        if let Some(policies) = policies {
            assert_eq!(
                policies.len(),
                source["TargetTrackingConfigurations"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default(),
                "case {label:?} / {identity:?} in {status}: one policy per target tracking \
                 configuration, got {policies:?}"
            );
        } else {
            let message = resource["ScalingStatusMessage"].as_str().unwrap_or_default();
            for claim in POLICY_CLAIMS {
                assert!(
                    !message.contains(claim),
                    "case {label:?} / {identity:?} in {status}: a resource reporting no \
                     policy cannot be told {claim:?}, got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_resource_narrative_never_claims_a_policy_the_same_read_denies() {
    for case in cases() {
        let emulator = Emulator::new();
        let name = "my-plan";
        let version = json!({ "ScalingPlanName": name, "ScalingPlanVersion": 1 });
        let statuses = reported_statuses();

        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": case.instructions.clone(),
                }),
            )
            .await;
        // Hold each transition open rather than racing the real span, so the transient
        // narratives are observed deterministically.
        emulator.hold_pending(name);
        assert_report(&emulator, name, &case, &statuses[0]).await;

        emulator.make_all_due();
        assert_report(&emulator, name, &case, &statuses[1]).await;

        emulator.ok("UpdateScalingPlan", version.clone()).await;
        emulator.hold_pending(name);
        assert_report(&emulator, name, &case, &statuses[2]).await;

        emulator.make_all_due();
        assert_report(&emulator, name, &case, &statuses[1]).await;

        emulator.ok("DeleteScalingPlan", version.clone()).await;
        emulator.hold_pending(name);
        assert_report(&emulator, name, &case, &statuses[3]).await;
    }
}

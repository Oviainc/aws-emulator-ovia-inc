Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# Report the target-tracking ScalingPolicies of each scaling plan resource

_Filed: 2026-10-24T06:54:30Z_
_Operation: DescribeScalingPlanResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeScalingPlanResources returns every `ScalingPlanResource` with `ScalingPolicies` unset (serialized as absent), so a caller never sees the scaling policies AWS Auto Scaling materialises for a scaling plan.

Expected (real service): for each scalable resource covered by a plan, the response carries a `ScalingPolicies` list with one `ScalingPolicy` per `TargetTrackingConfiguration` in the plan's corresponding `ScalingInstruction` — `PolicyType` = `TargetTrackingScaling`, a non-empty `PolicyName` derived from the tracked metric/resource, and `TargetTrackingConfiguration` echoing the stored configuration (predefined or customized metric specification, target value, cooldowns, disable-scale-in). This is the documented purpose of a scaling plan: the service creates target tracking scaling policies on the resources it manages, and `DescribeScalingPlanResources` is how a client inspects them.

Actual (emulator): `scaling_policies: None` is hardcoded in the projection, with a comment that "No scaling policies are materialised by this emulator". Since `crate::state::validate_target_tracking_configurations` REQUIRES at least one target tracking configuration per stored instruction, the data needed to project the policies is always present in state — it is simply not projected.

## Evidence

Smithy model: `ScalingPlanResource$ScalingPolicies` (list<ScalingPolicy>) with `ScalingPolicy{PolicyName (required), PolicyType (required, TargetTrackingScaling), TargetTrackingConfiguration}` — smithy_query(query="shape",shape="ScalingPlanResource,ScalingPolicy"). Handler: services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/describe_scaling_plan_resources.rs lines ~190-203 set `scaling_policies: None` unconditionally. State already holds the configurations (src/state.rs `ScalingPlanRecord.scaling_instructions`, and `validate_target_tracking_configurations` rejects an empty list). Test coverage: tests/all/scaling_plan_lifecycle.rs asserts ScalingPlanName/Version/ServiceNamespace/ResourceId/ScalableDimension/ScalingStatusCode per resource; no test mentions `ScalingPolicies` at all, so the omission is unasserted either way.

## Steps to Reproduce

1. CreateScalingPlan with one ScalingInstruction for `autoscaling:autoScalingGroup:DesiredCapacity`, ResourceId `autoScalingGroup/my-asg`, and one TargetTrackingConfiguration {PredefinedScalingMetricSpecification:{PredefinedScalingMetricType:"ASGAverageCPUUtilization"}, TargetValue: 40}.
2. Wait for the plan to report StatusCode `Active` (DescribeScalingPlans).
3. Call DescribeScalingPlanResources {"ScalingPlanName":"my-plan","ScalingPlanVersion":1}.
4. Observe the single ScalingPlanResource has no `ScalingPolicies` member; the real service returns one TargetTrackingScaling policy carrying the configuration supplied at create time.

## Resolution

The code base drifted since the commit this report names: the projection no longer hardcodes
`scaling_policies: None`. An applied resource reports one `TargetTrackingScaling` policy per
target tracking configuration of its own scaling instruction, projected from the stored plan
(`src/state.rs`, `ScalingPlanRecord::applied_policies`, read by
`src/ops/describe_scaling_plan_resources.rs`), with the configuration echoed and a name that is
unique within the plan and stable across reads.

The rule is now pinned over the WHOLE catalog of scalable resources a plan can cover — one case per
`ScalableDimension` the model lists, in both predefined and customized metric form, with one and
with several configurations on one resource — and over a paged read, so a resource reports its own
policies whichever page it arrives on (`tests/all/scaling_plan_resource_policies.rs`). The number
and form of the configurations on one resource, and the resource of an instruction that disabled
dynamic scaling, stay covered by `tests/all/applied_policies_and_status_narratives.rs`.

`ScalingPolicies[*].PolicyName` remains a service-generated name: the model only requires 1-256
printable characters, so the tests assert that bound, uniqueness within the plan and stability
across reads rather than any one literal.

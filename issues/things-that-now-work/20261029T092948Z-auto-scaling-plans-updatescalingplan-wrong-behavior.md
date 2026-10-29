Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0184958fe7a8a2f43d9a0a5d46b055181f8d4624

# UpdateScalingPlan keeps the in-force TargetValue even for a resource that has no applied policy (DisableDynamicScaling)

_Filed: 2026-10-29T09:29:48Z_
_Operation: UpdateScalingPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0184958fe7a8a2f43d9a0a5d46b055181f8d4624`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateScalingPlan` merges the requested `ScalingInstructions` with the ones in force through `crate::state::merge_scaling_instructions`, which keeps the stored `TargetValue` for any scaling metric the resource already tracks. The crate states the reason for that rule explicitly: the kept value is the one "the applied policy tracks to", i.e. an update must not re-point a target tracking policy the resource already has.

But `keep_tracked_target_values` never asks whether a policy was in fact applied. The crate already owns that question — `ScalingPlanRecord::applied_policies` calls `configures_dynamic_scaling(instruction)` and returns NO policy for an instruction that set `DisableDynamicScaling: true` — and the merge path does not call it. So for a resource whose in-force instruction disabled dynamic scaling (no target tracking policy exists, as the crate's own test asserts), an update that supplies a new `TargetValue` is answered 200, nothing is re-pointed, and `DescribeScalingPlans` keeps reporting the OLD target value. The caller's value is silently discarded with no policy to justify keeping the old one.

This is the one branch where the rule's own stated precondition ("that value is what the applied policy tracks to") is false: the plan is necessarily `Active` at update time (an in-flight plan is refused with `ConcurrentUpdateException`), so the only reachable no-policy case is `DisableDynamicScaling`.

## Evidence

The rule and its reason, in the merge helper's own doc — services/.../src/state.rs:970-980:
```
/// An update does not re-point a target tracking policy a resource already has: for a
/// resource the plan already covers, the TARGET VALUE in force for a scaling metric the
/// resource already tracks is kept, because that value is what the applied policy tracks
/// to.
```
The code under it, which consults nothing about applied policies — services/.../src/state.rs:1016-1035 (assignment at :1030):
```
fn keep_tracked_target_values(
    applied: &ScalingInstruction,
    mut requested: ScalingInstruction,
) -> ScalingInstruction {
    ...
            if let Some(in_force) = in_force {
                configuration.target_value = in_force.target_value;
            }
```
The helper the crate already has for "is there an applied policy?", which the merge path skips — services/.../src/state.rs:297-302 and its doc at :278-283:
```
/// scaling policies based on the specified target tracking configurations" — so an
/// applied resource carries one `TargetTrackingScaling` policy per target tracking
/// configuration of its instruction, echoing that configuration, and an instruction
/// that turned dynamic scaling off carries none.
pub fn applied_policies(&self, instruction: &ScalingInstruction) -> Vec<ScalingPolicy> {
    if self.scaling_status_code() == SCALING_STATUS_INACTIVE
        || !configures_dynamic_scaling(instruction)
    {
        return Vec::new();
    }
```
The handler that reaches the merge — services/.../src/ops/update_scaling_plan.rs:240-243:
```
        if let Some(instructions) = input.scaling_instructions {
            plan.scaling_instructions =
                crate::state::merge_scaling_instructions(&plan.scaling_instructions, instructions);
        }
```
Test coverage: services/.../tests/all/applied_policies_and_status_narratives.rs:356-366 pins that such a resource has no policy at all ("An instruction that disabled dynamic scaling mints no policy at all, so the member is absent rather than an empty list" / `assert_eq!(resource.get("ScalingPolicies"), None, ...)`), and the merge table at the same file :432-655 pins the keep-the-target-value rule ONLY on instructions with dynamic scaling enabled — its own comment at :496-497 states the precondition: "Only the target value an applied policy tracks to is kept; every other member of the configuration is the update's to set." No row of `merge_cases()` carries `DisableDynamicScaling`, so this branch is unpinned.

## Steps to Reproduce

1. CreateScalingPlan {"ScalingPlanName":"p","ApplicationSource":{"CloudFormationStackARN":"arn:aws:cloudformation:us-east-1:000000000000:stack/s/abc"},"ScalingInstructions":[{"ServiceNamespace":"autoscaling","ResourceId":"autoScalingGroup/my-asg","ScalableDimension":"autoscaling:autoScalingGroup:DesiredCapacity","MinCapacity":1,"MaxCapacity":10,"TargetTrackingConfigurations":[{"PredefinedScalingMetricSpecification":{"PredefinedScalingMetricType":"ASGAverageCPUUtilization"},"TargetValue":40.0}],"DisableDynamicScaling":true}]} -> 200
2. Let the clock settle the plan to Active (SETTLE_AFTER), then DescribeScalingPlanResources {"ScalingPlanName":"p","ScalingPlanVersion":1} -> the single resource carries NO ScalingPolicies member (no policy was ever applied)
3. UpdateScalingPlan {"ScalingPlanName":"p","ScalingPlanVersion":1,"ScalingInstructions":[ same instruction but "TargetValue":70.0 ]} -> 200, empty body
4. Let the clock settle again, then DescribeScalingPlans {"ScalingPlanNames":["p"]}
5. Observe ScalingInstructions[0].TargetTrackingConfigurations[0].TargetValue is still 40.0: the new value was dropped to avoid re-pointing an applied policy that, by the crate's own `applied_policies`, does not exist for this resource

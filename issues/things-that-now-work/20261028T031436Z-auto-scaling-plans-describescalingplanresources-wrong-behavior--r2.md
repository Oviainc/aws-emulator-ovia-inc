Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8e0ac146ffee686dd3a46765dec581c593b08a30

# A scaling plan resource's PolicyName is keyed on the instruction's list POSITION, so an update that drops or reorders an instruction renames the policies of resources it never touched

_Filed: 2026-10-28T03:14:36Z_
_Operation: DescribeScalingPlanResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8e0ac146ffee686dd3a46765dec581c593b08a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ScalingPlanResource$ScalingPolicies` names each target-tracking policy with `crate::state::policy_name`, whose own documentation says the name is "derived from the state that identifies the policy — the plan that owns it, the resource the policy was applied to and the configuration within that resource's instruction it came from", and that therefore "the name a resource reports does not change between reads".

The implementation does not use the resource at all: it uses the index of the instruction inside `ScalingPlanRecord::scaling_instructions`. `applied_policies` admits this in the line right above itself ("`resource` is the position of `instruction` in [`Self::scaling_instructions`], which is what names the policies apart"), and `DescribeScalingPlanResources` passes that enumerate() position straight into it.

Because `UpdateScalingPlan` replaces the stored instruction list with the list the request carries (in the request's order), the position of an instruction for an UNCHANGED resource moves whenever the caller drops, inserts or reorders another instruction. The consequence is observable on the very next read of the very same resource:

* a resource whose instruction and target tracking configurations are byte-identical before and after an update reports a different `PolicyName`;
* worse, two resources that swap places swap policy names, so the name `my-plan/TargetTracking/0/0` that used to identify the policy applied to resource A now identifies the policy applied to a DIFFERENT resource B — a client that tracks policies by name silently follows the wrong resource.

The stated invariant ("derived from ... the resource the policy was applied to", "does not change between reads") can only hold if the name is keyed on the resource identity the crate already has — the `(ServiceNamespace, ResourceId, ScalableDimension)` triple it calls `ResourceIdentity` and already uses as the identity of the one resource an instruction applies to — rather than on a list offset. No test pins the index form: the policy tests assert only uniqueness within a plan, the 1–256 printable bound, and stability across two reads of UNCHANGED state.

## Evidence

The rule the crate states for the name (src/state.rs:352-365):
```
/// The service generates this name; `ScalingPolicy$PolicyName` only requires a
/// printable string of 1-256 characters. It is therefore derived from the state that
/// identifies the policy — the plan that owns it, the resource the policy was applied
/// to and the configuration within that resource's instruction it came from — so no two
/// policies of one plan share a name, every name stays inside the modelled 256
/// characters however long a resource id is, and the name a resource reports does not
/// change between reads.
fn policy_name(plan_name: &str, resource: usize, position: usize) -> String {
    format!("{plan_name}/TargetTracking/{resource}/{position}")
}
```

The code under it keys the name on the LIST POSITION instead (src/state.rs:286-287, 293-308):
```
/// `resource` is the position of `instruction` in [`Self::scaling_instructions`],
/// which is what names the policies apart.
...
    pub fn applied_policies(
        &self,
        resource: usize,
        instruction: &ScalingInstruction,
    ) -> Vec<ScalingPolicy> {
...
                policy_name: policy_name(&self.name, resource, position),
```

The read passes the enumerate() index as that `resource` (src/ops/describe_scaling_plan_resources.rs:203-214):
```
                    let page: Vec<ScalingPlanResource> = plan
                        .scaling_instructions
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(page_size)
                        .map(|(resource, instruction)| {
...
                            let policies = plan.applied_policies(resource, instruction);
```

The write that moves those positions (src/ops/update_scaling_plan.rs:240-243):
```
        if let Some(instructions) = input.scaling_instructions {
            plan.scaling_instructions =
                crate::state::merge_scaling_instructions(&plan.scaling_instructions, instructions);
        }
```
and the merge keeps the REQUEST's order (src/state.rs:891-899): `requested.into_iter().map(|instruction| match applied.get(&resource_identity(&instruction)) { ... }).collect()`.

The identity the crate already has for a resource, unused by the name (src/state.rs:930-945):
```
/// "Each scaling instruction applies to one resource" (`ScalingInstruction` doc), and
/// that resource is named by the `(ServiceNamespace, ResourceId, ScalableDimension)`
/// triple
pub type ResourceIdentity<'a> = (&'a str, &'a str, &'a str);
```

Test coverage — nothing pins the index form: tests/all/scaling_plan_resource_policies.rs:269-279 asserts only uniqueness ("every policy of a plan needs its own name") and that "Reading does not re-mint: the same state reports the same policies" (two reads, no update in between), plus the 1-256 printable-character bound at tests/all/scaling_plan_resource_policies.rs:228-233 and tests/all/applied_policies_and_status_narratives.rs:389-394. No test exercises a PolicyName across an UpdateScalingPlan, so keying the name on the resource identity leaves every existing assertion satisfied.

## Steps to Reproduce

1. CreateScalingPlan "my-plan" with two scaling instructions in this order: [A] autoscaling / autoScalingGroup/asg-a / autoscaling:autoScalingGroup:DesiredCapacity, [B] autoscaling / autoScalingGroup/asg-b / same dimension, each with one target tracking configuration; let the clock settle the plan to Active.
2. DescribeScalingPlanResources {"ScalingPlanName":"my-plan","ScalingPlanVersion":1} → resource asg-a reports PolicyName "my-plan/TargetTracking/0/0" and asg-b reports "my-plan/TargetTracking/1/0".
3. UpdateScalingPlan {"ScalingPlanName":"my-plan","ScalingPlanVersion":1,"ScalingInstructions":[B, A]} — the SAME two instructions, only listed in the other order — and let the plan settle back to Active.
4. DescribeScalingPlanResources again → asg-a now reports "my-plan/TargetTracking/1/0" and asg-b reports "my-plan/TargetTracking/0/0": the policy name of a resource whose instruction never changed has changed, and "my-plan/TargetTracking/0/0" now names a policy on a different resource — contradicting src/state.rs:356-361 ("derived from ... the resource the policy was applied to" / "the name a resource reports does not change between reads").
5. Simpler variant with the same outcome: UpdateScalingPlan carrying only [B] (dropping A) makes B's policy jump from ".../1/0" to ".../0/0".

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a91edd84994317051b56d01cfdbb3583a165d015

# A resource whose instruction disables dynamic scaling reports "Target tracking scaling policies have been applied to the resource." while reporting no policies at all

_Filed: 2026-10-29T14:05:28Z_
_Operation: DescribeScalingPlanResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a91edd84994317051b56d01cfdbb3583a165d015`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeScalingPlanResources` derives both halves of a resource's scaling-status report from the PLAN record alone, while the policy list beside them is derived from the resource's own `ScalingInstruction`. The two disagree whenever the instruction turns dynamic scaling off.

For a settled (`Active`) plan whose instruction carries `DisableDynamicScaling: true`, the handler reports:

* `ScalingStatusCode: "Active"`,
* `ScalingStatusMessage: "Target tracking scaling policies have been applied to the resource."`,
* and NO `ScalingPolicies` member at all, because `ScalingPlanRecord::applied_policies` returns an empty list for exactly that instruction.

So one read asserts, in the member the model points a caller at for detail, that target tracking scaling policies have been applied to a resource that this very same read says carries none. This crate states the opposite principle for itself: the `UNAPPLIED_STATUS` row exists precisely so a resource whose configuration did not take is "never 'applied'".

The same mismatch shows up in the two transient statuses of such a resource — `UpdateInProgress` reports "Target tracking scaling policies are being updated on the resource." and `DeletionInProgress` "Target tracking scaling policies are being removed from the resource." — for a resource that has, and had, no policy to update or remove. The plan-level `ScalingPlan$StatusMessage` of such a plan ("Scaling plan has been created and applied to all resources.") carries the same claim.

The narrative is a function of the plan's status only; it needs to account for the one thing the crate already lets an instruction change about whether policies reach its resource (`configures_dynamic_scaling`), which is the same predicate the policy projection next to it consults.

## Evidence

The narrative the crate reports for every resource of an applied plan — `src/state.rs:99-104`:

```
/// What a plan whose configuration is applied to its resources reports.
const APPLIED_STATUS: ReportedStatus = ReportedStatus {
    status_message: "Scaling plan has been created and applied to all resources.",
    scaling_status_code: SCALING_STATUS_ACTIVE,
    scaling_status_message: "Target tracking scaling policies have been applied to the resource.",
};
```

and the crate's own statement that it is reported unconditionally — `src/state.rs:79-81`:

```
// to "Check the status message for more information" — so the narrative is a fixed
// sentence per status, saying what the service did or is doing to reach it, and is
// reported on EVERY described plan and resource.
```

The other side, in the SAME module — `src/state.rs:280-283`:

```
/// scaling policies based on the specified target tracking configurations" — so an
/// applied resource carries one `TargetTrackingScaling` policy per target tracking
/// configuration of its instruction, echoing that configuration, and an instruction
/// that turned dynamic scaling off carries none.
```

implemented at `src/state.rs:297-302`:

```
pub fn applied_policies(&self, instruction: &ScalingInstruction) -> Vec<ScalingPolicy> {
    if self.scaling_status_code() == SCALING_STATUS_INACTIVE
        || !configures_dynamic_scaling(instruction)
    {
        return Vec::new();
    }
```

The crate explicitly holds that a resource must not report "applied" when the configuration did not take — `src/state.rs:111-114`:

```
/// asserts as much in a debug build. Should one ever be set, what a resource reports is
/// `PartiallyActive` — "the scaling configuration could not be fully applied. Check the
/// status message for more information" — never "applied".
```

The handler puts both into one response object — `src/ops/describe_scaling_plan_resources.rs:209-210` and `:245-255`:

```
                    let scaling_status_code = plan.scaling_status_code();
                    let scaling_status_message = plan.scaling_status_message();
...
                            let policies = plan.applied_policies(instruction);
                            ScalingPlanResource {
...
                                scaling_policies: (!policies.is_empty()).then_some(policies),
                                scaling_status_code: scaling_status_code.to_string(),
                                scaling_status_message: Some(scaling_status_message.to_string()),
```

Model text the message member is bound to (`smithy_query(query="doc",shape="ScalingPlanResource",member="ScalingStatusMessage")`): "A simple message about the current scaling status of the resource."

Test coverage: NOT pinned. `tests/all/applied_policies_and_status_narratives.rs:317-333` adds the "dynamic scaling disabled" row, but the assertion for it at `:359-367` checks only the absent policy member and then stops:

```
            if disabled {
                assert_eq!(
                    resource.get("ScalingPolicies"),
                    None,
                    "case {label:?}: dynamic scaling is off, so no policy is applied, \
                     got {resource:?}"
                );
                continue;
            }
```

so no assertion is ever made about that resource's `ScalingStatusMessage`. The narrative test beside it (`tests/all/applied_policies_and_status_narratives.rs:102-106`) builds its plan from `table_instruction("DynamoDBReadCapacityUtilization", 50.0)`, which leaves dynamic scaling enabled, and `tests/all/documented_member_defaults.rs:78` only crosses `DisableDynamicScaling` omitted against its documented default `false` — never `true`.

## Steps to Reproduce

1. CreateScalingPlan with ScalingPlanName="no-dynamic", a CloudFormation stack ApplicationSource, and one ScalingInstruction for autoscaling / autoScalingGroup/my-asg / autoscaling:autoScalingGroup:DesiredCapacity carrying one TargetTrackingConfiguration and "DisableDynamicScaling": true
2. Wait for the lifecycle clock (crate::state::SETTLE_AFTER) so the plan settles to Active
3. Call DescribeScalingPlanResources with ScalingPlanName="no-dynamic", ScalingPlanVersion=1
4. Observe the single ScalingPlanResource: ScalingStatusCode "Active" and ScalingStatusMessage "Target tracking scaling policies have been applied to the resource.", while the ScalingPolicies member is absent entirely — the read asserts policies were applied to a resource it simultaneously reports as carrying none
5. Repeat after UpdateScalingPlan (and after DeleteScalingPlan) while the transition is still in flight: the resource reports policies "are being updated on" / "are being removed from" it, again with no ScalingPolicies member

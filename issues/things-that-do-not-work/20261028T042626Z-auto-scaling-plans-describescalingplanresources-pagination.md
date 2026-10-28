Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 21d30b53374928fd663edcc8ae486344ab510a4e

# DescribeScalingPlanResources pages by list OFFSET, so an UpdateScalingPlan between pages silently skips or repeats resources

_Filed: 2026-10-28T04:26:26Z_
_Operation: DescribeScalingPlanResources | Class: pagination_

> _Note: this issue was identified against the code base at commit `21d30b53374928fd663edcc8ae486344ab510a4e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeScalingPlanResources` encodes its `NextToken` as the integer OFFSET reached in `ScalingPlanRecord::scaling_instructions` and resumes the next page with `.skip(start)`. The offset is not anchored to anything that identifies a resource, and `UpdateScalingPlan` replaces the whole stored instruction list with the list the request carries, in the REQUEST's order (the merge preserves `requested`'s order). So any update that drops, inserts or reorders an instruction while a caller is mid-walk moves every later resource's offset, and the caller's next page resumes at the wrong place:

* an update that removes an earlier instruction shifts the remaining ones down by one, so the resource that sat at the token's offset is never returned on any page of the walk — the walk silently loses a scalable resource that is still covered by the plan;
* an update that prepends an instruction shifts them up by one, so the resource just returned is returned a second time.

This is the exact failure mode the SIBLING read in this same crate was written to avoid: `DescribeScalingPlans` deliberately makes its cursor the identifying KEY the previous page stopped after (the plan name) and says so in a comment, "so paging is stable over the ordered map even when plans are created or deleted between pages". `DescribeScalingPlanResources` is the only place in the crate where a resource is addressed by its position in the instruction list rather than by the `(ServiceNamespace, ResourceId, ScalableDimension)` identity the crate already owns as `crate::state::ResourceIdentity` and already matches resources on (both in `validate_scaling_instructions`, to hold one instruction per resource, and in `merge_scaling_instructions`, to pair a requested instruction with the one in force).

Note this is a distinct defect from the already-open `PolicyName` issue on this operation: that one is about the NAME a resource reports, this one is about which resources a paged walk returns at all.

## Evidence

The sibling read in this crate anchors its cursor on the identifying key, and states why — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/describe_scaling_plans.rs:191-200:
```
        // The cursor is the name the previous page stopped after, so paging is stable
        // over the ordered map even when plans are created or deleted between pages.
        let after: Option<String> = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                crate::state::decode_cursor(CURSOR_KIND, token).ok_or_else(|| {
                    invalid_next_token("The NextToken provided is not valid")
                })?,
            ),
        };
```
and resumes by comparing that key (describe_scaling_plans.rs:231-234):
```
        let mut page: Vec<&crate::state::ScalingPlanRecord> = matching
            .filter(|plan| after.as_ref().is_none_or(|last| plan.name > *last))
            .take(page_size + 1)
```

The operation under review resumes from a bare list offset instead — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/describe_scaling_plan_resources.rs:176-183:
```
        // The cursor is the index into the plan's own instruction list, which is
        // ordered exactly as the plan stores it.
        let start: usize = match input.next_token.as_deref() {
            None => 0,
            Some(token) => crate::state::decode_cursor(CURSOR_KIND, token)
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| invalid_next_token("The NextToken provided is not valid"))?,
        };
```
and describe_scaling_plan_resources.rs:203-208, 231-233:
```
                    let page: Vec<ScalingPlanResource> = plan
                        .scaling_instructions
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(page_size)
...
        let returned_through = start.saturating_add(scaling_plan_resources.len());
        let next_token = (returned_through < total)
            .then(|| crate::state::encode_cursor(CURSOR_KIND, &returned_through.to_string()));
```

The write that moves those offsets — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/update_scaling_plan.rs:240-243:
```
        if let Some(instructions) = input.scaling_instructions {
            plan.scaling_instructions =
                crate::state::merge_scaling_instructions(&plan.scaling_instructions, instructions);
        }
```
and the merge keeps the REQUEST's order and length (src/state.rs:891-899):
```
    requested
        .into_iter()
        .map(|instruction| {
            match applied.get(&resource_identity(&instruction)) {
                Some(applied) => keep_tracked_target_values(applied, instruction),
                None => instruction,
            }
        })
        .collect()
```

The identity the crate already uses to address one resource, unused by this cursor (src/state.rs:930-936):
```
/// "Each scaling instruction applies to one resource" (`ScalingInstruction` doc), and
/// that resource is named by the `(ServiceNamespace, ResourceId, ScalableDimension)`
/// triple — so this is the identity [`validate_scaling_instructions`] holds one
/// instruction per, and the one the update merge matches on.
pub type ResourceIdentity<'a> = (&'a str, &'a str, &'a str);
```

Test coverage — nothing pins the offset form, and no test pages across a mutation: tests/all/scaling_plan_lifecycle.rs:1029-1066 pages `plan-5` one resource at a time over UNCHANGED state ("paging must reassemble every instruction of the plan exactly once, in order"), tests/all/scaling_plan_resource_policies.rs:305-335 pages the same way, and tests/all/scaling_plan_lifecycle.rs:1093-1103 only asserts that tokens this service did not mint ("garbage", "", "0", "7a7a7a", "plans:plan-1") are refused with `InvalidNextTokenException` — all of which stay satisfied by a cursor anchored on the resource rather than on the offset.

## Steps to Reproduce

1. CreateScalingPlan "my-plan" with three scaling instructions in this order: [A] autoscaling / autoScalingGroup/asg-a / autoscaling:autoScalingGroup:DesiredCapacity, [B] .../asg-b, [C] .../asg-c, each with one target tracking configuration; let the clock settle the plan to Active.
2. DescribeScalingPlanResources {"ScalingPlanName":"my-plan","ScalingPlanVersion":1,"MaxResults":1} → the page reports asg-a and a NextToken that decodes (hex) to "plan-resources:1".
3. UpdateScalingPlan {"ScalingPlanName":"my-plan","ScalingPlanVersion":1,"ScalingInstructions":[B, C]} — dropping the instruction for asg-a, which the caller has already paged past — and let the plan settle back to Active.
4. Resume the walk: DescribeScalingPlanResources {"ScalingPlanName":"my-plan","ScalingPlanVersion":1,"MaxResults":1,"NextToken":"<the token from step 2>"} → the page reports asg-c. asg-b is still covered by the plan but is returned on NO page of the walk: the offset 1 that used to address asg-b now addresses asg-c.
5. Mirror case: with the same step-2 token, UpdateScalingPlan carrying [X, A, B, C] (prepending one instruction) makes the resumed page report asg-a again, so the walk returns the same resource twice.
6. Compare with the sibling walk over plans, which does not drift: page DescribeScalingPlans with MaxResults=1, delete or create another plan between pages, and the next page still resumes strictly after the plan NAME the previous page stopped at (src/ops/describe_scaling_plans.rs:191-192, 232).

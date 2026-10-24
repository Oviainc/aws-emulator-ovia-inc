Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# UpdateScalingPlan accepts an ApplicationSource another scaling plan already covers

_Filed: 2026-10-24T03:08:25Z_
_Operation: UpdateScalingPlan | Class: validation_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ApplicationSource` is documented as "A CloudFormation stack or set of tags. You can create one scaling plan per application source." (CreateScalingPlanRequest$ApplicationSource / UpdateScalingPlanRequest$ApplicationSource docs), so at most one plan may ever claim a given stack ARN or tag-filter set. CreateScalingPlan in this emulator enforces exactly that: it computes the source key and refuses with ValidationException ("The application source is already covered by the scaling plan '<name>'") when another stored plan already claims it.

UpdateScalingPlan does not. Its handler only checks that the supplied source names exactly one application (stack XOR tag filters, valid stack ARN) and then overwrites `plan.application_source` unconditionally. A caller can therefore create plan A over stack-1 and plan B over stack-2, then UpdateScalingPlan(B, ApplicationSource=stack-1) and get HTTP 200 — leaving two plans claiming one application source, a state AWS can never produce and that CreateScalingPlan in this same emulator refuses to produce.

Expected: UpdateScalingPlan refuses an ApplicationSource already covered by a different stored plan with a 400 ValidationException (the same refusal CreateScalingPlan gives), leaving the stored plan unchanged.
Actual: HTTP 200, the stored source is replaced, and the plan enters UpdateInProgress; a later DescribeScalingPlans filtered by `ApplicationSources: [stack-1]` returns TWO plans.

## Evidence

Spec: `UpdateScalingPlanRequest$ApplicationSource` / `CreateScalingPlanRequest$ApplicationSource` both document "You can create one scaling plan per application source"; UpdateScalingPlan models ValidationException (HTTP 400) for input it refuses (OPERATIONS.md, UpdateScalingPlan errors).
Handler: `src/ops/update_scaling_plan.rs` — `handle` calls only `crate::state::validate_application_source(source)` (shape check) and then `plan.application_source = source;`. It never calls `crate::state::application_source_key` to scan `state.scaling_plans` for an existing claimant, which is precisely what `src/ops/create_scaling_plan.rs` does (the `covered_by` scan returning "The application source is already covered by the scaling plan '<name>'").
Tests: `tests/all/scaling_plan_lifecycle.rs::a_name_or_an_application_source_is_claimed_by_exactly_one_plan` asserts the uniqueness rule for CreateScalingPlan only; its single UpdateScalingPlan row asserts a CONTRADICTORY source (`{"TagFilters": []}`) is refused. No test exercises updating a plan onto a source another plan already covers, so this is uncovered.
Not covered by existing issues: the open issue `20261022T112048Z-autoscaling-plans-describe-envelopes-drop-status-and-policy` touches UpdateScalingPlan only for ScalingInstructions merge semantics and the missing status/policy members.

## Steps to Reproduce

1. CreateScalingPlan with ScalingPlanName="plan-a", ApplicationSource={"CloudFormationStackARN": "arn:aws:cloudformation:us-east-1:000000000000:stack/stack-1/abc"} and one valid scaling instruction; wait for the plan to settle to Active.
2. CreateScalingPlan with ScalingPlanName="plan-b", ApplicationSource={"CloudFormationStackARN": "arn:aws:cloudformation:us-east-1:000000000000:stack/stack-2/def"} and one valid scaling instruction; wait for it to settle to Active.
3. Call UpdateScalingPlan with ScalingPlanName="plan-b", ScalingPlanVersion=1, ApplicationSource={"CloudFormationStackARN": "arn:aws:cloudformation:us-east-1:000000000000:stack/stack-1/abc"}.
4. Observe HTTP 200 instead of a 400 ValidationException reporting that the application source is already covered by "plan-a".
5. Call DescribeScalingPlans with ApplicationSources=[{"CloudFormationStackARN": "arn:aws:cloudformation:us-east-1:000000000000:stack/stack-1/abc"}] and observe two plans ("plan-a" and "plan-b") claiming the same application source.

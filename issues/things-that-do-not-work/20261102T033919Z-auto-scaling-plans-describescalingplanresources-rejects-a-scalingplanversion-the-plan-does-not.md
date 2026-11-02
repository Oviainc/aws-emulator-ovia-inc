Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling-plans.asp-absent-plan-reads-are-empty-but-writes-are-rejected
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: auto-scaling-plans|DescribeScalingPlanResources|ValidationException
Issue-Commit: 46f0b06b741958855bcde96340f933e8bc769db3

# DescribeScalingPlanResources rejects a ScalingPlanVersion the plan does not hold with ValidationException instead of answering 200 with an empty ScalingPlanResources list

> _Note: this issue was identified against the code base at commit `46f0b06b741958855bcde96340f933e8bc769db3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingPlanResources

## Description
This issue has 1 cause. It restates, with the failing replay key bound to it, the cause already described in open issue 20261102T001453Z-auto-scaling-plans-describescalingplanresources-refuses-a-scalingplanversion-other-than-1-with.md — that issue names this same ScalingPlanVersion gate but was not registered as owning this check's replay key, so the suite reports the check as net-new and a fixer replaying that issue would not re-run it. If the two are merged, keep this issue's trace_ids.

Cause 1 of 1: the ScalingPlanVersion member of DescribeScalingPlanResources is gated as a constrained value rather than treated as a selector, so a read for a version the named plan does not hold is turned into a client error instead of an empty result.
AWS: Smithy (services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/smithy-model/com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice.json): DescribeScalingPlanResources input = {ScalingPlanName (required, @length 1..128, @pattern ^[\p{Print}&&[^|:/]]+$), ScalingPlanVersion (required, type long, NO @range and NO enum), MaxResults, NextToken}; errors = ConcurrentUpdateException(500), InternalServiceException(500), InvalidNextTokenException(400), ValidationException(400) — ObjectNotFoundException is NOT bound to this operation, so a selector matching nothing cannot be an error here at all. The only statement about the value is member prose ("Currently, the only valid value is 1"), which documents which versions exist today, not an input constraint. Recorded behavior: a DescribeScalingPlanResources whose (ScalingPlanName, ScalingPlanVersion) pair the service does not hold — an absent name, or version 2 of a live plan that only has version 1 — answers HTTP 200 with ScalingPlanResources = [] and no NextToken. The WRITE operations over the same absent coordinate are the ones that refuse: DeleteScalingPlan/UpdateScalingPlan answer HTTP 400 ObjectNotFoundException with message "No scaling plan found for scaling plan name: <ScalingPlanName>, version: <ScalingPlanVersion>" — fixed text "No scaling plan found for scaling plan name: " + ", version: ", with the plan name and version number taken from the request.
Emulator: the call is refused HTTP 400, Error.Code ValidationException, Error.Message "ScalingPlanVersion 2 is not valid: the only valid value is 1" (fixed text "ScalingPlanVersion " + " is not valid: the only valid value is 1", with the request's own version number interpolated; the 2 in the recording is that request's value filling the template, not part of the requirement). The refusal is raised before any lookup, so it fires for every version != 1 whatever the store holds. The emulator handles the sibling absent-coordinate case the opposite way — an unknown ScalingPlanName reads back as 200 + empty list, which the same check verified in this run — so the two halves of "absent coordinate" are inconsistent.
Why: this read is the poll primitive for the plan lifecycle. A client that probes or walks versions (a caller that does not know which version is current, an upgrade path checking version N+1, any poll-until-visible loop keyed on a version) gets a thrown exception from the SDK where AWS hands back an empty list, so the loop aborts; and because the code is ValidationException rather than ObjectNotFoundException, generic not-found handling does not catch it either. It also inverts the contract this API relies on: on AWS an absent plan coordinate is safe to READ and refused only on WRITE.
Fix: DescribeScalingPlanResources' request validation — the ScalingPlanVersion gate in the operation handler (the validate_plan_version call in services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/describe_scaling_plan_resources.rs, helper in src/state.rs) must not reject a version the model does not constrain; the existing plan(name, version) lookup immediately after it already yields the correct empty answer. The same shared helper is reached from the forecast read's copy of this member, so the fix belongs in the shared gate rather than one handler.
Traces: auto-scaling-plans.asp-absent-plan-reads-are-empty-but-writes-are-rejected (errored on the version-2 read of a live plan). The check aborts there, so its remaining assertions — DeleteScalingPlan for version 2 answering ObjectNotFoundException "No scaling plan found for scaling plan name: <name>, version: 2", and DescribeScalingPlans refusing ScalingPlanNames together with ApplicationSources with ValidationException "Application sources cannot be provided with scaling plan names or version" — were never reached and remain unverified; the whole check must be replayed after the fix.

Related but not covering this cause: 20261030T033853Z-autoscaling-plans-auto-scaling-plans-client-error-refusals-diverge-no.md (this operation's invalid-NextToken wording and CreateScalingPlan's name pattern) and 20261022T175157Z-auto-scaling-plans-plan-lifecycle-has-no-application.md (lifecycle / Application Auto Scaling side effects).

Not asserted as requirements: the generated plan and table names the check mints, and any request id or clock value in the envelope.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: auto-scaling-plans 2/10 (20%), 8 failed. Seven of the eight are marked OWNED by existing issues; this check carries no ALREADY FILED / OWNED marker.

Failing check auto-scaling-plans.asp-absent-plan-reads-are-empty-but-writes-are-rejected, verdict error:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the DescribeScalingPlanResources operation: ScalingPlanVersion 2 is not valid: the only valid value is 1

Failing call (check source line 214):
  asp.describe_scaling_plan_resources(ScalingPlanName=plan, ScalingPlanVersion=2)
against a plan the same check had just created and seen reach StatusCode Active. Expected: HTTP 200 with ScalingPlanResources == [] and no NextToken. Actual: HTTP 400, Error.Code ValidationException, Error.Message 'ScalingPlanVersion 2 is not valid: the only valid value is 1'.

The assertions before it passed, which localizes the defect to the version selector and not to absent-coordinate handling in general: describe_scaling_plans on a never-created name returned [], describe_scaling_plan_resources(absent name, version 1) returned [], get_scaling_plan_resource_forecast_data(absent name, version 1) returned no datapoints, delete_scaling_plan/update_scaling_plan on the absent name both answered 400 ObjectNotFoundException 'No scaling plan found for scaling plan name: <name>, version: 1', and the positive control read version 1 of the live plan back with exactly 1 resource.

Model evidence (smithy_query, operation DescribeScalingPlanResources): ScalingPlanVersion is a required long with no range and no enum; bound errors are ConcurrentUpdateException(500), InternalServiceException(500), InvalidNextTokenException(400), ValidationException(400) — no ObjectNotFoundException. DeleteScalingPlan, by contrast, does bind ObjectNotFoundException(400), matching the write-side refusal this check observed.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an autoscaling-plans client (plus dynamodb and application-autoscaling for the fixture resources) at it.
2. Create a DynamoDB table and a scaling plan over it with one target-tracking ScalingInstruction; wait for DescribeScalingPlans to report StatusCode 'Active'.
3. Positive control: DescribeScalingPlanResources(ScalingPlanName=<plan>, ScalingPlanVersion=1) returns 200 with exactly 1 resource.
4. Call DescribeScalingPlanResources(ScalingPlanName=<plan>, ScalingPlanVersion=2). Observed: HTTP 400 ValidationException 'ScalingPlanVersion 2 is not valid: the only valid value is 1'. Expected: HTTP 200 with ScalingPlanResources == [] and no NextToken.
5. Call DescribeScalingPlanResources(ScalingPlanName=<a name never created>, ScalingPlanVersion=2); the same empty 200 is expected, since neither member can produce a not-found on this operation.
6. Confirm the write half still refuses: DeleteScalingPlan(ScalingPlanName=<plan>, ScalingPlanVersion=2) must answer HTTP 400 ObjectNotFoundException 'No scaling plan found for scaling plan name: <plan>, version: 2'.
7. Re-replay auto-scaling-plans.asp-absent-plan-reads-are-empty-but-writes-are-rejected end to end; the assertions after the version-2 read were never reached in this run and must also pass before closing.

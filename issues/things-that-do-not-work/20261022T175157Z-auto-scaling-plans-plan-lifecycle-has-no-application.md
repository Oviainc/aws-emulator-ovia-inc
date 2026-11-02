Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling-plans.asp-delete-deregisters-the-scalable-target-it-created autoscaling-plans.asp-plan-activates-with-readable-instructions autoscaling-plans.asp-update-replaces-instruction-without-new-version autoscaling-plans.asp-absent-resource-plan-is-rejected-asynchronously autoscaling-plans.asp-concurrent-update-is-rejected
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans.asp-delete-deregisters-the-scalable-target-it-created|run#16|AssertionError|assert len(targets) == 1, (
Issue-Key: autoscaling-plans.asp-plan-activates-with-readable-instructions|run#15|KeyError|assert settled["StatusMessage"] == (
Issue-Key: autoscaling-plans.asp-update-replaces-instruction-without-new-version|_await_status_message#12|Timeout|time.sleep(3)
Issue-Key: autoscaling-plans.asp-absent-resource-plan-is-rejected-asynchronously|run#11|AssertionError|assert settled["StatusCode"] == "ActiveWithProblems", (
Issue-Key: autoscaling-plans.asp-concurrent-update-is-rejected|run#18|AssertionError|assert rejection is not None, (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# auto-scaling-plans: plan lifecycle has no Application Auto Scaling side effects, no status narrative, no deferred resource validation and no update interlock

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScalingPlan

## Description
The emulator models a scaling plan as an inert stored document: it accepts CreateScalingPlan/UpdateScalingPlan, flips the plan to Active, and does nothing else. Four distinct root causes follow from that, all in the auto-scaling-plans service. This bundle has 4 causes; ALL FOUR must be closed before submitting.

Related already-open issue: 20261022T112048Z-autoscaling-plans-describe-envelopes-drop-status-and-policy.md (wire corpus) covers the *envelope* shape of DescribeScalingPlans/DescribeScalingPlanResources (missing StatusMessage, missing ScalingPolicies, update merge). It does not cover the cross-service registration, the deferred-validation state machine, the status-message TRANSITION across an update, or the concurrency interlock below; Cause 2 here overlaps its Cause 1 and should be fixed together with it.

Cause 1 of 4: a plan that reaches Active registers nothing with Application Auto Scaling, so DeleteScalingPlan has nothing to take down either.
AWS: ScalingInstruction's own model doc states that AWS Auto Scaling CREATES target tracking scaling policies from the scaling instructions (Smithy: shape ScalingInstruction; shape ScalingPlanResource declares ScalingPolicies and the required ScalingStatusCode). Recorded behavior: once the plan is Active, Application Auto Scaling DescribeScalableTargets for the instruction's ServiceNamespace/ResourceId/ScalableDimension returns exactly one scalable target carrying the instruction's own MinCapacity/MaxCapacity, and DescribeScalingPolicies returns exactly one policy whose name is derived from the plan name (the recorded name begins with "AutoScaling-<plan>-" and then a generated id, so only the derivation is a requirement, never the literal name). DeleteScalingPlan then deregisters both; its own response body is empty either way, so the registration is the only observable.
Emulator: the plan reaches Active, but aas.describe_scalable_targets(ServiceNamespace='dynamodb', ResourceIds=['table/<t>']) returns [] - no target, hence no policy and nothing for the delete to remove.
Why: the purpose of a scaling plan is the target and policy it installs on another service. A client that creates a plan and then drives or inspects the resource through Application Auto Scaling sees an unmanaged resource, and a client that deletes a plan cannot detect the registration leak the real API would have cleaned up (the symmetric defect) because nothing was ever registered.
Fix: the CreateScalingPlan/UpdateScalingPlan handlers plus the shared scaling-plan state must project each scaling instruction into the application-autoscaling service's scalable-target and scaling-policy state (propagating the instruction's capacity bounds), and the DeleteScalingPlan handler must withdraw exactly what it created.
Traces: auto-scaling-plans.asp-delete-deregisters-the-scalable-target-it-created.

Cause 2 of 4: ScalingPlan.StatusMessage is never populated, so neither the create narrative nor the update narrative is ever visible - and a caller polling for it never converges.
AWS: ScalingPlan models StatusMessage (target XmlString, doc "A simple message about the current status of the scaling plan.") next to the required StatusCode, and the StatusCode doc tells callers to "check the status message for more information" (Smithy: shape ScalingPlan, shape ScalingPlanResource.ScalingStatusMessage). Recorded behavior: an Active plan always carries a StatusMessage; it reads "Scaling plan has been created and applied to all resources." after a create and changes to "Scaling plan has been updated successfully." after an in-place update, while StatusCode stays Active throughout. Since an in-place update moves neither ScalingPlanVersion nor CreationTime, this message is the ONLY field that distinguishes an updated plan from a freshly created one.
Emulator: DescribeScalingPlans returns the plan with the member absent entirely - the check raises KeyError: 'StatusMessage' on an Active plan, and a poll-until-message caller hangs until the harness kills it at 60s.
Why: a client cannot tell why a plan is in its status, and cannot tell that its update was applied at all; any convergence or drift check built on the status narrative blocks forever against the emulator.
Fix: the plan record in the service's state must persist a status message, set on create and re-set on update, and the DescribeScalingPlans serializer must emit the member on every described plan. The recorded texts above are examples of the two states, not required strings; what is required is that the member is present and that it CHANGES on a successful update.
Traces: auto-scaling-plans.asp-plan-activates-with-readable-instructions, auto-scaling-plans.asp-update-replaces-instruction-without-new-version.

Cause 3 of 4: an instruction naming a resource that does not exist is accepted and then never fails - the plan settles Active instead of ActiveWithProblems.
AWS: CreateScalingPlan does NOT validate the named resource synchronously; it answers 200 with {"ScalingPlanVersion": 1} and rejects asynchronously. Recorded behavior: the plan settles StatusCode=ActiveWithProblems (a value the model's ScalingPlanStatusCode enum declares) with a StatusMessage saying the plan was created but could not be applied to all resources and naming the count of failed resources; DescribeScalingPlanResources reports that resource with ScalingStatusCode=Inactive (modeled value) and a ScalingStatusMessage that names the absent resource and carries Application Auto Scaling's "Error Code: ValidationException" and "Status Code: 400"; the derived policy record still exists, and nothing is registered with Application Auto Scaling for the absent resource. (Smithy: ScalingPlanStatusCode values include ActiveWithProblems; ScalingStatusCode values include Inactive.)
Emulator: the create is accepted as version 1 (correct), but the plan settles StatusCode='Active' with no StatusMessage, and the per-resource failure state is never produced.
Why: the emulator reports success for a plan that cannot scale anything. A client that creates a plan and gates on ActiveWithProblems / per-resource Inactive to surface a misconfiguration never sees the failure and believes a dead plan is healthy.
Fix: the plan's asynchronous settle path in the service's state machine must validate each instruction's resource against the target service when applying it, record the per-resource failure (ScalingStatusCode=Inactive + ScalingStatusMessage carrying the downstream error code and status) and settle the plan ActiveWithProblems when any resource failed, rather than unconditionally Active.
Traces: auto-scaling-plans.asp-absent-resource-plan-is-rejected-asynchronously.

Cause 4 of 4: UpdateScalingPlan has no concurrency interlock - overlapping updates are all accepted.
AWS: ConcurrentUpdateException is modeled on the mutating operations, documented as "Concurrent updates caused an exception, for example, if you request an update to a scaling plan that already has a pending update" (Smithy: shape ConcurrentUpdateException; note the on-disk model classifies it httpError 500, but the recorded behavior is an HTTP 400 with Error.Code 'ConcurrentUpdateException' - the recording is the oracle here). Recorded message example: "You already have a pending update to the scaling plan". The plan serialises mutations: a second update issued while the first is still settling is REFUSED rather than queued or applied last-writer-wins, and the refusal leaves the plan intact (it settles Active at version 1 with its single instruction).
Emulator: 8 pairs of back-to-back UpdateScalingPlan calls on one plan were all accepted (16 of 16, status 200); the interlock never fires, i.e. update is implemented as an idempotent setter with no pending-mutation state.
Why: a client that relies on the service to serialise mutations - retry loops, and this API's own DeleteScalingPlan callers, which must retry ConcurrentUpdateException - is never exercised, and concurrent writers silently clobber each other instead of one being told to back off.
Fix: the UpdateScalingPlan (and DeleteScalingPlan) handler must consult a per-plan pending-mutation flag in the service's state and reject an overlapping mutation with HTTP 400 / Error.Code ConcurrentUpdateException, clearing the flag when the mutation settles.
Traces: auto-scaling-plans.asp-concurrent-update-is-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, auto-scaling-plans 1/10 passed (9 failed). Two failures excluded: asp-absent-plan-reads-are-empty-but-writes-are-rejected (ALREADY FILED - blocked by unimplemented GetScalingPlanResourceForecastData) and asp-predictive-forecast-without-history-has-no-datapoints (OWNED by an existing issue).

Cause 1 - check auto-scaling-plans.asp-delete-deregisters-the-scalable-target-it-created:
  AssertionError: expected the plan to register exactly 1 scalable target for table/sim2real-behav-asp-tbl-<id>, got []
  Expected: after the plan reaches Active, DescribeScalableTargets returns 1 target with ScalableDimension 'dynamodb:table:ReadCapacityUnits' and the instruction's bounds (2, 8), and DescribeScalingPolicies returns 1 policy named 'AutoScaling-<plan>-...'; DeleteScalingPlan then removes both.
  Actual: zero scalable targets registered for the resource the plan names, so the delete-side deregistration could not even be reached.

Cause 2 - check auto-scaling-plans.asp-plan-activates-with-readable-instructions:
  KeyError: 'StatusMessage' at `settled["StatusMessage"]` on a plan whose StatusCode is already 'Active'.
  Expected: StatusMessage present (recorded example 'Scaling plan has been created and applied to all resources.').
  Actual: member absent from the DescribeScalingPlans response.
  check auto-scaling-plans.asp-update-replaces-instruction-without-new-version:
  'Check timed out after 60.0s' while polling DescribeScalingPlans for a non-None StatusMessage, which never appears; the update's expected transition to 'Scaling plan has been updated successfully.' is therefore unobservable.

Cause 3 - check auto-scaling-plans.asp-absent-resource-plan-is-rejected-asynchronously:
  AssertionError: expected the plan to settle ActiveWithProblems, got 'Active': None
  Expected: CreateScalingPlan over a table that does not exist -> accepted as ScalingPlanVersion 1 (the emulator does this correctly), then plan StatusCode=ActiveWithProblems with a StatusMessage naming 1 problem resource, DescribeScalingPlanResources reporting that resource ScalingStatusCode=Inactive with a ScalingStatusMessage containing 'Error Code: ValidationException' and 'Status Code: 400', 1 derived policy still present, and no scalable target registered for the absent table.
  Actual: StatusCode 'Active', StatusMessage None.

Cause 4 - check auto-scaling-plans.asp-concurrent-update-is-rejected:
  AssertionError: 8 pairs of back-to-back UpdateScalingPlan calls on plan 'sim2real-behav-asp-plan-<id>' were all accepted (16 in total); expected a ConcurrentUpdateException from the second of some pair.
  Expected: one of the overlapping updates refused with HTTP 400 and Error.Code 'ConcurrentUpdateException' (recorded message example 'You already have a pending update to the scaling plan'), after which the plan still settles Active at version 1 with 1 instruction.
  Actual: every update returned 200; the interlock never fires.

Model evidence (Smithy, com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice): ScalingPlan declares StatusMessage/StatusStartTime/CreationTime alongside required StatusCode; ScalingPlanStatusCode enum includes ActiveWithProblems; ScalingStatusCode enum includes Inactive; ScalingPlanResource declares ScalingPolicies and ScalingStatusMessage; ConcurrentUpdateException is declared with the pending-update documentation.

## Steps to Reproduce
1. Point the behavioral harness at the emulator and run the auto-scaling-plans checks (service token 'auto-scaling-plans', crate services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice).
2. Cause 1: create a PROVISIONED DynamoDB table (RCU 2), then CreateScalingPlan with one dynamodb:table:ReadCapacityUnits instruction (MinCapacity 2, MaxCapacity 8, one predefined target-tracking configuration). Poll DescribeScalingPlans until StatusCode=Active, then call application-autoscaling DescribeScalableTargets(ServiceNamespace='dynamodb', ResourceIds=['table/<table>']) -> observe [] instead of one target with bounds (2, 8), and DescribeScalingPolicies -> [] instead of one 'AutoScaling-<plan>-*' policy. Then DeleteScalingPlan and observe there was nothing to deregister.
3. Cause 2: with that same Active plan, read DescribeScalingPlans(ScalingPlanNames=['<plan>']) -> the plan entry has no StatusMessage key at all. Then UpdateScalingPlan(ScalingPlanName='<plan>', ScalingPlanVersion=1, ScalingInstructions=[same instruction with TargetValue 75.0, ScalingPolicyUpdateBehavior='ReplaceExternalPolicies']) and poll DescribeScalingPlans for a StatusMessage -> it never becomes non-None, so the poll never terminates.
4. Cause 3: CreateScalingPlan naming a DynamoDB table that was never created (ResourceId 'table/<absent>') -> it is accepted as ScalingPlanVersion 1 (correct), then poll DescribeScalingPlans to a terminal status -> StatusCode 'Active' with StatusMessage None instead of 'ActiveWithProblems'; DescribeScalingPlanResources for that plan does not report the resource as Inactive with a ValidationException/400 status message.
5. Cause 4: on an Active plan over a real table, issue two UpdateScalingPlan calls back to back with no wait between them (varying TargetValue), repeated up to 8 times -> all 16 calls return 200; no call is refused with HTTP 400 ConcurrentUpdateException.

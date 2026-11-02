Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling-plans.asp-create-rejects-capacity-and-target-bounds autoscaling-plans.asp-plan-activates-with-readable-instructions autoscaling-plans.asp-update-replaces-instruction-without-new-version
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: auto-scaling-plans|CreateScalingPlan|ValidationException
Issue-Key: autoscaling-plans.asp-plan-activates-with-readable-instructions|run#37|AssertionError|assert pol["PolicyName"].startswith(f"AutoScaling-{plan}-"), (
Issue-Key: autoscaling-plans.asp-update-replaces-instruction-without-new-version|_await_target_value#19|Timeout|time.sleep(3)
Issue-Commit: 24dcbcf55b45bb9d0573a38e9c06262a825c087d

# auto-scaling-plans: 3 net-new causes — empty-ScalingInstructions refusal wording, derived target-tracking PolicyName not built from the plan name, and UpdateScalingPlan never converging

> _Note: this issue was identified against the code base at commit `24dcbcf55b45bb9d0573a38e9c06262a825c087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateScalingPlan

## Description
Three distinct root causes in the auto-scaling-plans service, all surviving failures of this run (neither blocked by an unimplemented stub nor owned by an open issue's replay key). This bundle has 3 causes; ALL THREE must be closed before submitting.

Related open issue: 20261022T175157Z-auto-scaling-plans-plan-lifecycle-has-no-application.md still carries the cross-service registration (Cause 1), the deferred-validation state machine (Cause 3) and the concurrency interlock (Cause 4), and its Cause 2 (StatusMessage never populated) is now partly served — an Active plan does report 'Scaling plan has been created and applied to all resources.', which is why the two checks below now fail further on. None of the three causes here is that issue's replay key.

Cause 1 of 3: CreateScalingPlan refuses an empty ScalingInstructions list with the crate's own generic sentence instead of AWS's text.
AWS: ScalingInstructions is an @required list with NO @length constraint in the model, so the at-least-one rule and its wording are recorded behavior only: HTTP 400, Error.Code 'ValidationException', Error.Message 'Must provide at least 1 scaling instruction'. That whole sentence is fixed text — it interpolates nothing from the request (the '1' is the service's own minimum, not a request value). Smithy: CreateScalingPlan.ScalingInstructions -> list ScalingInstructions (@required); errors include ValidationException (client, httpError 400).
Emulator: the same request is correctly refused with HTTP 400 ValidationException, but the message is 'ScalingInstructions must specify at least one scaling instruction' — a member-name-prefixed constraint template rather than the service's sentence. The sibling refusals in the same check match AWS exactly ('Maximum capacity cannot be less than minimum capacity', 'Minimum capacity cannot be less than 1', and the target-value range sentence), so only this one message is templated.
Why: a client that branches on, logs, or asserts the refusal text of an empty instruction set (and every test or CLI surface that reproduces AWS's message) sees a sentence AWS never emits, while status and code look right — the divergence is invisible until it breaks a string match.
Fix: the CreateScalingPlan handler's instruction-list validation must raise the service's own text for this rule instead of the shared member-constraint template, the same way its capacity-bound and target-value neighbours already do. Compare the resolved issue 20261022T112048Z-autoscaling-plans-error-messages-do-not-follow-the.md, which closed other messages of this crate but not this one.
Traces: auto-scaling-plans.asp-create-rejects-capacity-and-target-bounds.

Cause 2 of 3: the target-tracking policy a plan derives from an instruction is named with an internal composite key instead of AWS's plan-derived name.
AWS: ScalingPlanResource.ScalingPolicies carries ScalingPolicy, whose PolicyName is @required (length 1..256, pattern ^\p{Print}+$) and whose PolicyType is TargetTrackingScaling. Recorded behavior: the policy AWS Auto Scaling creates from a scaling instruction is named 'AutoScaling-<ScalingPlanName>-<generated id>' — the literal prefix 'AutoScaling-', then the plan's own name verbatim from the request, then '-' and a service-generated id. Only the derivation is a requirement; the generated id differs per run and is never part of it.
Emulator: PolicyName is '<ScalingPlanName>/TargetTracking/<32-hex>/<32-hex>' (observed: 'sim2real-behav-asp-plan-d67944b77a1f/TargetTracking/b4d5b5aae7af7fe6a13972022d0bb05f/2caadef89be48acd2dbde9e21f608a54') — no 'AutoScaling-' prefix and a slash-joined hash of the instruction/configuration instead of a generated id suffix. Everything else the check asserts about that policy passes (one TargetTrackingScaling policy, TargetValue 50.0 carried through).
Why: the policy name is the only handle a caller has to join a scaling plan to the policy it installed in Application Auto Scaling (DescribeScalingPolicies / PutScalingPolicy / CloudWatch alarm names are all keyed on it). A name that does not derive from the plan name cannot be matched back to its plan, and clients that filter policies by the 'AutoScaling-<plan>-' prefix to find plan-managed policies find none. It is also account-visible state, not an internal key.
Fix: the plan-settle path that projects an instruction into a derived policy (the crate's scaling-plan state plus the DescribeScalingPlanResources serializer that reports ScalingPolicies) must mint the name as 'AutoScaling-' + the plan name + '-' + a stable generated id for that instruction. Note the resolved issue 20261028T031436Z-auto-scaling-plans-describescalingplanresources-wrong-behavior--r2.md moved this name off the instruction's list POSITION onto a content hash — the stability it added is right, the shape of the name is still not AWS's — and Cause 1 of the open issue 20261022T175157Z records the same 'AutoScaling-<plan>-' derivation from the Application Auto Scaling side.
Traces: auto-scaling-plans.asp-plan-activates-with-readable-instructions.

Cause 3 of 3: an in-place UpdateScalingPlan never becomes observable, so a poll-until-converged caller never returns.
AWS: UpdateScalingPlan takes ScalingPlanName + ScalingPlanVersion + ScalingInstructions and answers 200 with an EMPTY body (Smithy: UpdateScalingPlan output has no members; errors ObjectNotFoundException/ValidationException(400), ConcurrentUpdateException/InternalServiceException). The update is applied in place: recorded behavior is that the plan keeps ScalingPlanVersion 1 and its original CreationTime, its single instruction is REPLACED (the new TargetValue reads back from DescribeScalingPlans and from the derived policy in DescribeScalingPlanResources), StatusCode stays 'Active' throughout, and the plan's StatusMessage changes from the create narrative 'Scaling plan has been created and applied to all resources.' to 'Scaling plan has been updated successfully.' Both sentences are fixed text. Because neither the version nor CreationTime moves, that message transition is the ONLY field that distinguishes an updated plan from a freshly created one.
Emulator: the check hangs and is killed — 'Check timed out after 60.0s' — while polling DescribeScalingPlans after the update. Hypothesis (the timeout names no frame): the create-side narrative is now served correctly (the sibling check asserts it and gets past it), so the poll that never terminates is the post-update one: the plan keeps the create StatusMessage (and/or never reports the updated TargetValue while Active) after UpdateScalingPlan returns 200, so no observable in the response ever reflects the update.
Why: a client cannot tell that its update was applied at all — an emulator that silently dropped the update is indistinguishable from one that applied it — and any convergence/drift loop built on polling the plan after an update blocks forever instead of returning. This is the behavior the open issue 20261022T175157Z Cause 2 predicted would remain once the create message alone was populated.
Fix: the UpdateScalingPlan handler and the plan record in the service's state — a successful in-place update must re-set the plan's status narrative to the update sentence (while leaving StatusCode Active, ScalingPlanVersion 1 and CreationTime untouched), replace rather than extend the instruction set, and propagate the new TargetValue into the derived policy reported by DescribeScalingPlanResources, all within the settle the create path already models.
Traces: auto-scaling-plans.asp-update-replaces-instruction-without-new-version.

## Evidence
Suite: simparity-behavioral-aws-v1, auto-scaling-plans 2/10 passed (8 failed). Four failures skipped as OWNED (asp-absent-resource-plan-is-rejected-asynchronously, asp-concurrent-update-is-rejected, asp-delete-deregisters-the-scalable-target-it-created -> 20261022T175157Z; asp-predictive-forecast-without-history-has-no-datapoints -> 20261022T093447Z). asp-absent-plan-reads-are-empty-but-writes-are-rejected is filed separately as a reopened forecast refusal.

Cause 1 — check auto-scaling-plans.asp-create-rejects-capacity-and-target-bounds, verdict mismatch:
  AssertionError: no scaling instructions at all: expected message 'Must provide at least 1 scaling instruction', got 'ScalingInstructions must specify at least one scaling instruction'
  Request: CreateScalingPlan(ScalingPlanName='<new name>', ApplicationSource={TagFilters:[{Key:'sim2real-asp-rej', Values:['<tag>']}]}, ScalingInstructions=[]).
  Expected: HTTP 400, Error.Code 'ValidationException', Error.Message 'Must provide at least 1 scaling instruction'.
  Actual: HTTP 400, Error.Code 'ValidationException' (both correct), Error.Message 'ScalingInstructions must specify at least one scaling instruction'.
  The three earlier refusals in the same check passed, so only the wording of this rule diverges.

Cause 2 — check auto-scaling-plans.asp-plan-activates-with-readable-instructions, verdict mismatch:
  AssertionError: expected a policy name derived from the plan name, got 'sim2real-behav-asp-plan-d67944b77a1f/TargetTracking/b4d5b5aae7af7fe6a13972022d0bb05f/2caadef89be48acd2dbde9e21f608a54'
  Sequence: create a PROVISIONED DynamoDB table, CreateScalingPlan with one dynamodb:table:ReadCapacityUnits instruction (Min 1, Max 10, predefined DynamoDBReadCapacityUtilization target tracking at 50.0), poll to StatusCode Active (StatusMessage now correct), then DescribeScalingPlanResources -> 1 resource, ScalingStatusCode Active, 1 TargetTrackingScaling policy whose TargetValue is 50.0.
  Expected: PolicyName beginning 'AutoScaling-<ScalingPlanName>-' (remainder a generated id, not asserted).
  Actual: the plan name followed by '/TargetTracking/<hash>/<hash>'.

Cause 3 — check auto-scaling-plans.asp-update-replaces-instruction-without-new-version, verdict mismatch:
  'Check timed out after 60.0s' (the check's own poll deadlines are 300s, so it never converged).
  Sequence: create the plan over a real table with TargetValue 50.0, wait for StatusMessage 'Scaling plan has been created and applied to all resources.' at StatusCode Active (this now succeeds), capture the derived policy, then UpdateScalingPlan(ScalingPlanName=plan, ScalingPlanVersion=1, ScalingInstructions=[same instruction with TargetValue 75.0, ScalingPolicyUpdateBehavior='ReplaceExternalPolicies']), then poll DescribeScalingPlans for TargetValue 75.0 at StatusCode Active and for StatusMessage 'Scaling plan has been updated successfully.'
  Expected: both polls return within seconds; afterwards ScalingPlanVersion still 1, CreationTime unchanged, exactly 1 instruction, the derived policy keeps its name and reports TargetValue 75.0.
  Actual: the polling loop never satisfies its condition and the harness kills the check at 60s, so the update is unobservable.

Model evidence (com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService): ScalingInstructions is a plain list with no @length, CreateScalingPlan binds ValidationException(400); ScalingPolicy.PolicyName @required (length 1..256, pattern ^\p{Print}+$) with @required PolicyType; UpdateScalingPlan output has no members and binds ObjectNotFoundException(400)/ValidationException(400)/ConcurrentUpdateException/InternalServiceException.

Not asserted as requirements: the generated plan and table names, the generated-id suffix of a policy name, and any timestamp or request id in the recorded bodies.

## Steps to Reproduce
1. Boot the emulator from this checkout and point the behavioral harness at it (service token 'auto-scaling-plans', crate services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice).
2. Cause 1: CreateScalingPlan with a valid ApplicationSource and ScalingInstructions=[] -> observe HTTP 400 ValidationException with Message 'ScalingInstructions must specify at least one scaling instruction'; expected 'Must provide at least 1 scaling instruction'. Confirm the name is still free afterwards (it is).
3. Cause 2: create a PROVISIONED DynamoDB table (RCU/WCU 1) tagged sim2real-asp=<table>, then CreateScalingPlan with one instruction (ServiceNamespace 'dynamodb', ResourceId 'table/<table>', ScalableDimension 'dynamodb:table:ReadCapacityUnits', MinCapacity 1, MaxCapacity 10, one predefined DynamoDBReadCapacityUtilization target-tracking configuration at TargetValue 50.0). Poll DescribeScalingPlans to StatusCode Active, then DescribeScalingPlanResources(ScalingPlanName=plan, ScalingPlanVersion=1) and read ScalingPolicies[0].PolicyName -> '<plan>/TargetTracking/<hash>/<hash>' instead of 'AutoScaling-<plan>-<generated id>'.
4. Cause 3: on that same Active plan, note its StatusMessage, then call UpdateScalingPlan(ScalingPlanName=plan, ScalingPlanVersion=1, ScalingInstructions=[the same instruction with TargetValue 75.0 and ScalingPolicyUpdateBehavior='ReplaceExternalPolicies']). It answers 200 with an empty body. Now poll DescribeScalingPlans(ScalingPlanNames=[plan]) -> the instruction's TargetValue and/or the StatusMessage never reach 75.0 / 'Scaling plan has been updated successfully.', so the poll never terminates (the harness kills it at 60s). Also check DescribeScalingPlanResources version 1: the derived policy must keep its name and report TargetValue 75.0.
5. Re-replay the three checks named in trace_ids; all three must pass.

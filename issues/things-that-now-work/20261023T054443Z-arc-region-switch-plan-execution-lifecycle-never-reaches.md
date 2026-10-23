Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-region-switch.start-plan-execution-manual-approval-completes arc-region-switch.concurrent-start-plan-execution-single-winner arc-region-switch.active-execution-blocks-start-and-delete-rejected arc-region-switch.create-plan-invalid-region-count-rejected arc-region-switch.get-plan-nonexistent-not-found arc-region-switch.delete-plan-lifecycle-not-found-after-delete arc-region-switch.eventbridge-execution-started-event-delivered
Issue-Shape-Id: com.amazonaws.arcregionswitch#ArcRegionSwitch
Issue-Key: arc-region-switch.start-plan-execution-manual-approval-completes|run#10|AssertionError|assert pending["executionState"] == "pendingManualApproval", (
Issue-Key: arc-region-switch.concurrent-start-plan-execution-single-winner|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: arc-region-switch.active-execution-blocks-start-and-delete-rejected|_error#13|AssertionError|raise AssertionError("expected a rejection, call unexpectedly succeeded")
Issue-Key: arc-region-switch.create-plan-invalid-region-count-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: arc-region-switch.get-plan-nonexistent-not-found|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: arc-region-switch.delete-plan-lifecycle-not-found-after-delete|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: arc-region-switch.eventbridge-execution-started-event-delivered|_poll_for_matching_events#23|Timeout|resp = sqs.receive_message(
Issue-Commit: 1d15d4851343e3a31be69cd8cea36918aab666fe

# arc-region-switch: plan-execution lifecycle never reaches pendingManualApproval, CreatePlan accepts a duplicated Region, not-found/illegal-state messages and EventBridge emission diverge (5 causes)

> _Note: this issue was identified against the code base at commit `1d15d4851343e3a31be69cd8cea36918aab666fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPlanExecution

## Description
Five distinct root causes in the arc-region-switch emulator, all grounded in failing behavioral checks of this suite. No existing open issue covers them; the open arc-region-switch issues in this repo are the unimplemented-stub issues for ApprovePlanExecutionStep, GetPlanInRegion, GetPlanEvaluationStatus, ListPlanExecutions, ListRoute53HealthChecks, ListTagsForResource and UpdatePlan, which are separate work items and are skipped here.

=== Cause 1 of 5: a plan execution settles to `failed` instead of progressing through the workflow, so `pendingManualApproval` is unreachable ===
Failing checks: arc-region-switch.start-plan-execution-manual-approval-completes, arc-region-switch.concurrent-start-plan-execution-single-winner.
AWS: Smithy model com.amazonaws.arcregionswitch (version 2022-07-26), GetPlanExecution output member `executionState` targets enum ExecutionState with values inProgress | pausedByFailedStep | pausedByOperator | completed | completedWithExceptions | canceled | planExecutionTimedOut | pendingManualApproval | failed | pending | completedMonitoringApplicationHealth, and `stepStates` (StepState.status targets StepStatus). Recorded behavior: for a plan whose workflow contains a manual-approval step, StartPlanExecution drives the execution pending -> pendingManualApproval, the step's own status becomes `pendingApproval`, and after approval the execution reaches `completed` with the step `completed`.
Emulator: StartPlanExecution stores the execution `pending` with every step `notStarted` and the clock then settles it to the modelled FAILURE state (see services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/start_plan_execution.rs module docs: outcome = crate::state::FAILED after crate::state::EXECUTION_START_SPAN). GetPlanExecution therefore reports `failed`: "AssertionError: expected pendingManualApproval, got 'failed'". The concurrency check's warm-up waits for the Approve step to reach pendingApproval, which never happens, and the check times out after 60s.
Why: a client that starts a plan execution with a manual-approval workflow can never observe an approval-pending execution, can never approve it, and sees a recovery run reported as failed when AWS reports it awaiting approval — every manual-approval and progression path is unusable, and the execution becomes terminal within seconds so the plan is never held.
Fix: the execution-progression model in the service state's settle path plus the StartPlanExecution handler that seeds step states (state.rs settle/EXECUTION_START_SPAN, ops/start_plan_execution.rs).
Traces: start-plan-execution-manual-approval-completes (assertion at check line 38); concurrent-start-plan-execution-single-winner (timeout in warm-up _wait_for_step_pending_approval).

=== Cause 2 of 5: no active execution is ever held, so the single-active-execution guard on StartPlanExecution and the DeletePlan refusal never fire; their IllegalStateException messages also differ ===
Failing checks: arc-region-switch.active-execution-blocks-start-and-delete-rejected.
AWS: Smithy: StartPlanExecution errors include IllegalStateException (HTTP 400, doc "The operation failed because the current state of the resource doesn't allow the operation to proceed"); DeletePlan errors are IllegalStateException (400) and ResourceNotFoundException (404). Recorded behavior: while a plan has a non-terminal execution, a second StartPlanExecution is rejected IllegalStateException with message "There is already an execution ongoing for plan <planArn>" (message built from the request's planArn), and DeletePlan is rejected IllegalStateException with "Active executions found for <planArn>: <region>: <id>" (built from the plan ARN and the active execution id); the plan and its single execution are untouched.
Emulator: the second StartPlanExecution returned 200 and minted a second execution — "AssertionError: expected a rejection, call unexpectedly succeeded". The guard exists in ops/start_plan_execution.rs (and ops/delete_plan.rs) but only fires while the holding execution is non-terminal, and because of Cause 1 the first execution is already terminal; once that is fixed, the messages still diverge: the emulator formats "The plan <arn> already has execution <id> in progress" and "The plan <arn> cannot be deleted while execution <id> is in progress".
Why: a client can start overlapping region-switch executions against one plan and can delete a plan that is mid-switch — AWS guarantees at most one active execution per plan and refuses both calls, so orchestration that relies on the 400 rejection to serialize failovers proceeds concurrently.
Fix: the active-execution holding semantics in the service state plus the IllegalStateException construction in the StartPlanExecution and DeletePlan handlers (ops/start_plan_execution.rs ~line 329, ops/delete_plan.rs ~line 155).
Traces: active-execution-blocks-start-and-delete-rejected (check line 55).

=== Cause 3 of 5: CreatePlan accepts the same Region twice instead of rejecting a non-distinct Region pair ===
Failing checks: arc-region-switch.create-plan-invalid-region-count-rejected.
AWS: Smithy: CreatePlan.regions is required, targets RegionList with @length min 2 / max 2 and member pattern ^[a-z]{2}-[a-z-]+-\d+$; the model states no distinctness trait, so recorded behavior only: real AWS rejects regions=[r, r] with error code ValidationException (HTTP 400) and creates nothing (ValidationException is not a shape in this service's model — it is the service-framework validation error observed on the wire).
Emulator: the call SUCCEEDED with HTTP 200 and stored the plan, e.g. regions: ['us-west-2', 'us-west-2'], returning plan arn arn:aws:arc-region-switch::<account>:plan/<name>:<suffix> with version '1'.
Why: a client that mis-configures both participating Regions to the same value gets a plan AWS would never create; the plan can then be executed and no activate/deactivate pair exists, so the failure surfaces far from the request that caused it instead of as a 400 at CreatePlan.
Fix: request validation in the CreatePlan handler (services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/create_plan.rs — no distinctness check exists between the MAX_PLANS check and the store insert).
Traces: create-plan-invalid-region-count-rejected (check line 21).

=== Cause 4 of 5: ResourceNotFoundException message for a missing plan is rendered with a different template ===
Failing checks: arc-region-switch.get-plan-nonexistent-not-found, arc-region-switch.delete-plan-lifecycle-not-found-after-delete.
AWS: Smithy: GetPlan error ResourceNotFoundException (HTTP 404, required member `message`); DeletePlan likewise. The message is built from the request: recorded AWS renders exactly "Plan not found for ARN: <the arn in the request>" for both GetPlan on a never-created (but well-formed, in-account) plan ARN and for GetPlan/DeletePlan on a deleted plan.
Emulator: the code and status are correct (ResourceNotFoundException), but the message is rendered "No plan found for arn <arn>" — see ops/get_plan.rs and ops/delete_plan.rs ~line 144 (format!("No plan found for arn {}", ...)), also the same template in ops/start_plan_execution.rs ~line 283.
Why: a client or test harness that matches the documented not-found message for a plan ARN (the request-derived template) does not match, so a deleted-or-absent plan is not recognised by message-matching code paths even though the code is right.
Fix: the ResourceNotFoundException message construction in the plan-lookup path of the GetPlan, DeletePlan and StartPlanExecution handlers.
Traces: get-plan-nonexistent-not-found (check line 42); delete-plan-lifecycle-not-found-after-delete (check line 38).

=== Cause 5 of 5: StartPlanExecution emits no 'ARC Region Switch Plan Execution' EventBridge event ===
Failing checks: arc-region-switch.eventbridge-execution-started-event-delivered.
AWS: recorded behavior only (the Smithy model does not describe eventing): starting a plan execution publishes an EventBridge event with source "aws.arc-region-switch" and detail-type "ARC Region Switch Plan Execution", carrying the plan ARN in `resources` and the execution id in detail.executionId, so an EventBridge rule matching that source/detail-type routes it to an SQS target.
Emulator: after an EventBridge rule and SQS target were created and StartPlanExecution succeeded, no correlating event was ever delivered to the queue; the check polls for matching events (120s budget) and the check timed out after 60.0s with no match.
Why: a client that drives automation off ARC Region Switch execution events (notification, audit, cross-service orchestration) sees nothing happen when an execution starts, so event-driven recovery workflows never fire.
Fix: event emission on execution start/state change in the StartPlanExecution handler and the execution-progression path in the service state, publishing into the emulator's EventBridge event bus.
Traces: eventbridge-execution-started-event-delivered.

## Evidence
Cause 1 — expected GetPlanExecution.executionState 'pendingManualApproval' (step 'Approve' status 'pendingApproval'), got 'failed': "AssertionError: expected pendingManualApproval, got 'failed'" (start-plan-execution-manual-approval-completes). concurrent-start-plan-execution-single-winner: "Check timed out after 60.0s" while waiting for the warm-up execution's Approve step to reach pendingApproval.

Cause 2 — expected a second StartPlanExecution against a plan with an active execution to fail IllegalStateException (HTTP 400) with "There is already an execution ongoing for plan <arn>", and DeletePlan to fail IllegalStateException with "Active executions found for <arn>: <region>: <id>"; got a 200 success: "AssertionError: expected a rejection, call unexpectedly succeeded". Emulator templates instead: "The plan <arn> already has execution <id> in progress" / "The plan <arn> cannot be deleted while execution <id> is in progress".

Cause 3 — expected CreatePlan(regions=[r, r]) to be rejected with ValidationException (HTTP 400) and create nothing; got HTTP 200: "AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'plan': {'arn': 'arn:aws:arc-region-switch::000000000000:plan/<name>:<suffix>', ..., 'regions': ['us-west-2', 'us-west-2'], 'recoveryApproach': 'activePassive', 'version': '1'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".

Cause 4 — code matches (ResourceNotFoundException) but message diverges: "AssertionError: expected message 'Plan not found for ARN: arn:aws:arc-region-switch::000000000000:plan/<name>:<suffix>', got 'No plan found for arn arn:aws:arc-region-switch::000000000000:plan/<name>:<suffix>'" — same divergence from GetPlan on an absent ARN and from GetPlan/DeletePlan after DeletePlan.

Cause 5 — expected at least one delivered event with source 'aws.arc-region-switch', detail-type 'ARC Region Switch Plan Execution', the plan ARN in resources and detail.executionId == the started execution id; none arrived: "Check timed out after 60.0s" while polling the SQS target of the matching EventBridge rule.

## Steps to Reproduce
Replay the behavioral suite against the emulator and run the listed arc-region-switch checks.

Cause 1: CreatePlan with two distinct Regions and a workflow containing a manual-approval step 'Approve'; StartPlanExecution(planArn, targetRegion, action='activate'); poll GetPlanExecution until executionState != 'pending' — the emulator reports 'failed' (AWS: 'pendingManualApproval' with stepStates[0].status == 'pendingApproval').

Cause 2: with that execution still non-terminal, call StartPlanExecution again on the same plan (AWS: IllegalStateException 400) and DeletePlan on the plan (AWS: IllegalStateException 400). The emulator answers the second StartPlanExecution 200.

Cause 3: CreatePlan(name=<32-char-safe name>, regions=[<region>, <same region>], recoveryApproach='activePassive', primaryRegion=<region>, executionRole=<iam role arn>, workflows=[{'workflowTargetAction': 'activate'}]) — the emulator returns 200 and stores the plan; AWS returns ValidationException 400.

Cause 4: GetPlan on a well-formed but never-created plan ARN, and GetPlan/DeletePlan after a successful DeletePlan — compare Error.Message to 'Plan not found for ARN: <arn>'.

Cause 5: PutRule with EventPattern {"source":["aws.arc-region-switch"],"detail-type":["ARC Region Switch Plan Execution"]}, PutTargets to an SQS queue, then StartPlanExecution on a plan; poll ReceiveMessage for an event whose detail.executionId equals the returned executionId — nothing is delivered.

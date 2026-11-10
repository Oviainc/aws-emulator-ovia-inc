Issue-Source: trace-sweep-divergence
Issue-Trace: arc-region-switch-lambda-step-plan-execution-completes
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: arc-region-switch-lambda-step-plan-execution-completes
Issue-Shape-Id: com.amazonaws.arcregionswitch#ArcRegionSwitch
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.endTime
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.executionState
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.plan.associatedAlarms
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.plan.serviceQuotaChecksEnabled
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.plan.triggers
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.plan.updatedAt
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.startTime
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.stepStates[*].endTime
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.stepStates[*].startTime
Issue-Key: arc-region-switch|GetPlanExecution|2xx->2xx|$.stepStates[*].status
Issue-Key: arc-region-switch|ListPlanExecutions|2xx->2xx|$.items[*]
Issue-Commit: 69cb8c4786d799a8a66c2117bf8eebb4fb69db3f

# arc-region-switch: Lambda custom-action plan execution never completes, and the plan projection omits defaulted members

> _Note: this issue was identified against the code base at commit `69cb8c4786d799a8a66c2117bf8eebb4fb69db3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPlanExecution,ListPlanExecutions

## Description
Three root causes in the arc-region-switch emulator, all grounded in the wire fixture arc-region-switch-lambda-step-plan-execution-completes, which creates a plan whose activate workflow contains ONE step with executionBlockType=CustomActionLambda (executionBlockConfiguration.customActionLambdaConfig), starts an execution of it, polls GetPlanExecution to a terminal state and then lists the plan's completed executions. Each cause below must be closed before submitting; they are numbered Cause 1 of 3 .. Cause 3 of 3 and each names its own fixture step. (Not covered by the open arc-region-switch issues: 20261029T193105Z-arc-region-switch-10-net-new-causes-startplanexecution.md bundles a declined-APPROVAL step's status, the missing executionStarted event, evaluation warnings, EventBridge emission and Route 53 health checks — none of its ten causes is about a CustomActionLambda step being performed, about the Plan projection's defaulted members, or about ListPlanExecutions; 20261107T221850Z (no EventBridge plan-execution event) and 20261109T123922Z (CloudFormation resource type) are unrelated.)

=== Cause 1 of 3: a CustomActionLambda execution block is never performed, so the step and the whole execution reach `failed` instead of `completed` ===
AWS: Smithy: Step.executionBlockConfiguration is a union whose customActionLambdaConfig member is documented "An Amazon Web Services Lambda execution block"; StepState.status targets StepStatus (notStarted | running | failed | completed | canceled | skipped | pendingApproval) and GetPlanExecution.executionState targets ExecutionState (inProgress | pausedByFailedStep | pausedByOperator | completed | completedWithExceptions | canceled | planExecutionTimedOut | pendingManualApproval | failed | pending | completedMonitoringApplicationHealth). Recorded behavior: for a plan whose only step is a CustomActionLambda block, AWS invokes the configured Lambda function(s) under the plan's executionRole and, when the invocation succeeds, that step's StepState.status becomes `completed` and the execution's executionState becomes `completed` (with endTime set). A successfully performed step is NOT a failure state.
Emulator: the step is treated as an execution block the emulator cannot perform, so it is failed on sight: GetPlanExecution reports stepStates[0].status == 'failed' and executionState == 'failed' (HTTP 200 both expected and actual; only the body diverges). The replay's poll gave up because 'failed' is in its fail_on set after 2 attempts. This is deliberate in the current code: services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/state.rs documents FAILED as the state "an execution reaches when the clock gets to a step this emulator cannot perform" and treats every non-manual-approval block that way (Advance::UnsupportedStep), with UNSUPPORTED_EXECUTION_REASON recorded on the event. For a CustomActionLambda block that reasoning does not hold here: the target of the block is a Lambda function, and Lambda IS emulated in this project, so the block can be performed (or at minimum completed) rather than failed.
Why: the single most common Region-switch workflow — a custom Lambda action — can never succeed. A client that drives a switch and waits for executionState `completed` instead observes a terminal `failed`, gets the opposite verdict from the one AWS gives for the same plan, and any code that branches on failure (alarms, rollback, retry of the switch) fires on a run that actually succeeded. Downstream reads of the execution (step status, ListPlanExecutions filtered by state, the event timeline) are all wrong as a consequence.
Fix: the plan-execution state machine in the service's state module — the step advance that classifies an execution block (Advance::UnsupportedStep) must perform a customActionLambdaConfig block (invoke the configured function ARNs) and stamp the step `completed` and the execution `completed` on success, instead of failing every block that is not a manual approval.
Traces: arc-region-switch-lambda-step-plan-execution-completes#task#9 (and #task#10, see Cause 3).

=== Cause 2 of 3: GetPlanExecution's embedded `plan` omits associatedAlarms, triggers and serviceQuotaChecksEnabled whenever CreatePlan did not supply them ===
AWS: Smithy: GetPlanExecution output carries `plan` (shape Plan). Plan.associatedAlarms targets AssociatedAlarmMap ("The associated application health alarms for a plan"), Plan.triggers targets TriggerList ("The triggers for a plan"), Plan.serviceQuotaChecksEnabled targets Boolean ("Indicates whether service quota checks are enabled for the Region switch plan"); none is @required, so none is forced by the boundary. Recorded behavior: for a plan created WITHOUT any of the three, AWS still materialises them on every read of the plan — associatedAlarms is present as an empty map {}, triggers is present as an empty list [], and serviceQuotaChecksEnabled is present and true (the service's default). The values are state-derived: the empty collections are the plan's (empty) alarm/trigger sets, and the boolean is the stored flag, defaulted on creation.
Emulator: all three members are absent from the `plan` object GetPlanExecution returns ($.plan.associatedAlarms, $.plan.triggers and $.plan.serviceQuotaChecksEnabled all reported 'missing ... got None'). The cause is that the plan record stores the request members verbatim and the projection clones the record: services/.../src/ops/create_plan.rs builds Plan { associated_alarms: input.associated_alarms, triggers: input.triggers, service_quota_checks_enabled: input.service_quota_checks_enabled, .. } with no defaulting, and state.rs StoredPlan::as_plan only clones that record and fills `version`. So any read of the plan (GetPlan/CreatePlan/GetPlanExecution) is missing them, not just this operation.
Why: a client that reads plan.serviceQuotaChecksEnabled to decide whether quota checks are on sees an absent member and must guess, and a client that iterates plan.triggers / plan.associatedAlarms gets a null where AWS guarantees a (possibly empty) collection — in strongly typed SDKs that is a null-collection vs empty-collection difference, and in the quota-check case an absent boolean reads as "off" when AWS has it on.
Fix: plan state — default the three members when a plan is created/updated (empty map, empty list, serviceQuotaChecksEnabled true) so the stored record carries them, and keep the Plan projection a straight read of the record.
Traces: arc-region-switch-lambda-step-plan-execution-completes#task#9.

=== Cause 3 of 3: ListPlanExecutions reports no execution at all for the plan's finished execution ===
AWS: Smithy: ListPlanExecutions is documented "Lists the executions of a Region switch plan. This operation returns information about both current and historical executions", returns `items` (AbbreviatedExecutionsList) and accepts an optional `state` (ExecutionState) selector. Recorded behavior: after the Lambda-step execution of the plan finishes, the list carries one AbbreviatedExecution for it, with planArn, executionId, executionRegion, executionAction 'activate', mode 'graceful' and executionState 'completed'.
Emulator: the response is HTTP 200 with an EMPTY items list — items[0] is missing entirely, so the finished execution is reported nowhere. Hypothesis: this is the same root cause as Cause 1 — the handler's state selector (services/.../src/ops/list_plan_executions.rs filters `execution.execution_state == wanted`) correctly matches nothing, because the execution the emulator stored is `failed` while the step asks for the `completed` ones; the element is absent rather than merely different in its executionState, which is what a state-filtered read of a wrongly-failed execution looks like. This cause is therefore expected to close with Cause 1, but it must be re-replayed and confirmed on its own step: if the step's read is unfiltered, then ListPlanExecutions is dropping a stored execution from the plan's execution log and that is an independent defect in this handler's selection/paging.
Why: a client that enumerates a plan's executions (to find the current one, to audit history, or to poll the execution it just started) sees an empty log and concludes the switch never ran.
Fix: closes with the execution state machine fix in Cause 1; re-verify the ListPlanExecutions handler's state selection and its projection of the plan's execution log.
Traces: arc-region-switch-lambda-step-plan-execution-completes#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1: 15/17 steps passed (88.2%), 2 mismatched, 0 errored, 0 blocked; emulator up (/__admin/health 200). Both divergences are in fixture arc-region-switch-lambda-step-plan-execution-completes, service arc-region-switch, and neither carries a stub_path or an owned_by.

Causes 1 and 2 — step task#9, GetPlanExecution, expected HTTP 200 / actual HTTP 200, body diverges:
  $.executionState: expected 'completed', got 'failed'
  $.stepStates[0].status: expected 'completed', got 'failed'
  $.executionState: poll: 'failed' is in fail_on after 2 attempt(s) (expected 'completed', got 'failed')
  $.plan.associatedAlarms: missing (expected {}, got None)
  $.plan.triggers: missing (expected [], got None)
  $.plan.serviceQuotaChecksEnabled: missing (expected True, got None)
  (Also reported, but NOT part of this finding: $.startTime, $.endTime, $.plan.updatedAt, $.stepStates[0].startTime and $.stepStates[0].endTime are '<shaped-not-asserted>' — per-run timestamps, no requirement on their values.)

Cause 3 — step task#10, ListPlanExecutions, expected HTTP 200 / actual HTTP 200:
  $.items[0]: missing (expected {'endTime': '<shaped-not-asserted>', 'executionAction': 'activate', 'executionId': '<region>/<execution-id>', 'executionRegion': '<region>', 'executionState': 'completed', 'mode': 'graceful', 'planArn': '<planArn>', 'startTime': '<shaped-not-asserted>'}, got None)
  (The recorded executionId, plan ARN and region are per-run values; the requirement is that one AbbreviatedExecution for the finished execution is reported, with executionState 'completed', executionAction 'activate' and mode 'graceful'.)

Code the causes were traced to: services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/state.rs lines 103-120 (FAILED / UNSUPPORTED_EXECUTION_REASON: "a step it cannot perform fails the whole execution") and lines 386-391 (StoredPlan::as_plan clones the stored record); services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/create_plan.rs lines 223-248 (associated_alarms / triggers / service_quota_checks_enabled stored straight from the request, never defaulted); services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/list_plan_executions.rs lines 197-212 (the `state` selector).

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus) and re-run fixture arc-region-switch-lambda-step-plan-execution-completes, steps task#9 and task#10. Manually:

1. CreatePlan with regions=[R1, R2], recoveryApproach/primaryRegion set, executionRole set, and ONE activate workflow whose single step has executionBlockType='CustomActionLambda' and executionBlockConfiguration.customActionLambdaConfig naming a Lambda function that exists; supply NO associatedAlarms, NO triggers and NO serviceQuotaChecksEnabled.
2. StartPlanExecution(planArn, targetRegion=R1, action='activate') (mode defaults to graceful).
3. Poll GetPlanExecution(planArn, executionId) until executionState is terminal -> expect executionState 'completed' and stepStates[0].status 'completed'; observe both 'failed' (Cause 1).
4. In the same GetPlanExecution response, read `plan` -> expect associatedAlarms {}, triggers [] and serviceQuotaChecksEnabled true to be present; observe all three absent (Cause 2). The same omission is observable on CreatePlan's and GetPlan's `plan`.
5. Call ListPlanExecutions(planArn, state='completed') -> expect one AbbreviatedExecution for the execution just finished (executionState 'completed', executionAction 'activate', mode 'graceful'); observe an empty items list (Cause 3). Repeat with no `state` selector to confirm whether the execution is listed at all.

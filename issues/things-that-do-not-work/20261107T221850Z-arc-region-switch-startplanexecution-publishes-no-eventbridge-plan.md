Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-region-switch.eventbridge-execution-started-event-delivered
Issue-Shape-Id: com.amazonaws.arcregionswitch#ArcRegionSwitch
Issue-Key: arc-region-switch.eventbridge-execution-started-event-delivered|run#24|AssertionError|assert len(matches) > 0, "no EventBridge event correlating to this execution was delivered"
Issue-Commit: e8e30f14f63b62e6a8d9143bf50f26c979187615

# arc-region-switch: StartPlanExecution publishes no EventBridge plan-execution event, so an EventBridge rule's target never receives anything

> _Note: this issue was identified against the code base at commit `e8e30f14f63b62e6a8d9143bf50f26c979187615`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPlanExecution

## Description
One net-new root cause, grounded in one failing behavioral check of this suite. (Related but NOT covering this: the open issue 20261029T193105Z-arc-region-switch-10-net-new-causes-startplanexecution.md lists this check id in its header and describes the same emission gap as its Cause 9, but the suite does not report this check as OWNED by it — the recorded failure key has changed from a harness timeout while polling the SQS target to a prompt in-body assertion that zero correlating events were received — so the check is re-filed here with its current, now-deterministic key. If that issue's Cause 9 is closed, this one closes with it.)

=== Cause 1 of 1: starting a plan execution emits no EventBridge event, so a rule matching the service's events routes nothing to its target ===
AWS: recorded behavior only — the Smithy model carries no event schema; Smithy: StartPlanExecution (input planArn/targetRegion/action required, output executionId; errors AccessDeniedException 403, ConflictException 409, IllegalArgumentException 400, IllegalStateException 400, ResourceNotFoundException 404) models only the API call, not the emission. The recording shows that starting a plan execution publishes an event on the account's default EventBridge bus in the execution's Region with source "aws.arc-region-switch" and detail-type "ARC Region Switch Plan Execution" (both fixed text), carrying the plan's ARN in `resources` and the id of the execution that was just started in `detail.executionId` (both variable, taken from the request's planArn and the newly minted execution id respectively). A rule whose pattern matches that source/detail-type therefore delivers the event to its target — here an SQS queue whose resource policy allows events.amazonaws.com.
Emulator: no event is ever published. The check creates the queue, PutRule with that exact pattern, SetQueueAttributes with the allow policy and PutTargets — all of which succeed — then StartPlanExecution (which also succeeds and returns an executionId) and polls ReceiveMessage on the queue for up to 120 s. Every poll returns no message whose `detail.executionId` equals the started execution, and the body assertion fires: `AssertionError: no EventBridge event correlating to this execution was delivered` at check_arc_region_switch_eventbridge_execution_started_event_delivered.py line 77 (`assert len(matches) > 0`). Because the poll now drains and returns empty rather than hanging, the downstream assertions on source, detail-type, `resources` containing the plan ARN and `detail.executionId` are never reached — they are all unverifiable while nothing is emitted. The eventing wiring itself is not at fault: the control-plane calls all returned 2xx and no error of any kind was raised.
Why: any client that drives automation off region-switch events — a rule fanning out to SQS/SNS/Lambda/Step Functions for alarms, audit pipelines, runbook orchestration or cross-service coordination — observes absolutely nothing when a switch starts, and the absence is silent: there is no error, no partial event and no alternative signal, so the client simply never fires. Event-driven integrations cannot be developed or tested against the emulator at all, and a test that asserts "starting a switch notifies my pipeline" passes vacuously only if it has no negative control.
Fix: the StartPlanExecution handler / the plan-execution state machine in the arc-region-switch service crate — on admitting a new execution it must publish the plan-execution event to EventBridge on the default bus of the execution's Region, with source "aws.arc-region-switch", detail-type "ARC Region Switch Plan Execution", the plan ARN in `resources`, and `detail.executionId` set to the execution id it returns, so the emulator's EventBridge rule matching and target delivery can route it.
Traces: arc-region-switch.eventbridge-execution-started-event-delivered.

## Evidence
Suite: 10/20 passed (50.0%), 10 mismatched, all in arc-region-switch. Nine of the ten failures are reported OWNED by 20261029T193105Z-arc-region-switch-10-net-new-causes-startplanexecution.md and are skipped; this check is the one surviving failure.

arc-region-switch.eventbridge-execution-started-event-delivered [mismatch]:
  File "check_arc_region_switch_eventbridge_execution_started_event_delivered.py", line 77, in run
    assert len(matches) > 0, "no EventBridge event correlating to this execution was delivered"
  AssertionError: no EventBridge event correlating to this execution was delivered

expected: within 120 s of StartPlanExecution returning an executionId, at least one message on the rule's SQS target whose JSON body has source == "aws.arc-region-switch" (fixed), detail-type == "ARC Region Switch Plan Execution" (fixed), some entry of `resources` containing the plan ARN from the request, and detail.executionId == the returned execution id.
actual:  zero messages matching detail.executionId over the whole 120 s window; the queue stays empty. PutRule, SetQueueAttributes, PutTargets and StartPlanExecution all returned success, so the emission — not the rule/target wiring or an authorization refusal — is what is missing. No HTTP error status or error code was produced anywhere in the run.

## Steps to Reproduce
Replay arc-region-switch.eventbridge-execution-started-event-delivered, or by hand:
1. sqs.CreateQueue; read its QueueArn via GetQueueAttributes.
2. events.PutRule(Name=<rule>, State="ENABLED", EventPattern={"source":["aws.arc-region-switch"],"detail-type":["ARC Region Switch Plan Execution"]}).
3. sqs.SetQueueAttributes(Policy allowing events.amazonaws.com sqs:SendMessage on that queue with ArnEquals aws:SourceArn = the rule ARN).
4. events.PutTargets(Rule=<rule>, Targets=[{Id:..., Arn: <queue ARN>}]).
5. iam.CreateRole trusting arc-region-switch.amazonaws.com with an allow-all inline policy; arc-region-switch.CreatePlan(regions=[<other region>, <this region>], recoveryApproach="activePassive", primaryRegion=<other region>, executionRole=<role ARN>, workflows=[{workflowTargetAction:"activate", steps:[{name:"Approve", executionBlockType:"ManualApproval", executionBlockConfiguration:{executionApprovalConfig:{approvalRole:<role ARN>, timeoutMinutes:60}}}]}]).
6. arc-region-switch.StartPlanExecution(planArn=<plan ARN>, targetRegion=<this region>, action="activate") and record the returned executionId.
7. Poll sqs.ReceiveMessage(MaxNumberOfMessages=10, WaitTimeSeconds=10) for up to 120 s -> expect a message whose body has source "aws.arc-region-switch", detail-type "ARC Region Switch Plan Execution", the plan ARN in `resources` and detail.executionId == the executionId from step 6; observe that no message is ever delivered.

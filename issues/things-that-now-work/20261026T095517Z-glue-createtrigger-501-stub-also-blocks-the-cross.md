Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached glue__sqs.glue-workflow-state-change-to-sqs-positive
Issue-Key: events__glue|CreateTrigger|NotImplemented
Issue-Key: glue__sqs|CreateTrigger|NotImplemented
Issue-Commit: b2c6dc26a07e1ce9903ca7f077d62bde4dbfa91f

# glue CreateTrigger 501 stub also blocks the cross-service event-batching and workflow-to-SQS checks

> _Note: this issue was identified against the code base at commit `b2c6dc26a07e1ce9903ca7f077d62bde4dbfa91f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrigger

## Description
Cause 1 of 1: glue `CreateTrigger` is an unimplemented scaffold stub, so it refuses every request with 501 NotImplemented. The suite marks only the single-service check `glue.reject-update-trigger-without-actions` as covered by the open issue 20261025T022820Z-glue-createtrigger-stub.md ("Implement glue CreateTrigger: unimplemented scaffold stub"); the two CROSS-SERVICE checks listed below hit the same stub and were NOT marked, so they carry no replay key on that issue. This issue exists to attach those two check ids to the same root cause — a fixer of 20261025T022820Z-glue-createtrigger-stub.md must re-replay these two as well, because they exercise behavior beyond the single CreateTrigger call (EventBatchingCondition batching and workflow-run/job-state-change event propagation) that the stub hides entirely.

AWS: `CreateTrigger` answers HTTP 200 with `CreateTriggerResponse{Name}`. Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): input requires `Name` (NameString, len 1..255), `Type` (TriggerType) and `Actions` (ActionList), and optionally accepts `WorkflowName`, `Schedule`, `Predicate`, `Description`, `StartOnCreation` (default false), `Tags` and `EventBatchingCondition`; output is `Name`. Its only modeled errors are AlreadyExistsException, ConcurrentModificationException, EntityNotFoundException, IdempotentParameterMismatchException, InvalidInputException, OperationTimeoutException, ResourceNumberLimitExceededException (all HTTP 400) and InternalServiceException (HTTP 500). `NotImplemented` is not a modeled error for this operation, and 501 is not a status it can answer. A trigger created with `Type="ON_DEMAND"`, `WorkflowName=<wf>` and `Actions=[{JobName: <job>}]` is then startable via `StartTrigger` and drives a workflow run whose job emits the standard "Glue Job State Change" EventBridge event; a trigger with an `EventBatchingCondition{BatchSize:3, BatchWindow:900}` fires its workflow only once the third matching event is delivered, not on the window.

Emulator: `CreateTrigger` returns the scaffold refusal — `ClientError: An error occurred (NotImplemented) when calling the CreateTrigger operation: operation CreateTrigger is scaffolded but not implemented in this emulator`. Both checks abort at the create_trigger call, before any trigger, workflow run, EventBridge rule or SQS target is exercised.

Why: a client that builds a Glue workflow the documented way (CreateWorkflow + CreateTrigger + StartTrigger, or an EventBridge-driven trigger with an EventBatchingCondition) cannot create a trigger at all; it receives a 501 with an unmodeled error code that no SDK retry/handling path expects, instead of a 200 carrying the trigger name. Every downstream contract — workflow runs being recorded, the job-state-change event reaching an EventBridge target, and batch-size gating of event triggers — is unreachable and therefore silently ungraded.

Fix: implement the operation's handler and its state, services/service-com-amazonaws-glue-awsglue/src/ops/create_trigger.rs, with the trigger registry it needs so StartTrigger/GetTrigger/UpdateTrigger/DeleteTrigger and workflow-run creation can read it, including `EventBatchingCondition` so a batched event trigger fires on BatchSize rather than on BatchWindow.

Traces: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached and glue__sqs.glue-workflow-state-change-to-sqs-positive, both aborting on the CreateTrigger call. Related open issue: 20261025T022820Z-glue-createtrigger-stub.md (same operation and handler file; these two check ids are the ones it does not list).

## Evidence
Expected (AWS, per the Smithy model): `glue create_trigger(Name=<trigger>, WorkflowName=<workflow>, Type="ON_DEMAND", Actions=[{"JobName": <job>}], Tags={...})` -> HTTP 200, body `{"Name": "<trigger>"}`. For the batching check, `create_trigger(... Type="EVENT", EventBatchingCondition={"BatchSize": 3, "BatchWindow": 900} ...)` -> HTTP 200 with the trigger name, after which the workflow run appears only once the third matching event is delivered.

Actual (emulator): HTTP 501, error code `NotImplemented`, message `operation CreateTrigger is scaffolded but not implemented in this emulator`. Replay detail for both checks:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTrigger operation: operation CreateTrigger is scaffolded but not implemented in this emulator

In that message, the fixed text is the scaffold template `operation <Name> is scaffolded but not implemented in this emulator` and the error code `NotImplemented`; the only variable part is the operation name taken from the request. Neither the code nor the 501 status is modeled for CreateTrigger — every modeled failure for it is a 400 (or 500 InternalServiceException).

## Steps to Reproduce
1. Boot the emulator and point a Glue client at it.
2. `glue create_workflow(Name=<wf>)` (succeeds).
3. `glue create_trigger(Name=<trig>, WorkflowName=<wf>, Type="ON_DEMAND", Actions=[{"JobName": <existing job>}])`.
4. Observe HTTP 501 with code `NotImplemented` and the scaffold message, instead of 200 with `{"Name": "<trig>"}`.
5. Re-replay both behavioral checks to confirm a fix: `events__glue.event-trigger-fires-only-after-its-batch-size-is-reached` (needs `Type="EVENT"` with `EventBatchingCondition{BatchSize:3, BatchWindow:900}` and asserts the workflow run appears only on the third delivery) and `glue__sqs.glue-workflow-state-change-to-sqs-positive` (needs ON_DEMAND StartTrigger, a workflow run reaching COMPLETED, and the delivered "Glue Job State Change" event's jobRunId to match the workflow's own job run).

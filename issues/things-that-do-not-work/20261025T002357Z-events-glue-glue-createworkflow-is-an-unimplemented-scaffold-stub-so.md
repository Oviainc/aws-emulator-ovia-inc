Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached
Issue-Key: events__glue|CreateWorkflow|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Glue CreateWorkflow is an unimplemented scaffold stub, so no event-based trigger can be built and batch-size-gated firing is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
Cause 1 of 1 — Glue `CreateWorkflow` answers NotImplemented.
AWS: `CreateWorkflow` takes Name (plus Description, DefaultRunProperties, Tags, MaxConcurrentRuns) and returns the workflow `Name`; its modeled client errors are AlreadyExistsException, InvalidInputException, OperationTimeoutException, ResourceNumberLimitExceededException, ConcurrentModificationException and InternalServiceException. A workflow is the container an EVENT-type trigger attaches to: a trigger with `Type: EVENT` and an EventBatchingCondition (BatchSize, optional BatchWindow) starts the workflow only once BatchSize matching events have arrived (or the window elapses), which is the behaviour under test.
Emulator: answers HTTP 400 with error code `NotImplemented` and "operation CreateWorkflow is scaffolded but not implemented in this emulator". `NotImplemented` is not a modeled CreateWorkflow error.
Why: without a workflow there is nowhere to attach an EVENT trigger, so EventBridge-driven Glue orchestration cannot be configured at all, and the batch-size gating — the only thing that stops a workflow starting once per event — can never be observed. The client also gets an error code absent from the Glue API reference and cannot classify it.
Fix: the CreateWorkflow handler, services/service-com-amazonaws-glue-awsglue/src/ops/create_workflow.rs, and the workflow/trigger state that the EVENT trigger's EventBatchingCondition must consult.
Traces: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached.

## Evidence
"botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateWorkflow operation: operation CreateWorkflow is scaffolded but not implemented in this emulator". Expected: HTTP 200 with the workflow Name.

## Steps to Reproduce
1. CreateWorkflow(Name=w) -> HTTP 400 with code NotImplemented instead of 200 with {'Name': w}.
2. The follow-on CreateTrigger(Type='EVENT', WorkflowName=w, EventBatchingCondition={'BatchSize': 3}) and the EventBridge rule that feeds it cannot be exercised.

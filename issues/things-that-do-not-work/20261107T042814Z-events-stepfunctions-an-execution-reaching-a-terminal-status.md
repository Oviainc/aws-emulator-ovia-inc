Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__stepfunctions.execution-status-change-event-reaches-the-default-bus
Issue-Key: events__stepfunctions.execution-status-change-event-reaches-the-default-bus|_receive#43|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Commit: 69fbb491696def14b5ee7dd45913958907d046a9

# events__stepfunctions: an execution reaching a terminal status publishes no Execution Status Change event to the default EventBridge bus (1 cause)

> _Note: this issue was identified against the code base at commit `69fbb491696def14b5ee7dd45913958907d046a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: an execution that reaches a terminal status emits nothing on the default event bus, so neither the positive delivery nor the calibrated negative (a Pass-only machine can never reach FAILED) can be graded. Failing check: events__stepfunctions.execution-status-change-event-reaches-the-default-bus.

AWS: Step Functions publishes to the DEFAULT event bus with `source` = `aws.states` and `detail-type` = `Step Functions Execution Status Change`, the execution ARN in the envelope's `resources`, and a `detail` carrying executionArn, stateMachineArn, name, status (one of the modelled ExecutionStatus values RUNNING / SUCCEEDED / FAILED / TIMED_OUT / ABORTED), startDate, stopDate, input and output. Delivery is best effort but observable well inside a normal poll window. recorded behavior only for the exact detail body.

Emulator: a rule on {"source":["aws.states"],"detail-type":["Step Functions Execution Status Change"]} with a consumer target receives nothing after an execution of a single-Pass-state machine reaches SUCCEEDED; the check exhausts the harness budget with `Check timed out after 60.0s`. Because the positive control never arrives, the check's twin negative — that `status: FAILED` is structurally unreachable for a Pass-only machine — is also ungradable: an absence proves nothing when no event is ever published.

Why: any consumer that reacts to a workflow reaching a terminal status never fires, and a client cannot distinguish "the workflow did not run" from "the status event was never published".

Fix: the Step Functions execution status-transition path must publish the Execution Status Change envelope to the default EventBridge bus.

Related filing: open issue 20261029T182228Z-events-sfn-step-functions-publishes-no-execution-status-change-event.md names this same check id in its Issue-Trace-Ids but was not reported as owning this run's replay key, so the failure is clustered and filed here; a fixer should treat the two as one defect.

Traces: events__stepfunctions.execution-status-change-event-reaches-the-default-bus.

## Evidence
Expected (AWS): a rule on {"source":["aws.states"],"detail-type":["Step Functions Execution Status Change"]} delivers at least one envelope whose detail.executionArn is the started execution and whose detail.status is its terminal status (SUCCEEDED for a single Pass state), within the check's 180 s delivery budget.
Actual (emulator): `Error: Check timed out after 60.0s` — the consumer target is never written to, so neither the positive delivery nor the calibrated absence window for `status: FAILED` can be established.

## Steps to Reproduce
1. PutRule on the DEFAULT bus with pattern {"source":["aws.states"],"detail-type":["Step Functions Execution Status Change"]} and PutTargets with a consumer queue holding a policy that admits events.amazonaws.com.
2. CreateStateMachine with a single Pass state; StartExecution; poll DescribeExecution until SUCCEEDED.
3. Poll the consumer target. Nothing arrives.
4. Re-replay the check in trace_ids to confirm.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs__stepfunctions.step-functions-execution-status-change-reaches-the-default-bus events__sqs__stepfunctions.xsvc-sfn-2-redrive-status-change
Issue-Key: events__sqs__stepfunctions.step-functions-execution-status-change-reaches-the-default-bus|_drain#37|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Key: events__sqs__stepfunctions.xsvc-sfn-2-redrive-status-change|_drain_once#14|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: 69fbb491696def14b5ee7dd45913958907d046a9

# events__sqs__stepfunctions: Step Functions publishes no "Step Functions Execution Status Change" event to the default bus, so per-status routing and RedriveExecution status events are both unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `69fbb491696def14b5ee7dd45913958907d046a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: an execution reaching a terminal status, and a subsequent RedriveExecution of it, emit nothing on the default EventBridge bus. Failing checks: events__sqs__stepfunctions.step-functions-execution-status-change-reaches-the-default-bus, events__sqs__stepfunctions.xsvc-sfn-2-redrive-status-change.

AWS: Step Functions publishes execution status changes to the DEFAULT event bus on a best-effort basis (https://docs.aws.amazon.com/eventbridge/latest/ref/events-ref-states.html) with `source` = `aws.states` and `detail-type` = `Step Functions Execution Status Change`. The envelope's `resources` carries the execution ARN, and its `detail` carries executionArn, stateMachineArn, name, status (RUNNING / SUCCEEDED / FAILED / TIMED_OUT / ABORTED), startDate and stopDate as epoch MILLISECONDS, input and output, and — for an execution produced by RedriveExecution — the redrive bookkeeping (redriveCount / redriveDate) alongside the new status. recorded behavior only for the exact detail body; the status vocabulary is the modelled ExecutionStatus.

Emulator: a rule on {"source":["aws.states"],"detail-type":["Step Functions Execution Status Change"]} with an SQS target stays empty after StartExecution runs to a terminal status, and equally empty after RedriveExecution. Both checks exhaust the harness budget: `Check timed out after 60.0s`. Nothing is published, so the startDate/stopDate unit, the status value and the redrive fields are all unobservable.

Why: any orchestration that reacts to a workflow finishing — chaining the next stage, alarming on FAILED, recording the output, or noticing that a redrive re-ran a failed execution — never fires, and a client cannot distinguish a workflow that never ran from one whose status event was never published.

Fix: the Step Functions execution state machine must publish the Execution Status Change envelope to the default EventBridge bus on each status transition, including the transitions a redrive produces (the execution-status transition path and its EventBridge publication).

Related filings: open issue 20261029T182228Z-events-sfn-step-functions-publishes-no-execution-status-change-event.md describes the same emission gap under the events__stepfunctions grouping and does NOT carry these two checks' replay keys; resolved issue 20261029T182228Z-events-sfn-sqs-no-step-functions-execution-status-change-event-ever.md claimed this surface for the events__sfn__sqs grouping, but these two checks still observe no event at all (an absence, not a refusal), so they are reported here as still-diverging rather than as that issue reopening.

Traces: events__sqs__stepfunctions.step-functions-execution-status-change-reaches-the-default-bus, events__sqs__stepfunctions.xsvc-sfn-2-redrive-status-change.

## Evidence
Expected (AWS): after StartExecution reaches a terminal status, the SQS target behind a rule on {"source":["aws.states"],"detail-type":["Step Functions Execution Status Change"]} receives at least one envelope whose detail.executionArn is the started execution, whose detail.status is its terminal status, and whose detail.startDate/stopDate are epoch milliseconds bracketing the run; after RedriveExecution, a further envelope carries the redriven execution's new status.
Actual (emulator): `Error: Check timed out after 60.0s` on both checks — the queue is never written to, in either the terminal-status or the redrive scenario.

## Steps to Reproduce
1. CreateQueue with a policy admitting events.amazonaws.com; PutRule on the DEFAULT bus with pattern {"source":["aws.states"],"detail-type":["Step Functions Execution Status Change"]}; PutTargets with the queue.
2. CreateStateMachine and StartExecution; poll DescribeExecution until it is terminal.
3. ReceiveMessage on the queue — nothing arrives.
4. RedriveExecution the (failed) execution and poll the queue again — still nothing.
5. Re-replay the two checks in trace_ids to confirm.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs__stepfunctions.step-functions-execution-status-change-reaches-the-default-bus events__sqs__stepfunctions.events-workload-state-change-sfn-each-terminal-status-rule-receives-only-its-own-status events__sqs__stepfunctions.events-workload-state-change-sfn-aborted-event-follows-running-and-carries-the-stop-error-and-cause events__sqs__stepfunctions.events-workload-state-change-sfn-failed-event-carries-the-fail-states-runtime-error-and-cause events__sqs__stepfunctions.events-workload-state-change-sfn-express-execution-status-change-not-delivered-while-standard-twin-is events__sqs__stepfunctions.xsvc-sfn-2-large-input-status-change-not-included events__sqs__stepfunctions.xsvc-sfn-2-redrive-status-change events__sqs__stepfunctions.xsvc-sfn-2-waittoken-producer
Issue-Key: events__sqs__stepfunctions.step-functions-execution-status-change-reaches-the-default-bus|_drain#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Key: events__sqs__stepfunctions.events-workload-state-change-sfn-each-terminal-status-rule-receives-only-its-own-status|_drain_once#19|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: events__sqs__stepfunctions.events-workload-state-change-sfn-aborted-event-follows-running-and-carries-the-stop-error-and-cause|_drain_once#17|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: events__sqs__stepfunctions.events-workload-state-change-sfn-failed-event-carries-the-fail-states-runtime-error-and-cause|_drain_once#20|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: events__sqs__stepfunctions.events-workload-state-change-sfn-express-execution-status-change-not-delivered-while-standard-twin-is|_drain_once#18|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: events__sqs__stepfunctions.xsvc-sfn-2-large-input-status-change-not-included|_drain_once#16|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: events__sqs__stepfunctions.xsvc-sfn-2-redrive-status-change|_drain_once#14|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: events__sqs__stepfunctions.xsvc-sfn-2-waittoken-producer|run#20|Timeout|time.sleep(2)
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# No Step Functions Execution Status Change event ever reaches EventBridge, so per-status routing, the stop/fail error and cause, Express suppression, redrive and large-input omission are all unobservable

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: the aws.states "Step Functions Execution Status Change" event is never published, which collapses eight distinct behaviours into one silence.
AWS: Step Functions publishes on the default bus source "aws.states", detail-type "Step Functions Execution Status Change", once per status transition. The detail carries executionArn, stateMachineArn, name, status, startDate, stopDate, input, inputDetails, output and outputDetails, plus error and cause when the execution failed or was stopped: StopExecution's Error and Cause are echoed on the ABORTED event, and a Fail state's Error/Cause (or the runtime error, e.g. States.Runtime) appear on the FAILED event. A rule scoped to one status in detail.status receives only that status and not its siblings. Express (STANDARD vs EXPRESS) workflows do not publish execution status-change events at all, while a STANDARD twin does. When input or output exceeds the event size limit the corresponding member is omitted and its inputDetails/outputDetails.included is false rather than the payload being truncated into the event. A redriven execution publishes a further status change carrying its redrive accounting.
Emulator: no aws.states envelope is ever delivered to any rule's SQS target; all eight checks end "Check timed out after 60.0s". Consequently the per-status routing, the error/cause echo, the Express-vs-Standard differential, the large-input omission and the redrive event cannot be observed at all.
Why: every workflow-completion-driven integration (stage chaining, failure alarms, output capture, redrive monitoring) is dead; and because nothing is delivered, a correctly suppressed Express event is indistinguishable from the broken path, so a client cannot tell which of its assumptions hold.
Fix: the Step Functions execution state machine must publish Execution Status Change events to the EventBridge bus on each transition, with error/cause populated from StopExecution and from the terminal failure, with no event for EXPRESS workflows, and with input/output omitted plus inputDetails/outputDetails.included=false when they exceed the event size limit.
Traces: all eight check ids listed in trace_ids.

## Evidence
All eight checks end "Error: Check timed out after 60.0s" with an empty SQS target behind a rule matching {"source":["aws.states"],"detail-type":["Step Functions Execution Status Change"]} (and, in the per-status checks, rules scoped by detail.status). Expected: one envelope per status transition of a STANDARD execution, with detail.status, and with detail.error/detail.cause set on ABORTED (from StopExecution's Error/Cause) and FAILED (from the terminal failure); none for an EXPRESS execution; and detail.input omitted with detail.inputDetails.included=false for an oversize input.

## Steps to Reproduce
PutRule on {"source":["aws.states"]} (and variants scoped by detail.status) with SQS targets and permissive queue policies. Then: StartExecution to SUCCEEDED, FAILED (via a Fail state), and ABORTED (via StopExecution with Error/Cause); StartExecution on an EXPRESS and a STANDARD state machine; StartExecution with an oversize input; RedriveExecution a failed execution. Poll every queue — nothing arrives for any of them.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative|run#11|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f

# lambda__sqs__stepfunctions: an SQS event source mapping never invokes its function for the queue it is wired to, so the 'not triggered for a mismatched queue ARN' check can never complete its positive control

> _Note: this issue was identified against the code base at commit `8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an SQS event source mapping created against a standard queue reports a usable State but never polls that queue, so the function is never invoked and no observable progress is made; a check that first proves the mapping does NOT fire for an unrelated queue and then proves it DOES fire for its own queue runs to its budget and is killed.

AWS: recorded behavior only — Lambda's Smithy model declares CreateEventSourceMapping (EventSourceArn, FunctionName, BatchSize) and GetEventSourceMapping returning State, but the poller's delivery semantics are not modelled. The recording shows that after CreateEventSourceMapping(EventSourceArn=<queue B ARN>, FunctionName=<fn>, BatchSize=10) reaches State 'Enabled', a message sent to an unrelated queue A is never delivered to the function and stays in A (GetQueueAttributes ApproximateNumberOfMessages = the number sent, 1 in this scenario), while a message sent to the WIRED queue B is delivered to the function within seconds as an event whose Records[].body is that message body — the handler forwards it to an independent results queue, where the client reads it back unchanged. The queue names, URLs, ARNs and the message bodies are taken from the request; only the wiring rule (deliver from the mapping's EventSourceArn, never from another queue) is the requirement.

Emulator: the run never gets past this scenario: 'Error: Check timed out after 60.0s'. Nothing ever reaches the results queue — neither wrongly from the unwired queue nor rightly from the wired queue — so the mapping's poller does not deliver for the queue it is actually attached to and the positive control cannot be satisfied. (Had the emulator instead delivered from the unwired queue, the check would have failed with its negative assertion 'lambda was invoked for a message sent to a queue the mapping does not watch' rather than running out of budget, so the observed outcome is an absence of delivery, not a mis-wired delivery.)

Why: SQS-triggered Lambda is one of the most common AWS integrations. A client that wires a consumer this way sees a queue that is never drained and a function that is never invoked, with no error anywhere to explain it — CreateEventSourceMapping succeeds and GetEventSourceMapping reports a State that implies it is running. Any application whose correctness depends on the mapping consuming its queue (fan-in workers, DLQ reprocessors, Step Functions callbacks driven from a queue) simply does not run, and the wiring guarantee the check exercises — that a mapping delivers from exactly its EventSourceArn and from no other queue — cannot be observed at all.

Fix: the Lambda event-source-mapping poller/runtime for SQS sources (the lambda service's ESM component): the mapping must actually long-poll its EventSourceArn, build the Records[] event (body, receipt handle, attributes) and invoke the target function, honouring BatchSize, while never reading from any other queue. Related open issue 20261107T081525Z-lambda-sqs-stepfunctions-3-net-new-causes-a.md describes the ESM scale-out side of this area (its cause 3) but carries a different check's replay key, so this failure is filed separately and should be closed together with it.

Traces: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative.

## Evidence
Failing check: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative [mismatch].

Expected (AWS): with an ESM wired to queue B, (a) a message sent to unrelated queue A produces no invocation and GetQueueAttributes on A reports ApproximateNumberOfMessages = 1, then (b) a message sent to B is consumed and the handler's forwarded copy appears on the results queue with body 'should-be-consumed'.

Actual (emulator): 'Error: Check timed out after 60.0s'. No message ever appears on the results queue, so the positive control ('positive control: mapping never invoked lambda for its own wired queue') cannot be reached within the budget. The negative assertion was never triggered, which shows the absence of any delivery rather than a delivery from the wrong queue.

## Steps to Reproduce
1. Create three standard SQS queues: unwired A, wired B, and a results queue.
2. Create a Lambda execution role with AWSLambdaSQSQueueExecutionRole and SQS send permission, and a python3.12 function whose handler forwards each Records[].body to the results queue and returns {'batchItemFailures': []}; wait for State 'Active'.
3. CreateEventSourceMapping(EventSourceArn=<B ARN>, FunctionName=<fn>, BatchSize=10) and poll GetEventSourceMapping until State is Enabled/Disabled.
4. SendMessage to A; poll the results queue — nothing must arrive, and GetQueueAttributes(A, ApproximateNumberOfMessages) must still report 1.
5. SendMessage 'should-be-consumed' to B and poll the results queue — AWS delivers the forwarded body within seconds; against the emulator nothing ever arrives and the check is killed at its budget.

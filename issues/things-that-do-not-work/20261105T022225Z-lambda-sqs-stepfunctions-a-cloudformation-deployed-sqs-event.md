Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.deploy-lambda-esm-reportbatchitemfailures-enabled-stack
Issue-Key: lambda__sqs__stepfunctions.deploy-lambda-esm-reportbatchitemfailures-enabled-stack|_drain_results#25|Timeout|resp = sqs.receive_message(
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# lambda__sqs__stepfunctions: a CloudFormation-deployed SQS event-source mapping with FunctionResponseTypes ReportBatchItemFailures never delivers, so the partial-batch contract never reaches a verdict

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One root cause. The failing check is not marked OWNED or ALREADY FILED in the replay report.

AWS: recorded behavior only for the delivery; the configuration is modeled (lambda AWS::Lambda::EventSourceMapping / CreateEventSourceMapping carries FunctionResponseTypes, whose only value is ReportBatchItemFailures). A stack that declares an AWS::Lambda::EventSourceMapping with FunctionResponseTypes: [ReportBatchItemFailures] and reaches CREATE_COMPLETE produces a mapping that polls the queue, invokes the function with an aws:sqs Records batch, and honours the handler's `{"batchItemFailures": [{"itemIdentifier": <messageId>}]}` reply: only the reported message ids become visible again, every other message in the batch is deleted.
Emulator: the stack settles but the mapping never consumes the queue and never invokes the function — the check waits for the invocation and its redelivery to become observable and is abandoned with "Check timed out after 60.0s" without reaching a verdict. The declared FunctionResponseTypes contract is therefore unobservable end to end.
Why: a client that deploys partial-batch-failure handling declaratively gets a mapping that neither processes nor redrives anything: messages sit in the queue, and the distinction the feature exists for — redelivering only the failed items instead of the whole batch — can never be verified or relied on.
Fix: the Lambda event-source-mapping poller for SQS sources (delivery plus batchItemFailures handling) and the CloudFormation resource provider for AWS::Lambda::EventSourceMapping that must register the mapping, with its FunctionResponseTypes, in Lambda's state.
Traces: lambda__sqs__stepfunctions.deploy-lambda-esm-reportbatchitemfailures-enabled-stack.

## Evidence
Expected: after the stack reaches CREATE_COMPLETE and messages are sent to the declared queue, the function is invoked with the batch and only the message ids it returns in batchItemFailures are redelivered.
Actual (replay): `Error: Check timed out after 60.0s` — no invocation is ever observed, so neither the delivery nor the partial-batch redelivery can be evaluated.

## Steps to Reproduce
1. CreateStack with a template declaring an SQS queue, a Lambda function whose handler returns a batchItemFailures list, and an AWS::Lambda::EventSourceMapping from the queue to the function with FunctionResponseTypes: [ReportBatchItemFailures].
2. Wait for CREATE_COMPLETE.
3. SendMessageBatch several messages to the queue and wait for the function to be invoked — it never is, and nothing is redelivered.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs.serverless-sqs-event-processes-message-and-drains-queue cloudformation__lambda__sqs.serverless-sqs-event-filter-criteria-blocks-nonmatching-message
Issue-Key: cloudformation__lambda__sqs.serverless-sqs-event-processes-message-and-drains-queue|_npm_install_serverless#14|Timeout|r = subprocess.run(
Issue-Key: cloudformation__lambda__sqs.serverless-sqs-event-filter-criteria-blocks-nonmatching-message|_npm_install_serverless#14|Timeout|r = subprocess.run(
Issue-Commit: 0c065617979d1ee76fa77b423bd58183fc04c861

# cloudformation__lambda__sqs: Serverless Framework deploys of an SQS event source never settle — both checks time out before any AWS assertion

> _Note: this issue was identified against the code base at commit `0c065617979d1ee76fa77b423bd58183fc04c861`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in two failing checks.

Cause 1 of 1: a `serverless deploy` (Serverless Framework 3.40.0) of a service whose function is bound to an `sqs` event never reaches a verdict against the emulator.

AWS: recorded behavior only — nothing in the Smithy model governs the Serverless Framework. The recording shows `serverless deploy` driving CloudFormation (CreateStack/UpdateStack plus DescribeStacks/DescribeStackEvents polling) to a terminal stack status, after which (a) a message sent to the queue is consumed by the function and the queue drains, and (b) a message that does not match the event source mapping's FilterCriteria is left unconsumed while a matching one is processed. In both cases the deploy command terminates.

Emulator: neither check reaches any AWS-level assertion — the harness reports `Error: Check timed out after 60.0s` for both. The Serverless Framework toolchain itself works in this environment (cloudformation__iam__lambda__logs__s3.serverless-remove-bucket-not-empty-blocks-stack-delete reaches a real AWS assertion in the same suite), so it is this deploy path that does not finish. Hypothesis: the stack the template creates (queue, function, role, log group and an AWS::Lambda::EventSourceMapping) never leaves an *_IN_PROGRESS status — the event-source-mapping resource in particular never reports a terminal resource status — so the framework's stack poll loop never returns.

Why: a client gets a deploy command that never returns and a stack with no terminal status: it cannot tell a hung deploy from a slow one, cannot roll back, and cannot safely retry because it does not know whether the queue, function and mapping exist. Every assertion about SQS-to-Lambda delivery and about FilterCriteria is unreachable, so filter behaviour is entirely unverified.

Fix: the CloudFormation stack-lifecycle state machine for the resource set a Serverless Framework `sqs` event template declares — the create/update path must reach a terminal StackStatus — and the AWS::Lambda::EventSourceMapping resource handler in that set, which must report a terminal resource status.

Traces: cloudformation__lambda__sqs.serverless-sqs-event-processes-message-and-drains-queue, cloudformation__lambda__sqs.serverless-sqs-event-filter-criteria-blocks-nonmatching-message.

## Evidence
Expected: `serverless deploy` returns, the stack reaches a terminal status, a sent message is consumed and the queue drains; with FilterCriteria set, a non-matching message is not delivered while a matching one is.

Actual (suite report, both checks, verdict mismatch):
```
cloudformation__lambda__sqs.serverless-sqs-event-processes-message-and-drains-queue
Error: Check timed out after 60.0s

cloudformation__lambda__sqs.serverless-sqs-event-filter-criteria-blocks-nonmatching-message
Error: Check timed out after 60.0s
```
No stack status and no delivery observation were ever made.

## Steps to Reproduce
1. Install Serverless Framework 3.40.0 pointed at the emulator.
2. Deploy a service with one function bound to an `sqs` event on a queue declared in the same service (once plain, once with `filterPatterns`).
3. Poll `cloudformation describe-stacks` for the generated stack: the StackStatus never leaves *_IN_PROGRESS and `serverless deploy` never returns.

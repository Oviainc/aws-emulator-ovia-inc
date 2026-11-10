Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.deploy-pipes-sqs-lambda-stack-positive lambda__pipes__sqs.deploy-pipes-missing-role-negative
Issue-Key: lambda__pipes__sqs.deploy-pipes-sqs-lambda-stack-positive|_poll_until#8|Timeout|last = probe()
Issue-Key: lambda__pipes__sqs.deploy-pipes-missing-role-negative|_poll_until#7|Timeout|last = probe()
Issue-Commit: 145c20a94e0bd5c99471752c6f247be888f6ff73

# lambda__pipes__sqs: 2 net-new causes — a CloudFormation-deployed SQS -> Pipe -> Lambda chain never delivers, so neither the happy path nor the under-scoped-role control settles

> _Note: this issue was identified against the code base at commit `145c20a94e0bd5c99471752c6f247be888f6ff73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Two root causes, both in the CloudFormation-deployed EventBridge Pipes path of this scenario service.

Cause 1 of 2: a stack-deployed SQS source -> Pipe -> Lambda target never delivers a sent message.
AWS: recorded behavior only for the end-to-end delivery (the pipes Smithy model describes CreatePipe/DescribePipe and the RUNNING state, not the runtime). The recording shows that once the stack is CREATE_COMPLETE and the pipe reports RUNNING, a message sent to the source queue is polled by the pipe, the Lambda target is invoked with it, and the target writes its marker plus the message's body field through to the result queue.
Emulator: nothing reaches the result queue — the check polls until its budget expires and reports `Check timed out after 60.0s`, so either the stack never settles or the pipe, though created, never polls its source.
Why: the standard declarative Pipes topology is inert: a client that deploys it and sends a message observes no invocation and no result, with the pipe still advertising RUNNING, which is indistinguishable from a healthy pipe with an empty queue.
Fix: the pipes runtime poller for an SQS source with a Lambda target, and the CloudFormation provisioning of AWS::Pipes::Pipe that must hand it a live pipe (the pipes service's pipe-execution component). Related but not covering: 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md and 20261102T094631Z-lambda-pipes-sqs-2-net-new-causes-a.md are open on API-created pipes; these two checks are the CloudFormation-deployed path.
Traces: lambda__pipes__sqs.deploy-pipes-sqs-lambda-stack-positive.

Cause 2 of 2: the under-scoped-pipe-role scenario cannot be graded because its positive sibling never delivers either.
AWS: recorded behavior only. In one stack, a pipe whose role lacks the source queue's `sqs:ReceiveMessage`/`DeleteMessage` permissions may still report RUNNING but never delivers its message, which stays on the source queue; a sibling pipe in the same stack with a correctly scoped role delivers its message to the target.
Emulator: neither leg is observable — the correctly scoped sibling never delivers, so the check exhausts its 60s budget before it can assert that the under-permissioned pipe withheld delivery.
Why: a client cannot verify that pipe role scoping actually gates delivery; a pipe that silently delivers nothing looks exactly like a pipe correctly blocked by IAM, so an authorization regression is invisible.
Fix: the same pipes runtime/polling component as Cause 1, plus the source-permission evaluation the poller must apply per pipe role.
Traces: lambda__pipes__sqs.deploy-pipes-missing-role-negative.

## Evidence
Cause 1 — deploy-pipes-sqs-lambda-stack-positive: 'Check timed out after 60.0s'; the check deploys the stack with timeout_s=420 and then polls ResultQueueUrl for the target's marker and the message's body field, which never appear.
Cause 2 — deploy-pipes-missing-role-negative: 'Check timed out after 60.0s'; the check deploys one stack containing both a correctly scoped pipe and a pipe whose role lacks source-queue poll permissions, and never observes the correctly scoped pipe's delivery.

## Steps to Reproduce
1. CreateStack with a template declaring a source SQS queue, a result SQS queue, a Lambda forwarder function + role, a pipe role, and an AWS::Pipes::Pipe wiring source -> Lambda target.
2. Wait for CREATE_COMPLETE and DescribePipe reporting RUNNING.
3. SendMessage to the source queue and poll the result queue: nothing ever arrives.
4. Repeat with a second stack that declares two pipes, one whose role omits sqs:ReceiveMessage/DeleteMessage on the source: the correctly scoped pipe also delivers nothing, so the negative guarantee cannot be observed.

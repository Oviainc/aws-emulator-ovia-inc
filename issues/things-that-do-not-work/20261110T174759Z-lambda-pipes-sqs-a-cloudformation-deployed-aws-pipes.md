Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.deploy-pipes-missing-role-negative
Issue-Key: lambda__pipes__sqs.deploy-pipes-missing-role-negative|run#18|Timeout|resp = sqs.receive_message(
Issue-Commit: ef9cd1f472238b59a7ccf5edc608de1fa0253c97

# lambda__pipes__sqs: a CloudFormation-deployed AWS::Pipes::Pipe with a correctly scoped role never delivers its SQS message to the Lambda target, so the under-permissioned negative has no control

> _Note: this issue was identified against the code base at commit `ef9cd1f472238b59a7ccf5edc608de1fa0253c97`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
One net-new root cause (1 of 1).

Cause 1 of 1: the positive control of the missing-role negative check — an AWS::Pipes::Pipe declared in a CloudFormation template with an SQS source, a REQUEST_RESPONSE Lambda target and a role granting sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source plus lambda:InvokeFunction on the target — never delivers. The check is killed at the harness's 60s budget while waiting for that delivery, so the under-permissioned arm it exists to contrast with is never reached.

AWS: recorded behavior only for the runtime — Smithy (services/service-com-amazonaws-pipes-pipes) models CreatePipe/DescribePipe with Source, SourceParameters.SqsQueueParameters.BatchSize, Target and TargetParameters.LambdaFunctionParameters.InvocationType, and states no runtime semantics. The recording shows that a stack-created pipe whose role can poll the source reaches RUNNING and then, for each SendMessage on the source queue, invokes the Lambda target synchronously with the SQS record envelope; the handler's SendMessage puts the marker on the result queue within the stack's stabilization window. A pipe whose role lacks sqs:ReceiveMessage still reports RUNNING but delivers nothing and leaves the message on the source queue.

Emulator: the positive control's result queue never receives the marker. The check waits up to 150s for it and is killed at the 60s budget: "Error: Check timed out after 60.0s". No pipe error, no StateReason, no partial delivery.

Why: a CloudFormation-declared pipe is inert, so a stack that looks fully deployed moves no messages, and the role-scoping behaviour a customer relies on for least privilege (delivery with the grant, silence without it) cannot be distinguished from the emulator's silence in both cases.

Fix: the pipes crate's SQS source poller and Lambda target invoker (services/service-com-amazonaws-pipes-pipes), and the AWS::Pipes::Pipe resource handler in the cloudformation crate that must produce a pipe which actually starts polling.

Related but not covering: open issues 20261107T081525Z-lambda-pipes-sqs-2-net-new-causes-a-40c11a.md and 20261104T213346Z-lambda-pipes-sqs-5-net-new-causes-an.md report CFN-deployed and role-scoped pipe delivery failures and carry other replay keys; neither carries this check's key.

Traces: lambda__pipes__sqs.deploy-pipes-missing-role-negative.

## Evidence
Expected: after the stack reaches CREATE_COMPLETE, a SendMessage on SourceQueueGood is polled by PipeGood and the forwarder Lambda's SendMessage puts the marker on ResultQueueGood; then an SDK-created pipe with a role lacking sqs:ReceiveMessage reports RUNNING but delivers nothing and the message stays on SourceQueueBad.
Actual: "Check timed out after 60.0s" — the positive control's marker never reaches ResultQueueGood (the check allows 150s for it), so the negative arm is never exercised.

## Steps to Reproduce
1. CreateStack with a template holding SourceQueueGood/ResultQueueGood (AWS::SQS::Queue), a python3.12 AWS::Lambda::Function forwarding each record body to ResultQueueGood, a pipes.amazonaws.com role granting sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on SourceQueueGood plus lambda:InvokeFunction on the function, and an AWS::Pipes::Pipe wiring them with SqsQueueParameters.BatchSize 1 and LambdaFunctionParameters.InvocationType REQUEST_RESPONSE.
2. SendMessage on SourceQueueGood and poll ResultQueueGood.
3. Nothing arrives. Replay lambda__pipes__sqs.deploy-pipes-missing-role-negative.

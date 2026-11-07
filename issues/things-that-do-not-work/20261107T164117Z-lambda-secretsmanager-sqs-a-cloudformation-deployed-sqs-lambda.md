Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret
Issue-Key: lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret|_poll_until#11|Timeout|last = probe()
Issue-Commit: 6bba0ea93be6d4db7a2caa629527597bd59ab221

# lambda__secretsmanager__sqs: a CloudFormation-deployed SQS→Lambda ESM whose function reads a secret never records an invocation, so the deployed pipeline never reaches a working end state

> _Note: this issue was identified against the code base at commit `6bba0ea93be6d4db7a2caa629527597bd59ab221`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
1 net-new root cause in this service.

Cause 1 of 1: a CloudFormation stack that creates a queue, a secret, a function and an SQS event source mapping deploys, but a message sent to the queue never produces an invocation that is observable in the function's CloudWatch Logs group, so the deployed pipeline never reaches a working end state. Failing check id: lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret

AWS: an enabled SQS event source mapping polls its source queue and invokes the function with an `aws.sqs` batch; the function's first invocation creates the log group `/aws/lambda/<FunctionName>` and a log stream, and the lines the handler writes become filterable events there (DescribeLogStreams/FilterLogEvents). The stack's Outputs (QueueUrl, FunctionName) must resolve to the created resources. Smithy: lambda#CreateEventSourceMapping takes EventSourceArn + FunctionName and reports `State`; the invocation side is recorded behavior, not modeled. Nothing in the model states a latency bound — the requirement is that the invocation happens at all and that its log lines appear under the function's own log group.

Emulator: the scenario never reaches that end state. The replay ends with `Check timed out after 60.0s` — no assertion text, no HTTP status, no error code — so after SendMessage no log stream/log event attributable to the ESM-driven invocation of the deployed function is ever observed inside the run budget.

Why: a client that deploys this extremely common shape by template (queue -> ESM -> function -> secret read) gets a stack that reports success while the data plane is inert: messages accumulate, nothing runs, and nothing is logged. Any integration test or consumer that waits for the function's output blocks indefinitely, with no error to branch on.

Fix: the SQS event-source-mapping poller/invoker and the function log-group plumbing behind it — a mapping created by CloudFormation must be polled and must invoke the deployed function, and that invocation must materialize `/aws/lambda/<FunctionName>` with the handler's output.

Traces: lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret. Related open issues in this service — 20261105T081139Z-lambda-secretsmanager-sqs-the-in-process-lambda-python.md (the in-process Lambda Python runtime) and 20261106T134622Z-lambda-secretsmanager-sqs-the-in-lambda-sdk-reports.md (what the in-Lambda SDK reports) — describe in-function SDK behavior; neither is registered as owning this check and neither covers the CloudFormation-deployed mapping never invoking at all, which is the subject here.

## Evidence
Expected (AWS): after deploying the template, Outputs carry QueueUrl and FunctionName; sqs:SendMessage(QueueUrl, "cfn-trigger") is followed by an ESM-driven invocation; logs:DescribeLogStreams on `/aws/lambda/<FunctionName>` returns at least one stream and its events carry the handler's output for that message.

Actual (emulator): `Check timed out after 60.0s`. No log stream or log event attributable to the invocation ever appears; the replay produces no HTTP status and no error code to quote, the pipeline is simply silent.

Note on values: the stack, queue, secret and function names and the log stream names are per-run values and are not part of the requirement; the requirement is that the message is delivered to the function and that the invocation is observable under the function's own log group.

## Steps to Reproduce
1. cloudformation:CreateStack / deploy the template `cfn-sqs-lambda-secret.yaml` (an AWS::SQS::Queue, an AWS::SecretsManager::Secret, an AWS::Lambda::Function whose role may read that secret, and an AWS::Lambda::EventSourceMapping from the queue to the function); wait for CREATE_COMPLETE and read Outputs QueueUrl and FunctionName.
2. sqs:SendMessage(QueueUrl=<QueueUrl>, MessageBody="cfn-trigger").
3. Poll logs:DescribeLogStreams(logGroupName="/aws/lambda/<FunctionName>") and then the stream's events for the handler's output.
4. Expected: a stream exists and carries the invocation's output. Observed: nothing appears and the check hits the 60s budget.
5. Replay check id lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret to confirm.

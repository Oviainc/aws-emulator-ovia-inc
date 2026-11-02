Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.deep-sfn-3-http-task-echo-roundtrip events__lambda__stepfunctions.deep-sfn-3-http-task-deauthorized-connection-rejected events__lambda__stepfunctions.deep-sfn-3-http-task-unknown-connection-rejected dynamodb__events__lambda__sqs__stepfunctions.eventbridge-fanout-lambda-sqs-sfn dynamodb__events__stepfunctions.eventbridge-to-sfn-to-ddb-chain events__iam__sqs__stepfunctions.xsvc-iam-3-events-role-starts-sfn-execution-with-the-event-as-input events__iam__sqs__stepfunctions.xsvc-iam-3-role-without-events-trust-rejected-at-delivery-failed-to-assume-role events__iam__sqs__stepfunctions.xsvc-iam-3-role-without-start-execution-denied-dead-letters-and-counts-failed-invocations events__lambda__s3__stepfunctions.s3-upload-triggers-lambda-then-putevents-starts-sfn events__lambda__s3__stepfunctions.s3-write-triggers-eventbridge-rule-to-stepfunctions events__lambda__stepfunctions.refarch-sfn-eventbridge-envelope-id-becomes-the-job-id events__lambda__stepfunctions.refarch-sfn-nonmatching-event-source-starts-no-execution
Issue-Key: events__lambda__stepfunctions|CreateConnection|NotImplemented
Issue-Key: dynamodb__events__lambda__sqs__stepfunctions.eventbridge-fanout-lambda-sqs-sfn|_poll_until#25|AssertionError|raise AssertionError(
Issue-Key: dynamodb__events__stepfunctions.eventbridge-to-sfn-to-ddb-chain|run#28|Timeout|time.sleep(3)
Issue-Key: events__iam__sqs__stepfunctions.xsvc-iam-3-events-role-starts-sfn-execution-with-the-event-as-input|_drain#21|Timeout|resp = sqs.receive_message(
Issue-Key: events__iam__sqs__stepfunctions.xsvc-iam-3-role-without-events-trust-rejected-at-delivery-failed-to-assume-role|_drain#29|Timeout|resp = sqs.receive_message(
Issue-Key: events__iam__sqs__stepfunctions.xsvc-iam-3-role-without-start-execution-denied-dead-letters-and-counts-failed-invocations|_drain#28|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__s3__stepfunctions.s3-upload-triggers-lambda-then-putevents-starts-sfn|_fire_until#20|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3__stepfunctions.s3-write-triggers-eventbridge-rule-to-stepfunctions|_poll_for_new_execution#18|Timeout|time.sleep(3)
Issue-Key: events__lambda__stepfunctions.refarch-sfn-eventbridge-envelope-id-becomes-the-job-id|run#17|Timeout|time.sleep(6)
Issue-Key: events__lambda__stepfunctions.refarch-sfn-nonmatching-event-source-starts-no-execution|run#14|Timeout|time.sleep(6)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# EventBridge: CreateConnection is a 501 scaffold stub, and rules never deliver events to their targets (2 causes)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
This issue bundles TWO distinct root causes in EventBridge. Both must be closed before submitting. Re-replay every check id in trace_ids.

Cause 1 of 2: `CreateConnection` is an unimplemented scaffold stub, so no EventBridge connection can exist.
AWS: CreateConnection creates a connection with an `AuthorizationType` (API_KEY / BASIC / OAUTH_CLIENT_CREDENTIALS) and returns `ConnectionArn`, `ConnectionState`, `CreationTime` and `LastModifiedTime`; the connection is then referenced by a Step Functions HTTP Task (`arn:aws:states:::http:invoke`) via its `Authentication.ConnectionArn`. `NotImplemented` is not a modeled error for this operation.
Emulator: `ClientError: An error occurred (NotImplemented) when calling the CreateConnection operation: operation CreateConnection is scaffolded but not implemented in this emulator`.
Why: the whole Step Functions HTTP-Task surface is unreachable — the happy-path round trip, the deauthorized-connection refusal and the unknown-connection refusal all abort at setup, so a client cannot emulate any third-party HTTP integration. Note: this stub was NOT flagged as already-filed by the replay harness, and no open issue names CreateConnection.
Fix: implement the CreateConnection handler in the eventbridge/events crate (`.../src/ops/create_connection.rs`) and the connection state it stores, so the connection ARN resolves for DescribeConnection/DeleteConnection and for the sfn HTTP-Task authentication lookup.
Traces: events__lambda__sfn.deep-sfn-3-http-task-echo-roundtrip, events__lambda__sfn.deep-sfn-3-http-task-deauthorized-connection-rejected, events__lambda__sfn.deep-sfn-3-http-task-unknown-connection-rejected.

Cause 2 of 2: a rule never delivers a matched event to its targets.
AWS: PutEvents against a bus with an enabled rule whose EventPattern matches causes EventBridge to assume the rule's RoleArn and invoke each target — `states:StartExecution` with the full event envelope as the execution input, `lambda:Invoke`, `sqs:SendMessage` — and to record FailedInvocations / dead-letter the event when the target call is denied or the role cannot be assumed.
Emulator: no target is ever invoked. The dependent checks make no progress and are killed by the harness with `Check timed out after 60.0s`; the one check that polls for the downstream write reports `Lambda fanout consumer wrote to DDB not met within 30s. Last result: None`.
Why: every event-driven architecture is inert — no fan-out, no S3-to-Lambda-to-EventBridge-to-Step-Functions chain, and none of the delivery-failure behaviours (role without the bus's trust policy, role without `states:StartExecution`, DLQ, FailedInvocations metric) can be observed. A client's integration tests hang instead of failing.
Fix: the rule-matching and target-dispatch component of the events crate (PutEvents -> pattern match -> role assumption -> per-target invoke), together with its failure accounting (DLQ + FailedInvocations).
Traces: dynamodb__events__lambda__sfn__sqs.eventbridge-fanout-lambda-sqs-sfn, dynamodb__events__sfn.eventbridge-to-sfn-to-ddb-chain, events__iam__sfn__sqs.xsvc-iam-3-events-role-starts-sfn-execution-with-the-event-as-input, events__iam__sfn__sqs.xsvc-iam-3-role-without-events-trust-rejected-at-delivery-failed-to-assume-role, events__iam__sfn__sqs.xsvc-iam-3-role-without-start-execution-denied-dead-letters-and-counts-failed-invocations, events__lambda__s3__sfn.s3-upload-triggers-lambda-then-putevents-starts-sfn, events__lambda__s3__sfn.s3-write-triggers-eventbridge-rule-to-stepfunctions, events__lambda__stepfunctions.refarch-sfn-eventbridge-envelope-id-becomes-the-job-id, events__lambda__stepfunctions.refarch-sfn-nonmatching-event-source-starts-no-execution.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConnection operation: operation CreateConnection is scaffolded but not implemented in this emulator. Expected: HTTP 200 with ConnectionArn/ConnectionState.
Cause 2: expected a Step Functions execution (or a Lambda invocation / SQS message) per matched event; got none — 'Check timed out after 60.0s' for eight checks and 'AssertionError: Lambda fanout consumer wrote to DDB not met within 30s. Last result: None' for the fan-out check.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: call events:CreateConnection with AuthorizationType='API_KEY' and an ApiKeyAuthParameters block.
3. Cause 2: events:PutRule with an EventPattern, events:PutTargets naming a Step Functions state machine (and separately a Lambda function and an SQS queue) with a RoleArn, then events:PutEvents with a matching event; poll sfn:ListExecutions / the Lambda's side effect / sqs:ReceiveMessage for the delivery.

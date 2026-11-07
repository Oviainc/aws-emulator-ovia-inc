Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage|_wait_for_200#16|Timeout|with urllib.request.urlopen(req, timeout=10) as resp:
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# REST stage access logging never converges: enabling AccessLogSettings never produces the per-stage log group

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: a REST API stage configured with AccessLogSettings never produces its access-log destination, so the check never converges and is killed at the harness deadline.

AWS: recorded behavior only. API Gateway accepts UpdateStage with accessLogSettings.destinationArn pointing at a CloudWatch Logs log group and, once the stage is deployed and a request is served through it, writes one access-log event per request into that log group. The recording shows the check polling for the per-stage log group to exist and then for records in it; on real AWS this converges well inside the 180 s the harness allows.

Emulator: the check produces no result at all — the replay detail is `Error: Check timed out after 180.0s`. Nothing is ever created at the configured destination and no request is ever observed through the stage, so every poll returns empty until the harness kills the check.

Why: a client that turns stage access logging on and then reads its log group to audit traffic (or that waits on the group's existence before wiring a subscription filter) blocks forever. Operators lose the only per-request record API Gateway produces, and any automation gated on that group never proceeds.

Fix: the REST stage access-logging path — UpdateStage/CreateStage validation and persistence of accessLogSettings, and the stage request path that must emit one access-log record per served request to the configured destination.

Traces: apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage.

## Evidence
Expected: within the check's polling window, the stage's access-log destination log group exists and receives one record per request served through the stage.
Actual: `Error: Check timed out after 180.0s` — no destination group and no records ever observed.

## Steps to Reproduce
1. Create a REST API with a Lambda proxy integration and deploy it to a stage.
2. Call UpdateStage with accessLogSettings { destinationArn: <log group arn>, format: <any> }.
3. Send a request through the deployed stage.
4. Poll logs:DescribeLogGroups / FilterLogEvents for the destination group. It never appears and never receives records; the behavioural check apigw-access-log-group-created-per-stage runs to the 180 s deadline.

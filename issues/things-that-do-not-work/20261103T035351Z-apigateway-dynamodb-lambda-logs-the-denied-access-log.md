Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected|_run_shell#36|Timeout|return result
Issue-Commit: 2d2373ac96719fb51b225d122133a9d558f5af51

# apigateway__dynamodb__lambda__logs: the denied access-log-destination check never reaches a verdict — the stage access-logging path hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `2d2373ac96719fb51b225d122133a9d558f5af51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1 — the flow that configures a stage's accessLogSettings against a log group the account's CloudWatch push role is explicitly denied logs:CreateLogGroup on never completes. AWS: the denied destination log group is never created and receives nothing, while a sibling stage pointed at a pre-created log group does receive access-log entries; the account-level cloudWatchRoleArn gates delivery, and the denied destination simply stays absent (DescribeLogGroups does not list it, and no events are delivered to it). Recorded behavior only — the Smithy model expresses accessLogSettings on Stage and UpdateStage's patchOperations but says nothing about delivery or the role's permissions. Emulator: the check is killed with `Check timed out after 60.0s` — no status, error code or log-group state is ever produced for either the denied or the sibling destination, so the emulator gives no answer at all for this flow. Hypothesis: a step in the sequence (the apigateway Account cloudWatchRoleArn configuration, the stage deploy, or the invoke the check performs to generate access logs) never converges, so the check never gets far enough to compare the two destinations. Why: a client cannot establish that a mis-permissioned access-log destination fails closed — that the log group is NOT auto-created and that nothing leaks into it — while a correctly permissioned stage still delivers; a hang is also indistinguishable from a slow but correct emulator, so this negative-delivery guarantee is simply unverifiable. Fix: whichever component in the stage access-logging path does not converge — the apigateway account-settings (cloudWatchRoleArn) handler, UpdateStage's accessLogSettings application, or the execute-api access-log delivery into CloudWatch Logs. Related open issues 20261029T180026Z-apigateway-dynamodb-lambda-logs-api-gateway-deployed-stage-invoke-account-logging-role.md and 20261031T010204Z-apigateway-dynamodb-lambda-logs-the-account-cloudwatch-role.md cover neighbouring causes in this same path but do not own this check.

## Evidence
From the simparity-behavioral-aws-v1 replay, under '### apigateway__dynamodb__lambda__logs': check cli-apigateway-stage-access-log-missing-destination-rejected reports exactly 'Check timed out after 60.0s'. The check requires the `aws` CLI (present — other CLI checks in the same service group did produce responses), generates a bad log-group ARN `arn:aws:logs:<region>:<account>:log-group:<name>`, configures the apigateway account CloudWatch role with a Deny scoped to that exact ARN, then creates the API and stages. No HTTP exchange or error is recorded before the cap.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite.
2. Create an IAM role trusted by apigateway with a policy that Denies logs:CreateLogGroup on one specific log-group ARN, and set it as the apigateway account cloudWatchRoleArn.
3. Create a REST API with a Lambda-backed method, deploy two stages: one with accessLogSettings.destinationArn pointing at the denied (non-existent) log group, one at a pre-created log group.
4. Invoke both stages, then check DescribeLogGroups and the log streams of both destinations — the emulator produces no result within 60 s.

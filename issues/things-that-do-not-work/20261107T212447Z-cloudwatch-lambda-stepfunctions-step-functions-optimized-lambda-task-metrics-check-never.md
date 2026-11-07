Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series
Issue-Key: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series|_start#10|Timeout|arn = sfn.start_execution(stateMachineArn=sm_arn, input=json.dumps(payload))["executionArn"]
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# Step Functions optimized Lambda task metrics check never converges — no AWS/States function-ARN series appears

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: an execution whose Task state uses the optimized Lambda integration never produces the per-function-ARN AWS/States metric series, and the check is killed at the harness deadline.

AWS: recorded behavior only. Step Functions publishes LambdaFunctionsSucceeded / LambdaFunctionsFailed (and the denied variant) into the AWS/States namespace dimensioned by the invoked function's ARN for optimized `arn:aws:states:::lambda:invoke` Task states, so both the denied and the OK legs populate that series shortly after the execution ends. The recording polls GetMetricStatistics for that series.

Emulator: the check produces no verdict — the replay detail is `Error: Check timed out after 180.0s`. The series is never populated, so the polling loop exhausts the harness deadline.

Why: a client that alarms on per-function Step Functions task failures has no data at all — neither the denied nor the successful invocation is counted — so a task that stops working raises no alarm and dashboards stay blank.

Fix: the Step Functions execution engine's CloudWatch metric emission for Lambda Task states — the AWS/States LambdaFunctions* metrics dimensioned by LambdaFunctionArn.

Traces: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series.

## Evidence
Expected: after an execution with an optimized lambda:invoke Task state, AWS/States metrics dimensioned by the function ARN report the succeeded/failed counts.
Actual: `Error: Check timed out after 180.0s` — the series is never populated.

## Steps to Reproduce
1. Create a state machine with a Task state whose Resource is arn:aws:states:::lambda:invoke, once with an execution role that may invoke the function and once without.
2. StartExecution and wait for it to end.
3. GetMetricStatistics in namespace AWS/States for LambdaFunctionsSucceeded/LambdaFunctionsFailed dimensioned by LambdaFunctionArn — no datapoints are ever returned.

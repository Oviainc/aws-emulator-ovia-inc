Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__sfn.metrics-states-stream-lambda-invoke-denied-role-fails-execution-and-function-series-stays-empty cloudwatch__lambda__sfn.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series cloudwatch__lambda__sfn.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series
Issue-Key: cloudwatch__lambda__sfn.metrics-states-stream-lambda-invoke-denied-role-fails-execution-and-function-series-stays-empty|_warm#8|Timeout|last = _wait_terminal(sfn, _start(ctx, sfn, sm, {"mode": "ok", "n": 0}))
Issue-Key: cloudwatch__lambda__sfn.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series|_warm#6|Timeout|last = _wait_terminal(sfn, _start(ctx, sfn, sm, {"mode": "ok", "n": 0}))
Issue-Key: cloudwatch__lambda__sfn.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series|_warm#10|Timeout|last = _wait_terminal(sfn, _start(ctx, sfn, sm, {"n": 0}))
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# cloudwatch__lambda__sfn: Step Functions Lambda-task work is never accounted in the AWS/States [LambdaFunctionArn] or AWS/Lambda series, so all three metric-attribution checks hang

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Three distinct metric-attribution root causes, all in the Step Functions -> Lambda -> CloudWatch path. Every one of them ends in `Error: Check timed out after 60.0s`: the series the check polls never materialises, so the poller never settles.

Cause 1 of 3 — denied Lambda task: a state machine whose role carries no lambda:InvokeFunction grant.
AWS: recorded behavior only (CloudWatch's model defines GetMetricStatistics/GetMetricData but no publisher semantics). AWS publishes, for that denied task, an execution that FAILS with error `Lambda.AWSLambdaException`, leaves AWS/Lambda Invocations for the function empty, never increments AWS/States LambdaFunctionsSucceeded under Dimensions [LambdaFunctionArn], and DOES count the task as started/failed in the LambdaFunctionArn family.
Emulator: none of it is observable — the check never reaches a verdict within 60s, so neither the execution's terminal failure nor the presence/absence of the two metric families can be read.
Failing check: cloudwatch__lambda__sfn.metrics-states-stream-lambda-invoke-denied-role-fails-execution-and-function-series-stays-empty.

Cause 2 of 3 — granted Lambda task accounting: a Task whose Resource is a bare Lambda function ARN, run twice (one return, one raise).
AWS: recorded behavior only. AWS/States records, under Dimensions exactly [LambdaFunctionArn], LambdaFunctionsScheduled 2, LambdaFunctionsStarted 2, LambdaFunctionsSucceeded 1, LambdaFunctionsFailed 1 and two LambdaFunctionRunTime samples, while the same work reads ExecutionsSucceeded 1 / ExecutionsFailed 1 under the StateMachineArn dimension.
Emulator: the [LambdaFunctionArn] series never appears; the check polls to the 60s budget and is killed.
Failing check: cloudwatch__lambda__sfn.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series.

Cause 3 of 3 — optimized integration attribution: a function called only through `arn:aws:states:::lambda:invoke`.
AWS: recorded behavior only. The Resource form decides which AWS/States metric family records the work: the optimized integration populates the ServiceIntegration family (and the denied twin's executions fail), and the [LambdaFunctionArn] family is NOT populated for it.
Emulator: no series is ever readable for either machine, so the attribution cannot be distinguished; the check times out at 60s.
Failing check: cloudwatch__lambda__sfn.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series.

Why: dashboards and alarms built on AWS/States Lambda-task counters and on AWS/Lambda Invocations see nothing at all, so a workflow whose Lambda task is denied is indistinguishable from one that never ran, and the per-function attribution AWS guarantees is unavailable. Every consumer that polls these series blocks instead of reading zero-or-more datapoints.

Fix: the Step Functions execution path that dispatches Lambda tasks (both the bare-function-ARN and the optimized lambda:invoke Resource forms, including the authorization failure that must surface as Lambda.AWSLambdaException) and the CloudWatch metric publication for the AWS/States LambdaFunctions* / LambdaFunctionRunTime and AWS/Lambda Invocations series, plus whatever keeps those reads from returning. Related open issue 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md describes the optimized-integration dispatch gap but does not carry these three checks.

Traces: the three check ids listed above.

## Evidence
All three failures report `Error: Check timed out after 60.0s` (replay verdicts [mismatch]).
- metrics-states-stream-lambda-invoke-denied-role-fails-execution-and-function-series-stays-empty: expected execution FAILED with error Lambda.AWSLambdaException, AWS/Lambda Invocations empty, AWS/States LambdaFunctionsSucceeded absent under [LambdaFunctionArn]; actual: no verdict within 60s.
- metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series: expected Scheduled 2 / Started 2 / Succeeded 1 / Failed 1 and 2 LambdaFunctionRunTime samples under [LambdaFunctionArn], with ExecutionsSucceeded 1 / ExecutionsFailed 1 under StateMachineArn; actual: series never readable.
- obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series: expected the optimized integration NOT to populate the [LambdaFunctionArn] family while its executions succeed (and the denied twin's fail); actual: nothing observable.

## Steps to Reproduce
1. Create a state machine with a Task whose Resource is a Lambda function ARN; grant its role lambda:InvokeFunction.
2. Start two executions (one where the handler returns, one where it raises) and poll GetMetricStatistics for AWS/States LambdaFunctionsScheduled/Started/Succeeded/Failed and LambdaFunctionRunTime with Dimensions=[LambdaFunctionArn], plus AWS/Lambda Invocations for the function.
3. Repeat with a twin machine whose role has no lambda:InvokeFunction grant: AWS fails the execution with Lambda.AWSLambdaException and leaves the function's series empty.
4. Repeat with Resource arn:aws:states:::lambda:invoke and check the [LambdaFunctionArn] family stays unpopulated.
5. Against the emulator each of the three scenarios polls to the 60s harness budget without ever producing the execution outcome or the metric series.

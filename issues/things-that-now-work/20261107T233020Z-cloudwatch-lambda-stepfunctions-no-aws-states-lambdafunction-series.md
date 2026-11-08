Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series
Issue-Key: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series|_ledger#23|Timeout|time.sleep(_POLL_S)
Issue-Commit: 69a812023dc7b19082ccb88a67a077b5f3204d48

# cloudwatch__lambda__stepfunctions: no AWS/States LambdaFunction* series is ever published for an optimized lambda:invoke task, so the denied/allowed metric check never converges

> _Note: this issue was identified against the code base at commit `69a812023dc7b19082ccb88a67a077b5f3204d48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
One net-new root cause, 1 failing check.

Cause 1 of 1: the AWS/States per-function metric series a direct-ARN Lambda Task publishes never appear, so the check polling for them runs to the harness deadline.
  AWS: recorded behavior only for the metric emission (CloudWatch's Smithy model declares GetMetricData/ListMetrics but not which series Step Functions publishes). The recording shows that a Task whose Resource is a raw function ARN counts in the AWS/States LambdaFunctionsStarted/Succeeded/Failed series dimensioned by LambdaFunctionArn, while allowed and denied optimized arn:aws:states:::lambda:invoke calls do NOT populate those per-function series, and the denied ones never reach AWS/Lambda Invocations. All of those series settle inside the check's own 420 s settle budget, with each poll answered promptly.
  Emulator: 'Check timed out after 180.0s' — the check never gets a verdict because the series it polls for never materialize (and the executions it drives first must settle). No AWS/States function-ARN datapoint is observed at all.
  Why: a client cannot observe or alarm on Step Functions' Lambda-integration metrics, and cannot distinguish an optimized integration from a direct-ARN Task — which is precisely what these series are for. An empty series is indistinguishable from a healthy workflow.
  Note: two sibling checks in this service are already owned (20261105T081644Z-cloudwatch-lambda-stepfunctions-a-lambda-task-whose-function.md, 20261107T044335Z-cloudwatch-lambda-stepfunctions-aws-states-lambda-task-metric.md) and 20261107T212447Z-cloudwatch-lambda-stepfunctions-step-functions-optimized-lambda-task-metrics-check-never.md describes the same shape of non-convergence; none of them carries this check's replay key, so it is filed here.
  Fix: the AWS/States metric emission for Lambda task integrations in the sfn service crate (publishing LambdaFunctionsStarted/Succeeded/Failed with the LambdaFunctionArn dimension for direct-ARN Tasks and not for optimized lambda:invoke), and the cloudwatch crate's storage/retrieval of those datapoints.
  Traces: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series.

## Evidence
Error: Check timed out after 180.0s

The check creates two functions, an alias, a granting and a bare states role, warms the grant through the alias ARN, then runs an optimized, a denied and a legacy (direct-ARN) state machine and polls GetMetricData for the AWS/States LambdaFunction* series (its own budget is 420 s with a 2 s poll). Expected: the direct-ARN Task's LambdaFunctionArn series carries datapoints while the optimized/denied ones do not, with the denied call absent from AWS/Lambda Invocations. Observed: the run never reaches that verdict.

## Steps to Reproduce
1. Create two python Lambdas and a states.amazonaws.com role granted lambda:InvokeFunction on both (plus their :* ARNs), and a second states role with no permissions.
2. Create three state machines: one Task with Resource = the optimized arn:aws:states:::lambda:invoke on function M under the granting role, the same under the bare role, and one Task whose Resource is the raw function ARN of function L under the granting role.
3. StartExecution on each, then poll cloudwatch GetMetricData for AWS/States LambdaFunctionsStarted/Succeeded/Failed dimensioned by LambdaFunctionArn, and AWS/Lambda Invocations by FunctionName. The direct-ARN series must become non-empty; in the emulator nothing arrives and the check exhausts its budget.

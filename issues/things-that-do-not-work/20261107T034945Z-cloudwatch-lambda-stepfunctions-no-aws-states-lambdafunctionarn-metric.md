Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series
Issue-Key: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series|_stats#22|Timeout|return cw.get_metric_statistics(
Issue-Commit: 04fdd45ea890c13ea5d31cc41f99075331e6a703

# cloudwatch__lambda__stepfunctions: no AWS/States LambdaFunctionArn metric series is ever published for a Lambda-ARN Task, so the metric read never settles

> _Note: this issue was identified against the code base at commit `04fdd45ea890c13ea5d31cc41f99075331e6a703`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1 — the AWS/States Lambda-function metric family is not emitted.
AWS: recorded behavior (Step Functions dev guide, procinfo-metrics): a Task whose Resource is a Lambda function ARN is counted by the AWS/States namespace under the single dimension [LambdaFunctionArn] — LambdaFunctionsScheduled, LambdaFunctionsStarted, LambdaFunctionsSucceeded, LambdaFunctionsFailed as counts and LambdaFunctionRunTime as a timed series — while the execution-level counters (ExecutionsStarted/Succeeded/Failed) are published under [StateMachineArn], and neither family crosses into the other's dimension.
Emulator: the check starts the executions and then polls CloudWatch for those series; no reading for the dimension under test ever appears, so the replay ends on its budget with the series empty. The execution itself is driven by the emulator, and the metric read is answered with no datapoints.
Why: every dashboard, alarm and autoscaling rule built on the AWS/States Lambda metric family sees a permanently empty series — indistinguishable from "the task never ran" — so a client cannot observe or alarm on task-level failures at all.
Fix: the Step Functions interpreter's metric emission for Lambda task integrations (publish the AWS/States LambdaFunctions* and LambdaFunctionRunTime series under [LambdaFunctionArn]) and the CloudWatch metric store that serves them.
Traces: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series

The failure surfaces as the replay's wall-clock budget ending while polling; the observable that never appears is the metric series, not a client-side resource.

## Evidence
From this replay of simparity-behavioral-aws-v1: the check 'metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series' ended with "Check timed out after 60.0s" while polling CloudWatch (its own metric budget is 600 s) for Scheduled 2 / Started 2 / Succeeded 1 / Failed 1 and two RunTime samples under [LambdaFunctionArn], plus the execution counters under [StateMachineArn]. No datapoint for either dimension had been served when the budget ended.

## Steps to Reproduce
1. Create a Lambda function and an IAM role trusted by states.amazonaws.com with lambda:InvokeFunction on it.
2. Create a STANDARD state machine with a Task whose Resource is the function ARN (raw ARN, not arn:aws:states:::lambda:invoke) and run it twice — once succeeding, once failing.
3. Poll cloudwatch:GetMetricStatistics / GetMetricData in namespace AWS/States for LambdaFunctionsScheduled, LambdaFunctionsStarted, LambdaFunctionsSucceeded, LambdaFunctionsFailed and LambdaFunctionRunTime with Dimension LambdaFunctionArn=<the function ARN>, and for ExecutionsStarted/Succeeded/Failed with Dimension StateMachineArn=<the state machine ARN>.

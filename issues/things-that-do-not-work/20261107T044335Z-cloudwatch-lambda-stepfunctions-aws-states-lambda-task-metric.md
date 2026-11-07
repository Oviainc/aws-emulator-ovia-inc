Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series
Issue-Key: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series|_poll#22|Timeout|time.sleep(_POLL_S)
Issue-Commit: 5c9d64a1694efbc03a63b34aabdcd776fcb5feb2

# cloudwatch__lambda__stepfunctions: AWS/States Lambda-task metric series under [LambdaFunctionArn] are never published, so metric attribution for a function-ARN Task is unreadable

> _Note: this issue was identified against the code base at commit `5c9d64a1694efbc03a63b34aabdcd776fcb5feb2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: a Step Functions Task whose Resource is a bare Lambda function ARN produces no AWS/States metric datapoints in the [LambdaFunctionArn] dimension family, and the check polling for them never settles.

AWS: recorded behavior only — CloudWatch's Smithy model defines GetMetricStatistics/GetMetricData but no publisher semantics, and the Step Functions model states nothing about metric emission. The recording shows that running such a Task twice (one handler return, one handler raise) is accounted in namespace AWS/States under Dimensions exactly [{Name: LambdaFunctionArn, Value: <function ARN>}] as LambdaFunctionsScheduled = 2, LambdaFunctionsStarted = 2, LambdaFunctionsSucceeded = 1, LambdaFunctionsFailed = 1, plus two LambdaFunctionRunTime samples; the same work is accounted under Dimensions [{Name: StateMachineArn, Value: <state machine ARN>}] as ExecutionsSucceeded = 1 / ExecutionsFailed = 1, and the two families never cross (LambdaFunctionsSucceeded/Scheduled must be absent under [StateMachineArn] and ExecutionsSucceeded absent under [LambdaFunctionArn]). AWS/Lambda Invocations for the function is at least the two task invocations, and AWS/Lambda Errors is 1. The counts above are the shape of the requirement, not literal values: they are a function of how many times the state machine schedules, starts, succeeds and fails the task in the request window.

Emulator: none of those series ever becomes readable. GetMetricStatistics keeps returning no datapoints for every one of the polled metrics, so the check polls to the harness budget and is killed — 'Error: Check timed out after 60.0s'. No verdict about either dimension family can be obtained, and the execution-level assertions about the function's return value and its 'ValueError' error cannot be confirmed either.

Why: every dashboard, alarm and autoscaling rule built on the AWS/States Lambda-task counters sees nothing. A client cannot distinguish a workflow whose Lambda task failed from one that never dispatched a task at all, and the per-function attribution AWS guarantees (task counters keyed by LambdaFunctionArn, execution counters keyed by StateMachineArn) is simply unavailable. Consumers that poll for a datapoint block instead of reading zero-or-more datapoints.

Fix: the Step Functions execution path that dispatches a Lambda Task given a bare function ARN must publish the AWS/States LambdaFunctionsScheduled / LambdaFunctionsStarted / LambdaFunctionsSucceeded / LambdaFunctionsFailed / LambdaFunctionRunTime metrics under the LambdaFunctionArn dimension and the Executions* metrics under the StateMachineArn dimension, and the CloudWatch metric store must serve them to GetMetricStatistics. Related open issue 20261101T210635Z-cloudwatch-lambda-sfn-step-functions-lambda-task-work.md describes the same area but does not carry this check's replay key, so this failure is filed separately and should be closed together with it.

Traces: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series

## Evidence
Expected: after two executions of a Task whose Resource is the function ARN, GetMetricStatistics returns AWS/States LambdaFunctionsScheduled 2, LambdaFunctionsStarted 2, LambdaFunctionsSucceeded 1, LambdaFunctionsFailed 1 and 2 LambdaFunctionRunTime samples under Dimensions [LambdaFunctionArn], ExecutionsSucceeded 1 / ExecutionsFailed 1 under [StateMachineArn], AWS/Lambda Errors 1 and Invocations >= 2; and empty results for the cross-family reads.
Actual: 'Error: Check timed out after 60.0s' — the polled sums never reach the expected values because no datapoints are ever published for these series.

## Steps to Reproduce
1. Create a Lambda function whose handler returns {'echo': n} and raises ValueError when event['mode'] == 'boom'.
2. Create a state machine role granting lambda:InvokeFunction on the function and a state machine with a single Task whose Resource is the bare function ARN.
3. StartExecution twice: once with {'mode':'ok'} and once with {'mode':'boom'}; wait for both to be terminal (SUCCEEDED and FAILED with error ValueError).
4. Poll GetMetricStatistics for AWS/States LambdaFunctionsScheduled/Started/Succeeded/Failed and LambdaFunctionRunTime with Dimensions=[{LambdaFunctionArn}], and for ExecutionsSucceeded/ExecutionsFailed with Dimensions=[{StateMachineArn}].
5. Against the emulator every read stays empty until the check is killed at the 60s budget.

Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series
Issue-Key: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series|_stat#23|Timeout|return float(sum(p[stat] for p in pts))
Issue-Commit: 01d998bd46b5a2138daf20ca3b3d6fda22ea2f8c

# cloudwatch__lambda__stepfunctions: no AWS/States LambdaFunction* per-function series is published for a direct-ARN Lambda Task, so the optimized-vs-legacy metric ledger never converges

> _Note: this issue was identified against the code base at commit `01d998bd46b5a2138daf20ca3b3d6fda22ea2f8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause, 1 failing check.

Cause 1 of 1: the AWS/States Lambda-task metric series dimensioned by LambdaFunctionArn are never published, so a check that polls for them (and for the AWS/Lambda Invocations series of the invoked function) never gets a verdict and is killed at the harness deadline.

  AWS: recorded behavior only for the metric emission itself — CloudWatch's Smithy model declares the read path but not which series Step Functions publishes (Smithy: GetMetricStatistics requires Namespace, MetricName, StartTime, EndTime, Period, with Dimensions @length max 30 and Statistics @length 1..5; modeled errors InvalidParameterCombination/InvalidParameterValue/MissingParameter/InternalServiceError; ListMetrics is the companion discovery call). The recording shows the behavior under test: a Task whose Resource is a RAW Lambda function ARN (the legacy integration) counts in AWS/States LambdaFunctionsScheduled, LambdaFunctionsStarted, LambdaFunctionsSucceeded and LambdaFunctionRunTime dimensioned by LambdaFunctionArn, with LambdaFunctionRunTime Minimum at least the function's own sleep duration; a Task using the OPTIMIZED arn:aws:states:::lambda:invoke integration does NOT populate those per-function series (Scheduled/Started/Succeeded/Failed all zero for that function ARN) but does count in AWS/Lambda Invocations by FunctionName; a denied optimized call fails the execution (error Lambda.AWSLambdaException, cause naming the missing lambda:InvokeFunction permission), counts in AWS/States ExecutionsFailed by StateMachineArn, and never reaches AWS/Lambda Invocations. Every one of those series settles well inside the check's own 420 s settle budget, each GetMetricStatistics poll answered promptly.

  Emulator: the check reports only 'Error: Check timed out after 180.0s'. No AWS/States LambdaFunctionArn-dimensioned datapoint is ever observed, so the polling loop (_ledger, 2 s poll) can never reach its expected ledger and the run is cut off before any assertion can be evaluated. The read path answers, but with an empty Datapoints list for these series.

  Why: a client cannot observe or alarm on Step Functions' Lambda-integration metrics at all, and cannot distinguish an optimized arn:aws:states:::lambda:invoke Task from a direct-ARN Task — which is exactly what the per-LambdaFunctionArn series exist for. An always-empty series is indistinguishable from a healthy, idle workflow, so latency and failure alarms built on LambdaFunctionRunTime / LambdaFunctionsFailed silently never fire. The denied-invoke leg additionally cannot be told apart from the allowed one.

  Note: an open issue already describes this same non-convergence and even carries this check id — 20261107T233020Z-cloudwatch-lambda-stepfunctions-no-aws-states-lambdafunction-series.md — but the replay report does not mark this check as owned by it (its recorded Issue-Key pins a frame/location that this run's bare timeout no longer matches), so the failure is re-filed here rather than dropped. Sibling causes in this service are owned by 20261105T081644Z-cloudwatch-lambda-stepfunctions-a-lambda-task-whose-function.md, 20261107T044335Z-cloudwatch-lambda-stepfunctions-aws-states-lambda-task-metric.md and 20261107T212447Z-cloudwatch-lambda-stepfunctions-step-functions-optimized-lambda-task-metrics-check-never.md; none of those carries this check's replay key either.

  Fix: the AWS/States metric emission for Lambda task integrations in the Step Functions service crate — publish LambdaFunctionsScheduled/Started/Succeeded/Failed and LambdaFunctionRunTime with the LambdaFunctionArn dimension for direct-ARN Tasks and deliberately NOT for the optimized lambda:invoke integration — together with the CloudWatch crate's storage and GetMetricStatistics retrieval of those datapoints, and the ExecutionsSucceeded/ExecutionsFailed emission keyed by StateMachineArn.

  Traces: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series.

## Evidence
Failing check: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series
Verdict: mismatch
Error: Check timed out after 180.0s

Expected (recorded AWS behavior), read with GetMetricStatistics over a window starting 2 minutes before the first execution, Period=60:
  AWS/States LambdaFunctionsScheduled [LambdaFunctionArn=<legacy function ARN>] Sum = number of direct-ARN Task invocations (non-zero)
  AWS/States LambdaFunctionsStarted   [LambdaFunctionArn=<legacy function ARN>] Sum = same non-zero count
  AWS/States LambdaFunctionsSucceeded [LambdaFunctionArn=<legacy function ARN>] Sum = same non-zero count
  AWS/States LambdaFunctionRunTime    [LambdaFunctionArn=<legacy function ARN>] SampleCount = same non-zero count, Minimum >= the handler's sleep duration in ms
  AWS/States LambdaFunctionsScheduled/Started/Succeeded/Failed [LambdaFunctionArn=<optimized function ARN>] Sum = 0
  AWS/Lambda  Invocations [FunctionName=<optimized function name>] Sum = number of ALLOWED optimized invocations only (denied ones absent)
  AWS/States ExecutionsSucceeded [StateMachineArn=<optimized machine>] Sum = its succeeded execution count
  AWS/States ExecutionsFailed    [StateMachineArn=<denied machine>] Sum = its denied execution count
The denied executions themselves end FAILED with error "Lambda.AWSLambdaException" and a cause containing "lambda:InvokeFunction".

Observed (emulator): the AWS/States LambdaFunctionArn-dimensioned series return no datapoints, so the ledger never reaches the expected value and the check is killed at 180 s without a verdict. The variable parts above (function ARNs/names, state-machine ARNs, counts, the sleep duration) come from the request and from how many executions were started; only the namespaces, metric names, dimension names, the error code "Lambda.AWSLambdaException" and the permission string "lambda:InvokeFunction" are fixed text.

## Steps to Reproduce
1. Create an iam role trusted by lambda.amazonaws.com and two python3.12 Lambda functions (M and L) whose handler sleeps ~1.5 s and returns {'echo': event['n']}; create alias 'warm' on L.
2. Create a states.amazonaws.com role granted lambda:InvokeFunction on both function ARNs and their :* forms, and a second states.amazonaws.com role with no inline policy.
3. Create three STANDARD state machines: (a) one Task with Resource = arn:aws:states:::lambda:invoke and Parameters.FunctionName = M's ARN under the granting role; (b) the same definition under the bare role; (c) one Task whose Resource is L's raw function ARN, under the granting role.
4. StartExecution three times on (a) — each must SUCCEED with output wrapping the function result in "Payload"; twice on (b) — each must FAIL with error "Lambda.AWSLambdaException" and a cause naming lambda:InvokeFunction; twice on (c) — each must SUCCEED with the raw function result.
5. Call cloudwatch GetMetricStatistics with Period=60 for AWS/States LambdaFunctionsScheduled/Started/Succeeded and LambdaFunctionRunTime dimensioned by LambdaFunctionArn (for both L and M), for AWS/Lambda Invocations by FunctionName=M, and for AWS/States ExecutionsSucceeded/ExecutionsFailed by StateMachineArn.
6. AWS: L's per-function series are non-zero and M's are zero while M still shows AWS/Lambda Invocations for the allowed calls only. Emulator: every AWS/States LambdaFunctionArn series stays empty, so the poll loop never settles and the check is terminated at the harness deadline.

Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-invoke-denied-role-fails-execution-function-series-empty cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series
Issue-Key: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-invoke-denied-role-fails-execution-function-series-empty|_warm#8|Timeout|time.sleep(5.0) # the invoke grant is still propagating; the retry is the wait
Issue-Key: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series|_warm#6|Timeout|time.sleep(5.0) # the invoke grant is still propagating; the retry is the wait
Issue-Key: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series|_warm#10|Timeout|time.sleep(5.0) # the invoke grant is still propagating; the retry is the wait
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# Step Functions Lambda task invocations never populate the AWS/Lambda function-ARN metric series (optimized and stream invoke paths)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
This issue carries 1 net-new root cause (three checks ground it).

Cause 1 of 1: a Step Functions Lambda task produces no AWS/Lambda series attributed to the invoked function's ARN, and the denied-invoke twin produces no observable either.
AWS: recorded behavior only (neither the sfn nor the lambda model describes emitted metrics). A Lambda task in a state machine attributes its invocation metrics to the FUNCTION's series (the AWS/Lambda namespace dimensioned by the function/ARN) when the task actually invokes the function; conversely, when the state machine's role is denied lambda:InvokeFunction the execution FAILS and the function's series stays empty — the emptiness is the assertion, held against a window derived from the positive control's measured latency, and the optimized (`arn:aws:states:::lambda:invoke`) integration behaves the same way.
Emulator: all three checks exhaust their 60s budget. The positive control never populates the function-ARN series, so neither the attribution claim nor its negative twin can be evaluated — the whole Step Functions→Lambda metric attribution is unobservable.
Why: operators cannot see Step Functions-driven Lambda traffic in the function's own dashboards and alarms, and cannot distinguish "the task ran" from "the task was denied" from metrics at all.
Fix: the Step Functions Lambda-task execution path must perform a real function invocation (and authorize it against the state machine role), and the Lambda invocation path must publish its AWS/Lambda series under the function dimension. The open issue 20261024T043150Z-sfn-step-functions-control-plane-no-iam-passrole-authorization.md covers control-plane PassRole authorization only; these three replay keys are not on it.
Traces: cloudwatch__lambda__sfn.metrics-states-stream-lambda-lambda-task-metrics... (see trace_ids for the exact three ids).

## Evidence
All three checks report `Error: Check timed out after 60.0s`:
  cloudwatch__lambda__sfn.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series
  cloudwatch__lambda__sfn.metrics-states-stream-lambda-invoke-denied-role-fails-execution-and-function-series-stays-empty
  cloudwatch__lambda__sfn.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series
Each starts an execution containing a Lambda task and then polls CloudWatch for the AWS/Lambda series dimensioned by the invoked function; the positive-control series never appears, so the poll runs out the check budget.

## Steps to Reproduce
Replay the three check ids: create a function and a state machine with a Lambda task (both the stream/SDK-integration form and the optimized `arn:aws:states:::lambda:invoke` form), start an execution with a role that may invoke and with a twin role that may not, then poll CloudWatch for the AWS/Lambda series under the function's dimension.

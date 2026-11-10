Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series
Issue-Key: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series|_ledger#23|Timeout|time.sleep(_POLL_S)
Issue-Commit: 5598dd881b8e225c34bc5a9814ddc1ff981b448f

# cloudwatch__lambda__stepfunctions: the AWS/States LambdaFunction* / AWS/Lambda Invocations attribution scenario never completes — the direct-ARN vs optimized-integration metric series are never observable

> _Note: this issue was identified against the code base at commit `5598dd881b8e225c34bc5a9814ddc1ff981b448f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause, grounded in one failing behavioral check the replay report left unmarked (no ALREADY-FILED marker and no OWNED marker).

Cause 1 of 1 — the scenario that distinguishes a Step Functions Lambda call made through a direct function-ARN Task Resource from one made through the arn:aws:states:::lambda:invoke optimized integration never reaches its metric reads: the emulator never produces the executions and datapoints the check polls for.
Failing check id: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series.
AWS: recorded behavior only (the Smithy models of neither service describe CloudWatch publication). A Task whose Resource is a raw Lambda function ARN is accounted for in namespace AWS/States under the LambdaFunctionArn dimension (LambdaFunctionsScheduled/Started/Succeeded/Failed etc. for that function ARN), whereas a Task using the arn:aws:states:::lambda:invoke optimized integration is NOT: neither an allowed nor a denied optimized call writes a datapoint into the function-ARN series. A denied optimized call additionally never reaches Lambda at all, so AWS/Lambda Invocations for that function does not move for it, while an allowed one does.
Emulator: the check is killed by the harness with "Error: Check timed out after 60.0s" (its own settle budget is 420 s) — the three warm-up/subject executions plus the subsequent GetMetricStatistics polls never converge, so no series under LambdaFunctionArn, no AWS/Lambda Invocations reading and no denied-call outcome is ever observed.
Why: a client that builds dashboards or alarms on Step Functions' Lambda attribution cannot tell which integration style it is looking at — the whole distinction between the legacy direct-ARN Task and the optimized integration is invisible, and a denied invoke cannot be told apart from one that never happened.
Fix: the AWS/States metric emission for the LambdaFunctionArn dimension family in the interpreter's Lambda task path (it must publish for a direct-ARN Resource and must NOT publish for the optimized integration), together with the AWS/Lambda Invocations emission on the Invoke path and the authorization check that must stop a denied optimized call before it reaches the function.
Traces: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series.

## Evidence
cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series — verdict mismatch.
Expected: after warming an allowed grant through an alias ARN, three executions each of (a) an optimized arn:aws:states:::lambda:invoke Task under a granting role, (b) the same Task under a role with no lambda:InvokeFunction, and (c) a direct function-ARN Task under the granting role, AWS/States carries LambdaFunction* datapoints under the LambdaFunctionArn dimension ONLY for (c); the optimized cases — allowed and denied alike — leave that series empty, and AWS/Lambda Invocations for the function moves for (a) but not for (b).
Actual: "Error: Check timed out after 60.0s" — the check never got past driving the executions and polling for datapoints; no metric assertion was ever evaluated.

## Steps to Reproduce
1. iam:CreateRole for Lambda; lambda:CreateFunction twice (-> fn_m, fn_l) and lambda:CreateAlias on fn_l.
2. iam:CreateRole trusting states.amazonaws.com with lambda:InvokeFunction on [fn_m, fn_m:*, fn_l, fn_l:*] (the granting role), and a second role trusting states.amazonaws.com with no permissions (the bare role).
3. sfn:CreateStateMachine three times: an optimized Task {"Resource":"arn:aws:states:::lambda:invoke","Parameters":{"FunctionName":"<fn_m>", ...}} under the granting role; the same under the bare role; and a Task whose Resource is the raw <fn_l> ARN under the granting role.
4. sfn:StartExecution three times against each and wait for terminal status.
5. cloudwatch:GetMetricStatistics in namespace AWS/States for LambdaFunctionsScheduled/Started/Succeeded/Failed with Dimensions exactly [LambdaFunctionArn=<fn>] — expected: datapoints only for the direct-ARN machine; none for either optimized machine. And in AWS/Lambda for Invocations with Dimensions [FunctionName=<fn_m>] — expected: it moves for the allowed optimized run and not for the denied one.
6. Observed: the check is killed at the 60 s harness cap before any of step 5 is read.

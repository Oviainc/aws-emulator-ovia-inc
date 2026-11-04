Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.lambda-task-sfn-resultpath-merges-with-original-input events__lambda__s3__stepfunctions.sfn-startexecution-input-passthrough-to-lambda-task events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3 events__lambda__s3__stepfunctions.sfn-task-lambda-s3-write-denied-without-permission events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtasksuccess-resumes events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtaskfailure-fails-execution events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task
Issue-Key: events__lambda__s3__stepfunctions.lambda-task-sfn-resultpath-merges-with-original-input|run#11|AssertionError|assert resp["status"] == "SUCCEEDED", f"execution ended in {resp['status']}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-startexecution-input-passthrough-to-lambda-task|run#11|AssertionError|assert execution["status"] == "SUCCEEDED", (
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3|run#12|AssertionError|assert resp["status"] == "SUCCEEDED", f"execution ended in {resp['status']}: {resp.get('output')}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-lambda-s3-write-denied-without-permission|run#13|AssertionError|assert denied_resp["status"] == "SUCCEEDED", f"expected SUCCEEDED, got {denied_resp['status']}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtasksuccess-resumes|run#9|AssertionError|assert final["status"] == "SUCCEEDED", f"unexpected status {final['status']}: {final}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtaskfailure-fails-execution|run#10|AssertionError|assert final["error"] == "LambdaReportedFailure", final
Issue-Key: events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task|run#14|AssertionError|assert "not authorized to assume" in wrong_final.get("cause", ""), wrong_final
Issue-Commit: 39700f19148179f97c0fef93ca04528bc9d9d02f

# events__lambda__s3__stepfunctions: a Lambda-invoking Task state never runs the function (States.Runtime "'Invoke' ... is not implemented") and the execution role's trust policy is never checked (2 causes)

> _Note: this issue was identified against the code base at commit `39700f19148179f97c0fef93ca04528bc9d9d02f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two net-new root causes, both reached through StartExecution on a STANDARD state machine whose single Task state targets a Lambda function. The checks below are NOT carried as replay keys by any open issue; the closest open issues are named per cause.

Cause 1 of 2: a Task state that targets a Lambda function never invokes it, and the execution terminates FAILED with an emulator-authored interpreter error instead of the function's result.
AWS: a Task whose Resource is a Lambda function ARN (or the optimized `arn:aws:states:::lambda:invoke`) calls Lambda Invoke with the state's Parameters/Payload and binds the function's returned JSON as the state result; the execution then reaches a terminal status of SUCCEEDED and DescribeExecution reports `output` (the result after ResultSelector/ResultPath/OutputPath processing) with NO `error` and NO `cause`. Smithy (sfn): DescribeExecutionOutput.status is the required ExecutionStatus enum, `output` is SensitiveData @length max 262144, and `error` (SensitiveError @length max 256) / `cause` (SensitiveCause @length max 32768) are populated only for a failed execution. ResultPath semantics are AWS's documented input/output processing rule: the task result is merged INTO the state input at the given reference path rather than replacing it.
Emulator: every one of these executions terminates with status FAILED, `error` = the fixed string `States.Runtime` and `cause` = the emulator-authored sentence "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task" (the operation name `Invoke` and the service name `Lambda` are the only variable parts; the rest is emulator prose, and AWS never emits this sentence). `output` is absent (None). DescribeExecution itself answers HTTP 200, so the refusal is entirely inside the interpreter/Lambda-runtime path. Because the function is never run, no downstream effect exists either: no S3 object is written by the handler, no ResultPath merge is observable, and no payload is propagated.
Why: a client cannot orchestrate Lambda at all. Every Task outcome collapses to one interpreter error name — `States.Runtime`, which AWS reserves for definition/interpreter faults — so a Catch or Retry keyed on the function's own error name never matches, code that parses `cause` as Lambda's {errorType, errorMessage, trace} JSON gets prose, and code that reads the execution `output` gets nothing. Negative scenarios are equally wrong: a check that expects the execution to SUCCEED while the FUNCTION's own S3 write is denied cannot distinguish a denied S3 write from a missing Lambda runtime.
Fix: the Lambda `Invoke` handler (nothing in-process runs a deployment package, so Invoke refuses), and the Step Functions Task-state integration path that maps a Lambda invocation result/error into the task outcome, the history TaskSucceeded/TaskFailed events and the execution `output`. Related open issues that do NOT carry these checks: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (the Invoke refusal itself), 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md (it records a DIFFERENT, now-superseded cause sentence about the integration table) and cause 1 of 20261024T043150Z-sfn-5-of-the-6-interpreter-causes-remain-open.md.
Traces (failing checks): events__lambda__s3__stepfunctions.lambda-task-sfn-resultpath-merges-with-original-input, events__lambda__s3__stepfunctions.sfn-startexecution-input-passthrough-to-lambda-task, events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3, events__lambda__s3__stepfunctions.sfn-task-lambda-s3-write-denied-without-permission, events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtasksuccess-resumes, events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtaskfailure-fails-execution, events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task.

Cause 2 of 2: the state machine's execution role is accepted and used without ever checking that its trust policy allows states.amazonaws.com to assume it, so a wrongly-trusted role produces no assume-role failure.
AWS: CreateStateMachine accepts a roleArn whose AssumeRolePolicyDocument trusts only lambda.amazonaws.com, but at execution time Step Functions cannot assume it: the execution terminates FAILED and DescribeExecution's `cause` names the role and states that Step Functions is not authorized to assume it — the fixed fragment the recording pins is "not authorized to assume", with the role ARN taken from the state machine's roleArn (a request-derived value); the surrounding full sentence is not fixed by this recording, so treat the fragment plus the role ARN as the requirement. Recorded behavior only: the model has no trait for this; the paired correctly-trusted twin (same definition, role trusting states.amazonaws.com) must SUCCEED, so the verdict must depend on the trust policy and nothing else.
Emulator: the wrongly-trusted machine fails for an unrelated reason — `error` = `States.Runtime`, `cause` = "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task" — i.e. the same outcome the correctly-trusted twin gets. The trust document is never consulted, so the two twins are indistinguishable and no assume-role diagnosis is ever produced.
Why: a client cannot use the emulator to validate IAM wiring. A broken trust policy looks exactly like a working one (today both fail; once Invoke runs, both will succeed), so a misconfiguration that would break the workflow in AWS passes, and the one diagnostic that tells an operator the role is at fault never appears.
Fix: the Step Functions execution-start/role-assumption step in the interpreter's state (evaluate the roleArn's trust policy for the states.amazonaws.com service principal before the first Task integration call) and the ExecutionFailed error/cause it must emit when assumption is refused.
Traces (failing check): events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task.

## Evidence
Cause 1 — check events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtasksuccess-resumes
Expected: DescribeExecution status SUCCEEDED after SendTaskSuccess.
Actual: AssertionError: unexpected status FAILED: {... 'status': 'FAILED', 'error': 'States.Runtime', 'cause': "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task", 'redriveStatus': 'REDRIVABLE', HTTPStatusCode 200}

Check events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtaskfailure-fails-execution
Expected: execution FAILED with error == 'LambdaReportedFailure' (the error name the function sent via SendTaskFailure).
Actual: AssertionError on error 'States.Runtime' with the same "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task" cause.

Check events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3
Expected: status SUCCEEDED with the task result as the execution output.
Actual: AssertionError: execution ended in FAILED: None (no `output` member at all).

Check events__lambda__s3__stepfunctions.sfn-startexecution-input-passthrough-to-lambda-task
Expected: expected status SUCCEEDED.
Actual: AssertionError: execution must run to completion: expected status SUCCEEDED, got 'FAILED'.

Check events__lambda__s3__stepfunctions.lambda-task-sfn-resultpath-merges-with-original-input
Expected: SUCCEEDED, with the handler's {'computed': 42, 'note': 'done'} merged into the original state input at the configured ResultPath.
Actual: AssertionError: execution ended in FAILED — the merge is unobservable because the function never ran.

Check events__lambda__s3__stepfunctions.sfn-task-lambda-s3-write-denied-without-permission
Expected: the denied twin's execution still ends SUCCEEDED (the denial is visible only in the function's own result / the absent S3 object).
Actual: AssertionError: expected SUCCEEDED, got FAILED.

Cause 2 — check events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task
Expected: for the machine whose role trusts lambda.amazonaws.com only, DescribeExecution.cause contains "not authorized to assume" (naming that role's ARN), while the correctly-trusted twin SUCCEEDS.
Actual: AssertionError: {... 'status': 'FAILED', 'error': 'States.Runtime', 'cause': "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task" ...} — no trust-policy diagnosis, and the verdict is identical for both twins.

## Steps to Reproduce
1. IAM CreateRole trusted by lambda.amazonaws.com with an inline s3:PutObject grant; lambda CreateFunction with a python handler that returns a small JSON object (and, for one leg, writes an object to S3 via boto3).
2. IAM CreateRole trusted by states.amazonaws.com with an inline Allow of lambda:InvokeFunction on that function ARN.
3. sfn CreateStateMachine (STANDARD) with one state: {"Type":"Task","Resource":"<function ARN>" (or "arn:aws:states:::lambda:invoke" with Parameters.FunctionName), "ResultPath":"$.result", "End":true}, roleArn = the states-trusted role.
4. StartExecution with any JSON input, then poll DescribeExecution to a terminal status.
5. Observe status FAILED, error 'States.Runtime', cause "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task", no `output`, and no S3 object written — instead of SUCCEEDED with the function's payload merged at ResultPath.
6. For cause 2: repeat step 3 twice — once with the states.amazonaws.com-trusted role, once with the lambda.amazonaws.com-trusted role from step 1 — and compare DescribeExecution. Both executions report the identical States.Runtime/'Invoke' not implemented outcome; neither cause contains "not authorized to assume", so the trust policy is demonstrably never evaluated.

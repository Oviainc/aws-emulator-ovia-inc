Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.lambda-sdk-putevents-custom-detailtype-triggers-sfn events__lambda__s3__stepfunctions.lambda-sdk-putevents-denied-without-permission events__lambda__s3__stepfunctions.lambda-sdk-putevents-triggers-rule events__lambda__s3__stepfunctions.lambda-sdk-startexecution-denied-without-permission events__lambda__s3__stepfunctions.lambda-sdk-startexecution-via-stepfunctions events__lambda__s3__stepfunctions.lambda-sdk-stopexecution-via-stepfunctions events__lambda__s3__stepfunctions.sfn-task-lambda-writes-result-to-s3 events__lambda__s3__stepfunctions.sfn-task-lambda-s3-write-denied-without-permission events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task events__lambda__s3__stepfunctions.sfn-startexecution-input-passthrough-to-lambda-task events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtasksuccess-resumes events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtaskfailure-fails-execution
Issue-Key: events__lambda__s3__stepfunctions.lambda-sdk-putevents-custom-detailtype-triggers-sfn|_invoke_lambda#10|AssertionError|raise AssertionError(f"lambda invocation raised: {body}")
Issue-Key: events__lambda__s3__stepfunctions.lambda-sdk-putevents-denied-without-permission|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation raised: {body}")
Issue-Key: events__lambda__s3__stepfunctions.lambda-sdk-putevents-triggers-rule|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation raised: {body}")
Issue-Key: events__lambda__s3__stepfunctions.lambda-sdk-startexecution-denied-without-permission|_invoke_lambda#8|AssertionError|raise AssertionError(f"lambda invocation raised: {body}")
Issue-Key: events__lambda__s3__stepfunctions.lambda-sdk-startexecution-via-stepfunctions|_invoke_lambda#7|AssertionError|raise AssertionError(f"lambda invocation raised: {body}")
Issue-Key: events__lambda__s3__stepfunctions.lambda-sdk-stopexecution-via-stepfunctions|_invoke_lambda#13|AssertionError|raise AssertionError(f"lambda invocation raised: {body}")
Issue-Key: events__lambda__s3__stepfunctions|GetObject|NoSuchKey
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-lambda-s3-write-denied-without-permission|run#15|KeyError|assert denied_payload["success"] is False, f"expected success=False, got {denied_payload}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task|run#13|AssertionError|assert wrong_final["status"] == "FAILED", f"unexpected status: {wrong_final}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-startexecution-input-passthrough-to-lambda-task|run#16|StopIteration|scheduled = next(e for e in events if e["type"] == "LambdaFunctionScheduled")
Issue-Key: events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtasksuccess-resumes|_poll_execution#8|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtaskfailure-fails-execution|_poll_execution#8|Timeout|time.sleep(interval_s)
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# events__lambda__s3__stepfunctions: the Lambda Python runtime ships no AWS SDK, the execution role's trust is never enforced, Lambda Task history events are missing, and waitForTaskToken never resumes (4 causes)

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Four net-new root causes. Related open issues 20261104T221115Z-events-lambda-s3-stepfunctions-every-lambda-task-state.md and 20261104T221123Z-events-lambda-s3-stepfunctions-a-lambda-invoking-task.md describe a Lambda Task that fails with States.Runtime because the function is never run — that is no longer what happens: the function IS run now, and these are the divergences that remain. The fixed issue 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md is also distinct (Invoke now answers 200 and executes the package); none of these four causes is a refusal of Invoke.

Cause 1 of 4: the Lambda managed Python runtime provides no AWS SDK, so every handler that uses boto3 dies at import.
AWS: the managed Python runtimes bundle the AWS SDK for Python — `import boto3` succeeds in a function deployed as a bare .zip with no layer, and the SDK picks up the region and the function role's temporary credentials from the runtime environment (AWS_REGION, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN). Invoke then answers HTTP 200 with no X-Amz-Function-Error header and the handler's return document as the payload (Smithy: lambda#Invoke output FunctionError -> httpHeader X-Amz-Function-Error, Payload -> httpPayload).
Emulator: Invoke answers HTTP 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-run id>", "stackTrace": []}`. The fixed text is "Unable to import module '<module>': No module named 'boto3'" with errorType `Runtime.ImportModuleError`; the module name is the handler's module from the function's Handler setting and the requestId is per-invocation. The handler body never executes, so no PutEvents, StartExecution, StopExecution or S3 write it would perform ever happens.
Why: every Lambda that calls another AWS service — the single most common Lambda shape — is non-functional. Both positive and negative (IAM-denial) assertions become unobservable: the caller sees an import error instead of the service's own success or AccessDenied answer, and downstream state (an EventBridge event, a started execution, an S3 object) is simply absent.
Fix: the Lambda Invoke data-plane runtime/sandbox for the Python runtimes must make the AWS SDK importable on the handler's sys.path and export the region plus the function role's credentials into the handler environment.
Traces: events__lambda__s3__stepfunctions.lambda-sdk-putevents-custom-detailtype-triggers-sfn, .lambda-sdk-putevents-denied-without-permission, .lambda-sdk-putevents-triggers-rule, .lambda-sdk-startexecution-denied-without-permission, .lambda-sdk-startexecution-via-stepfunctions, .lambda-sdk-stopexecution-via-stepfunctions, .sfn-task-lambda-writes-result-to-s3, .sfn-task-lambda-s3-write-denied-without-permission.

Cause 2 of 4: a state machine whose execution role does not trust states.amazonaws.com still runs its Tasks and the execution SUCCEEDS.
AWS: Step Functions assumes the state machine's roleArn through sts:AssumeRole before dispatching any Task. When the role's trust policy does not name states.amazonaws.com the assume fails and the execution ends FAILED — it never reaches SUCCEEDED and no Task side effect is applied.
Emulator: DescribeExecution reports `status: SUCCEEDED` for an execution whose role has a wrong trust policy, with the Lambda invoke envelope as output. No trust evaluation happens at execution start.
Why: a misconfigured trust policy — a top cause of real deployment failures — is invisible, and the emulator applies side effects AWS would refuse outright.
Fix: the Step Functions execution-start path must resolve the state machine's roleArn and evaluate its trust policy for states.amazonaws.com, failing the execution when the role cannot be assumed.
Traces: events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task.

Cause 3 of 4: GetExecutionHistory emits no Lambda Task sub-events.
AWS: a Task state invoking Lambda contributes LambdaFunctionScheduled (carrying the resolved resource and the input the task was scheduled with), LambdaFunctionStarted and LambdaFunctionSucceeded (carrying the function's output) to the execution history — all three are modelled members of Smithy sfn#HistoryEventType, together with LambdaFunctionFailed, LambdaFunctionScheduleFailed, LambdaFunctionStartFailed and LambdaFunctionTimedOut.
Emulator: the history contains no event of type LambdaFunctionScheduled at all, so a consumer scanning for it finds nothing (the check's `next(e for e in events if e["type"] == "LambdaFunctionScheduled")` raised StopIteration).
Why: the documented way to observe what input a Task was given and what it returned does not exist, so input/output passthrough and failure attribution cannot be inspected through the API AWS provides for it.
Fix: the Step Functions execution-history recorder for Task states with a Lambda resource must emit the LambdaFunction* event family alongside the TaskStateEntered/TaskStateExited pair.
Traces: events__lambda__s3__stepfunctions.sfn-startexecution-input-passthrough-to-lambda-task.

Cause 4 of 4: a `.waitForTaskToken` Lambda Task is never resumed or failed by SendTaskSuccess/SendTaskFailure.
AWS: a Task whose Resource ends in `.waitForTaskToken` pauses with a token in its payload. SendTaskSuccess with that token completes the task with the supplied output and the execution continues to the next state; SendTaskFailure with that token fails the task with the supplied Error/Cause, so the execution ends FAILED (unless a Catch handles it). Both calls are modelled in sfn and both make the execution leave RUNNING.
Emulator: after SendTaskSuccess (and after SendTaskFailure) the execution never leaves RUNNING, so a client polling for the terminal status exhausts its budget — both checks ended with "Check timed out after 60.0s".
Why: the human-in-the-loop / external-callback pattern never terminates. Any client that starts a callback workflow and waits for it hangs forever.
Fix: the Step Functions callback registry — SendTaskSuccess/SendTaskFailure handlers and the Task state's waitForTaskToken suspension in the interpreter — must match the delivered token to the paused task and resume or fail it.
Traces: events__lambda__s3__stepfunctions.sfn-waitfortasktoken-lambda-sendtasksuccess-resumes, .sfn-waitfortasktoken-lambda-sendtaskfailure-fails-execution.

## Evidence
Cause 1 — expected: the handler's boto3 call runs and the invoke returns the handler's document (e.g. {FailedEntryCount, EventId}); or, in the negative arms, the service's own AccessDeniedException. Actual: `AssertionError: lambda invocation raised: {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '...', 'stackTrace': []}`. Downstream: `botocore.errorfactory.NoSuchKey: An error occurred (NoSuchKey) when calling the GetObject operation: The specified key does not exist.` (the object the handler was to write) and `KeyError: 'success'`.

Cause 2 — expected: an execution whose role does not trust states.amazonaws.com must not SUCCEED. Actual: `AssertionError: unexpected status: {... 'status': 'SUCCEEDED', ... 'output': '{"StatusCode":200,"ExecutedVersion":"$LATEST","FunctionError":"Unhandled","Payload":{...}}' ...}`.

Cause 3 — expected: a LambdaFunctionScheduled event in GetExecutionHistory. Actual: `StopIteration` from `next(e for e in events if e["type"] == "LambdaFunctionScheduled")` — no such event exists.

Cause 4 — expected: SendTaskSuccess resumes the execution to SUCCEEDED and SendTaskFailure drives it to FAILED. Actual: `Error: Check timed out after 60.0s` for both — the execution stays RUNNING.

## Steps to Reproduce
Cause 1: CreateFunction with a Python runtime and a one-file .zip whose handler does `import boto3` and calls any client (events.put_events / stepfunctions.start_execution / s3.put_object), then Invoke it. Observe 200 with X-Amz-Function-Error: Unhandled and payload errorType Runtime.ImportModuleError, "No module named 'boto3'".
Cause 2: Create a role whose trust policy names a principal other than states.amazonaws.com, CreateStateMachine with it, StartExecution, then DescribeExecution — observe SUCCEEDED.
Cause 3: Run any state machine with a Task state invoking Lambda, then GetExecutionHistory and look for type == "LambdaFunctionScheduled" — none is present.
Cause 4: Create a state machine with a Task whose Resource is arn:aws:states:::lambda:invoke.waitForTaskToken, StartExecution, read the token from the invoked function's event, call SendTaskSuccess (then, in a twin, SendTaskFailure) with it, and poll DescribeExecution — the execution stays RUNNING.

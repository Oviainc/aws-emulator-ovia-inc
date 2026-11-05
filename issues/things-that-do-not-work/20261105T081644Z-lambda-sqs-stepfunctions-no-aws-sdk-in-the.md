Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.lambda-start-sfn-execution-positive lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative lambda__sqs__stepfunctions.lambda-describe-sfn-execution-positive lambda__sqs__stepfunctions.lambda-invokes-sfn-and-polls-execution-status-until-complete lambda__sqs__stepfunctions.lambda-list-sfn-executions-missing-permission-negative lambda__sqs__stepfunctions.lambda-sfn-list-executions-filtered-by-status lambda__sqs__stepfunctions.lambda-sfn-start-execution-idempotency lambda__sqs__stepfunctions.lambda-sfn-stopexecution-halts-running-workflow lambda__sqs__stepfunctions.lambda-sfn-stopexecution-missing-permission-negative lambda__sqs__stepfunctions.lambda-send-sqs-message-positive lambda__sqs__stepfunctions.lambda-send-sqs-missing-permission-negative lambda__sqs__stepfunctions.lambda-sqs-send-invalid-queue-url-negative lambda__sqs__stepfunctions.cli-deploy-and-run-sfn-lambda-sqs-pipeline-e2e lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative lambda__sqs__stepfunctions.sfn-lambda-sqs-pipeline-positive lambda__sqs__stepfunctions.sfn-catch-lambda-error-to-sqs lambda__sqs__stepfunctions.sfn-lambda-retry-backoff-then-catch-to-sqs lambda__sqs__stepfunctions.sfn-parallel-branch-failure-negative-no-partial-sqs-delivery lambda__sqs__stepfunctions.sfn-lambda-task-timeoutseconds-triggers-retry lambda__sqs__stepfunctions.sfn-lambda-invoke-payload-too-large-negative lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative lambda__sqs__stepfunctions.sfn-task-token-sendtaskfailure-from-lambda-propagates-error lambda__sqs__stepfunctions.sfn-invoke-lambda-missing-permission-negative
Issue-Key: lambda__sqs__stepfunctions.lambda-start-sfn-execution-positive|run#18|AssertionError|assert "executionArn" in payload, (
Issue-Key: lambda__sqs__stepfunctions.lambda-start-sfn-missing-permission-negative|_poll_invoke_until#18|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sqs__stepfunctions.lambda-describe-sfn-execution-positive|run#18|AssertionError|assert "executionArn" in payload, "executionArn missing from response"
Issue-Key: lambda__sqs__stepfunctions.lambda-invokes-sfn-and-polls-execution-status-until-complete|_invoke_retry#10|AssertionError|raise AssertionError(f"lambda invoke failed: {body}")
Issue-Key: lambda__sqs__stepfunctions.lambda-list-sfn-executions-missing-permission-negative|run#18|AssertionError|assert positive_payload.get("ok") is True, (
Issue-Key: lambda__sqs__stepfunctions.lambda-sfn-list-executions-filtered-by-status|_invoke_retry#18|AssertionError|raise AssertionError(f"lambda invoke failed: {body}")
Issue-Key: lambda__sqs__stepfunctions.lambda-sfn-start-execution-idempotency|run#17|AssertionError|assert payload1.get("executionArn"), (
Issue-Key: lambda__sqs__stepfunctions.lambda-sfn-stopexecution-halts-running-workflow|_invoke_retry#15|AssertionError|raise AssertionError(f"lambda invoke failed: {body}")
Issue-Key: lambda__sqs__stepfunctions.lambda-sfn-stopexecution-missing-permission-negative|_invoke_retry#15|AssertionError|raise AssertionError(f"lambda invoke crashed unexpectedly: {body}")
Issue-Key: lambda__sqs__stepfunctions.lambda-send-sqs-message-positive|run#7|AssertionError|assert "error" not in result, f"Lambda invocation errored: {result.get('error')}"
Issue-Key: lambda__sqs__stepfunctions.lambda-send-sqs-missing-permission-negative|run#6|AssertionError|assert positive_payload.get("ok") is True, (
Issue-Key: lambda__sqs__stepfunctions.lambda-sqs-send-invalid-queue-url-negative|run#10|AssertionError|assert payload.get("ok") is False, (
Issue-Key: lambda__sqs__stepfunctions.cli-deploy-and-run-sfn-lambda-sqs-pipeline-e2e|run#42|AssertionError|assert messages, "lambda did not deliver a message to the queue"
Issue-Key: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative|run#40|AssertionError|assert signal1 is not None, "positive control succeeded but no invocation signal arrived at the queue"
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-sqs-pipeline-positive|_wait_for_messages#25|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sqs__stepfunctions.sfn-catch-lambda-error-to-sqs|_wait_for_messages#21|AssertionError|raise AssertionError(
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-retry-backoff-then-catch-to-sqs|_drain_queue#12|AssertionError|raise AssertionError(f"expected at least {min_count} message(s) on {queue_url}, got {len(bodies)}")
Issue-Key: lambda__sqs__stepfunctions.sfn-parallel-branch-failure-negative-no-partial-sqs-delivery|run#14|AssertionError|assert desc["status"] == "FAILED", desc
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-task-timeoutseconds-triggers-retry|run#10|AssertionError|assert desc["status"] == "FAILED", f"expected FAILED, got {desc['status']}"
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-invoke-payload-too-large-negative|run#16|AssertionError|assert desc_big.get("cause") == expected_cause, desc_big.get("cause")
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative|_wait_execution_terminal#7|AssertionError|raise AssertionError(f"execution still RUNNING after {timeout_s}s")
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-sendtaskfailure-from-lambda-propagates-error|_wait_execution_terminal#8|Timeout|time.sleep(poll_s)
Issue-Key: lambda__sqs__stepfunctions.sfn-invoke-lambda-missing-permission-negative|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# lambda__sqs__stepfunctions: no AWS SDK in the Lambda Python runtime, Lambda Task failures/timeouts/role denials never fail the Task, task tokens never resume, and the output-size refusal is misworded (6 causes)

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Six net-new root causes. The open issue 20261029T183117Z-lambda-sfn-sqs-nine-net-new-causes-sqs.md owns a different set of checks in this service (they are marked OWNED in the replay); none of the checks below is owned by it.

Cause 1 of 6: the Lambda managed Python runtime provides no AWS SDK, so every handler that uses boto3 dies at module import.
AWS: the managed Python runtimes bundle the AWS SDK for Python — `import boto3` succeeds in a bare .zip function with no layer, and the SDK resolves the region and the function role's temporary credentials from the runtime environment (AWS_REGION, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN). Invoke then answers HTTP 200 with no X-Amz-Function-Error header and the handler's returned document as the payload (Smithy: lambda#Invoke output FunctionError -> httpHeader X-Amz-Function-Error, Payload -> httpPayload). In a negative arm the handler surfaces the called service's own refusal (sqs `AccessDenied`/`AWS.SimpleQueueService.NonExistentQueue`, states `AccessDeniedException`).
Emulator: Invoke answers HTTP 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-run id>", "stackTrace": []}` — fixed text "Unable to import module '<module>': No module named 'boto3'", errorType `Runtime.ImportModuleError`, with the module name from the function's Handler and a per-invocation requestId. The handler body never runs, so no message is sent to SQS, no execution is started/stopped/described, and the result document carries none of its own keys.
Why: every Lambda that calls another AWS service is non-functional, directly and as a Step Functions Task; both arms of every permission check collapse to the same import error, and queue/workflow side effects never happen.
Fix: the Lambda Invoke data-plane runtime/sandbox for the Python runtimes must make the AWS SDK importable on the handler's sys.path and export the region plus the function role's credentials into the handler environment.
Traces: lambda__sqs__stepfunctions.lambda-start-sfn-execution-positive, .lambda-start-sfn-missing-permission-negative, .lambda-describe-sfn-execution-positive, .lambda-invokes-sfn-and-polls-execution-status-until-complete, .lambda-list-sfn-executions-missing-permission-negative, .lambda-sfn-list-executions-filtered-by-status, .lambda-sfn-start-execution-idempotency, .lambda-sfn-stopexecution-halts-running-workflow, .lambda-sfn-stopexecution-missing-permission-negative, .lambda-send-sqs-message-positive, .lambda-send-sqs-missing-permission-negative, .lambda-sqs-send-invalid-queue-url-negative, .cli-deploy-and-run-sfn-lambda-sqs-pipeline-e2e, .cli-detach-lambda-invoke-policy-breaks-sfn-task-negative, .sfn-lambda-sqs-pipeline-positive.

Cause 2 of 6: an unhandled Lambda handler error does not fail the Task state, so Retry and Catch never fire.
AWS: Invoke answers 200 with X-Amz-Function-Error: Unhandled when the handler raises; Step Functions fails the task, recording LambdaFunctionFailed then TaskFailed (both modelled in Smithy sfn#HistoryEventType), with the task error set to the errorType from the function's error document (the thrown type, e.g. `Exception`, or `Lambda.Unknown` when the runtime reports none) and the cause set to that JSON error document. Retry entries matching States.TaskFailed back off and re-invoke; a Catch routes to its handler state; with neither, the execution ends FAILED.
Emulator: the Task is reported successful with the full Invoke envelope (`"FunctionError":"Unhandled"`, `"Payload":{"errorMessage":...,"errorType":"Exception",...}`) as its result; Retry never re-invokes, Catch branches are never entered, and a Parallel state whose branch raises reports `status: SUCCEEDED` with both branch results.
Why: all workflow error handling is dead — no retry, no compensation, no failure notification — and a Parallel/Map run that AWS aborts here completes and lets partial side effects through.
Fix: the Step Functions ASL interpreter's Task-state Lambda integration must map a non-empty FunctionError on the Invoke response to a task failure before Retry/Catch evaluation.
Traces: lambda__sqs__stepfunctions.sfn-catch-lambda-error-to-sqs, .sfn-lambda-retry-backoff-then-catch-to-sqs, .sfn-parallel-branch-failure-negative-no-partial-sqs-delivery.

Cause 3 of 6: a Task state's TimeoutSeconds is not enforced for a Lambda invoke.
AWS: when a Task runs longer than its TimeoutSeconds, Step Functions times the task out: the history records LambdaFunctionTimedOut/TaskTimedOut (Smithy sfn#HistoryEventType; sfn#TaskTimedOutEventDetails carries required resourceType and resource plus error and cause) with error `States.Timeout`, a Retry entry on States.Timeout re-invokes, and once retries are exhausted the execution ends FAILED with States.Timeout.
Emulator: the Task waits for the slow function and the execution reports `status: SUCCEEDED`; TimeoutSeconds is ignored and no TaskTimedOut event is produced.
Why: a hung dependency can never be bounded. A workflow that AWS aborts in TimeoutSeconds runs to completion here, so timeout/retry tuning is untestable and a stuck task is indistinguishable from a fast one.
Fix: the Step Functions ASL interpreter's Task-state dispatcher must arm the state's TimeoutSeconds/TimeoutSecondsPath and fail the task with States.Timeout plus a TaskTimedOut event when it elapses.
Traces: lambda__sqs__stepfunctions.sfn-lambda-task-timeoutseconds-triggers-retry.

Cause 4 of 6: the output-size refusal uses emulator wording instead of AWS's States.DataLimitExceeded message.
AWS: when a Task's result exceeds the 262144-byte state-payload limit the task fails with the cause "The state/task '<resource>' returned a result with a size exceeding the maximum number of bytes service limit." — fixed text, with `<resource>` the Task's resolved resource (the Lambda function ARN) as the only variable part.
Emulator: the cause is `the 'Task' state 'InvokeLambda' produced a document of at least 300351 bytes, more than the 262144 a state's output may carry` — emulator-authored text that names the state rather than the resource and quotes a measured byte count AWS never reports.
Why: a client matching on AWS's cause text (or the States.DataLimitExceeded error) to distinguish a payload-limit failure from any other task failure cannot do so, and the message leaks an implementation detail ("at least N bytes") that is not part of the contract.
Fix: the Step Functions ASL interpreter's state-payload size guard must emit AWS's States.DataLimitExceeded error and the "The state/task '<resource>' returned a result with a size exceeding the maximum number of bytes service limit." cause.
Traces: lambda__sqs__stepfunctions.sfn-lambda-invoke-payload-too-large-negative.

Cause 5 of 6: a `.waitForTaskToken` task is never resumed by SendTaskSuccess, so it can neither complete nor reject a duplicate callback.
AWS: SendTaskSuccess with a live token completes the paused task with the given output and the execution leaves RUNNING; a SECOND SendTaskSuccess (or SendTaskFailure) for the same token is refused with `TaskTimedOut`/`TaskDoesNotExist` because the token is no longer live. SendTaskFailure propagates its Error/Cause into the task failure, so the execution ends FAILED (unless a Catch handles it).
Emulator: after the first SendTaskSuccess the execution is still RUNNING 30 s later, and the SendTaskFailure arm never reaches a terminal status inside the 60 s budget — the callback registry never matches the delivered token to the paused task, so the duplicate-rejection behaviour cannot occur either.
Why: the external-callback pattern never terminates; a client that starts a callback workflow and waits hangs, and the at-most-once guarantee on a task token is unobservable.
Fix: the Step Functions callback registry — SendTaskSuccess/SendTaskFailure handlers and the Task state's waitForTaskToken suspension in the interpreter — must resume or fail the paused task on the first call and refuse subsequent calls for a consumed token.
Traces: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative, .sfn-task-token-sendtaskfailure-from-lambda-propagates-error.

Cause 6 of 6: an execution role with no lambda:InvokeFunction grant is never denied on the data plane.
AWS: Step Functions assumes the state machine's roleArn and calls lambda:Invoke under it; when the role is not permitted lambda:InvokeFunction on the target function the Task fails and the execution ends FAILED, with a cause naming the assumed-role principal, the action lambda:InvokeFunction and the function ARN. Hypothesis (neither the Smithy model nor the recording states the exact token): the error is `Lambda.AccessDeniedException`. What the recording establishes is that the execution must reach a terminal FAILED status.
Emulator: the execution is never failed, so a client polling for the denied arm's terminal status exhausted the 60 s budget — no authorization of the execution role happens before the integration call.
Why: negative IAM tests are unverifiable and a least-privilege regression is invisible; the emulator performs an invoke AWS refuses.
Fix: the Step Functions Task-dispatch path must authorize the integration's action and resource against the assumed execution role and fail the task when it is not permitted.
Traces: lambda__sqs__stepfunctions.sfn-invoke-lambda-missing-permission-negative.

## Evidence
Cause 1 — `AssertionError: Expected executionArn in response, got: {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}`; `AssertionError: executionArn missing from response`; `AssertionError: lambda invoke failed: {... Runtime.ImportModuleError ...}`; `AssertionError: Positive control: expected success with sqs:SendMessage granted, got: {... Runtime.ImportModuleError ...}`; `AssertionError: Expected failure for nonexistent queue, got: {... Runtime.ImportModuleError ...}`; `AssertionError: Lambda invocation errored: Unhandled`; `AssertionError: lambda did not deliver a message to the queue`; `AssertionError: positive control succeeded but no invocation signal arrived at the queue`; plus two 60 s timeouts where the check waits for a side effect that never arrives.

Cause 2 — `AssertionError: Expected 1 messages, got 0 within 30s` (Catch-to-SQS never ran); `AssertionError: expected at least 3 message(s) on https://sqs.../sim2real-q-..., got 0` (Retry never re-invoked); and a Parallel run reported `'status': 'SUCCEEDED'` whose own output contains `"FunctionError":"Unhandled","Payload":{"errorMessage":"intentional-failure","errorType":"Exception"}`.

Cause 3 — expected FAILED (States.Timeout after retries), actual `AssertionError: expected FAILED, got SUCCEEDED`.

Cause 4 — expected cause "The state/task '<function ARN>' returned a result with a size exceeding the maximum number of bytes service limit."; actual `the 'Task' state 'InvokeLambda' produced a document of at least 300351 bytes, more than the 262144 a state's output may carry`.

Cause 5 — `AssertionError: execution still RUNNING after 30s` after the first SendTaskSuccess; `Error: Check timed out after 60.0s` for the SendTaskFailure arm.

Cause 6 — `Error: Check timed out after 60.0s`: the execution whose role lacks lambda:InvokeFunction never reaches a terminal FAILED status.

## Steps to Reproduce
Cause 1: CreateFunction (Python, one-file .zip) whose handler does `import boto3` and calls sqs.send_message / stepfunctions.start_execution|stop_execution|describe_execution|list_executions; Invoke it and observe 200 with X-Amz-Function-Error: Unhandled and payload errorType Runtime.ImportModuleError, "No module named 'boto3'", and no queue message / execution.
Cause 2: state machine with a Task invoking a raising handler plus Retry on States.TaskFailed and Catch to an SQS-sending state; StartExecution — no retry, no catch, task reported successful. Repeat inside a Parallel state: the execution reports SUCCEEDED.
Cause 3: Task with TimeoutSeconds smaller than the function's sleep, plus Retry on States.Timeout; StartExecution — observe SUCCEEDED and no TaskTimedOut history event.
Cause 4: Task invoking a function that returns a >262144-byte document; read DescribeExecution's error/cause.
Cause 5: Task with Resource arn:aws:states:::lambda:invoke.waitForTaskToken; call SendTaskSuccess with the delivered token, poll DescribeExecution (still RUNNING), then call SendTaskSuccess again and observe no refusal.
Cause 6: Give the execution role no lambda:InvokeFunction, StartExecution and poll DescribeExecution for a terminal status.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause|run#6|AssertionError|assert desc["status"] == "FAILED", (
Issue-Commit: 0132e18e8d244b584dc3ef112dad5b0380ef0fd9

# stepfunctions: an unhandled Lambda handler exception leaves the lambda:invoke Task SUCCEEDED, so no TaskFailed/ExecutionFailed carries the thrown errorType or the JSON error document (1 cause)

> _Note: this issue was identified against the code base at commit `0132e18e8d244b584dc3ef112dad5b0380ef0fd9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause under the `stepfunctions` heading. Every other failing check in this service in this replay is marked OWNED by an open issue and is skipped here.

Cause 1 of 1: a Lambda function that throws an unhandled exception does not fail its optimized `arn:aws:states:::lambda:invoke` Task state — the execution ends SUCCEEDED, and none of the failure surfaces (TaskFailed, ExecutionFailed, DescribeExecution.error/cause) is produced.

AWS: Lambda's Invoke answers HTTP 200 with the function-error indicator set when the handler throws (Smithy lambda#Invoke output member FunctionError -> httpHeader X-Amz-Function-Error, value "Unhandled" for an uncaught error), and the response payload is the runtime's JSON error document — for the Node.js runtimes an object with exactly the members errorType, errorMessage and trace, where errorType is the thrown error's `name`, errorMessage is its `message`, and trace is a JSON array of stack lines whose first element is the fixed form "<errorType>: <errorMessage>". Step Functions treats a non-empty FunctionError as a task failure: the execution history records LambdaFunctionFailed and TaskFailed (both modelled members of Smithy sfn#HistoryEventType) and no TaskSucceeded for that state. The TaskFailed event's details (Smithy sfn#TaskFailedEventDetails: required resourceType and resource, plus error and cause) name resourceType "lambda" and resource "invoke" for the optimized integration, set `error` to the errorType from the function's error document (or `Lambda.Unknown` when the runtime reports none), and set `cause` to that JSON error document verbatim. With no Retry/Catch on the state the execution terminates FAILED and the ExecutionFailed event (Smithy sfn#ExecutionFailedEventDetails) plus DescribeExecution repeat the same error and cause unchanged. The error code and the cause document are the requirement; the thrown type name, the message text and the stack lines are values taken from the function and its input, not fixed text.

Emulator: the execution ends `status: SUCCEEDED`. DescribeExecution reports no `error` and no `cause`, the history contains no TaskFailed (and no LambdaFunctionFailed) event, and the Invoke result — including its function-error indicator and the runtime's error document — is passed through as if it were a successful task result. Replay detail: `AssertionError: the execution ended 'SUCCEEDED', expected FAILED from the unhandled exception`.

Why: a client cannot tell a failed workflow from a successful one. Status, error/cause, and the history event family a client inspects to attribute a failure to a state all report success, so Retry/Catch never evaluate, alarms and alerting built on FAILED executions never fire, and downstream states receive the function's error document as if it were a valid result.

Fix: the Step Functions ASL interpreter's Task-state Lambda integration must map a non-empty FunctionError on the Invoke response to a task failure before Retry/Catch evaluation and before the execution's terminal status is set — emitting LambdaFunctionFailed and TaskFailed with resourceType "lambda"/resource "invoke", the function's errorType as the error and its JSON error document as the cause, and propagating the same error/cause to ExecutionFailed and DescribeExecution. The Lambda Invoke handler must also surface the runtime's error document in the Node.js errorType/errorMessage/trace shape so the interpreter has that errorType to report.

Related open issues (neither carries this check's replay key, so neither covers it): 20261105T081644Z-stepfunctions-a-raw-function-arn-task-emits-no.md (its Cause 3 reports the same FunctionError-to-task-failure gap from the Retry/Catch angle under check stepfunctions.sfn-retry-with-backoff) and 20261105T081644Z-lambda-stepfunctions-a-raising-lambda-handler-never-fails.md (same gap under the lambda__stepfunctions heading). This check additionally pins the TaskFailed details (resourceType/resource, error = the thrown type, cause = the errorType/errorMessage/trace document) and their agreement with ExecutionFailed and DescribeExecution, which neither of those issues asserts.

Traces: stepfunctions.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause.

## Evidence
Check stepfunctions.deep-sfn-1-sfn-sync-lambda-unhandled-exception-error-and-json-cause, verdict mismatch:

  File "check_stepfunctions_deep_sfn_1_sfn_sync_lambda_unhandled_exception_error_and_json_cause.py", line 46, in run
    assert desc["status"] == "FAILED", (
  AssertionError: the execution ended 'SUCCEEDED', expected FAILED from the unhandled exception

Expected (AWS): DescribeExecution status FAILED with error = the thrown error's name and cause = the function's JSON error document; exactly one TaskFailed event with resourceType "lambda", resource "invoke", error = that same name and a cause parsing to exactly {errorType, errorMessage, trace} where trace[0] == "<errorType>: <errorMessage>"; no TaskSucceeded for the state; exactly one ExecutionFailed repeating that error and cause.
Actual (emulator): status SUCCEEDED, no error/cause on DescribeExecution, no TaskFailed/LambdaFunctionFailed in GetExecutionHistory. The assertion fails on the status before the remaining TaskFailed/ExecutionFailed assertions are reached.

## Steps to Reproduce
1. Create a nodejs22.x Lambda function whose handler throws a custom Error subclass whose `name` is set (e.g. class CustomBoomError extends Error, thrown with a message embedding a value from the event).
2. Create an execution role trusted by states.amazonaws.com with lambda:InvokeFunction on that function.
3. CreateStateMachine with a single Task state: Resource "arn:aws:states:::lambda:invoke", Parameters {"FunctionName": <fn arn>, "Payload.$": "$"}, End true — no Retry and no Catch.
4. StartExecution with any JSON object input, then poll DescribeExecution to a terminal status.
5. Observe status SUCCEEDED with no error/cause (AWS: FAILED with error = the thrown type name, cause = the runtime error document).
6. GetExecutionHistory on that execution: no TaskFailed and no LambdaFunctionFailed event exists (AWS: one TaskFailed with resourceType "lambda"/resource "invoke" and one ExecutionFailed carrying the same error and cause, and no TaskSucceeded).

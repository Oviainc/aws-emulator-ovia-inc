Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__stepfunctions.sfn-saga-compensate-on-failure
Issue-Key: dynamodb__lambda__stepfunctions.sfn-saga-compensate-on-failure|run#13|AssertionError|assert comp_item is not None, "Compensation Lambda was not invoked"
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# dynamodb__lambda__stepfunctions: a saga's failing Lambda Task SUCCEEDS, so the Catch-routed compensation Lambda is never invoked (1 cause)

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: an unhandled Lambda handler error does not fail the Task state, so no Catch transition fires and the saga's compensating action never runs.
AWS: Lambda Invoke answers HTTP 200 with the X-Amz-Function-Error header set to "Unhandled" when the handler raises (Smithy: lambda#Invoke output member FunctionError -> httpHeader X-Amz-Function-Error). Step Functions turns that into a task failure: the history records LambdaFunctionFailed then the Catch's transition (Smithy sfn#HistoryEventType enumerates LambdaFunctionFailed/TaskFailed), the task error is the errorType from the function's error document (the thrown type, e.g. `Exception`, or `Lambda.Unknown` when the runtime reports none), and the cause is that JSON error document. A `Catch` entry matching `States.TaskFailed` (or that errorType) routes the execution to the compensation state, which then runs.
Emulator: the Task is reported successful. The state result is the whole Invoke envelope including `"FunctionError":"Unhandled"` and the error payload, the execution continues down the success branch, and the Catch target — the compensation Lambda — is never scheduled.
Why: the saga/compensation pattern is silently broken. A client that relies on Catch for rollback gets a workflow that reports success while leaving the partially-applied DynamoDB writes uncompensated — the exact data-integrity outcome the pattern exists to prevent.
Fix: the Step Functions ASL interpreter's Task-state Lambda integration must map a non-empty FunctionError on the Invoke response to a task failure (LambdaFunctionFailed/TaskFailed with the function's errorType and error document) so Retry and Catch evaluation runs.
Traces: dynamodb__lambda__stepfunctions.sfn-saga-compensate-on-failure.

## Evidence
Expected: the failing Lambda Task fails, the state's Catch routes to the compensation state, and the compensation Lambda is invoked. Actual: `AssertionError: Compensation Lambda was not invoked` — the failing Task returned a success result carrying the Invoke envelope (`FunctionError: "Unhandled"`, `Payload.errorType`), so the Catch branch was never taken.

## Steps to Reproduce
1. Create a DynamoDB table and two Python Lambdas: a "work" handler that writes an item then raises, and a "compensate" handler that deletes it.
2. Create a state machine whose Task state invokes the work function via arn:aws:states:::lambda:invoke and declares Catch: [{ErrorEquals: ["States.TaskFailed"], Next: "Compensate"}], with Compensate invoking the compensation function.
3. StartExecution.
4. Observe the work Task reported as succeeded (result = the Invoke envelope with FunctionError "Unhandled"), the Catch branch never entered and the compensation function never invoked.

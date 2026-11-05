Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.deep-sfn-1-sfn-sync-lambda-invoke-envelope-vs-raw-arn-bare-payload stepfunctions.deep-sfn-1-sfn-sync-lambda-result-over-256kib-rejected stepfunctions.sfn-retry-with-backoff
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-lambda-invoke-envelope-vs-raw-arn-bare-payload|run#19|AssertionError|assert len(raw_ok) == 1 and json.loads(raw_ok[0]["output"]) == bare, (
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-lambda-result-over-256kib-rejected|run#11|AssertionError|assert bad.get("cause") == expected_cause, f"expected cause {expected_cause!r}, got {bad.get('cause')!r}"
Issue-Key: stepfunctions.sfn-retry-with-backoff|run#9|AssertionError|assert output == "recovered-after-retries", (
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# stepfunctions: a raw-function-ARN Task emits no LambdaFunction* history events, the >256 KiB result refusal is misworded, and a raising handler never fails the Task so Retry/Catch never run (3 causes)

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
Three net-new root causes under the `stepfunctions` heading. Each of the other 48 failures in this service is marked OWNED by an open issue in the replay and is skipped here.

Cause 1 of 3: a Task state whose Resource is a raw Lambda function ARN produces no LambdaFunction* history events (and so no observable bare-payload result).
AWS: a Task state can name a function ARN directly, in which case the state result is the function's bare payload (not the Invoke envelope). Either form contributes LambdaFunctionScheduled, LambdaFunctionStarted and LambdaFunctionSucceeded to the execution history — all modelled members of Smithy sfn#HistoryEventType, alongside LambdaFunctionFailed, LambdaFunctionScheduleFailed, LambdaFunctionStartFailed and LambdaFunctionTimedOut — with the LambdaFunctionSucceeded event's output carrying the function's returned document.
Emulator: GetExecutionHistory contains no LambdaFunctionSucceeded event for the raw-ARN state (the filtered list was `[]`), so the documented difference between the raw-ARN form (bare payload) and the optimized `arn:aws:states:::lambda:invoke` form (envelope with StatusCode/Payload/ExecutedVersion) cannot be observed at all.
Why: the history API is the documented way to see what a Task was given and what it returned. A client that inspects LambdaFunctionSucceeded to read a task's output, or that relies on the raw-ARN form's bare payload, gets nothing.
Fix: the Step Functions execution-history recorder must emit the LambdaFunction* event family for both Lambda Task forms, and the interpreter must return the bare function payload (not the envelope) for a raw-function-ARN Resource.
Traces: stepfunctions.deep-sfn-1-sfn-sync-lambda-invoke-envelope-vs-raw-arn-bare-payload.

Cause 2 of 3: the >256 KiB Task-result refusal uses emulator wording instead of AWS's message.
AWS: when a Task's result exceeds the 262144-byte state-payload limit the task fails with the cause "The state/task '<resource>' returned a result with a size exceeding the maximum number of bytes service limit." — that sentence is fixed text, and `<resource>` (the Task's resolved resource, here the Lambda function ARN) is its only variable part.
Emulator: the cause is `the 'Task' state 'Invoke' produced a document of at least 307212 bytes, more than the 262144 a state's output may carry` — it names the state instead of the resource and reports a measured byte count AWS never emits.
Why: a client that matches AWS's cause text (or the States.DataLimitExceeded error) to tell a payload-limit failure apart from any other task failure cannot do so, and the message exposes an implementation detail that is not part of the contract.
Fix: the Step Functions ASL interpreter's state-payload size guard must emit AWS's States.DataLimitExceeded error with the "The state/task '<resource>' returned a result with a size exceeding the maximum number of bytes service limit." cause.
Traces: stepfunctions.deep-sfn-1-sfn-sync-lambda-result-over-256kib-rejected.

Cause 3 of 3: an unhandled Lambda handler error does not fail the Task state, so Retry backoff and the Catch transition never run.
AWS: Lambda Invoke answers 200 with X-Amz-Function-Error: Unhandled when the handler raises (Smithy: lambda#Invoke output FunctionError -> httpHeader X-Amz-Function-Error). Step Functions fails the task — LambdaFunctionFailed then TaskFailed — with the error taken from the function's errorType (the thrown type, e.g. `Exception`, or `Lambda.Unknown` when the runtime reports none) and the cause set to the function's JSON error document. A Retry entry matching States.TaskFailed re-invokes with the configured IntervalSeconds/BackoffRate up to MaxAttempts, and when they are exhausted the state's Catch routes the execution to its handler state, whose output becomes the execution output.
Emulator: the Task is reported successful; the execution output is the raw Invoke envelope `{"StatusCode":200,"ExecutedVersion":"$LATEST","FunctionError":"Unhandled","Payload":{"errorMessage":"intentional transient failure","errorType":"Exception",...}}` instead of the Catch-routed Pass state's output, and no retry occurs.
Why: Retry-with-backoff and Catch — the two mechanisms every production workflow uses for transient failures — are inert, so a transient fault is reported as a success and the error document is handed to downstream states as if it were a valid result.
Fix: the Step Functions ASL interpreter's Task-state Lambda integration must map a non-empty FunctionError on the Invoke response to a task failure before Retry/Catch evaluation.
Traces: stepfunctions.sfn-retry-with-backoff.

## Evidence
Cause 1 — expected exactly one LambdaFunctionSucceeded event whose output is the function's bare payload. Actual: `AssertionError: the raw-ARN state's LambdaFunctionSucceeded events were []`.

Cause 2 — `AssertionError: expected cause "The state/task 'arn:aws:lambda:<region>:<account>:function:<name>' returned a result with a size exceeding the maximum number of bytes service limit.", got "the 'Task' state 'Invoke' produced a document of at least 307212 bytes, more than the 262144 a state's output may carry"`.

Cause 3 — `AssertionError: Expected the Catch-routed Pass state's output, got {'StatusCode': 200, 'ExecutedVersion': '$LATEST', 'FunctionError': 'Unhandled', 'Payload': {'errorMessage': 'intentional transient failure', 'errorType': 'Exception', ...}, ...}`.

## Steps to Reproduce
Cause 1: Create a state machine with two Task states over the same function — one with Resource set to the raw function ARN, one with arn:aws:states:::lambda:invoke — run it, then GetExecutionHistory and filter for type == "LambdaFunctionSucceeded": none exists for the raw-ARN state.
Cause 2: Create a function that returns a >262144-byte document, run it from a Task state, and read the task's error/cause from the history or DescribeExecution.
Cause 3: Create a state machine whose Task invokes a handler that raises, with Retry [{ErrorEquals:["States.TaskFailed"], IntervalSeconds:1, BackoffRate:2, MaxAttempts:2}] and Catch to a Pass state; StartExecution and read the execution output — it is the Invoke envelope, not the Pass state's output, and no retry was attempted.

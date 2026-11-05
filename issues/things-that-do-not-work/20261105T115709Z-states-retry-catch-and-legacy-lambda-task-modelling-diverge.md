Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-legacy-lambda-arn-task-failure-events
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-legacy-lambda-arn-task-failure-events sfn-retry-catch-error-name-matching sfn-test-state-choice-and-error-details sfn-validate-retry-catch-definition-diagnostics
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].lambdaFunctionFailedEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].stateExitedEventDetails
Issue-Key: states|TestState|2xx->2xx|$.inspectionData.errorDetails.retryIndex
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.diagnostics[*].message
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.diagnostics[*]
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.result
Issue-Related: 20261029T171911Z-states-step-functions-teststate-does-not-evaluate-jsonata-states.md
Issue-Related: 20261103T052008Z-states-step-functions-execution-history-omits-task-lifecycle-events.md
Issue-Commit: 5368d81c71b1007c24003ba7dd128ee31ad4f292

# Retry/Catch and legacy Lambda-task modelling diverge across GetExecutionHistory, TestState and ValidateStateMachineDefinition (5 causes)

> _Note: this issue was identified against the code base at commit `5368d81c71b1007c24003ba7dd128ee31ad4f292`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
Five net-new root causes share one fault: the emulator does not model the Retry/Catch and Lambda-task machinery of a Step Functions workflow, so every surface that reports on it — execution history, TestState inspection, and definition validation — reports a run that did not happen. ALL FIVE must be closed before this issue is submitted.

Adjacent open issues that do NOT cover these steps (they carry no replay key for them): 20261103T052008Z-states-step-functions-execution-history-omits-task-lifecycle-events.md (Cause 1, Cause 2) and 20261029T171911Z-states-step-functions-teststate-does-not-evaluate-jsonata-states.md (Cause 3). A similar open issue does not cover a failure; these are filed separately and named here.

=== Cause 1 of 5: a Task state whose Resource is a bare Lambda function ARN emits the generic Task* history family instead of the LambdaFunction* family ===
AWS: when a Task state's Resource field is a bare Lambda function ARN (arn:aws:lambda:<region>:<account>:function:<name>, the legacy integration, as opposed to arn:aws:states:::lambda:invoke), the execution history records the Lambda-specific event family: LambdaFunctionScheduled, then LambdaFunctionStarted, then LambdaFunctionFailed (or LambdaFunctionSucceeded / LambdaFunctionTimedOut), then the execution-level terminal event. Smithy: HistoryEventType enum members LambdaFunctionScheduled, LambdaFunctionScheduleFailed, LambdaFunctionStarted, LambdaFunctionStartFailed, LambdaFunctionSucceeded, LambdaFunctionFailed, LambdaFunctionTimedOut; LambdaFunctionScheduledEventDetails carries required `resource` (the scheduled function's ARN) plus `input` and `inputDetails` (HistoryEventExecutionDataDetails, with `truncated`); LambdaFunctionFailedEventDetails carries `error` and `cause`. The recorded response shows the shape of that run: events[2] LambdaFunctionScheduled with lambdaFunctionScheduledEventDetails {resource = the function ARN from the state's Resource field, input = the state input JSON, inputDetails.truncated = false}, events[3] LambdaFunctionStarted, events[4] LambdaFunctionFailed with lambdaFunctionFailedEventDetails {error = the function's error name, cause = its cause text}, and events[5] ExecutionFailed with executionFailedEventDetails {error, cause} and id 6 / previousEventId 5.
Emulator: GetExecutionHistory returns 200 but with the wrong event family and a short history — events[2].type is "TaskScheduled" with NO lambdaFunctionScheduledEventDetails member at all, events[3].type is "TaskFailed" where LambdaFunctionStarted belongs, events[4].type is "ExecutionFailed" where LambdaFunctionFailed belongs and carries no lambdaFunctionFailedEventDetails, and the history simply stops there: the final ExecutionFailed event is absent entirely.
Why: a client that reads a failed Lambda task's diagnosis out of the history — any console, any log shipper, any retry/alerting tool that switches on HistoryEventType — finds no LambdaFunction* event and no lambdaFunctionFailedEventDetails, so the function's error name and cause are unreachable; and because the terminal ExecutionFailed event is missing, a consumer that waits for the execution-level terminal event never sees the run end.
Fix: the execution engine's history recorder for Task states, in the states crate — classify a Resource that is a bare Lambda function ARN as the legacy Lambda integration and emit the LambdaFunction* event family with its typed detail members, and emit the execution-level terminal event after it.
Traces: sfn-legacy-lambda-arn-task-failure-events#task#10 (GetExecutionHistory).

=== Cause 2 of 5: Retry attempts and the Catch branch leave no history at all, and taskScheduledEventDetails.parameters is serialized empty ===
AWS: when a Task state carries a Retry, EACH attempt is recorded as its own TaskScheduled -> TaskStarted -> TaskFailed triple (with taskFailedEventDetails {error, cause, resource, resourceType} on each failed attempt) before the state is finally resolved; when the retries are exhausted and a Catch matches, the state exits and the Catch's target branch is recorded (for a Pass target: PassStateEntered then PassStateExited for that state's name) before the execution's terminal event. Every TaskScheduled event carries taskScheduledEventDetails whose `parameters` is the JSON payload actually sent to the integration. Smithy: TaskScheduledEventDetails requires resourceType, resource, region AND parameters (ConnectorParameters, length 0..262144); TaskFailedEventDetails carries error/cause/resource/resourceType; HistoryEventType includes TaskScheduled/TaskStarted/TaskFailed/TaskStateExited/PassStateEntered/PassStateExited.
Emulator: taskScheduledEventDetails.parameters is serialized as the empty string instead of the task's parameter JSON (for a lambda:invoke task, the object naming the function and its payload). The retry attempts are not recorded at all and the outcome is inverted: events[4].type is "TaskSucceeded" where the first attempt's TaskFailed belongs, and the history then runs straight on to TaskStateExited / SucceedStateEntered / SucceedStateExited / ExecutionSucceeded — so none of the per-attempt TaskScheduled/TaskStarted/TaskFailed events exist, no taskFailedEventDetails exists anywhere, the Catch branch's PassStateEntered/PassStateExited events for the recovery state are absent, and the run is reported as having succeeded on the first attempt.
Why: a client cannot tell that a task was retried, how many times, or which error each attempt raised; worse, a run that AWS reports as retried-and-caught is reported by the emulator as a clean first-attempt success, so any test or operator dashboard asserting on retry behaviour or on the compensation branch passes vacuously. The empty `parameters` also hides what payload a task was actually sent, which is the member replay and debugging tools read.
Fix: the Retry/Catch handling in the state-machine interpreter together with the history recorder — record one TaskScheduled/TaskStarted/TaskFailed triple per attempt with taskFailedEventDetails, record the Catch target branch's state-entered/state-exited events, and populate TaskScheduledEventDetails.parameters from the parameters the task was dispatched with.
Traces: sfn-retry-catch-error-name-matching#task#13 (GetExecutionHistory).

=== Cause 3 of 5: TestState on a state whose invocation errors omits inspectionData.errorDetails.retryIndex and returns the state input as the output ===
AWS: TestState reports the tested state's actual evaluation. `output` is the JSON output data of the state, so for a state whose invocation produced an error result it is that error result (an object carrying the error name and cause), not the state's input echoed back. When a Retry handled the exception, inspectionData.errorDetails.retryIndex is populated with the 0-based array index of the Retry entry that handled it. Smithy: TestStateOutput.output (SensitiveData, "The JSON output data of the state"), TestStateOutput.inspectionData -> InspectionData.errorDetails -> InspectionErrorDetails with members catchIndex, retryIndex ("The array index of the Retry which handled the exception", range min 0) and retryBackoffIntervalSeconds. Recorded behavior shows retryIndex = 0 for the first Retry entry, and `output` carrying the error result object rather than the input.
Emulator: inspectionData.errorDetails.retryIndex is absent (None) even though a Retry handled the exception, and `output` is the raw state input echoed back unchanged instead of the state's error result.
Why: TestState exists so a workflow author can debug one state before running it. A caller gets back an output that claims the state produced its input untouched and an errorDetails object that does not say which Retry entry fired, so neither the failure nor the retry handling that AWS reports is visible — the state looks like it passed through cleanly.
Fix: the TestState handler's inspection-data construction and its state-evaluation path in the states crate — surface the evaluated state's error result as `output` and populate InspectionErrorDetails.retryIndex (and its siblings) from the Retry entry that handled the exception.
Traces: sfn-test-state-choice-and-error-details#task#6 (TestState).

=== Cause 4 of 5: the empty-ErrorEquals validation diagnostic is worded by the emulator, not by AWS ===
AWS: ValidateStateMachineDefinition reports an empty Retry/Catch ErrorEquals array as an ERROR-severity SCHEMA_VALIDATION_FAILED diagnostic whose message is the fixed text "Value cannot be empty" — the message for this diagnostic class has no variable part; the variable part of the finding lives in `location`, which is the JSON pointer to the offending field (of the form /States/<StateName>/Retry[<index>]/ErrorEquals, with the state name and retry index taken from the submitted definition). Smithy: ValidateStateMachineDefinitionDiagnostic requires severity, code and message and carries optional location ("For errors specific to a field, the location could be in the format /States/<StateName>/<FieldName>"); the error code list includes SCHEMA_VALIDATION_FAILED ("Schema validation reported errors").
Emulator: diagnostics[0].message is "'ErrorEquals' must name at least one error" — emulator-authored wording for a diagnostic AWS words "Value cannot be empty".
Why: validation output is what a CI gate or an authoring tool shows the user and, in practice, matches on. A message that is not AWS's words means golden-file assertions on validation output fail against the emulator and pass against AWS (or the reverse), and the text a user is shown differs from the documented one.
Fix: the ValidateStateMachineDefinition diagnostic builder in the states crate — emit AWS's fixed message text for the empty-value schema diagnostic and keep the field identification in `location`.
Traces: sfn-validate-retry-catch-definition-diagnostics#task#7 (ValidateStateMachineDefinition).

=== Cause 5 of 5: a Retry BackoffRate below 1.0 is not validated at all, so an invalid definition validates OK ===
AWS: a Retrier's BackoffRate has a minimum of 1.0. ValidateStateMachineDefinition on a definition whose Retry sets BackoffRate below that returns result = FAIL with one diagnostic {severity: "ERROR", code: "SCHEMA_VALIDATION_FAILED", location: the JSON pointer to the offending field — /States/<StateName>/Retry[<index>]/BackoffRate, state name and retry index taken from the submitted definition, message: the fixed text "Minimum value is 1.0"}. Smithy: ValidateStateMachineDefinitionDiagnostic (severity/code/message required, location optional) and its documented error code SCHEMA_VALIDATION_FAILED; the doc notes a caller should rely on the `result` field value (OK, FAIL), which is exactly the field that diverges here.
Emulator: result is "OK" and the diagnostics list is empty — the BackoffRate range is never checked, so an unsubmittable definition is reported as valid.
Why: `result` is the one field AWS tells callers to rely on, so a deployment pipeline that gates on ValidateStateMachineDefinition lets a definition through that CreateStateMachine/UpdateStateMachine would reject — the failure moves from validation time to deploy time, and the emulator never reproduces the rejection the pipeline was built to catch.
Fix: the definition schema validation used by ValidateStateMachineDefinition (and shared with CreateStateMachine/UpdateStateMachine) in the states crate — add the Retrier BackoffRate minimum check and have it contribute an ERROR SCHEMA_VALIDATION_FAILED diagnostic, which in turn flips `result` to FAIL.
Traces: sfn-validate-retry-catch-definition-diagnostics#task#10 and #task#11 (ValidateStateMachineDefinition).

## Evidence
Replayed trace set simparity-wire-aws-v1 against the emulator (emulator_up = true, 747 steps, 694 passed, 53 mismatched). These five clusters are the ones no open issue owns and no stub blocks.

Cause 1 — fixture sfn-legacy-lambda-arn-task-failure-events, step task#10, GetExecutionHistory, 200 -> 200:
  $.events[2].lambdaFunctionScheduledEventDetails: expected {'input': <state input JSON>, 'inputDetails': {'truncated': False}, 'resource': <lambda function ARN>}, got None
  $.events[2].type: expected 'LambdaFunctionScheduled', got 'TaskScheduled'
  $.events[3].type: expected 'LambdaFunctionStarted', got 'TaskFailed'
  $.events[4].lambdaFunctionFailedEventDetails: expected {'cause': <shaped>, 'error': 'RuntimeError'}, got None
  $.events[4].type: expected 'LambdaFunctionFailed', got 'ExecutionFailed'
  $.events[5]: expected a HistoryEvent {id 6, previousEventId 5, type 'ExecutionFailed', executionFailedEventDetails {error 'RuntimeError', cause <shaped>}}, got None

Cause 2 — fixture sfn-retry-catch-error-name-matching, step task#13, GetExecutionHistory, 200 -> 200:
  $.events[2].taskScheduledEventDetails.parameters: expected the dispatched payload JSON ({"FunctionName":<function ARN>,"Payload":{...}}), got '' (empty string)
  $.events[4].taskFailedEventDetails: expected {'error': 'RuntimeError', 'cause': <shaped>, 'resource': 'invoke', 'resourceType': 'lambda'}, got None
  $.events[4].type: expected 'TaskFailed', got 'TaskSucceeded'
  $.events[5].taskScheduledEventDetails: expected {'parameters': <payload JSON>, 'region': <region>, 'resource': 'invoke', 'resourceType': 'lambda'} (the retry attempt), got None
  $.events[5].type: expected 'TaskScheduled', got 'TaskStateExited'
  $.events[6].taskStartedEventDetails: expected {'resource': 'invoke', 'resourceType': 'lambda'}, got None
  $.events[6].type: expected 'TaskStarted', got 'SucceedStateEntered'
  $.events[7].taskFailedEventDetails: expected {'error': 'RuntimeError', 'cause': <shaped>, 'resource': 'invoke', 'resourceType': 'lambda'}, got None
  $.events[7].type: expected 'TaskFailed', got 'SucceedStateExited'
  $.events[8].stateExitedEventDetails: expected {'name': <task state name>}, got None
  $.events[8].type: expected 'TaskStateExited', got 'ExecutionSucceeded'
  $.events[9], $.events[10], $.events[11]: expected the Catch branch's PassStateEntered / PassStateExited events for the recovery state and the terminal event, got None (history ends at index 8)

Cause 3 — fixture sfn-test-state-choice-and-error-details, step task#6, TestState, 200 -> 200:
  $.inspectionData.errorDetails.retryIndex: expected 0, got None
  $.output: expected the state's error result object ({"Error":<error name>,"Cause":<cause text>}), got the state input echoed back

Cause 4 — fixture sfn-validate-retry-catch-definition-diagnostics, step task#7, ValidateStateMachineDefinition, 200 -> 200:
  $.diagnostics[0].message: expected 'Value cannot be empty', got "'ErrorEquals' must name at least one error"
  (fixed text differs; the variable part of this finding is `location`, not the message)

Cause 5 — fixture sfn-validate-retry-catch-definition-diagnostics, steps task#10 and task#11, ValidateStateMachineDefinition, 200 -> 200:
  $.result: expected 'FAIL', got 'OK'
  $.diagnostics[0]: expected {'severity': 'ERROR', 'code': 'SCHEMA_VALIDATION_FAILED', 'location': '/States/<StateName>/Retry[0]/BackoffRate', 'message': 'Minimum value is 1.0'}, got None

Model references used: com.amazonaws.sfn#HistoryEventType, #LambdaFunctionScheduledEventDetails, #LambdaFunctionFailedEventDetails, #TaskScheduledEventDetails, #TestStateOutput, #InspectionData, #InspectionErrorDetails, #ValidateStateMachineDefinitionDiagnostic.

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus). The five causes reproduce at these steps:

Cause 1: replay fixture sfn-legacy-lambda-arn-task-failure-events through step task#10. Create a state machine whose Task state sets "Resource" to a bare Lambda function ARN (not arn:aws:states:::lambda:invoke) and whose function raises, StartExecution, wait for the FAILED terminal state, then call GetExecutionHistory on the execution ARN and inspect the `type` of each event and the presence of lambdaFunctionScheduledEventDetails / lambdaFunctionFailedEventDetails and of the final ExecutionFailed event.

Cause 2: replay fixture sfn-retry-catch-error-name-matching through step task#13. Create a state machine whose Task state carries a Retry (matching the error the task raises) and a Catch routing to a Pass state, run it so the task fails every attempt, then call GetExecutionHistory and check for one TaskScheduled/TaskStarted/TaskFailed triple per attempt, for taskFailedEventDetails, for taskScheduledEventDetails.parameters being non-empty, and for the Catch branch's PassStateEntered/PassStateExited events.

Cause 3: replay fixture sfn-test-state-choice-and-error-details through step task#6. Call TestState on a state whose invocation errors and which carries a Retry, with an inspectionLevel that returns inspectionData, and read inspectionData.errorDetails.retryIndex and the top-level `output`.

Cause 4: replay fixture sfn-validate-retry-catch-definition-diagnostics through step task#7. Call ValidateStateMachineDefinition with a definition whose Retry entry has an empty "ErrorEquals": [] and read diagnostics[0].message.

Cause 5: replay fixture sfn-validate-retry-catch-definition-diagnostics through steps task#10 and task#11. Call ValidateStateMachineDefinition with a definition whose Retry entry sets "BackoffRate" below 1.0 and read `result` and `diagnostics`.

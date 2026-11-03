Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-failing-lambda-task-execution-diagnosis
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-failing-lambda-task-execution-diagnosis sfn-legacy-lambda-arn-task-failure-events sfn-retry-catch-error-name-matching sfn-runtime-error-bypasses-catch-and-redrive
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*]
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskFailedEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskStartedEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].type
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.nextToken
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskScheduledEventDetails.parameters
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].executionRedrivenEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].previousEventId
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskScheduledEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].lambdaFunctionScheduledEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskFailedEventDetails.cause
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskFailedEventDetails.error
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].executionFailedEventDetails.error
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskTimedOutEventDetails
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].executionFailedEventDetails
Issue-Commit: e331167544f8c61f25de403c8943054d68a37733

# Step Functions execution history omits Task lifecycle events

> _Note: this issue was identified against the code base at commit `e331167544f8c61f25de403c8943054d68a37733`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
GetExecutionHistory returns a truncated, mis-numbered event stream for every execution that contains a Task state. The emulator records only TaskStateEntered -> TaskFailed -> ExecutionFailed (and sometimes collapses even that), so the TaskScheduled/TaskStarted/TaskTimedOut/LambdaFunction* events, the retry and catch events, and the redrive events AWS records are all absent, and the ids/previousEventIds of the events that do appear are therefore shifted. 9 causes.

Cause 1 of 9 — TaskStarted and TaskFailed are never recorded as separate events, and ExecutionFailed is not appended after them.
AWS: Smithy: GetExecutionHistory.events -> HistoryEventList of HistoryEvent; HistoryEventType enumerates TaskScheduled, TaskStarted, TaskFailed, TaskTimedOut, TaskStateExited, ExecutionFailed, ExecutionRedriven, LambdaFunctionScheduled/Started/Failed. For a Task state that fails, AWS appends TaskStarted (taskStartedEventDetails {resource, resourceType}), then TaskFailed (taskFailedEventDetails {error, cause, resource, resourceType}), then ExecutionFailed (executionFailedEventDetails {error, cause}), each with its own id and previousEventId = id-1.
Emulator: event[3] is TaskFailed where AWS has TaskStarted, event[3].taskStartedEventDetails is absent, event[4] is ExecutionFailed where AWS has TaskFailed with taskFailedEventDetails, and the trailing ExecutionFailed event (id 6, previousEventId 5) is missing entirely.
Why: a client that polls GetExecutionHistory to find out which task failed and when it started sees no TaskStarted/TaskFailed pair and cannot attribute the failure to a task; event ids are not contiguous with AWS's, so any consumer that stores previousEventId linkage reconstructs a different graph.
Fix: the states execution interpreter's history recorder — it must emit one event per task lifecycle transition rather than one combined failure event.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#10

Cause 2 of 9 — because the lifecycle events above are missing, the terminal ExecutionFailed event carries id 5/previousEventId 4 instead of AWS's 6/5, so a reverseOrder page starts on the wrong event.
AWS: recorded behavior only — the first event of the reverse-ordered page is ExecutionFailed with id 6 and previousEventId 5 for this definition.
Emulator: events[0] is ExecutionFailed with id 5, previousEventId 4.
Why: paging backwards from the end returns events the caller has already seen / skips ones it has not, and ids quoted in one response do not resolve in another.
Fix: the history recorder's event-id allocator, downstream of cause 1.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#11

Cause 3 of 9 — taskScheduledEventDetails.parameters is serialized as the empty string.
AWS: recorded behavior only — TaskScheduled's taskScheduledEventDetails.parameters is the JSON the Task's Parameters/Arguments resolved to, e.g. the resolved FunctionName and Payload members of the task; AWS also sets region, resource and resourceType there.
Emulator: parameters is ''.
Why: a client that reads back what was actually submitted to the integration (the usual way to debug a Parameters mapping) gets nothing.
Fix: the Task-state handler must record the resolved parameter document when it writes TaskScheduled.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#12

Cause 4 of 9 — after RedriveExecution, the redrive event sequence is not recorded.
AWS: HistoryEventType includes ExecutionRedriven with executionRedrivenEventDetails {redriveCount}; AWS appends ExecutionRedriven and then a fresh TaskScheduled/TaskStarted/TaskFailed cycle, continuing the same id sequence.
Emulator: the page's events are the pre-redrive ExecutionFailed/TaskFailed pair with lower ids; the ExecutionRedriven event and the re-run task events are absent.
Why: a client cannot tell a redriven execution from its first attempt, and redriveCount is unobservable in history.
Fix: RedriveExecution's handler plus the history recorder.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#19

Cause 5 of 9 — a Task whose Resource is a bare Lambda function ARN (the legacy form) records Task* events instead of the LambdaFunction* family.
AWS: HistoryEventType distinguishes LambdaFunctionScheduled (lambdaFunctionScheduledEventDetails {input, inputDetails.truncated, resource}), LambdaFunctionStarted and LambdaFunctionFailed (lambdaFunctionFailedEventDetails {error, cause}) from the Task* family; a legacy function-ARN Resource uses the LambdaFunction* family.
Emulator: event[2] is TaskFailed (no lambdaFunctionScheduledEventDetails), event[3] is ExecutionFailed, and the LambdaFunctionFailed and trailing ExecutionFailed events are missing.
Why: tooling that keys off LambdaFunctionScheduled/Failed (the documented shape for legacy ARNs) sees no Lambda events at all for an execution that invoked a function.
Fix: the Task-state handler's event-family selection, keyed on whether Resource is a bare function ARN or an arn:aws:states:::lambda:invoke integration.
Traces: sfn-legacy-lambda-arn-task-failure-events#task#10

Cause 6 of 9 — a Retry on a Task records no second attempt, and the recorded failure carries emulator-authored text.
AWS: a Task with Retry re-emits TaskScheduled/TaskStarted/TaskFailed per attempt, then TaskStateExited and the Catch target's state-entered event once the retries are exhausted or the error is caught.
Emulator: events[4].taskFailedEventDetails.error is States.Runtime with cause "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task" instead of the function's own error; events[5] onwards (the retry attempt's TaskScheduled, TaskStarted, TaskFailed, TaskStateExited and the Recovered PassStateEntered) are absent, and ExecutionFailed is written instead.
Why: a client validating its Retry/Catch policy sees one attempt and a failed execution where AWS shows the configured number of attempts and a recovery branch. Note: the open issue 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md covers the lambda:invoke dispatch itself; it does not cover the missing retry-attempt history events recorded here.
Fix: the Retry/Catch driver in the states execution interpreter, and the history recorder it calls per attempt.
Traces: sfn-retry-catch-error-name-matching#task#13

Cause 7 of 9 — a Task that exceeds its TimeoutSeconds records TaskFailed instead of TaskTimedOut, and the execution fails with the wrong error name.
AWS: HistoryEventType includes TaskTimedOut with taskTimedOutEventDetails {error, resource, resourceType}; the error name for a task timeout is States.Timeout, and the resulting ExecutionFailed carries error States.Timeout.
Emulator: events[4].type is TaskFailed with no taskTimedOutEventDetails, and events[5].executionFailedEventDetails.error is States.Runtime.
Why: a client that retries only on States.Timeout, or that branches a Catch on it, never matches.
Fix: the Task-state timeout path in the interpreter must raise States.Timeout and record TaskTimedOut.
Traces: sfn-retry-catch-error-name-matching#task#17

Cause 8 of 9 — a Parameters path that resolves to nothing is recorded as a TaskFailed plus an extra ExecutionFailed, instead of a single ExecutionFailed.
AWS: an unresolvable JSONPath in a Task's Parameters is an interpreter error, not a task failure: AWS records ExecutionFailed with error States.Runtime directly on the state-entered event and records no TaskFailed, because the task was never submitted.
Emulator: events[2].type is TaskFailed (no executionFailedEventDetails) and an additional ExecutionFailed event is appended at events[3].
Why: the history says a task ran and failed when no integration was ever called, so failure accounting and any TaskFailed-driven alarm fire spuriously.
Fix: the Parameters/Arguments resolution step must fail the execution before the TaskScheduled/TaskFailed pair is recorded.
Traces: sfn-runtime-error-bypasses-catch-and-redrive#task#12

Cause 9 of 9 — after a redrive of that same execution, the reverse-ordered page's first two events are the spurious ExecutionFailed/TaskFailed pair instead of ExecutionFailed followed by ExecutionRedriven.
AWS: events[0] is ExecutionFailed (id 5, previousEventId 4) and events[1] is ExecutionRedriven with executionRedrivenEventDetails {redriveCount: 1}.
Emulator: events[0] is ExecutionFailed with id 7 and events[1] is a TaskFailed with id 6; no ExecutionRedriven event exists and the nextToken differs in shape.
Why: redriveCount and the redrive boundary are unobservable, and the ids are shifted by the extra events of cause 8.
Fix: the history recorder's redrive event emission, downstream of causes 4 and 8.
Traces: sfn-runtime-error-bypasses-catch-and-redrive#task#16

## Evidence
All nine causes are mismatch clusters from the replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout; every one returned HTTP 200 with a diverging body (no status divergence).

Cause 1 (sfn-failing-lambda-task-execution-diagnosis#task#10): $.events[3].taskStartedEventDetails: expected {'resource': 'invoke', 'resourceType': 'lambda'}, got None; $.events[3].type: expected 'TaskStarted', got 'TaskFailed'; $.events[4].taskFailedEventDetails: expected {'cause': <any>, 'error': 'RuntimeError', 'resource': 'invoke', 'resourceType': 'lambda'}, got None; $.events[4].type: expected 'TaskFailed', got 'ExecutionFailed'; $.events[5]: expected a HistoryEvent {type 'ExecutionFailed', id 6, previousEventId 5, executionFailedEventDetails {error 'RuntimeError', cause <any>}}, got None.

Cause 2 (#task#11): $.events[0]: expected {type 'ExecutionFailed', id 6, previousEventId 5}, got {type 'ExecutionFailed', id 5, previousEventId 4}.

Cause 3 (#task#12): $.events[2].taskScheduledEventDetails.parameters: expected '{"FunctionName":"<the task's resolved function arn>","Payload":{"orderId":"o-1"}}', got ''. Plus the same events[3]/events[4]/events[5] shifts as cause 1.

Cause 4 (#task#19): $.events[0] expected {type 'ExecutionFailed', id 11, previousEventId 10}, got {type 'ExecutionFailed', id 9, previousEventId 8}; $.events[1] expected {type 'TaskFailed', id 10, previousEventId 9}, got {type 'TaskFailed', id 8, previousEventId 7}; $.events[2].taskStartedEventDetails missing; $.events[4].executionRedrivenEventDetails missing.

Cause 5 (sfn-legacy-lambda-arn-task-failure-events#task#10): $.events[2].lambdaFunctionScheduledEventDetails: expected {'input': '{"orderId": "o-1"}', 'inputDetails': {'truncated': False}, 'resource': '<the legacy function arn>'}, got None; $.events[2].type expected 'LambdaFunctionScheduled', got 'TaskFailed'; $.events[3].type expected 'LambdaFunctionStarted', got 'ExecutionFailed'; $.events[4] expected {type 'LambdaFunctionFailed', id 5, previousEventId 4, lambdaFunctionFailedEventDetails {error 'RuntimeError', cause <any>}}, got None; $.events[5] expected {type 'ExecutionFailed', id 6, previousEventId 5}, got None.

Cause 6 (sfn-retry-catch-error-name-matching#task#13): $.events[2].taskScheduledEventDetails.parameters expected the resolved FunctionName/Payload document, got ''; $.events[4].taskFailedEventDetails.error expected 'RuntimeError', got 'States.Runtime'; $.events[4].taskFailedEventDetails.cause got "the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task"; $.events[5].type expected 'TaskScheduled', got 'ExecutionFailed'; $.events[6] (TaskStarted id 7), $.events[7] (TaskFailed id 8), $.events[8] (TaskStateExited id 9, stateExitedEventDetails {name 'InvokeProcessingLambda'}), $.events[9] (PassStateEntered id 10, stateEnteredEventDetails {name 'Recovered'}), $.events[10] and $.events[11] all expected, got None.

Cause 7 (sfn-retry-catch-error-name-matching#task#17): $.events[4].taskTimedOutEventDetails expected {'error': 'States.Timeout', 'resource': 'invoke', 'resourceType': 'lambda'}, got None; $.events[4].type expected 'TaskTimedOut', got 'TaskFailed'; $.events[5].executionFailedEventDetails.error expected 'States.Timeout', got 'States.Runtime'.

Cause 8 (sfn-runtime-error-bypasses-catch-and-redrive#task#12): $.events[2].executionFailedEventDetails expected {error 'States.Runtime', cause "An error occurred while executing the state 'InvokeProcessingLambda' (entered at the event id #2). The JSONPath '$.targetBucket' specified for the field 'bucket.$' could not be found in the input ..."}, got None; $.events[2].type expected 'ExecutionFailed', got 'TaskFailed'; $.events[3] expected absent, got an extra ExecutionFailed event.

Cause 9 (sfn-runtime-error-bypasses-catch-and-redrive#task#16): $.events[0] expected {type 'ExecutionFailed', id 5, previousEventId 4}, got {type 'ExecutionFailed', id 7, previousEventId 6}; $.events[1] expected {type 'ExecutionRedriven', id 4, previousEventId 3, executionRedrivenEventDetails {redriveCount 1}}, got {type 'TaskFailed', id 6, previousEventId 5}.

## Steps to Reproduce
1. Replay fixture sfn-failing-lambda-task-execution-diagnosis against the emulator and compare step task#10 (the representative). The fixture creates a STANDARD state machine whose single Task invokes a Lambda that raises, starts an execution, waits for it to reach a terminal status, then calls GetExecutionHistory with includeExecutionData.
2. Compare the returned $.events list against the recording: AWS has TaskStateEntered, TaskScheduled, TaskStarted, TaskFailed, TaskStateExited(if any) and ExecutionFailed as separate events with contiguous id/previousEventId; the emulator returns a shorter list whose event types are shifted.
3. For the other causes replay, in the same way, sfn-failing-lambda-task-execution-diagnosis#task#11, #task#12 and #task#19, sfn-legacy-lambda-arn-task-failure-events#task#10, sfn-retry-catch-error-name-matching#task#13 and #task#17, and sfn-runtime-error-bypasses-catch-and-redrive#task#12 and #task#16.
4. task#19 and sfn-runtime-error-bypasses-catch-and-redrive#task#16 must be replayed after their fixture's RedriveExecution step, since they assert the post-redrive history.

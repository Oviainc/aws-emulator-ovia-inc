Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-runtime-error-bypasses-catch-and-redrive
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-runtime-error-bypasses-catch-and-redrive sfn-retry-catch-error-name-matching
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].executionFailedEventDetails.cause
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].taskFailedEventDetails.cause
Issue-Related: 20261109T234059Z-states-getexecutionhistory-omits-the-task-lambdafunction-started-event-shifting.md
Issue-Commit: 0a7cae03cb1d4210d38997f5682a54b52a42bce2

# Execution-history failure events carry the wrong cause text for state-evaluation and task-timeout failures (3 causes)

> _Note: this issue was identified against the code base at commit `0a7cae03cb1d4210d38997f5682a54b52a42bce2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
The failure events GetExecutionHistory returns carry a `cause` string the emulator authors itself rather than the sentence AWS words for that failure class. The status (200) and the event types are right; only the cause text diverges, and it diverges the same way on every read path. 3 causes.

Cause 1 of 3 — executionFailedEventDetails.cause for a failed path-reference evaluation drops AWS's state-context prefix and re-serializes the input document.
AWS: Smithy: HistoryEvent.executionFailedEventDetails targets ExecutionFailedEventDetails (error, cause); HistoryEventType includes ExecutionFailed and EvaluationFailed. Recorded behavior for the wording: when a field's JSONPath cannot be resolved against the state's input, AWS's cause is one sentence pair — fixed text "An error occurred while executing the state '" + the failing state's name from the definition + "' (entered at the event id #" + the id of that state's state-entered event + "). The JSONPath '" + the path as written in the definition + "' specified for the field '" + the field name as written in the definition (including its '.$' suffix) + "' could not be found in the input '" + the state's input document + "'". Everything outside those insertion points is fixed; the state name, event id, path, field name and input document are taken from the definition and the execution's state.
Emulator: the cause is only the second sentence — "The JSONPath '<path>' specified for the field '<field>' could not be found in the input '<input>'" — with no "An error occurred while executing the state '<State>' (entered at the event id #<id>). " prefix, and the echoed input document is re-serialized compactly (no space after the key separator) instead of being echoed as the execution received it.
Why: the cause string is the only place the failing state is named for this failure class, so a client (or an operator reading the history) cannot tell WHICH state failed, and cannot link the failure to the state-entered event id. A client matching the cause against AWS's wording, or diffing the echoed input against the document it sent, does not match.
Fix: the execution interpreter's path/field evaluation failure path — the component that builds the cause for ExecutionFailed/EvaluationFailed — must prepend AWS's state-context sentence and echo the input document verbatim rather than re-serializing it.
Traces: sfn-runtime-error-bypasses-catch-and-redrive#task#12

Cause 2 of 3 — the same wrong cause is served on the paged/tail read of the same history, so no read path returns AWS's text.
AWS: as cause 1; the same executionFailedEventDetails.cause sentence is returned whichever page or order the event is read in.
Emulator: the same truncated, re-serialized cause appears at $.events[0] on this read.
Why: a client cannot work around the defect by re-reading the history differently; and this step also shows the non-opaque nextToken filed separately (see the companion issue on internal values echoed in states read responses).
Fix: same cause builder — it is the stored event, not the read path, that is wrong.
Traces: sfn-runtime-error-bypasses-catch-and-redrive#task#16

Cause 3 of 3 — taskFailedEventDetails.cause for a Lambda task timeout carries an emulator-authored JSON envelope instead of the payload AWS records.
AWS: Smithy: HistoryEvent.taskFailedEventDetails targets TaskFailedEventDetails (resourceType, resource, error, cause). Recorded behavior only for this failure class: the recorded cause is not the document the emulator produces; the corpus asserts its shape, not a literal value.
Emulator: the cause is a JSON object the emulator composes with the members errorMessage, errorType, requestId and stackTrace — e.g. errorType "Sandbox.Timedout" and an errorMessage of the form "<request id> Task timed out after <N>.00 seconds" — which is not the shape the recording carries, on both the first attempt and the retried attempt of the same execution.
Why: a Catch/Retry handler or an operator that parses taskFailedEventDetails.cause for the function's failure payload reads members AWS does not put there for this failure, so error classification built on the cause document behaves differently against AWS.
Fix: the Lambda-task timeout path of the interpreter's task-failure recorder — the cause document it stores on the TaskFailed event. (Only the shape/members are the requirement here; the request id and the elapsed seconds inside the message are per-run values and are not.)
Traces: sfn-retry-catch-error-name-matching#task#15

Note on already-filed issues: the open 20261103T052008Z-states-execution-outcome-error-name-and-cause-text-diverge.md owns the error name and cause text on DescribeExecution's execution-level outcome, and 20261103T052008Z-states-startsyncexecution-response-omits-outputdetails-and-leaks-failure-members.md owns StartSyncExecution's cause text; neither carries the replay keys for these three GetExecutionHistory clusters, which the replay reports as unowned.

## Evidence
All three causes are mismatch clusters from the replay of simparity-wire-aws-v1 against the emulator built from this checkout; all returned HTTP 200 as expected.

Cause 1 — sfn-runtime-error-bypasses-catch-and-redrive#task#12, GetExecutionHistory, 200 -> 200:
  $.events[2].executionFailedEventDetails.cause:
    expected "An error occurred while executing the state 'InvokeProcessingLambda' (entered at the event id #2). The JSONPath '$.targetBucket' specified for the field 'bucket.$' could not be found in the input '{\"orderId\": \"o-1\"}'"
    got      "The JSONPath '$.targetBucket' specified for the field 'bucket.$' could not be found in the input '{\"orderId\":\"o-1\"}'"
  Fixed text: "An error occurred while executing the state '", "' (entered at the event id #", "). The JSONPath '", "' specified for the field '", "' could not be found in the input '". Variable: the state name (InvokeProcessingLambda here) and the field/path as written in the definition, the state-entered event id, and the input document echoed as received.

Cause 2 — sfn-runtime-error-bypasses-catch-and-redrive#task#16, GetExecutionHistory, 200 -> 200:
  $.events[0].executionFailedEventDetails.cause: same expected-vs-actual pair as cause 1 (prefix absent, input re-serialized without the space after the separator).
  ($.nextToken also diverges on this step: expected an opaque token, got '2' — filed in the companion issue.)

Cause 3 — sfn-retry-catch-error-name-matching#task#15, GetExecutionHistory, 200 -> 200:
  $.events[4].taskFailedEventDetails.cause: expected a shaped (not literally asserted) value, got '{"errorMessage":"2ce015ec-f1b9-47c6-a3d4-a55fa1a73c25 Task timed out after 2.00 seconds","errorType":"Sandbox.Timedout","requestId":"2ce015ec-f1b9-47c6-a3d4-a55fa1a73c25","stackTrace":[]}'
  $.events[7].taskFailedEventDetails.cause: same divergence on the retried attempt.
  The uuid-shaped request ids and the "2.00 seconds" figure in those strings are per-run values; the divergence asserted is the member set of the cause document.

## Steps to Reproduce
1. Build the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Fixture sfn-runtime-error-bypasses-catch-and-redrive, step task#12 (the representative): start an execution whose Task state has a 'bucket.$' field referencing a path absent from the input, let it fail, then call GetExecutionHistory and compare $.events[2].executionFailedEventDetails.cause — AWS's sentence begins "An error occurred while executing the state '<StateName>' (entered at the event id #<id>). "; the emulator's begins at "The JSONPath" and re-serializes the echoed input.
3. Same fixture, step task#16: read the same history on the other page/order and compare $.events[0].executionFailedEventDetails.cause — identical divergence.
4. Fixture sfn-retry-catch-error-name-matching, step task#15: run a Lambda Task that times out and is retried, then call GetExecutionHistory and compare $.events[4] and $.events[7] taskFailedEventDetails.cause against the recorded document shape.

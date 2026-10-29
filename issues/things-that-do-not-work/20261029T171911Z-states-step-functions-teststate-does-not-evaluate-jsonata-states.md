Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-test-state-choice-and-error-details
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-test-state-choice-and-error-details stepfunctions-test-state-in-isolation
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|TestState|2xx->2xx|$.inspectionData.afterInputPath
Issue-Key: states|TestState|2xx->2xx|$.cause
Issue-Key: states|TestState|2xx->2xx|$.error
Issue-Key: states|TestState|2xx->2xx|$.inspectionData.afterArguments
Issue-Key: states|TestState|2xx->2xx|$.inspectionData.errorDetails
Issue-Key: states|TestState|2xx->2xx|$.status
Issue-Key: states|TestState|2xx->2xx|$.nextState
Issue-Key: states|TestState|2xx->2xx|$.output
Issue-Key: states|TestState|4xx->4xx|$.message
Issue-Key: states|TestState|2xx->2xx|$.inspectionData.afterResultSelector
Issue-Key: states|TestState|4xx->4xx|$.__type
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Step Functions TestState does not evaluate JSONata states, omits inspectionData members and words its own refusals (8 causes)

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestState

## Description
This issue bundles EIGHT net-new wire divergence clusters on sfn TestState. Smithy models TestState (host prefix `sync-`) with input {definition (required), roleArn, input, inspectionLevel, revealSecrets, variables, stateName, mock, stateConfiguration, context} and output {output, error, cause, inspectionData, nextState, status} with errors InvalidArn, InvalidDefinition, InvalidExecutionInput and ValidationException (all 400); `status` targets TestExecutionStatus whose values are SUCCEEDED | FAILED | RETRIABLE | CAUGHT_ERROR, and InspectionData declares the members afterArguments, afterInputPath, afterParameters, afterResultSelector, afterResultPath, errorDetails, variables and others. All eight causes must be closed before submitting; re-replay every fixture#step listed in trace_ids.

=== Cause 1 of 8: inspectionData payload members are re-serialized instead of echoing the submitted input ===
AWS: recorded behavior only — inspectionData.afterInputPath (and the sibling after* members) carry the state's data as the caller's own JSON text, preserving the whitespace of the `input` string supplied to TestState.
Emulator: it parses the input and re-emits it with separators stripped, so a caller's `{"n": 42}` comes back as `{"n":42}`.
Why: TestState exists so a developer can diff the data at each filter stage against what they sent; a payload the emulator rewrote cannot be diffed or hashed, and every recorded comparison fails on a difference the caller did not introduce.
Fix: TestState's response serializer / inspection-data construction — carry the raw input bytes through the filter stages rather than round-tripping them through a JSON writer.
Traces: sfn-test-state-choice-and-error-details#task#1 (also #task#2).

=== Cause 2 of 8: a JSONata Choice state is not evaluated at all; the emulator substitutes its own error and cause ===
AWS: recorded behavior — a Choice state written in JSONata whose condition compares incompatible types fails the test with `error` = `States.QueryEvaluationError` and `cause` = `The JSONata expression '<expression>' specified for the field '<field path>' threw an error during evaluation. T2009: The values <lhs> and <rhs> either side of operator ">" must be of the same data type`. Fixed text: `The JSONata expression '`, `' specified for the field '`, `' threw an error during evaluation. `, then the JSONata engine's own `T<code>: ...` diagnostic; variable parts: the expression text and field path from the submitted definition and the two offending values from the input.
Emulator: `error` = `States.Runtime` and `cause` = `the state under test is written in 'JSONata', and this emulator evaluates only 'JSONPath' states, so nothing was run for it` — a wholly emulator-authored refusal admitting the JSONata query language is unimplemented.
Why: a caller cannot tell a genuine runtime fault (States.Runtime) from a query-evaluation fault (States.QueryEvaluationError, which is what a JSONata authoring mistake raises and what a retrier must NOT retry), and gets no diagnostic pointing at the expression.
Fix: the TestState evaluation path's JSONata support (the query-language selection in the interpreter's expression evaluator) and the error classification that maps an evaluation failure to States.QueryEvaluationError with AWS's cause template. Related open BEHAVIORAL issue 20261028T095020Z-sfn-step-functions-jsonata-definitions-are-never-evaluated-at.md describes JSONata not being evaluated at run time; the replay report marks this wire cluster as owned by nobody, so this fixture/step is not gated by it.
Traces: sfn-test-state-choice-and-error-details#task#3.

=== Cause 3 of 8: the Choice "condition path selects nothing" cause is reworded ===
AWS: recorded behavior — when a Choice state's condition references a path absent from the input, the test's `cause` is `Invalid path '<path>': The choice state's condition path references an invalid value.` — fixed text with the path from the definition as the only variable part.
Emulator: `the path '<path>' selected no value`, and the same response re-serializes afterInputPath (Cause 1).
Why: the sentence is how a caller distinguishes a missing-path Choice fault from any other States.Runtime cause; a reworded one breaks both human reading and assertions.
Fix: the Choice-state evaluation error message construction in the TestState/interpreter path.
Traces: sfn-test-state-choice-and-error-details#task#4.

=== Cause 4 of 8: a mocked Task that retries reports FAILED instead of RETRIABLE and omits afterArguments/errorDetails ===
AWS: recorded behavior — TestState on a Task state with a `mock` that raises an error matched by the state's Retry policy answers `status` = `RETRIABLE` (a modelled TestExecutionStatus value), `error` and `cause` from the mock (here Custom.Transient / "mocked transient failure" — the mock's own values, not part of the requirement), inspectionData.afterArguments holding the resolved task arguments, and inspectionData.errorDetails holding {retryIndex, retryBackoffIntervalSeconds} describing which retry attempt would come next and after how long.
Emulator: `status` = `FAILED`, error `States.Runtime`, the JSONata not-implemented cause from Cause 2, and neither afterArguments nor errorDetails.
Why: RETRIABLE is the whole signal a developer uses to confirm a Retry policy matches an error, and errorDetails.retryBackoffIntervalSeconds is the only way to verify the computed backoff; both are unavailable, and the status is a terminal FAILED that says the opposite.
Fix: TestState's mock handling plus the retry/catch bookkeeping it shares with the interpreter — classify a retry-matched mock error as RETRIABLE, populate afterArguments and errorDetails, and evaluate the state's query language (Cause 2).
Traces: sfn-test-state-choice-and-error-details#task#5.

=== Cause 5 of 8: a mocked Task whose error is caught reports FAILED instead of CAUGHT_ERROR and omits nextState/output/errorDetails ===
AWS: recorded behavior — when the mock's error is matched by the state's Catch, TestState answers `status` = `CAUGHT_ERROR`, `nextState` = the Catch's Next target, `output` = the error output object `{"Error":"<error>","Cause":"<cause>"}` the caught branch receives, inspectionData.errorDetails = {catchIndex, retryIndex} identifying which catcher matched, and inspectionData.afterArguments.
Emulator: `status` = `FAILED`, error `States.Runtime`, the JSONata not-implemented cause, and no nextState, output, errorDetails or afterArguments.
Why: without nextState and the error output a developer cannot verify that a Catch routes where intended or what payload the handler state will see — the primary use of TestState on an error path.
Fix: as Cause 4, plus the catch-selection result (nextState and the synthesized error output) in TestState's response construction.
Traces: sfn-test-state-choice-and-error-details#task#6.

=== Cause 6 of 8: the "mock not supported for this state type" refusal is reworded ===
AWS: recorded behavior — TestState refuses a `mock` supplied for an unsupported state type with HTTP 400 and the message `State type '<Type>' is not supported when a mock is specified`, where the only variable part is the Type of the state under test.
Emulator: HTTP 400 (correct) with `a mock may only be specified for a [Map, Parallel, Task] state, and the state under test is a '<Type>' state` — emulator-authored, and it additionally advertises a different supported set than AWS's refusal implies.
Why: a caller matching the documented sentence never matches, and the emulator's enumerated set contradicts Cause 7's AWS-supported-type list, so a developer cannot learn the real rule from either message.
Fix: TestState's mock validation message construction.
Traces: sfn-test-state-choice-and-error-details#task#7 (cluster also covers stepfunctions-test-state-in-isolation#task#2 and #task#4).

=== Cause 7 of 8: a Map state under test is refused with the wrong error shape and message ===
AWS: recorded behavior — TestState on a Map (or Parallel) state is refused with HTTP 400, `__type` = `com.amazonaws.swf.service.v2.model#InvalidDefinition` and the message `TestState API does not support Map or Parallel states. Supported state types include: [Task, Wait, Pass, Succeed, Fail, Choice]` — entirely fixed text, no variable part. Smithy confirms InvalidDefinition is a modelled TestState error.
Emulator: `__type` = `ValidationException` (a different modelled error shape) with `a 'Map' state is tested only when a mock is specified for its outcome`, which also states a rule AWS does not have.
Why: `except sfn.exceptions.InvalidDefinition` handlers never fire, and the emulator's message sends a developer off to supply a mock for a state type TestState will never support.
Fix: TestState's state-type validation — raise InvalidDefinition with AWS's fixed sentence for Map/Parallel states instead of ValidationException.
Traces: stepfunctions-test-state-in-isolation#task#3.

=== Cause 8 of 8: inspectionData.afterResultSelector is never populated ===
AWS: recorded behavior, consistent with the model — InspectionData.afterResultSelector carries the effective result after the state's ResultSelector is applied; AWS populates it for a state that declares one.
Emulator: the member is absent, and afterInputPath is re-serialized (Cause 1).
Why: a developer testing a ResultSelector cannot see its effect at all, so the stage is untestable.
Fix: TestState's inspection-data construction — emit afterResultSelector for a state declaring ResultSelector.
Traces: stepfunctions-test-state-in-isolation#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1; all clusters verdict=mismatch, service=states, operation TestState.

Cause 1 — sfn-test-state-choice-and-error-details#task#1 (200/200): `$.inspectionData.afterInputPath: value_mismatch (expected '{"n": 42}', got '{"n":42}')` (also #task#2).
Cause 2 — #task#3 (200/200): `$.error: value_mismatch (expected 'States.QueryEvaluationError', got 'States.Runtime')`; `$.cause: value_mismatch (expected 'The JSONata expression \'$states.input.n > 10\' specified for the field \'Choices[0]/Condition\' threw an error during evaluation. T2009: The values "abc" and 10 either side of operator ">" must be of the same data type', got "the state under test is written in 'JSONata', and this emulator evaluates only 'JSONPath' states, so nothing was run for it")`.
Cause 3 — #task#4 (200/200): `$.cause: value_mismatch (expected "Invalid path '$.n': The choice state's condition path references an invalid value.", got "the path '$.n' selected no value")`; `$.inspectionData.afterInputPath: expected '{"m": 1}', got '{"m":1}'`.
Cause 4 — #task#5 (200/200): `$.status: value_mismatch (expected 'RETRIABLE', got 'FAILED')`; `$.error: expected 'Custom.Transient', got 'States.Runtime'`; `$.cause: expected 'mocked transient failure', got <JSONata-not-implemented text>`; `$.inspectionData.afterArguments: missing (expected '{"FunctionName":"<fn>","Payload":{"a":1}}', got None)`; `$.inspectionData.errorDetails: missing (expected {'retryBackoffIntervalSeconds': 4, 'retryIndex': 0}, got None)`.
Cause 5 — #task#6 (200/200): `$.status: expected 'CAUGHT_ERROR', got 'FAILED'`; `$.nextState: missing (expected 'Handle', got None)`; `$.output: missing (expected '{"Error":"Custom.Transient","Cause":"mocked transient failure"}', got None)`; `$.inspectionData.errorDetails: missing (expected {'catchIndex': 0, 'retryIndex': 0}, got None)`; `$.inspectionData.afterArguments: missing`; error/cause as Cause 4.
Cause 6 — #task#7 (400/400): `$.message: value_mismatch (expected "State type 'Choice' is not supported when a mock is specified", got "a mock may only be specified for a [Map, Parallel, Task] state, and the state under test is a 'Choice' state")` (cluster also stepfunctions-test-state-in-isolation#task#2, #task#4).
Cause 7 — stepfunctions-test-state-in-isolation#task#3 (400/400): `$.__type: value_mismatch (expected 'com.amazonaws.swf.service.v2.model#InvalidDefinition', got 'ValidationException')`; `$.message: value_mismatch (expected 'TestState API does not support Map or Parallel states. Supported state types include: [Task, Wait, Pass, Succeed, Fail, Choice]', got "a 'Map' state is tested only when a mock is specified for its outcome")`.
Cause 8 — stepfunctions-test-state-in-isolation#task#1 (200/200): `$.inspectionData.afterResultSelector: missing (expected '{"result":"hello-world"}', got None)`; `$.inspectionData.afterInputPath: expected '{"value": "hello-world"}', got '{"value":"hello-world"}'`.

The state names, mock error names, function names and payload values quoted are the fixtures' own inputs, not requirements; the requirements are the status values, the error shapes/codes, the message templates and the presence of the inspectionData members.

## Steps to Reproduce
Cause 1/8: sfn:TestState(definition=<a Pass or Task state declaring InputPath and ResultSelector>, input='{"n": 42}', inspectionLevel=TRACE). Expect inspectionData.afterInputPath to echo '{"n": 42}' byte-for-byte and afterResultSelector to be present for a state declaring ResultSelector; observed compacted text and no afterResultSelector.
Cause 2: sfn:TestState(definition=<a Choice state with QueryLanguage JSONata and Choices[0].Condition "{% $states.input.n > 10 %}">, input='{"n": "abc"}'). Expect error States.QueryEvaluationError with the JSONata T2009 cause naming the expression and field; observed States.Runtime with the emulator's JSONata-not-implemented text.
Cause 3: sfn:TestState(definition=<a JSONPath Choice state whose Condition tests $.n>, input='{"m": 1}'). Expect cause "Invalid path '$.n': The choice state's condition path references an invalid value."; observed "the path '$.n' selected no value".
Cause 4: sfn:TestState(definition=<a Task state with Retry [{ErrorEquals:["Custom.Transient"], IntervalSeconds:4}]>, mock=<raise Custom.Transient / "mocked transient failure">, inspectionLevel=TRACE). Expect status RETRIABLE, the mock's error/cause, inspectionData.afterArguments and errorDetails{retryIndex, retryBackoffIntervalSeconds}; observed FAILED/States.Runtime with neither member.
Cause 5: same as Cause 4 but with Catch [{ErrorEquals:["Custom.Transient"], Next:"Handle"}]. Expect status CAUGHT_ERROR, nextState "Handle", output {"Error":...,"Cause":...} and errorDetails{catchIndex, retryIndex}; observed FAILED and none of them.
Cause 6: sfn:TestState(definition=<a Choice state>, mock=<anything>). Expect 400 "State type 'Choice' is not supported when a mock is specified".
Cause 7: sfn:TestState(definition=<a Map state>). Expect 400 with __type com.amazonaws.swf.service.v2.model#InvalidDefinition and "TestState API does not support Map or Parallel states. Supported state types include: [Task, Wait, Pass, Succeed, Fail, Choice]"; observed ValidationException with other wording.
Replay: the fixture#step tokens in Issue-Trace/Issue-Step and trace_ids.

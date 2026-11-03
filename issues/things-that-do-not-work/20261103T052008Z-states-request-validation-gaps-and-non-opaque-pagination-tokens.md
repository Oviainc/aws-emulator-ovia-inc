Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-failing-lambda-task-execution-diagnosis
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-failing-lambda-task-execution-diagnosis sfn-cfode-tenant-workflow-execution-lookup-mistakes sfn-poller-state-machine-reference-mistakes sfn-list-state-machines-paging sfn-poller-test-state-branch-walk
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|ListExecutions|4xx->2xx|$.__type
Issue-Key: states|ListExecutions|4xx->2xx|$.message
Issue-Key: states|ListExecutions|4xx->2xx|status
Issue-Key: states|ListExecutions|4xx->4xx|$.message
Issue-Key: states|GetExecutionHistory|2xx->2xx|$.events[*].executionSucceededEventDetails.output
Issue-Key: states|ListStateMachines|2xx->2xx|$.nextToken
Issue-Key: states|TestState|4xx->2xx|$.__type
Issue-Key: states|TestState|4xx->2xx|$.message
Issue-Key: states|TestState|4xx->2xx|status
Issue-Related: 20261103T052008Z-states-step-functions-execution-history-omits-task-lifecycle-events.md
Issue-Commit: e331167544f8c61f25de403c8943054d68a37733

# Request validation gaps and non-opaque pagination tokens across states reads

> _Note: this issue was identified against the code base at commit `e331167544f8c61f25de403c8943054d68a37733`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListExecutions

## Description
Read-side request validation and response serialization diverge across four operations: two parameter combinations AWS refuses are accepted, the enum value set quoted in a validation message is ordered differently, and the pagination token the list operations hand out is a bare decimal index rather than the opaque token AWS mints. 5 causes.

Cause 1 of 5 — ListExecutions accepts redriveFilter together with a stateMachineArn, which AWS refuses.
AWS: Smithy: ListExecutions.input has stateMachineArn, mapRunArn and redriveFilter (ExecutionRedriveFilter) and models ValidationException at HTTP 400. redriveFilter is only valid with mapRunArn; combined with a state machine ARN AWS answers HTTP 400 with __type com.amazon.coral.validate#ValidationException and the fixed message 'RedriveFilter cannot be applied to ListExecutions call with StateMachine ARN'.
Emulator: HTTP 200 with a normal execution list; no __type and no message.
Why: a client's invalid parameter combination silently succeeds against the emulator and is rejected by AWS, so the mistake is only found in production; and the filter it asked for is not applied.
Fix: ListExecutions' input validation — reject redriveFilter unless mapRunArn was supplied.
Traces: sfn-failing-lambda-task-execution-diagnosis#task#13

Cause 2 of 5 — the enum value set quoted in the statusFilter validation message is listed in a different order from AWS's.
AWS: recorded behavior only for the ordering. The message itself is AWS's standard coral constraint sentence: "1 validation error detected: Value '<the rejected value>' at '<memberName>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]" — fixed text everything outside the brackets, variable the rejected value and the member name from the request. AWS's recorded value list for ExecutionStatus in this message is [SUCCEEDED, TIMED_OUT, PENDING_REDRIVE, ABORTED, FAILED, RUNNING].
Emulator: same status (400) and same sentence, but the list is emitted in the Smithy declaration order [RUNNING, SUCCEEDED, FAILED, TIMED_OUT, ABORTED, PENDING_REDRIVE] (Smithy: ExecutionStatus values RUNNING, SUCCEEDED, FAILED, TIMED_OUT, ABORTED, PENDING_REDRIVE).
Why: a client or test that compares the refusal message verbatim against AWS's does not match, even though the rejection itself is correct.
Fix: the shared enum-constraint message builder used by ListExecutions' input validation — it must emit the value set in AWS's recorded order rather than the model's.
Traces: sfn-cfode-tenant-workflow-execution-lookup-mistakes#task#4

Cause 3 of 5 — GetExecutionHistory re-serializes the execution output with reordered object keys, and hands out a non-opaque nextToken.
AWS: recorded behavior only — AWS serializes the JSON object in executionSucceededEventDetails.output with its member names in ascending order, and GetExecutionHistory.nextToken is a PageToken (@length 1..1024) whose recorded value is an opaque token, not a positional index.
Emulator: the output object's members appear in the order the interpreter happened to insert them, and nextToken is the bare string '2'.
Why: a client that compares the output document byte-for-byte against the one DescribeExecution/the recording returned sees a different string for the same value; and a client that stores a nextToken and replays it against AWS sends a token AWS cannot decode, because the emulator's token is a positional index with no encoded cursor state.
Fix: the GetExecutionHistory serializer (the JSON object writer for execution data) and the pagination-token encoder it shares with the other list operations.
Traces: sfn-cfode-tenant-workflow-execution-lookup-mistakes#task#8

Cause 4 of 5 — ListStateMachines' nextToken is a bare decimal index.
AWS: recorded behavior only — the recorded nextToken is an opaque token of the shape the corpus asserts, not a positional index.
Emulator: nextToken is '1'.
Whey: same consequence as cause 3 — a token round-tripped from the emulator into AWS (or vice versa) is not decodable, and the token leaks the result's position.
Fix: the shared pagination-token encoder, as reached from ListStateMachines.
Traces: sfn-list-state-machines-paging#task#4

Cause 5 of 5 — TestState accepts a mock whose result does not match the integration's result schema.
AWS: Smithy: TestState models ValidationException at HTTP 400, with wire __type com.amazonaws.swf.service.v2.model#ValidationException. A mock result whose member has the wrong JSON type for the mocked integration is refused with the message "Mock result schema validation error: Field '<FieldName>' must be a <expected type>" — fixed text "Mock result schema validation error: Field '" + the offending field name from the submitted mock + "' must be a " + the type the integration's result schema requires.
Emulator: HTTP 200 — the mock is accepted and the state is tested against it; no __type and no message.
Why: a client validating a mock before using it in a test suite gets no signal that the mock is malformed, and the TestState result it reads back was computed from a value AWS would have rejected.
Fix: TestState's mock input validation, against the mocked integration's result schema. (The open issue 20261029T171911Z-states-step-functions-teststate-does-not-evaluate-jsonata-states.md covers TestState's JSONata evaluation and inspectionData members; it does not cover the mock result-schema refusal recorded here.)
Traces: sfn-poller-test-state-branch-walk#task#7

## Evidence
All five causes are mismatch clusters from the replay of simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 (sfn-failing-lambda-task-execution-diagnosis#task#13): status: expected 400, got 200; $.__type: expected 'com.amazon.coral.validate#ValidationException', got None; $.message: expected 'RedriveFilter cannot be applied to ListExecutions call with StateMachine ARN', got None.

Cause 2 (sfn-cfode-tenant-workflow-execution-lookup-mistakes#task#4; cluster also covers sfn-poller-state-machine-reference-mistakes#task#7; HTTP 400 expected and returned): $.message: expected "1 validation error detected: Value 'succeeded' at 'statusFilter' failed to satisfy constraint: Member must satisfy enum value set: [SUCCEEDED, TIMED_OUT, PENDING_REDRIVE, ABORTED, FAILED, RUNNING]", got "... enum value set: [RUNNING, SUCCEEDED, FAILED, TIMED_OUT, ABORTED, PENDING_REDRIVE]".

Cause 3 (sfn-cfode-tenant-workflow-execution-lookup-mistakes#task#8, HTTP 200): $.events[0].executionSucceededEventDetails.output: expected '{"status":...,"tenant_id":...,"url":...}' (members in ascending name order), got the same three members in insertion order '{"tenant_id":...,"url":...,"status":...}'; $.nextToken: expected a shaped opaque token, got '2'.

Cause 4 (sfn-list-state-machines-paging#task#4, HTTP 200): $.nextToken: expected a shaped opaque token, got '1'.

Cause 5 (sfn-poller-test-state-branch-walk#task#7): status: expected 400, got 200; $.__type: expected 'com.amazonaws.swf.service.v2.model#ValidationException', got None; $.message: expected "Mock result schema validation error: Field 'Payload' must be a string", got None.

## Steps to Reproduce
1. Replay fixture sfn-failing-lambda-task-execution-diagnosis against the emulator; step task#13 (the representative) calls ListExecutions with both a stateMachineArn and a redriveFilter. AWS answers HTTP 400 ValidationException; the emulator answers 200.
2. Replay sfn-cfode-tenant-workflow-execution-lookup-mistakes; step task#4 calls ListExecutions with statusFilter='succeeded' (wrong case) and compares the enum value set order in $.message; step task#8 calls GetExecutionHistory and compares the executionSucceededEventDetails.output member order and the $.nextToken shape.
3. Replay sfn-list-state-machines-paging; step task#4 pages ListStateMachines with a small maxResults and compares $.nextToken's shape.
4. Replay sfn-poller-test-state-branch-walk; step task#7 calls TestState with a mock whose Payload field has the wrong JSON type and expects HTTP 400 ValidationException.

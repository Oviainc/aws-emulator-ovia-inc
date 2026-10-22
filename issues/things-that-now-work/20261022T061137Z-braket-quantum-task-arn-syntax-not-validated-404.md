Issue-Source: trace-sweep-divergence
Issue-Trace: braket-quantum-task-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-quantum-task-error-paths
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket|GetQuantumTask|4xx->4xx|$.__type
Issue-Key: braket|GetQuantumTask|4xx->4xx|$.message
Issue-Key: braket|GetQuantumTask|4xx->4xx|status
Issue-Key: braket|CancelQuantumTask|4xx->4xx|$.__type
Issue-Key: braket|CancelQuantumTask|4xx->4xx|$.message
Issue-Key: braket|CancelQuantumTask|4xx->4xx|status
Issue-Commit: f95a296744f71657ae3614221270df79d0b4a393

# Braket quantum-task ARN syntax not validated: 404 ResourceNotFoundException instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `f95a296744f71657ae3614221270df79d0b4a393`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQuantumTask

## Description
Both operations that take `quantumTaskArn` as an HTTP label skip ARN syntax validation entirely and go straight to the task store, so a syntactically invalid ARN is reported as a missing resource (404) rather than as a malformed input (400). The shared fault is the absence of an ARN-shape check on the `quantumTaskArn` label before the lookup; there are 2 causes below, one per operation.

Cause 1 of 2: GetQuantumTask with a non-ARN `quantumTaskArn`.
- AWS: `GetQuantumTask` binds `quantumTaskArn` to the URI label `/quantum-task/{quantumTaskArn}` (Smithy: GetQuantumTask http GET /quantum-task/{quantumTaskArn}; member `quantumTaskArn` targets `QuantumTaskArn`, which carries `@arnReference{service: com.amazonaws.braket#Braket, resource: QuantumTaskResource}` and `@length(0..256)` but no `@pattern`). Both `ValidationException` (httpError 400) and `ResourceNotFoundException` (httpError 404) are modeled for the operation. The recording shows AWS classifying a value that is not an ARN at all as an input-validation failure: HTTP 400 with `__type` `ValidationException` (recorded message, as an example: 'quantumTaskArn is invalid.'). So the requirement is: a `quantumTaskArn` that is not a Braket quantum-task ARN is rejected as 400 / ValidationException, before any resource lookup.
- Emulator: answers HTTP 404 with `__type` `ResourceNotFoundException` (observed message, as an example: 'Quantum task not-a-valid-quantum-task-arn not found'). The label value is used as a map key with no shape check, so every malformed ARN is indistinguishable from an ARN for a task that does not exist.
- Why: a client cannot tell a programming error (a malformed or wrong-service ARN, which no retry or lookup will ever fix) from a legitimately absent/expired task. SDK and CLI error handling branches on the error code, and callers that treat ResourceNotFoundException as 'not created yet / poll again' will loop forever on a request that AWS would have failed immediately; retry and dead-letter logic keyed to ValidationException never fires.
- Fix: input validation for GetQuantumTask — validate the `quantumTaskArn` label as a Braket quantum-task ARN and return `ValidationException` (400) before consulting the task store; only a well-formed ARN that is absent should yield `ResourceNotFoundException` (404).
- Traces: braket-quantum-task-error-paths#task#2 (expected status 400, actual 404; `$.__type` expected 'ValidationException', got 'ResourceNotFoundException'). The unvalidated lookup is at services/service-com-amazonaws-braket-braket/src/ops/get_quantum_task.rs:236-246.

Cause 2 of 2: CancelQuantumTask with a non-ARN `quantumTaskArn`.
- AWS: `CancelQuantumTask` binds the same `QuantumTaskArn` shape to the label `/quantum-task/{quantumTaskArn}/cancel` (Smithy: CancelQuantumTask http PUT, errors include ValidationException 400 and ResourceNotFoundException 404). The recording shows HTTP 400 with `__type` `ValidationException` for a value that is not an ARN (recorded message, as an example: 'quantumTaskArn is not valid.'). Note AWS's prose differs between the two operations for the identical input, so the message text is an example only; the status and error code are the requirement.
- Emulator: answers HTTP 404 with `__type` `ResourceNotFoundException` (observed message example: 'Quantum task not-a-valid-quantum-task-arn not found'), i.e. the same unvalidated store lookup as Cause 1.
- Why: a cancel issued with a malformed ARN looks to the caller like a task that has already disappeared, so cleanup paths silently treat an un-cancelled task as gone instead of surfacing the bad input.
- Fix: input validation for CancelQuantumTask — same ARN-shape check on the label ahead of the store lookup; share it with GetQuantumTask so the two operations cannot drift.
- Traces: braket-quantum-task-error-paths#task#3 (expected status 400, actual 404; `$.__type` expected 'ValidationException', got 'ResourceNotFoundException').

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline:

1) fixture braket-quantum-task-error-paths, step task#2, GetQuantumTask, quantumTaskArn='not-a-valid-quantum-task-arn'
   expected: HTTP 400, $.__type = 'ValidationException', $.message = 'quantumTaskArn is invalid.' (message is an example)
   actual:   HTTP 404, $.__type = 'ResourceNotFoundException', $.message = 'Quantum task not-a-valid-quantum-task-arn not found'
   divergence_key: service=braket|GetQuantumTask|status=400->404|keys=$.__type,$.message,status

2) fixture braket-quantum-task-error-paths, step task#3, CancelQuantumTask, quantumTaskArn='not-a-valid-quantum-task-arn'
   expected: HTTP 400, $.__type = 'ValidationException', $.message = 'quantumTaskArn is not valid.' (message is an example)
   actual:   HTTP 404, $.__type = 'ResourceNotFoundException', $.message = 'Quantum task not-a-valid-quantum-task-arn not found'
   divergence_key: service=braket|CancelQuantumTask|status=400->404|keys=$.__type,$.message,status

Model evidence (services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json): QuantumTaskArn is an @arnReference string; GetQuantumTask and CancelQuantumTask both model ValidationException (400) and ResourceNotFoundException (404).
Code evidence: services/service-com-amazonaws-braket-braket/src/ops/get_quantum_task.rs:236-246 keys the store directly off the label with no ARN validation.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1: GET /quantum-task/not-a-valid-quantum-task-arn (fixture braket-quantum-task-error-paths, step task#1... i.e. step task#2 of that fixture). Observe HTTP 404 ResourceNotFoundException instead of HTTP 400 ValidationException.
3. Cause 2: PUT /quantum-task/not-a-valid-quantum-task-arn/cancel with a clientToken (fixture braket-quantum-task-error-paths, step task#3). Observe HTTP 404 ResourceNotFoundException instead of HTTP 400 ValidationException.
4. Re-replay braket-quantum-task-error-paths#task#2 and braket-quantum-task-error-paths#task#3 after the fix; both must report status 400 with $.__type 'ValidationException'.

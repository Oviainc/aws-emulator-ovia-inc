Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-error-conflict-and-source-notfound
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-error-conflict-and-source-notfound pipes-pipe-error-paths pipes-validation-and-conflict-errors pipes-update-pipe-config pipes-update-pipe-workflow pipes-pipe-lifecycle-immediate-conflicts pipes-sqs-to-sqs-lifecycle
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|CreatePipe|4xx->4xx|$.resourceType
Issue-Key: pipes|UpdatePipe|4xx->4xx|$.resourceType
Issue-Key: pipes|StartPipe|4xx->4xx|$.resourceType
Issue-Key: pipes|StopPipe|4xx->4xx|$.resourceType
Issue-Commit: 533efece7fa11c0e537273065624bf5d01aa80f8

# pipes ConflictException reports resourceType "pipe" instead of "Pipe" on every conflicting lifecycle call

> _Note: this issue was identified against the code base at commit `533efece7fa11c0e537273065624bf5d01aa80f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
This bundle has 4 causes. All four are the same defect — the ConflictException body the emulator builds carries a lower-cased resource-type token — but they are reached through four different operations, so each has its own handler path and its own replay binding. A fixer must close all 4 before submitting.

Shared fault: ConflictException (Smithy: com.amazonaws.pipes#ConflictException, httpError 409, required members message/resourceId/resourceType) models resourceType as "The type of resource that caused the exception." For EventBridge Pipes the only resource type is the pipe resource and AWS words that token with a leading capital: "Pipe". The emulator emits "pipe". The 409 status and the ConflictException error code itself are correct in all four cases; only the resourceType member's value diverges. In the message/resourceId/resourceType triple, "Pipe" is FIXED text for this service (it is the resource type name, not a request value), while resourceId and the pipe name inside message are values taken from the request.

Cause 1 of 4: CreatePipe on a name that already exists.
  AWS: 409 ConflictException with resourceType "Pipe" (Smithy: CreatePipe errors include ConflictException; ConflictException.resourceType required). Recorded response: resourceType "Pipe".
  Emulator: 409 ConflictException with resourceType "pipe".
  Why: a client that branches on resourceType (or asserts it when distinguishing which resource conflicted) sees a token that never appears on the real API; case-sensitive comparisons and generated-model enum/round-trip checks fail.
  Fix: the error-construction path for CreatePipe's already-exists branch (services/service-com-amazonaws-pipes-pipes/src/ops/create_pipe.rs) — or, better, the one shared ConflictException constructor these four handlers use, so the token is spelled once.
  Traces: pipes-error-conflict-and-source-notfound#task#9 (also pipes-pipe-error-paths#task#11, pipes-validation-and-conflict-errors#task#10).

Cause 2 of 4: UpdatePipe against a pipe that is in a transitional/conflicting state.
  AWS: 409 ConflictException, resourceType "Pipe" (Smithy: UpdatePipe errors include ConflictException).
  Emulator: 409 ConflictException, resourceType "pipe".
  Why: same as cause 1 — the conflict is reported with a type token AWS never returns.
  Fix: UpdatePipe's conflict branch (src/ops/update_pipe.rs) / the shared ConflictException constructor.
  Traces: pipes-error-conflict-and-source-notfound#task#10 (also pipes-update-pipe-config#task#8, pipes-update-pipe-workflow#task#8).

Cause 3 of 4: StartPipe when the pipe cannot be started from its current state.
  AWS: 409 ConflictException, resourceType "Pipe" (Smithy: StartPipe errors include ConflictException, httpError 409).
  Emulator: 409 ConflictException, resourceType "pipe".
  Why: same as cause 1.
  Fix: StartPipe's conflict branch (src/ops/start_pipe.rs) / the shared ConflictException constructor.
  Traces: pipes-pipe-lifecycle-immediate-conflicts#task#8 (also pipes-sqs-to-sqs-lifecycle#task#9).

Cause 4 of 4: StopPipe when the pipe cannot be stopped from its current state.
  AWS: 409 ConflictException, resourceType "Pipe" (Smithy: StopPipe errors include ConflictException, httpError 409).
  Emulator: 409 ConflictException, resourceType "pipe".
  Why: same as cause 1.
  Fix: StopPipe's conflict branch (src/ops/stop_pipe.rs) / the shared ConflictException constructor.
  Traces: pipes-pipe-lifecycle-immediate-conflicts#task#9 (also pipes-sqs-to-sqs-lifecycle#task#8).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, 4 mismatch clusters (10 affected steps total), each with expected_status 409 == actual_status 409 and detail "$.resourceType: value_mismatch (expected 'Pipe', got 'pipe')":
- CreatePipe, divergence_key service=pipes|CreatePipe|status=409->409|keys=$.resourceType, representative pipes-error-conflict-and-source-notfound#task#9, affected 3.
- UpdatePipe, divergence_key service=pipes|UpdatePipe|status=409->409|keys=$.resourceType, representative pipes-error-conflict-and-source-notfound#task#10, affected 3.
- StartPipe, divergence_key service=pipes|StartPipe|status=409->409|keys=$.resourceType, representative pipes-pipe-lifecycle-immediate-conflicts#task#8, affected 2.
- StopPipe, divergence_key service=pipes|StopPipe|status=409->409|keys=$.resourceType, representative pipes-pipe-lifecycle-immediate-conflicts#task#9, affected 2.
Expected (AWS, recorded): ConflictException.resourceType == "Pipe". Actual (emulator): "pipe". Status and error code match; only the required resourceType member's spelling diverges.

## Steps to Reproduce
1. Replay fixture pipes-error-conflict-and-source-notfound, step task#9: CreatePipe with the name of a pipe that already exists. Response is 409 ConflictException; read $.resourceType -> "pipe", AWS returns "Pipe".
2. Same fixture, step task#10: UpdatePipe on a pipe in a conflicting state -> 409, $.resourceType "pipe".
3. Replay fixture pipes-pipe-lifecycle-immediate-conflicts, step task#8 (StartPipe from a non-startable state) and step task#9 (StopPipe from a non-stoppable state) -> 409 each, $.resourceType "pipe".
4. All four must return resourceType "Pipe" with the 409 status and ConflictException code unchanged.

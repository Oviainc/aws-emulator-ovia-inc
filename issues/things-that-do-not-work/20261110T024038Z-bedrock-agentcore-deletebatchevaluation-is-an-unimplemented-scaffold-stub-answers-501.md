Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.delete-batch-evaluation-while-in-progress-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore.delete-batch-evaluation-while-in-progress-rejected|run#11|AssertionError|assert code == "ConflictException", (
Issue-Commit: 505d23cb73aeec2b6bc1a547ffda8ada5cc353f2

# DeleteBatchEvaluation is an unimplemented scaffold stub: answers 501 NotImplemented instead of ConflictException / 202 / ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `505d23cb73aeec2b6bc1a547ffda8ada5cc353f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBatchEvaluation

## Description
Cause 1 of 1: `DeleteBatchEvaluation` on the bedrock-agentcore data plane is still a scaffold stub, so the whole delete leg of the batch-evaluation state machine is unserved. (The suite did NOT mark this check as ALREADY FILED/OWNED — the check reaches the stub through a refusal-comparing helper, so the 501 surfaces as an error-code mismatch rather than as a blocked call, and no open issue names this operation. The related open issue 20261109T202040Z-bedrock-agentcore-getbatchevaluation-not-found-refusal-is-reworded-instead-of.md covers GetBatchEvaluation's wording only, and the StartBatchEvaluation stub issue is already marked fixed — StartBatchEvaluation and GetBatchEvaluation both answered normally in this replay.)

AWS: Smithy model com.amazonaws.bedrockagentcore#DeleteBatchEvaluation — `DELETE /evaluations/batch-evaluate/{batchEvaluationId}`, success code 202, idempotent; input `batchEvaluationId` (BatchEvaluationId, @required, @httpLabel, pattern ^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$); output members `batchEvaluationId`, `batchEvaluationArn`, `status` (BatchEvaluationStatus) all @required; modeled errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, UnauthorizedException 401, ValidationException 400. The model documents ConflictException as "the request conflicts with the current state of the resource" and ResourceNotFoundException as "the specified resource does not exist ... or when trying to access a resource that has been deleted". Recorded behavior (from the check's recorded expectations) pins three outcomes for this operation:
  (a) deleting a batch evaluation that is in a NON-terminal status (PENDING / IN_PROGRESS) is refused with HTTP 409 `ConflictException` whose message is the fixed sentence "Cannot delete batch evaluation in status <STATUS>. Only COMPLETED, COMPLETED_WITH_ERRORS, FAILED, or STOPPED evaluations can be deleted." — the fixed text is everything except `<STATUS>`, which is the job's current status read from emulator state at the moment of the delete (one of PENDING/IN_PROGRESS here); the refused delete must leave the job in place and still readable via GetBatchEvaluation;
  (b) once the job is terminal (STOPPED / COMPLETED / COMPLETED_WITH_ERRORS / FAILED) the same delete is ACCEPTED with 202 and the three required output members;
  (c) a second delete of the now-deleted id, and a GetBatchEvaluation of it, are refused with HTTP 404 `ResourceNotFoundException` whose message is the fixed sentence "Failed to retrieve the resource because it doesn't exist. Retry the request with a different resource identifier." (fixed text, no variable part) — never accepted a second time.

Emulator: every DeleteBatchEvaluation call, in every state, answers HTTP 501 with error code `NotImplemented` and the message "operation DeleteBatchEvaluation is scaffolded but not implemented in this emulator". Replay: `AssertionError: expected ConflictException deleting an active batch evaluation, got 'NotImplemented'` on a job the emulator itself had just reported as PENDING/IN_PROGRESS via StartBatchEvaluation + GetBatchEvaluation. Because the first delete never returns a modeled refusal, the terminal-state accept (b) and the post-delete 404s (c) are never exercised either.

Why: a client cannot delete a batch evaluation at all, and cannot distinguish "wrong state, stop it first" (409 ConflictException, retryable after Stop) from "gone" (404 ResourceNotFoundException) from "not supported". Code that drives the documented lifecycle — start → stop → delete → confirm 404 — fails on an unretryable 501, and cleanup paths that treat ConflictException as "retry after terminal" and ResourceNotFoundException as "already gone" loop or abort instead. Resources leak because nothing can be reclaimed.

Fix: implement the operation's handler and its state transition: look the id up in the service's batch-evaluation state, refuse a non-terminal job with the modeled ConflictException (409) carrying the status-naming sentence above, accept a terminal job by removing it and serializing the three required output members with 202, and refuse an unknown or already-deleted id with the modeled ResourceNotFoundException (404) carrying the fixed not-found sentence; then drop the scaffold 501 path and the SCAFFOLDED marker.

Traces: bedrock-agentcore.delete-batch-evaluation-while-in-progress-rejected. Handler: services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/delete_batch_evaluation.rs lines 169-187 (`handle` returns `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteBatchEvaluation"))`), SCAFFOLDED marker at line 4.

## Evidence
Check bedrock-agentcore.delete-batch-evaluation-while-in-progress-rejected [mismatch]:

  File "check_bedrock_agentcore_delete_batch_evaluation_while_in_progress_rejected.py", line 63, in run
    assert code == "ConflictException", (
  AssertionError: expected ConflictException deleting an active batch evaluation, got 'NotImplemented'

Sequence actually replayed: logs.CreateLogGroup -> bedrock-agentcore.StartBatchEvaluation (accepted, status PENDING/IN_PROGRESS) -> GetBatchEvaluation (accepted, non-terminal) -> DeleteBatchEvaluation.

Expected (AWS): HTTP 409, Error.Code = "ConflictException", Error.Message = "Cannot delete batch evaluation in status <STATUS>. Only COMPLETED, COMPLETED_WITH_ERRORS, FAILED, or STOPPED evaluations can be deleted." where <STATUS> is the job's current status (PENDING or IN_PROGRESS in this run) and the rest is fixed text; the job remains retrievable.
Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation DeleteBatchEvaluation is scaffolded but not implemented in this emulator".

Not reached because of the above, and equally unserved by the 501 stub:
- after Stop reaches a terminal status, DeleteBatchEvaluation must be accepted with HTTP 202 returning required batchEvaluationId / batchEvaluationArn / status;
- a second DeleteBatchEvaluation of the deleted id must answer HTTP 404 ResourceNotFoundException with the fixed message "Failed to retrieve the resource because it doesn't exist. Retry the request with a different resource identifier."

Source of the stub: services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/delete_batch_evaluation.rs:186 `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteBatchEvaluation"))`.

## Steps to Reproduce
1. Boot the emulator and point a botocore bedrock-agentcore client at it.
2. logs.create_log_group(logGroupName="/sim2real/bedrock-agentcore-behav-<rand>").
3. client.start_batch_evaluation(batchEvaluationName="sim2realbatchneg0", dataSourceConfig={"cloudWatchLogs": {"serviceNames": ["sim2real-test-service"], "logGroupNames": [<that log group>]}}, evaluators=[{"evaluatorId": "Builtin.Helpfulness"}]) and keep batchEvaluationId.
4. client.get_batch_evaluation(batchEvaluationId=<id>) and confirm status is PENDING or IN_PROGRESS (non-terminal).
5. client.delete_batch_evaluation(batchEvaluationId=<id>) -> observe HTTP 501 / NotImplemented instead of HTTP 409 ConflictException with the status-naming message.
6. client.stop_batch_evaluation(batchEvaluationId=<id>), poll get_batch_evaluation until terminal, then client.delete_batch_evaluation(batchEvaluationId=<id>) -> still 501 instead of 202; and a repeat delete / get -> still 501 instead of 404 ResourceNotFoundException.

Or replay the suite check directly: bedrock-agentcore.delete-batch-evaluation-while-in-progress-rejected.

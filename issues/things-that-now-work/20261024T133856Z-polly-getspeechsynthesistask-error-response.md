Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7bcb53c96ea0ea6c32faf644dec1ae2940515247

# GetSpeechSynthesisTask never answers InvalidTaskIdException: every unknown TaskId becomes SynthesisTaskNotFoundException

_Filed: 2026-10-24T13:38:56Z_
_Operation: GetSpeechSynthesisTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `7bcb53c96ea0ea6c32faf644dec1ae2940515247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model declares two distinct 400 errors for this operation: `InvalidTaskIdException` ("The provided Task ID is not valid. Please provide a valid Task ID and try again.") for a TaskId that is not a task identifier at all, and `SynthesisTaskNotFoundException` ("The Speech Synthesis task with requested Task ID cannot be found.") for a well-formed identifier that names no task. Amazon Polly mints TaskIds as UUIDs, so a value such as `notataskid` — which still satisfies the member's `^[a-zA-Z0-9_-]{1,100}$` pattern and therefore passes the boundary validator — is refused as an *invalid* id, while a well-formed but unknown UUID is refused as *not found*.

Expected: a TaskId that is syntactically acceptable to the model but is not a task identifier Polly could have minted answers HTTP 400 `InvalidTaskIdException`; only an unknown well-formed id answers `SynthesisTaskNotFoundException`.

Actual: the handler does a single store lookup and maps every miss to `SynthesisTaskNotFoundException`, so `InvalidTaskIdException` is unreachable — the crate has no constructor for it at all. A client that branches on the error code (retryable/typo-vs-expired handling) always sees the "not found" branch.

## Evidence

Spec: smithy_query(operation="GetSpeechSynthesisTask") lists both `InvalidTaskIdException` (HTTP 400) and `SynthesisTaskNotFoundException` (HTTP 400) with the documentation quoted above; two separately modeled errors for one operation cannot both mean "no such task". Handler: services/service-com-amazonaws-polly-parrot-v1/src/ops/get_speech_synthesis_task.rs lines 158-166 — `guard.synthesis_tasks.get_mut(&input.task_id).ok_or_else(|| ... SynthesisTaskNotFoundException ...)`, with a comment explicitly choosing that error for every miss. Proof the other code is dead: `grep -r invalid_task_id` over the crate returns nothing, and src/service.rs's `errors` module (lines 268-366) provides `synthesis_task_not_found()` but no `invalid_task_id()`. Test coverage: tests/all/error_semantics.rs and tests/all/synthesis_tasks.rs exercise the not-found path (tasks are seeded through tests/all/support.rs `stored_task`), but no test asserts an `InvalidTaskIdException` for a non-identifier TaskId, so the gap is uncovered. Distinct from the already-filed GetSpeechSynthesisTask wrong-behavior issue, which is about 72-hour task retention.

## Steps to Reproduce

1. Boot the emulator and call GET /v1/synthesisTasks/notataskid (the value matches the modeled TaskId pattern, so the boundary validator admits it)
2. Observe HTTP 400 with code `SynthesisTaskNotFoundException`
3. Real Polly answers HTTP 400 `InvalidTaskIdException` for an id that is not a task identifier, reserving `SynthesisTaskNotFoundException` for a well-formed (UUID-shaped) id that names no task
4. Confirm the modeled code is unreachable: no request shape makes the emulator emit `InvalidTaskIdException`

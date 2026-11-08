Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.multiplex-start-without-programs-rejected
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.multiplex-start-without-programs-rejected|run#27|AssertionError|assert start_message == f"Multiplex {multiplex_id} cannot be started while in DELETING state", start_message
Issue-Commit: d0ce8acb2d50b38f212ecb6be47b8b7720e8c1b4

# StartMultiplex on a DELETING multiplex returns ConflictException with the wrong message text ("is in state DELETING and can only be started from IDLE" instead of "cannot be started while in DELETING state")

> _Note: this issue was identified against the code base at commit `d0ce8acb2d50b38f212ecb6be47b8b7720e8c1b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMultiplex

## Description
One net-new root cause.

Cause 1 of 1: StartMultiplex against a multiplex whose delete has been accepted (state DELETING) is correctly refused with HTTP 409 ConflictException, but the emulator words the message with its own "can only be started from IDLE" template instead of AWS's wording for this refusal class. Failing check id: medialive.multiplex-start-without-programs-rejected.

AWS:      Smithy (medialive): StartMultiplex POST /prod/multiplexes/{MultiplexId}/start, success code 202, modeled errors include ConflictException (httpError 409) and NotFoundException (404). The model does not state message text, so the message is recorded behavior only: real AWS answers 409 ConflictException with the message template `Multiplex <MultiplexId> cannot be started while in <State> state`. The FIXED text is "Multiplex " ... " cannot be started while in " ... " state"; the VARIABLE parts are the multiplex id taken from the request URI label and the multiplex's current state taken from the emulator's state store (DELETING in the recorded run). The id and state value from any single run only fill that template and are not part of it. The recording also confirms the CODE for this situation is ConflictException (409), not NotFoundException — the record is still resolvable and the refusal is a state conflict.

Emulator: for the same call the emulator answers 409 ConflictException with `Multiplex <id> is in state DELETING and can only be started from IDLE` (observed: "Multiplex 6000000 is in state DELETING and can only be started from IDLE"), i.e. the generic not-startable template used for every non-IDLE state, which both reads as a retry instruction and does not match AWS's wording. Every other assertion of the check — StartMultiplex on an empty IDLE multiplex succeeding to RUNNING with ProgramCount 0, the ConflictException "Cannot delete a multiplex that is in RUNNING state" for DeleteMultiplex while RUNNING, the stop/delete path and disappearance from ListMultiplexes — passed, so the divergence is isolated to this message.

Why:     clients and SDK-based tooling that branch on, log, or surface the error message for a start of a multiplex being torn down see text AWS never emits. Anything matching the documented phrase ("cannot be started while in ... state") does not fire, and operators reading the emulator's text are told the multiplex can be retried once IDLE, which a DELETING/DELETED id can never become — a different operational conclusion than AWS gives.

Related: open issue 20261028T160823Z-medialive-startmultiplex-error-response.md covers the same call but argues the correct answer is a 404 NotFoundException tombstone. The replayed AWS behavior contradicts that premise: AWS keeps the 409 ConflictException classification here and only the message wording diverges. That issue does not cover this finding, and a fix that converts this path to 404 would newly break the recorded classification.

Fix:      the StartMultiplex handler's refusal-to-error mapping and the shared not-startable message builder it calls (the message formatter in the medialive state module, used by src/ops/start_multiplex.rs's MultiplexStartRefusal::NotStartable arm and by the BatchStart multiplex arm). The refusal message for a multiplex being torn down must be rendered with AWS's template, with the id and current state filled from the request and the stored record; the 409 ConflictException classification must be kept.

Traces:   medialive.multiplex-start-without-programs-rejected (assertion at check line 68: expected `Multiplex <id> cannot be started while in DELETING state`, got `Multiplex 6000000 is in state DELETING and can only be started from IDLE`).

## Evidence
Failing check: medialive.multiplex-start-without-programs-rejected [mismatch]

  File "check_medialive_multiplex_start_without_programs_rejected.py", line 68, in run
    assert start_message == f"Multiplex {multiplex_id} cannot be started while in DELETING state", start_message
  AssertionError: Multiplex 6000000 is in state DELETING and can only be started from IDLE

Expected (real AWS, recorded): HTTP 409, Error.Code = "ConflictException", Error.Message = "Multiplex <MultiplexId> cannot be started while in <State> state" (fixed text as quoted; <MultiplexId> from the request, <State> the record's current state).
Actual (emulator): HTTP 409, Error.Code = "ConflictException" (correct), Error.Message = "Multiplex <MultiplexId> is in state <State> and can only be started from IDLE".

The error code assertion in the same check (start_code == "ConflictException") PASSED, so the status and code are right and only the message text diverges.

Smithy (services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json): StartMultiplex -> POST /prod/multiplexes/{MultiplexId}/start, success 202; errors ConflictException(409), NotFoundException(404), BadRequestException(400), ForbiddenException(403), TooManyRequestsException(429), plus server-side 500/502/504. No message text is modeled.

## Steps to Reproduce
1. CreateMultiplex with a name, two availability zones and MultiplexSettings {TransportStreamBitrate: 10000000, TransportStreamId: 1}; note the returned multiplex id M.
2. Poll DescribeMultiplex(MultiplexId=M) until State == IDLE.
3. StartMultiplex(MultiplexId=M); poll until State == RUNNING (succeeds with zero programs).
4. StopMultiplex(MultiplexId=M); poll until State == IDLE.
5. DeleteMultiplex(MultiplexId=M) -> accepted; the id disappears from ListMultiplexes while DescribeMultiplex still reports State DELETING.
6. StartMultiplex(MultiplexId=M) while the record reads DELETING.
   Observed: 409 ConflictException, message "Multiplex M is in state DELETING and can only be started from IDLE".
   Expected: 409 ConflictException, message "Multiplex M cannot be started while in DELETING state".

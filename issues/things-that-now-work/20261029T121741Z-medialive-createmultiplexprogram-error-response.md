Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# A RequestId already spent on a program of this multiplex answers 201 after that multiplex's delete was accepted, the 409 the same handler answers a fresh token with

_Filed: 2026-10-29T12:17:41Z_
_Operation: CreateMultiplexProgram | Class: error-response_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateMultiplexProgram`'s idempotency replay branch resolves the `RequestId` and answers with the stored program BEFORE the multiplex's liveness is looked at. The liveness rule lives one call below, inside `insert_multiplex_program`, which refuses a multiplex whose delete has been accepted (`DELETING` or the terminal `DELETED`) with `MultiplexProgramRefusal::MultiplexGone` -> this operation's modelled 409 `ConflictException` ("Multiplex <id> is <state>; no program can be created in it").

The token reader `multiplex_program_for_token` re-checks only that the MULTIPLEX STILL HOLDS THE PROGRAM, and `MultiplexRecord::begin_delete` neither removes the multiplex's `programs` map nor forgets the program tokens, so the program of a deleted multiplex stays resolvable for ever. The consequence is that one and the same request is answered two opposite ways depending only on whether its `RequestId` was spent before:

* `POST /prod/multiplexes/M/programs` with a FRESH `requestId` on a `DELETING`/`DELETED` multiplex -> 409 `ConflictException` (pinned by tests/all/multiplex_deletion.rs, whose `mutations()` table lists `CreateMultiplexProgram` among "the mutations of a multiplex whose documentation makes the multiplex's state a PRECONDITION");
* the same call with the `requestId` that already created a program of M -> 201 with that program, i.e. a `RequestId` turns a request this operation refuses into one it serves, and the caller is handed a program of a multiplex that is gone.

This is the exact defect the crate already fixed on the partner-input replay path, and the rule the input slice states for its own token reader. It is NOT the cause of the earlier (fixed) `CreateMultiplexProgram` replay issue, which was about a token spent on ANOTHER multiplex's program: that fix added the multiplex to the token key and re-checked it on the record, and left the multiplex's own liveness unchecked.

## Evidence

The replay branch, which decides before any liveness check — services/service-com-amazonaws-medialive-medialive/src/ops/create_multiplex_program.rs:174-191:
```
        // A retried create (same `@idempotencyToken` `RequestId`) must answer
        // with the program the first one created, not a second program. The
        // token is scoped to the multiplex whose endpoint it was presented to:
        ...
        if let Some(program_name) = state
            .multiplex_program_for_token(&input.multiplex_id, &input.request_id)
            .map(str::to_string)
        {
            let program = state
                .multiplex_program_description(&input.multiplex_id, &program_name)
                .ok_or_else(unavailable)?;
            return Ok(CreateMultiplexProgramOutput {
                multiplex_program: Some(program),
            });
        }
```

The token reader, whose ONLY re-check is that the program exists — src/state/multiplexes.rs:1711-1732:
```
    /// The name of the program a stored `RequestId` created in `multiplex_id`,
    /// if that multiplex still holds it.
    ...
    pub fn multiplex_program_for_token(
        &self,
        multiplex_id: &str,
        request_id: &str,
    ) -> Option<&str> {
        let program_name = self
            .multiplex_program_tokens
            .created(&multiplex_program_token_key(multiplex_id, request_id))?;
        let program = self
            .multiplexes
            .get(multiplex_id)?
            .programs
            .get(program_name)?;
        Some(program.program_name.as_str())
    }
```

The liveness rule the non-replay path applies six lines below — src/state/multiplexes.rs:1500-1508:
```
        let multiplex = self
            .multiplexes
            .get_mut(multiplex_id)
            .ok_or(MultiplexProgramRefusal::MultiplexNotFound)?;
        if !multiplex.accepts_programs() {
            return Err(MultiplexProgramRefusal::MultiplexGone(
                multiplex.state.clone(),
            ));
        }
```
with `accepts_programs` covering BOTH halves of the delete — src/state/multiplexes.rs:834-850:
```
    /// Whether a delete has already been accepted for this multiplex: it is
    /// `DELETING`, or it has reached the terminal `DELETED`. ...
    fn delete_accepted(&self) -> bool {
        self.state == multiplex_state::DELETING || self.state == multiplex_state::DELETED
    }

    /// Whether a program can be created in this multiplex right now: in every
    /// state but the two that take the multiplex away, since a multiplex that
    /// is `DELETING` or has reached the terminal `DELETED` holds nothing new.
    fn accepts_programs(&self) -> bool {
        !self.delete_accepted()
    }
```
and the handler turning it into the modelled 409 — src/ops/create_multiplex_program.rs:252-258:
```
        Refusal::MultiplexGone(state) => {
            CreateMultiplexProgramErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "Multiplex {multiplex_id} is {state}; no program can be created in it"
                )),
```

The service-wide rule the replay breaks — src/state.rs:82-84:
```
//! A kept record is READABLE, not writable: once a delete has been accepted for
//! an id, that id names no resource a caller may still change. Every MUTATION
//! therefore resolves its id through the live lookups
```

The SIBLING rule, stated and pinned for the other create of this crate whose token mints a child of a parent the request names — tests/all/input_idempotency_after_delete.rs:334-341:
```
/// Rule: a partner is created FOR the input the request names, so that input must
/// be one the account still holds and may still change — a `RequestId` cannot
/// turn a request this operation refuses into one it serves, so a retry aimed at
/// an input whose delete has been accepted is refused exactly as a first attempt
/// is, with this operation's modelled `BadRequestException` ...
```
whose rows include "a token already spent on a partner of this input, inside the DELETING window" and "... once the delete settled" (tests/all/input_idempotency_after_delete.rs:350-371), and whose state-side implementation filters liveness — src/state.rs:3516-3528 (`input_for_token`): "LIVENESS is part of the lookup, through the same [`Self::live_input`] a mutation of an input resolves its id by ... so the retried create mints a new input rather than answering with one the caller could neither use nor change."

Test coverage: only the FRESH-token refusal is pinned — tests/all/multiplex_deletion.rs:103-116 sends `{"programName":"after-the-delete","requestId":"token-after-the-delete", ...}` (a token never spent) against the deleted multiplex, under the comment at :86-92 ("The mutations of a multiplex whose documentation makes the multiplex's state a PRECONDITION ... and the create of a program inside the multiplex"). No test replays a token that DID create a program of that multiplex after its delete; tests/all/multiplex_programs.rs:95-104 only builds bodies carrying a `request_id`.

## Steps to Reproduce

1. POST /prod/multiplexes with a name, two availabilityZones and valid multiplexSettings; note the id M, and poll GET /prod/multiplexes/M until state is IDLE.
2. POST /prod/multiplexes/M/programs with {"programName":"p1","requestId":"R","multiplexProgramSettings":{"programNumber":9, ...}} -> 201 with the program.
3. DELETE /prod/multiplexes/M -> 202 with state DELETING (optionally wait for the clock so GET /prod/multiplexes/M reports the terminal DELETED; the record and its programs are kept either way).
4. POST /prod/multiplexes/M/programs with a FRESH requestId and a new programName -> 409 ConflictException "Multiplex M is DELETED; no program can be created in it" (src/ops/create_multiplex_program.rs:252-258).
5. POST /prod/multiplexes/M/programs again with {"programName":"p1","requestId":"R", ...} -> the emulator answers 201 with program p1 of the deleted multiplex. Expected, per the same handler's own refusal and src/state.rs:82-84, the modelled 409 ConflictException naming the multiplex's state, exactly as the fresh-token call is answered.
6. Repeat step 5 inside the DELETING window for the same 201.

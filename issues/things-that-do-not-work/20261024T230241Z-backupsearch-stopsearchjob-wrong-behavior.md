Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# StopSearchJob accepts a job in CREATING, though its own modeled doc says only RUNNING can be stopped

_Filed: 2026-10-24T23:02:41Z_
_Operation: StopSearchJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's `StopSearchJob` documentation states the gate for this operation in one sentence: only a search job whose status is `RUNNING` can be stopped. The handler does not implement that gate. Its status check is purely "not already stop-satisfied and not terminal": a job in the crate's own `CREATING` status (the FIRST status any caller can observe after `StartSearchJob`, held for `CREATING_DELAY` = 250 ms) is neither in `STOP_SATISFIED_STATES` (`STOPPING`, `STOPPED`) nor in `TERMINAL_STATES` (`COMPLETED`, `STOPPED`, `FAILED`), so it falls straight through to the write and the handler moves it to `STOPPING` and re-owes it `STOPPED`, answering the empty 200.

The contradiction is inside the handler file itself: its own reproduced modeled contract (line 54-55) carries the "Only a search job with a status of RUNNING can be stopped" sentence, and its hand-written summary (line 4) says "Ends a running search job" — while the code under both accepts a job that is not running. Because `CREATING` is the status a `StopSearchJob` sent back-to-back with `StartSearchJob` sees, this is the common case for an "ensure stopped" immediately after a create: the emulator reports a successful stop of a job that was never running, and the job's scan is cancelled (`owe_only` discards the `RUNNING` and terminal statuses the scan was owed), so the job settles to `STOPPED` and `ListSearchJobResults` can never report the matches the scan would have recorded.

Scope note: this is NOT a request to change the `STOPPING`/`STOPPED` short-circuit — those answer the empty 200 because the operation is `@idempotent` and they already are, or are on their way to, the end state the call asks for. Only the `CREATING` fall-through contradicts the quoted sentence.

## Evidence

Smithy, the operation's own documentation (smithy_query(query="doc", shape="StopSearchJob")): "<p>This operations ends a search job.</p> <p>Only a search job with a status of <code>RUNNING</code> can be stopped.</p>" — reproduced verbatim in the handler at services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/stop_search_job.rs:54-55: "//! This operations ends a search job. Only a search job with a status of RUNNING can be\n//! stopped." and summarised by the handler's own doc at stop_search_job.rs:4: "//! Ends a running search job."

Handler code that does the opposite — src/ops/stop_search_job.rs:183-209:
```
        if job.is_stop_satisfied() {
            return Ok(StopSearchJobOutput {});
        }
        // A job that ended in a status this operation does not produce cannot be stopped
        // at all: ...
        if job.is_terminal() {
            return Err(StopSearchJobErrorModeled::ConflictException(...));
        }
        job.status = crate::state::STOPPING.to_string();
        job.owe_only(now + crate::state::TRANSITION_DELAY, crate::state::STOPPED, None);
        Ok(StopSearchJobOutput {})
```
The two sets those predicates read, in src/state.rs:113 and :148: `pub const TERMINAL_STATES: &[&str] = &[COMPLETED, STOPPED, FAILED];` and `pub const STOP_SATISFIED_STATES: &[&str] = &[STOPPING, STOPPED];` — neither contains `CREATING`, which src/state.rs:99-101 declares transient: "It is a TRANSIENT status (it is not in [`TERMINAL_STATES`]) and the clock owes the job [`RUNNING`] after [`CREATING_DELAY`]" / `pub const CREATING: &str = "CREATING";`.

Test coverage: tests/all/search_job_outcome_and_paging.rs:328 pins the current behaviour with the row `(state::CREATING, 200, state::STOPPING),`. Its stated reason (lines 324-326) is only "A status that already is the end state this `@idempotent` operation asks for, or is on its way to it, is a no-op success; only a terminal status this operation does not produce is a conflict" — a reason that does not cover `CREATING` (it is neither the end state nor on its way to it), and which the quoted model sentence contradicts. This row, and the existing-status assertion beside it, are what a fix moves.

No existing issue covers it: the fixed wire issue 20261022T215802Z-...-search-job-wire-replay (Cause 4) only required STOPPED/STOPPING to short-circuit to 200 — filed before `CREATING` existed in the crate — and the open 20261023T110504Z-...-export-stop-refusals issue (Cause 3) covers only the ConflictException MESSAGE template for a FAILED job.

## Steps to Reproduce

1. Boot the emulator and point a botocore backupsearch client at it.
2. start_search_job(Name="x", SearchScope={"BackupResourceTypes": ["S3"]}) and keep the returned SearchJobIdentifier.
3. Immediately (inside crate::state::CREATING_DELAY = 250 ms, before the clock owes RUNNING) call get_search_job(SearchJobIdentifier=<id>) and confirm Status == "CREATING".
4. Immediately call stop_search_job(SearchJobIdentifier=<id>): the emulator answers an empty 200 and the job moves to STOPPING, although the operation's documentation says only a job with a status of RUNNING can be stopped.
5. get_search_job again after ~1 s: Status == "STOPPED" and the scan the job was owed never ran (the RUNNING and terminal statuses were dropped by `owe_only`), so ListSearchJobResults stays empty for a job whose scope resolved to recovery points.
6. Positive control that must keep working: repeat but wait past CREATING_DELAY so the job is RUNNING before the stop — that stop must still answer 200; and a second stop against the already-STOPPED job must still answer 200 (the @idempotent repeat).

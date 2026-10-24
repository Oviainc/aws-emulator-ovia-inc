Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5c8d6c13f19e0574ca93569a583e30c59c58b477

# CancelContact accepts a contact in a terminal state and overwrites its status with CANCELLED

_Filed: 2026-10-24T00:13:24Z_
_Operation: CancelContact | Class: validation_

> _Note: this issue was identified against the code base at commit `5c8d6c13f19e0574ca93569a583e30c59c58b477`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CancelContact unconditionally overwrites whatever status the contact holds with `CANCELLED` and answers 200, for every contact that exists.

Expected: only a contact that still holds a future/live reservation is cancellable. A contact already in a terminal state — `CANCELLED`, `AWS_CANCELLED`, `COMPLETED`, `FAILED`, `FAILED_TO_SCHEDULE` — is not, and AWS refuses the call with the operation's modeled `InvalidParameterException` (HTTP 431), leaving the contact's status untouched.

Actual: the handler looks the contact up and writes `contact_status = "CANCELLED"`, clears `scheduling_settles_at` and returns `contactId`/`versionId` with 200. Two observable consequences: (1) a client that branches on the refusal (e.g. "already cancelled / already finished") never sees it; (2) the overwrite corrupts derived state — a contact that had already consumed antenna time (`COMPLETED`, or `PASS`/`POSTPASS`) is reclassified as CANCELLED, and `state::STATUSES_WITHOUT_ANTENNA_TIME` then excludes it, so GetMinuteUsage for that month drops minutes the account has in fact already used. Antenna time that was spent cannot be released by a cancel.

A secondary fidelity note on the same path: AWS reports `CANCELLING` while a cancellation of an in-progress contact is being carried out before settling on `CANCELLED` (the `ContactStatus` enum carries `CANCELLING` for exactly that), whereas this handler jumps straight to `CANCELLED`.

## Evidence

Spec: `smithy_query(query="operation", operation="CancelContact")` declares `InvalidParameterException` (client, HTTP 431) alongside `ResourceNotFoundException`; `smithy_query(query="shape", shape="ContactStatus")` enumerates the terminal states (`CANCELLED`, `AWS_CANCELLED`, `COMPLETED`, `FAILED`, `FAILED_TO_SCHEDULE`) and the in-progress `CANCELLING`. Well-established service semantics: cancelling a contact releases a reservation, so a contact whose pass is over or whose reservation is already gone has nothing to cancel.
Handler: `services/service-com-amazonaws-groundstation-groundstation/src/ops/cancel_contact.rs` lines ~146-168 — the only failure path is the not-found lookup; the status is then set to `CANCELLED` with no state check, justified in a comment by `@idempotent` (an HTTP retry-safety trait, which does not say AWS accepts a cancel of a finished contact). `src/state.rs` `STATUSES_WITHOUT_ANTENNA_TIME` (lines ~740-752) and `Contact::holds_antenna_time()` are what make the overwrite change GetMinuteUsage's answer.
Tests: `tests/all/scheduling_loop.rs` (~line 1125-1135) asserts cancel→CANCELLED for a SCHEDULED contact only; `tests/all/minute_usage.rs::cancelling_a_contact_releases_its_minutes` covers the legitimate case. No test exercises a cancel against a terminal-state contact, so the behavior is unasserted.

## Steps to Reproduce

1. Create a tracking config and a mission profile, then ReserveContact a window.
2. DELETE /contact/{contactId} — observe 200 and contactStatus CANCELLED (correct).
3. DELETE /contact/{contactId} again — observe a second 200 where AWS refuses the cancel of an already-CANCELLED contact with InvalidParameterException (HTTP 431).
4. Put a contact into a terminal, antenna-time-holding state (COMPLETED) in the store, call GetMinuteUsage for its month and note the minutes, then DELETE /contact/{contactId}: the call succeeds, the status becomes CANCELLED and GetMinuteUsage now reports fewer totalScheduledMinutes for a pass that already ran.

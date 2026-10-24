Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: be3ab9eb2c0ab303301f9b8637a58c2763937db6

# Every started automation event settles to Failed with an emulator-specific statusReason instead of completing

_Filed: 2026-10-24T04:27:47Z_
_Operation: StartAutomationEvent | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `be3ab9eb2c0ab303301f9b8637a58c2763937db6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartAutomationEvent` mints an event with `eventStatus=InProgress` and a `settles_at` instant 500 ms out; when that instant passes, `StoredEvent::settle` rewrites the event to the TERMINAL `Failed` status with `eventStatusReason = "The automation was not performed: this emulator does not change the resources a recommended action applies to."` and stamps `completedTimestamp`. So on the emulator every automation event ever started ends as `Failed`, and no event can ever reach `Complete` (or `RollbackReady`/`RollbackComplete`).

Expected (real service): starting an automation for a valid recommended action runs the action and the event progresses to `Complete` with a `completedTimestamp` and NO failure reason; `Failed` is reserved for an automation that actually could not be carried out, and its `eventStatusReason` is service text, never a sentence about an emulator. The model's `EventStatus` set (`Cancelled | Complete | Failed | InProgress | Ready | RollbackComplete | RollbackFailed | RollbackInProgress | RollbackReady`) and `RollbackAutomationEvent`'s documentation — "Initiates a rollback for a completed automation event" — both require a happy path that terminates in `Complete`; with the current behaviour that operation (once implemented) can never have a valid subject, and a client that polls `GetAutomationEvent`/`ListAutomationEvents` until success never succeeds.

Actual: HTTP 200 from StartAutomationEvent, then `GetAutomationEvent` reports `eventStatus=Failed` plus the emulator-authored reason string, which is also what `ListAutomationEvents` and any `EventStatus` filter see (`settle_due` applies the same transition for every reader).

## Evidence

Spec: `RollbackAutomationEvent` doc — "Initiates a rollback for a completed automation event" (smithy_query doc, shape=RollbackAutomationEvent); `EventStatus` declares `Complete` and the `Rollback*` statuses (smithy_query shape=EventStatus, listed in OPERATIONS.md for GetAutomationEvent/StartAutomationEvent); `eventStatusReason` is the service's explanation of the status, not emulator commentary.

Handler/state: src/ops/start_automation_event.rs:263-270 builds the event via `crate::state::event_for_action`, which sets `event_status = EVENT_STATUS_IN_PROGRESS` and `settles_at = started_at + EVENT_SETTLEMENT_DELAY` (src/state.rs:900-928). `StoredEvent::settle` (src/state.rs:803-812) unconditionally sets `event_status = EVENT_STATUS_FAILED` and `event_status_reason = EVENT_UNPERFORMED_REASON` — the constant at src/state.rs:103-112 whose text names the emulator. `GetAutomationEvent` (src/ops/get_automation_event.rs:211) and `ListAutomationEvents` (src/ops/list_automation_events.rs:254) both materialise that transition before reporting.

Test coverage: tests/all/automation_event_and_tagging.rs:182 (`a_started_event_is_built_from_the_action_and_settles_on_the_clock`) asserts the current Failed-with-reason outcome, i.e. the divergence is pinned by a test rather than uncovered — the terminal status and the reason text both need to change.

## Steps to Reproduce

1. Enroll the caller: `UpdateEnrollmentConfiguration` with `status=Active`.
2. Seed/obtain a recommended action and note its `recommendedActionId` (the inventory is only reachable in-process today — see the separate ListRecommendedActions issue).
3. Call `StartAutomationEvent` with that `recommendedActionId` — note the returned `eventId` and `eventStatus=InProgress`.
4. Wait ~1 second and call `GetAutomationEvent` with that `eventId`: expected `eventStatus=Complete` with a `completedTimestamp` and no `eventStatusReason`; actual `eventStatus=Failed` with `eventStatusReason` = "The automation was not performed: this emulator does not change the resources a recommended action applies to."
5. Call `ListAutomationEvents` with `filters=[{name:"EventStatus",values:["Complete"]}]` — expected the event; actual an empty list (it is only ever matched by `Failed`).
6. Observe that `RollbackAutomationEvent`, documented as rolling back a *completed* event, can therefore never be exercised against any event this emulator produces.

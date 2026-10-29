Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 35520d9c06247da719cc1093e5db81eb1d87c7db

# RollbackAutomationEvent judges eligibility and stamps the rollback from a clock read taken BEFORE the store lock, the one write path in the crate that does not read the clock under the lock

_Filed: 2026-10-28T18:17:13Z_
_Operation: RollbackAutomationEvent | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `35520d9c06247da719cc1093e5db81eb1d87c7db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RollbackAutomationEvent` captures `now` at `src/ops/rollback_automation_event.rs:188`, two lines before it takes the store lock, and then uses that pre-lock instant for everything it does under the lock: `event.settle(now)` (the clock transition eligibility is judged on), and `event.begin_rollback(now)` — which WRITES the rollback step's `start_timestamp` and the new `PendingTransition.due`.

Every other write path in this crate reads the clock after taking the lock: `CreateAutomationRule`, `UpdateAutomationRule`, `UpdateEnrollmentConfiguration`, `AssociateAccounts` and `DisassociateAccounts` all call `crate::state::now()` inside the guard, and `StartAutomationEvent` — the sibling that writes the very same two fields (`StoredStep::start_timestamp`, `PendingTransition::due`) — reads `std::time::SystemTime::now()` inside the guard at `src/ops/start_automation_event.rs:281`. The crate states the invariant on the clock accessor itself: "The ONE clock the handlers read, so a stored timestamp is always the real store time" (`src/state.rs:491-494`). On this path the stored instants are not the store time but whatever the clock said before the handler got the lock.

Two consequences follow from the same line, both while the lock is contended:

1. The handler's own stated rule is broken. The comment above `event.settle(now)` says eligibility is judged on "the status the event ALREADY has: an automation whose run window has passed is completed whether or not anybody read it since". With a pre-lock `now`, an automation whose `pending.due` falls between the clock read and the lock acquisition is NOT settled, `rollback_can_start()` is false, and the caller is refused 400 InvalidParameterValueException "A rollback can only be initiated for an automation event in the Complete status" — for an event that `GetAutomationEvent`, which reads the clock inside the lock (`src/ops/get_automation_event.rs:211`), reports as `Complete` on the very next call. A success is turned into an error by lock-wait time alone.

2. The rollback is stored stale. `begin_rollback(now)` writes `pending.due = now + EVENT_SETTLEMENT_DELAY` and the rollback step's `start_timestamp = now`, so a wait longer than the settlement delay stores a rollback whose terminal transition is already due while the handler answers `RollbackInProgress` (it does not settle again after `begin_rollback`), and stamps a step as having started before the store recorded it.

## Evidence

The pre-lock clock read, and the lock two lines later — `src/ops/rollback_automation_event.rs:184-190`:
```
        let mut probe = input.clone();
        probe.client_token = None;
        let request_fingerprint = crate::state::fingerprint(&probe);
        let client_token = input.client_token.as_deref();
        let now = ::std::time::SystemTime::now();

        let mut guard = state.lock();
```

The rule that instant is then used to decide, and the state it is written into — `src/ops/rollback_automation_event.rs:273-287`:
```
        // The clock's own transition first, so eligibility is judged on the
        // status the event ALREADY has: an automation whose run window has
        // passed is completed whether or not anybody read it since.
        event.settle(now);
        if !event.rollback_can_start() {
            return Err(
                RollbackAutomationEventErrorModeled::InvalidParameterValueException(
                    InvalidParameterValueException {
                        message: Some(crate::state::rollback_not_allowed_message()),
                    },
                )
                .into(),
            );
        }
        event.begin_rollback(now);
```
what `begin_rollback` stores from it — `src/state.rs:1961-1991`:
```
    pub fn begin_rollback(&mut self, at: SystemTime) {
        let first_step = self.steps.len();
        let started_at = ::aws_smithy_types::DateTime::from(at);
...
                start_timestamp: (offset == 0).then_some(started_at),
...
        self.pending = Some(PendingTransition {
            due: at + EVENT_SETTLEMENT_DELAY,
```

The invariant the crate states for a stored timestamp — `src/state.rs:491-494`:
```
/// Now, as the wire timestamp type. The ONE clock the handlers read, so a stored
/// timestamp is always the real store time.
pub fn now() -> ::aws_smithy_types::DateTime {
```

The sibling that writes the SAME two fields, reading the clock under the lock (`let mut guard = state.lock();` is `src/ops/start_automation_event.rs:194`) — `src/ops/start_automation_event.rs:276-283`:
```
        let event_id = guard.mint_event_id();
        let event = crate::state::event_for_action(
            event_id.clone(),
            &action,
            account_id,
            std::time::SystemTime::now(),
        );
        guard.events.insert(event);
```
and the other four writers, each reading the clock after its own `state.lock()`: `src/ops/create_automation_rule.rs:332` (`let created_at = crate::state::now();`, lock at `:244`), `src/ops/update_automation_rule.rs:380` (`let updated_at = crate::state::now();`, lock at `:276`), `src/ops/update_enrollment_configuration.rs:251` (`let stored_at = crate::state::now();`, lock at `:211`), `src/ops/associate_accounts.rs:247` (`let associated_at = crate::state::now();`, lock at `:212`), `src/ops/disassociate_accounts.rs:232` (`let disassociated_at = crate::state::now();`, lock at `:211`).

The sibling reader that settles the same event from an instant read INSIDE the lock, and so disagrees with the refusal above — `src/ops/get_automation_event.rs:190` + `:208-211`:
```
        let mut guard = state.lock();
...
        // The only write a read makes: the status transition the clock already
        // owed this event, which was due at its stored instant whether or not
        // anybody called. Nothing here is advanced BECAUSE the event was read.
        event.settle(::std::time::SystemTime::now());
```

Test coverage: the rollback suite drives one request at a time and sleeps past `EVENT_SETTLEMENT_DELAY` before rolling back (`tests/all/automation_event_rollback.rs` uses `GetAutomationEvent` at `:128` to wait for `Complete`), so no test holds the lock across the clock read and nothing pins the pre-lock instant. Moving the read under the guard changes no response of any existing test.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"}.
2. Call ListRecommendedActions with {} and take one recommendedActionId, then StartAutomationEvent with it; the event is stored InProgress with pending.due = start + EVENT_SETTLEMENT_DELAY (500 ms).
3. Issue RollbackAutomationEvent for that eventId concurrently with other traffic against the same service state (any other operation holds the same lock), so the rollback handler's `let now = SystemTime::now()` at src/ops/rollback_automation_event.rs:188 is taken while the event is still InProgress and the lock is granted after pending.due has passed.
4. Observe 400 InvalidParameterValueException "A rollback can only be initiated for an automation event in the Complete status": event.settle(now) at :276 is a no-op because the stale `now` is still below pending.due.
5. Call GetAutomationEvent for the same eventId — 200 with eventStatus "Complete", because src/ops/get_automation_event.rs:211 reads the clock inside the lock. The two handlers disagree about the status of one event at one instant, and the rollback the service refused is accepted on the next attempt.
6. For the write side, repeat with the lock contended for longer than 500 ms: the rollback is stored with pending.due already in the past (src/state.rs:1987-1990) while the response reports RollbackInProgress, and the rollback step's startTimestamp (src/state.rs:1977) precedes the instant the store recorded it — compare StartAutomationEvent, which takes the same two instants from a clock read under the lock (src/ops/start_automation_event.rs:281).

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba10b2e9c791e1406ed6cd1db28496a7b9239da3

# A new StartPrimaryEmailUpdate silently discards an ACCEPTED update whose completion the clock still owes, losing the accepted address change

_Filed: 2026-10-24T23:52:58Z_
_Operation: StartPrimaryEmailUpdate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba10b2e9c791e1406ed6cd1db28496a7b9239da3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate models an accepted primary email update as non-terminal: `AcceptPrimaryEmailUpdate` answers 200 with `ACCEPTED` and stores `completes_at = now + PRIMARY_EMAIL_UPDATE_COMPLETION` (500 ms), after which the clock is documented to owe the update its `COMPLETED` status and to make the requested address the account's `primary_email`.

`StartPrimaryEmailUpdate` calls `account.settle_primary_email_update(now)` before overwriting the record, but `PrimaryEmailUpdate::settle` returns `None` while `now < due`. So a start that lands inside the 500 ms window replaces the in-flight `ACCEPTED` record with a fresh `PENDING` one, and the owed completion never happens: `account.primary_email` keeps its old value forever, `GetPrimaryEmail` never reports the address the service already accepted, and `primary_email_in_use` reports that address as free again so another account can take it. Whether the accepted change survives depends purely on whether the next call arrives before or after the emulator's own 500 ms instant — the same call at 499 ms loses state, at 501 ms keeps it.

The crate protects its other clock-owed transition from exactly this: `EnableRegion`/`DisableRegion` refuse a change while the previous transition is still in flight (`ConflictException`) rather than overwriting it. `StartPrimaryEmailUpdate` models `ConflictException` too, and the handler's own comment states that the accepted address change must not be lost — it just only covers the already-due case.

## Evidence

The owed completion, services/.../src/state.rs:274-281:
```
/// The `PrimaryEmailUpdateStatus` an update carries from the moment
/// `AcceptPrimaryEmailUpdate` confirms it: `ACCEPTED` ...
/// `ACCEPTED` is NOT terminal — the request has been accepted and the address
/// change still has to be applied — so the clock owes the update its
/// [`COMPLETED_PRIMARY_EMAIL_UPDATE_STATUS`] after
/// [`PRIMARY_EMAIL_UPDATE_COMPLETION`].
```
The settle that declines while not yet due, src/state.rs:635-639:
```
pub fn settle(&mut self, now: SystemTime) -> Option<String> {
    let due = self.completes_at?;
    if now < due {
        return None;
    }
```
The handler's stated intent and the overwrite, src/ops/start_primary_email_update.rs:201-211:
```
// Materialise the completion the clock already owes this account before
// its update record is replaced: an accepted update whose completion
// came due has already made its address the account's primary email,
// and starting another update must not lose that.
account.settle_primary_email_update(now);
...
let update = crate::state::PrimaryEmailUpdate::started(input.primary_email, now);
let status = update.status.clone();
account.primary_email_update = Some(update);
```
Sibling that refuses instead of overwriting an in-flight transition, src/ops/enable_region.rs:244-251:
```
// "this happens if you try to enable a Region that is currently
// being disabled (in a status of DISABLING)" — ConflictException.
status @ RegionOptState::Disabling { .. } => {
    return Err(EnableRegionErrorModeled::ConflictException(
        crate::state::region_opt_in_progress(&region_name, status.as_wire()),
```
Tests: tests/all/primary_email_update_acceptance.rs:314 `accepting_a_pending_update_moves_it_to_accepted_and_then_to_completed` covers accept-then-wait, and tests/all/primary_email_update.rs:341 `a_restarted_update_is_the_one_reported` covers restart while PENDING; no test starts an update while one is ACCEPTED and not yet due, so the state loss is uncovered.

## Steps to Reproduce

1. POST /startPrimaryEmailUpdate with {"AccountId":"123456789012","PrimaryEmail":"first@example.com"} → 200, Status PENDING.
2. POST /acceptPrimaryEmailUpdate with {"AccountId":"123456789012","PrimaryEmail":"first@example.com","Otp":"a1B2c3"} → 200, Status ACCEPTED (completion due 500 ms later).
3. Within those 500 ms, POST /startPrimaryEmailUpdate with {"AccountId":"123456789012","PrimaryEmail":"second@example.com"} → 200, Status PENDING.
4. Sleep past the 500 ms instant, then POST /getPrimaryEmail with {"AccountId":"123456789012"}: it still reports the seeded root+123456789012@example.com — the accepted change to first@example.com was dropped and no COMPLETED status is ever reported for it.
5. Compare EnableRegion/DisableRegion, which answer 409 ConflictException rather than discarding a transition the clock still owes.

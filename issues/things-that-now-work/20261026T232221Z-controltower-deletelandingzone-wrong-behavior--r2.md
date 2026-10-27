Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# DeleteLandingZone ignores its @idempotent trait and refuses a repeated delete with ConflictException

_Filed: 2026-10-26T23:22:21Z_
_Operation: DeleteLandingZone | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteLandingZone` carries `@idempotent` in the model, and this crate has already decided what that trait means for a teardown: in `DisableBaseline` (the other `@idempotent` operation of this service) a repeat of the call while the teardown it started is still in flight REPORTS THE SAME `operationIdentifier`, and only a DIFFERENT operation in flight is refused with `ConflictException`. `DeleteLandingZone`'s handler applies no such rule: it refuses ANY landing zone with an operation in flight, so a client that retries the delete (the exact thing `@idempotent` exists to make safe) gets a 409 instead of the identifier its first call already produced, and after the 500 ms settle span it gets a 404 instead. The crate states the complementary half of the rule explicitly in `UpdateEnabledBaseline` ("not `@idempotent`, so a repeat is refused rather than reported as the same operation"), which is precisely the branch `DeleteLandingZone` takes while carrying the opposite trait. The two sibling handlers therefore implement two different readings of one modelled trait.

## Evidence

Model (OPERATIONS.md line 196, and `smithy_query(query="operation", operation="DeleteLandingZone")` → `"idempotent": true`):
```
## DeleteLandingZone
- **Route:** `POST /delete-landingzone -> 200`
- **Traits:** @idempotent
```

Sibling handler that honours the trait — services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/disable_baseline.rs:193-205:
```
// `@idempotent`: a disable already in flight against this resource IS this
// request's work, so repeating the call reports that same operation rather than
// starting a second teardown or refusing.
if let Some(in_flight) = store.baseline_operation_in_flight(&arn) {
    ...
    if operation_type == Some(crate::state::BaselineOperationType::DisableBaseline) {
        return Ok(DisableBaselineOutput { operation_identifier: in_flight });
    }
```

The handler under review — services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/delete_landing_zone.rs:202-214:
```
// Deleting a landing zone whose own asynchronous operation has not finished would
// leave it in an inconsistent state, which is what `ConflictException` reports.
if store.has_operation_in_flight(&arn) {
    return Err(DeleteLandingZoneErrorModeled::ConflictException(
```
There is no branch that recognises an in-flight `DELETE` against the same ARN as this request's own work, even though `crate::state::deletion_in_flight()` (src/state.rs:1406) already returns exactly that identifier and `CreateLandingZone` calls it.

The crate's own statement of the rule it is breaking — src/ops/update_enabled_baseline.rs:199-202:
```
// An operation still changing this enablement owns it: ... This operation is not `@idempotent`, so a repeat
// is refused rather than reported as the same operation.
```

Test coverage: tests/all/disable_baseline.rs:12-15 pins the idempotent reading ("the operation is `@idempotent`, so repeating the call while that teardown is in flight reports the SAME identifier rather than starting a second one, and it is refused with `ConflictException` (409) while any OTHER operation is still changing the enablement"), while tests/all/landing_zone_decommission.rs:296-300 and :340-347 pin the opposite for the `@idempotent` `DeleteLandingZone` ("a second delete" → 409 ConflictException). The divergence is encoded in the suite, not covered by it.

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.3","manifest":<a valid manifest>} and wait past the settle span so the landing zone is ACTIVE.
2. POST /delete-landingzone with {"landingZoneIdentifier":"<the ARN>"} — 200 with an operationIdentifier.
3. Immediately POST /delete-landingzone again with the identical body — the emulator answers 409 ConflictException instead of returning the same operationIdentifier, which is what the sibling `@idempotent` DisableBaseline does for a repeated teardown (tests/all/disable_baseline.rs:120).
4. Wait past the settle span and POST /delete-landingzone a third time with the same body — 404 ResourceNotFoundException. A retry of an @idempotent call therefore never succeeds.

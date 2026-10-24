Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6395e76ed9f748dc97d54e4052a4e63524456c87

# Apply DeleteLandingZone removal when its async DELETE operation completes, not instantly

_Filed: 2026-10-24T04:17:33Z_
_Operation: DeleteLandingZone | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6395e76ed9f748dc97d54e4052a4e63524456c87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteLandingZone is documented as starting an ASYNCHRONOUS decommission: it returns an `operationIdentifier` the caller polls with GetLandingZoneOperation, and the handler itself records that operation as `IN_PROGRESS` with a `due_at` 500ms in the future. But the handler removes the landing zone (and its tags) from the store SYNCHRONOUSLY, before the operation it just logged is due.

Expected (real AWS): while the DELETE operation is IN_PROGRESS the landing zone is still a resource — GetLandingZone answers 200 with `landingZone.status = PROCESSING`, ListLandingZones still returns its ARN, and a second DeleteLandingZone / a CreateLandingZone against the account is refused with ConflictException ("Updating or deleting the resource can cause an inconsistent state"). Only once the operation reaches SUCCEEDED does GetLandingZone start answering 404 ResourceNotFoundException and ListLandingZones return an empty list.

Actual (emulator): the instant DeleteLandingZone returns 200, GetLandingZone answers 404 ResourceNotFoundException and ListLandingZones returns `[]`, even though GetLandingZoneOperation reports the DELETE as IN_PROGRESS for the next 500ms. A repeated DeleteLandingZone during that window also answers 404 instead of 409 ConflictException (the landing zone is already out of the map, so `resolve_landing_zone_arn` fails before the in-flight check at src/ops/delete_landing_zone.rs:187 can fire). This is inconsistent with the emulator's own CreateLandingZone, which correctly keeps the landing zone visible in PROCESSING until its CREATE operation is due.

## Evidence

Smithy model: DeleteLandingZone (POST /delete-landingzone, @idempotent) returns `operationIdentifier` (OperationIdentifier) and binds ConflictException(409) and ResourceNotFoundException(404); the model doc for the operation and for LandingZoneOperationDetail$status (IN_PROGRESS/SUCCEEDED/FAILED) establish the delete as an asynchronous operation whose progress is polled. LandingZoneDetail$status models PROCESSING, i.e. a landing zone that exists while an operation runs against it.

Handler: services/.../src/ops/delete_landing_zone.rs lines 199-206 — `store.landing_zones.remove(&arn); store.tags.remove(&arn);` happens before `store.record_operation(LandingZoneOperationType::Delete, ...)`, which inserts a record with `status: InProgress` and `due_at: now + OPERATION_SETTLE_AFTER` (src/state.rs:283-305). Nothing re-applies the removal at settle time (src/state.rs:209-235 only flips operation status and derives landing zone status).

Tests: services/.../tests/all/landing_zone_lifecycle.rs lines 384-403 assert the CURRENT behavior (404 and empty list immediately after delete), i.e. the divergence is encoded in the suite; no test asserts the landing zone remains visible as PROCESSING during the in-flight DELETE, nor that a concurrent delete yields 409.

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.3","manifest":{...}} and wait past the settle span so the landing zone is ACTIVE.
2. POST /delete-landingzone with the returned ARN — note the 200 and the returned operationIdentifier.
3. Immediately POST /get-landingzone-operation with that operationIdentifier: status is IN_PROGRESS.
4. In the same window POST /get-landingzone with the ARN: emulator answers 404 ResourceNotFoundException; AWS still reports the landing zone with status PROCESSING.
5. In the same window POST /list-landingzones: emulator returns an empty landingZones list while the delete is still in flight.
6. In the same window repeat POST /delete-landingzone: emulator answers 404 ResourceNotFoundException instead of 409 ConflictException.

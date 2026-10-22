Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the ARC Zonal Shift emulator

_Filed: 2026-10-21T21:18:17Z_
_Operation: service-com-amazonaws-arczonalshift-percdataplane | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. Implement a FIRST, SMALL, stateful slice: the zonal-shift lifecycle for a managed resource, over the `restJson1` protocol (sigv4 name `arc-zonal-shift`).

Happy path to implement, in order:

1. **ListManagedResources** — `GET /managedresources`. Output `items` (list<ManagedResourceSummary>, required) + optional `nextToken`. Must return the managed resources held in state (seed a small fixed set of ALB/ASG-style resources at state init, each with `arn`, `name`, `availabilityZones`, `appliedWeights`, `zonalShifts`, `autoshifts`, `zonalAutoshiftStatus`, `practiceRunStatus`). Honour `maxResults` (1..100) and `nextToken`.
2. **StartZonalShift** — `POST /zonalshifts` -> **201**. Input `resourceIdentifier` (required), `awayFrom` (required), `expiresIn` (required, `^([1-9][0-9]*)(m|h)$`), `comment` (required). Must create and PERSIST a ZonalShift: generate a `zonalShiftId` matching `^[A-Za-z0-9-]+$` (len 6..36), set `startTime` = now, `expiryTime` = now + parsed `expiresIn`, `status` = `ACTIVE`, and return all seven required output members from the stored record. Unknown `resourceIdentifier` -> `ResourceNotFoundException` (404); a second ACTIVE customer-initiated shift for the same resource -> `ConflictException` (409).
3. **GetManagedResource** — `GET /managedresources/{resourceIdentifier}` -> 200. Output `appliedWeights` (required), `zonalShifts` (required, list<ZonalShiftInResource>), plus `arn`, `name`, `autoshifts`, `practiceRunConfiguration`, `zonalAutoshiftStatus`. Must reflect the shift just started: the new shift appears in `zonalShifts` with `appliedStatus` = `APPLIED`, and `appliedWeights` shows `0` for the `awayFrom` Availability Zone and `1` for the others. Unknown identifier -> `ResourceNotFoundException` (404).
4. **ListZonalShifts** — `GET /zonalshifts`. Output `items` (list<ZonalShiftSummary>) + `nextToken`. Must list shifts from state and FILTER by the `status` and `resourceIdentifier` query params, and page with `maxResults`/`nextToken`.
5. **UpdateZonalShift** — `PATCH /zonalshifts/{zonalShiftId}` -> 200. Input `comment`, `expiresIn`. Must mutate the stored shift (new `comment` overwrites the old one; `expiresIn` recomputes `expiryTime` from now) and return the updated record; a later ListZonalShifts/GetManagedResource must show the new values. Unknown id -> 404; updating a CANCELED/EXPIRED shift -> `ConflictException` (409).
6. **CancelZonalShift** — `DELETE /zonalshifts/{zonalShiftId}` -> 200. Must set the stored shift's `status` to `CANCELED`, remove it from the resource's active shifts and restore `appliedWeights` to all-`1`, and return the shift record. Cancelling twice (or an expired shift) -> `ConflictException` (409); unknown id -> 404.

No canned or echoed responses: every one of these six must compute its response from the persisted store, so a create is observable by every later read, and a cancel is observable as `CANCELED`. `status` must also honour the clock — a shift whose `expiryTime` has passed reads back as `EXPIRED` and is no longer applied to the resource.

## State Model

In `src/state.rs` (`ComAmazonawsArczonalshiftPercdataplaneState`, reached via `crate::state::service_state(&*self.engine)`):

- `managed_resources: BTreeMap<String /*resourceIdentifier (ARN)*/, ManagedResource>` where `ManagedResource { arn: String, name: String, availability_zones: Vec<String>, applied_weights: BTreeMap<String /*az*/, f32>, zonal_shift_ids: Vec<String>, autoshifts: Vec<AutoshiftInResource>, practice_run_configuration: Option<PracticeRunConfig>, zonal_autoshift_status: ZonalAutoshiftStatus }`
- `zonal_shifts: BTreeMap<String /*zonalShiftId*/, ZonalShift>` where `ZonalShift { zonal_shift_id: String, resource_identifier: String, away_from: String, start_time: SystemTime, expiry_time: SystemTime, status: ZonalShiftStatus /*ACTIVE|CANCELED|EXPIRED*/, comment: String, shift_type: ShiftType /*ZONAL_SHIFT for this slice*/, practice_run_outcome: Option<PracticeRunOutcome> }`
- `next_shift_seq: u64` — source of unique, pattern-valid `zonalShiftId`s.

`appliedWeights` is derived from the resource's ACTIVE (non-expired) shifts, not stored independently of them. Pagination tokens for both list operations encode the last-returned key.

The remaining nine operations (StartPracticeRun, CancelPracticeRun, Create/Update/DeletePracticeRunConfiguration, UpdateZonalAutoshiftConfiguration, ListAutoshifts, Get/UpdateAutoshiftObserverNotificationStatus) stay 501 for this slice and are filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-arczonalshift-percdataplane/OPERATIONS.md` lists all 15 operations with routes, required input/output members and errors: StartZonalShift (`POST /zonalshifts -> 201`, required resourceIdentifier/awayFrom/expiresIn/comment; outputs zonalShiftId/startTime/expiryTime/status/comment all required), GetManagedResource (`GET /managedresources/{resourceIdentifier}`, required appliedWeights + zonalShifts), ListZonalShifts (`GET /zonalshifts`, @paginated with status/resourceIdentifier/maxResults query params), UpdateZonalShift (`PATCH /zonalshifts/{zonalShiftId}`), CancelZonalShift (`DELETE /zonalshifts/{zonalShiftId}`), ListManagedResources (`GET /managedresources`, @paginated, required items). Smithy model shapes ZonalShiftSummary, ZonalShiftInResource (`appliedStatus` APPLIED/NOT_APPLIED) and ManagedResourceSummary (`availabilityZones` required, `appliedWeights` map az->1|0 documented as "whether resources are active in Availability Zones or not") ground the state model and the weight semantics; ZonalShiftStatus documents ACTIVE / EXPIRED (expiry time exceeded) / CANCELED. `src/ops/start_zonal_shift.rs:4` carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and line 226 returns `Scaffolded::not_implemented("StartZonalShift")`; every other file in `src/ops/` is the same generated stub, and there are no tests beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `arc-zonal-shift` at it.
2. `aws arc-zonal-shift list-managed-resources` — expect 200 with a non-empty `items` list of seeded managed resources; today: 501 NotImplemented.
3. `aws arc-zonal-shift start-zonal-shift --resource-identifier <arn from step 2> --away-from use1-az1 --expires-in 1h --comment 'bad deploy'` — expect 201 with a generated `zonalShiftId`, `status=ACTIVE`, `startTime` ~now and `expiryTime` ~now+1h.
4. `aws arc-zonal-shift get-managed-resource --resource-identifier <arn>` — expect the new shift in `zonalShifts` with `appliedStatus=APPLIED` and `appliedWeights` showing 0 for `use1-az1`, 1 for the other AZs.
5. `aws arc-zonal-shift list-zonal-shifts --status ACTIVE --resource-identifier <arn>` — expect exactly the shift from step 3; repeat with `--status CANCELED` and expect it absent.
6. `aws arc-zonal-shift update-zonal-shift --zonal-shift-id <id> --comment 'extending' --expires-in 2h` — expect 200 with the new comment and a later `expiryTime`, then re-run step 5 and confirm the stored values changed.
7. `aws arc-zonal-shift cancel-zonal-shift --zonal-shift-id <id>` — expect 200 with `status=CANCELED`; re-run step 4 and confirm `appliedWeights` is back to all 1 and the shift is no longer applied; call cancel again and expect ConflictException (409).
8. `aws arc-zonal-shift start-zonal-shift --resource-identifier arn:aws:elasticloadbalancing:us-east-1:111111111111:loadbalancer/app/does-not-exist/0 ...` — expect ResourceNotFoundException (404), not 501.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# GetSoftwareUpdates issues an update URL for a job that has no device yet

_Filed: 2026-10-24T15:57:00Z_
_Operation: GetSoftwareUpdates | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetSoftwareUpdates` refuses only a `Cancelled` job (`JobState::has_software_update` returns true for every other state). A job in `New`, `PreparingAppliance` or `PreparingShipment` has no device associated with it — the same crate's `JobMetadata` rendering deliberately omits `SnowballId` until the job leaves `New` — yet the call answers 200 with a presigned-style `UpdatesURI` for that non-existent device's update file and records a two-day grant in the store.

Expected: the update file is "an update file associated with a specified JobId", i.e. the software for the Snow device that job ordered; with no device prepared or shipped, AWS refuses with the declared `InvalidJobStateException` rather than handing out a download URL.

Actual: HTTP 200 with a fabricated `UpdatesURI` immediately after `CreateJob`.

## Evidence

Spec/model: `GetSoftwareUpdates` — "Returns an Amazon S3 presigned URL for an update file associated with a specified `JobId`" — declares `InvalidJobStateException` (HTTP 400) for a state that does not allow the action; `GetSoftwareUpdatesResult$UpdatesURI` describes a file for the device. Handler: src/ops/get_software_updates.rs (~lines 193-213) gates only on `JobState::has_software_update`, defined in src/state.rs (~line 314) as `!matches!(self, Cancelled)`. The same file's `to_job_metadata` (~lines 494-535) states that a device (and therefore `SnowballId`) exists only once the job has left `New`, so the two rules contradict each other. Tests: tests/all/software_updates.rs asserts the `Cancelled`-job and main-export-job refusals and asserts success for a newly created job, so the pre-shipment case is pinned to the current behavior rather than to AWS's.

## Steps to Reproduce

1. CreateAddress with a deliverable address; keep the `AddressId`.
2. CreateJob with `{"JobType": "IMPORT", "SnowballType": "EDGE", "AddressId": "<id>", "ShippingOption": "STANDARD"}`; keep the `JobId`.
3. DescribeJob: `JobState` is `New`, no `SnowballId`.
4. GetSoftwareUpdates with that `JobId`.
5. Observe 200 with an `UpdatesURI` instead of a 400 `InvalidJobStateException`.

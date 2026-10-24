Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# Harvest jobs never export anything to the S3 destination and always end FAILED instead of COMPLETED

_Filed: 2026-10-24T05:11:59Z_
_Operation: CreateHarvestJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A MediaPackage v2 harvest job exports the named manifests and their segments from the origin endpoint to `Destination.S3Destination` and then reports `Status: COMPLETED`; `HarvestJobStatus` enumerates `QUEUED | IN_PROGRESS | COMPLETED | CANCELLED | FAILED`, and the normal success path for a job whose scheduled window has elapsed is `COMPLETED` with the harvested objects present in the destination bucket/prefix.

The emulator runs no harvester and hard-codes the failure path: `HarvestJobRecord::materialise_due_transitions` moves `QUEUED -> IN_PROGRESS` after 400 ms and then `IN_PROGRESS -> FAILED` as soon as `ScheduleConfiguration.EndTime` passes, setting `ErrorMessage` to `HARVESTER_UNAVAILABLE_MESSAGE` ("This emulator does not run the MediaPackage harvester, so no content was exported to the harvest destination"). `COMPLETED` is therefore unreachable, no object is ever written to the destination bucket (there is no interaction with the S3 emulator at all), and `ErrorMessage` carries emulator-specific prose on a path where AWS reports no error at all.

A client workflow of "create harvest job → poll GetHarvestJob until COMPLETED → read the harvested manifest from S3" can never succeed: it observes FAILED and an empty destination.

## Evidence

Spec: Smithy `HarvestJobStatus` enum includes `COMPLETED`; `CreateHarvestJob` doc describes creating a harvest job "to export content from an origin endpoint to an S3 bucket"; `HarvestJob$ErrorMessage` is "An error message if the harvest job encountered any issues" — i.e. populated only on a real failure. `Destination$S3Destination` (`BucketName`, `DestinationPath`) is `@required`, so the destination is part of the contract.

Handler/state: `src/state.rs:768-850` — `HARVESTER_UNAVAILABLE_MESSAGE` and `materialise_due_transitions`, which only ever reaches `HARVEST_JOB_FAILED`; `HARVEST_JOB_COMPLETED` is referenced solely by `is_terminal()` and is never assigned anywhere in the crate. `src/ops/create_harvest_job.rs:355-370` persists the destination but nothing consumes it.

Tests: no test asserts a COMPLETED harvest job or any S3 side effect. The existing open issue 20261022T103649Z covers only the *validation* of an unwritable S3 destination (cause 1), not the export itself or the terminal status.

## Steps to Reproduce

1. Create a channel group, channel and an origin endpoint publishing an HLS manifest named `index`.
2. POST `/channelGroup/{g}/channel/{c}/originEndpoint/{e}/harvestJob` with `HarvestedManifests: {"HlsManifests": [{"ManifestName": "index"}]}`, a `ScheduleConfiguration` whose window ends a few seconds out, and a `Destination.S3Destination` naming a bucket that exists in the S3 emulator.
3. Poll GET `/channelGroup/{g}/channel/{c}/originEndpoint/{e}/harvestJob/{job}` past `EndTime`. Expected: `Status: COMPLETED` and no `ErrorMessage`. Actual: `Status: FAILED` with `ErrorMessage: "This emulator does not run the MediaPackage harvester…"`.
4. List the destination bucket/prefix in the S3 emulator. Expected: the harvested manifest/segment objects. Actual: nothing was written.

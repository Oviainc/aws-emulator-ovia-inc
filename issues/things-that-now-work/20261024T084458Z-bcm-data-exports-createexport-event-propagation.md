Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# Deliver the export file to the destination S3 bucket instead of failing every execution

_Filed: 2026-10-24T08:44:58Z_
_Operation: CreateExport | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service owes each persisted definition a delivery: it runs the query and writes the export file to the caller's S3 bucket under `S3Destination.S3Bucket`/`S3Prefix` at the `RefreshCadence`. The emulator schedules and records that delivery as an execution, but the delivery is never performed against the S3 emulator, and the recorded execution is hard-coded to end in a FAILED terminal state.

Expected (real service): a valid export with a writable destination progresses INITIATION_IN_PROCESS -> QUERY_QUEUED/QUERY_IN_PROCESS -> DELIVERY_IN_PROCESS -> `DELIVERY_SUCCESS` (every one a modelled `ExecutionStatusCode`), an object appears in the configured bucket under the configured prefix (honouring `Overwrite`: CREATE_NEW_REPORT adds a version, OVERWRITE_REPORT replaces it), and `ExportStatus.LastRefreshedAt` ("The timestamp of when the export was last generated") becomes set on GetExport/ListExports.

Actual (emulator): `complete_delivery` (src/state.rs lines 444-452) unconditionally writes `StatusCode=QUERY_FAILURE` and `StatusReason=INTERNAL_FAILURE` for every execution of every export, no S3 object is ever written (nothing in the crate reads `s3_bucket`; grep for `bucket` matches only comments), and `fresh_status` leaves `last_refreshed_at: None` for the life of the export (src/state.rs lines 718-738). A client polling GetExecution/ListExecutions therefore always observes a failed export pipeline, and DELIVERY_SUCCESS / DELIVERY_IN_PROCESS / QUERY_QUEUED / QUERY_IN_PROCESS are unreachable states.

## Evidence

Spec: `ExecutionStatusCode` models INITIATION_IN_PROCESS, QUERY_QUEUED, QUERY_IN_PROCESS, QUERY_FAILURE, DELIVERY_IN_PROCESS, DELIVERY_SUCCESS, DELIVERY_FAILURE (smithy_query shape `ExecutionStatusCode`); `RefreshCadence` is "The cadence for Amazon Web Services to update the data export in your S3 bucket"; `ExportStatus$LastRefreshedAt` is "The timestamp of when the export was last generated"; `S3Destination` is "the destination Amazon S3 bucket name and object keys of a data exports file" with required `S3Bucket`/`S3Prefix`/`S3Region`.

Handler/state: src/ops/create_export.rs lines 287-302 schedules `pending_delivery`; src/state.rs `materialize_due` -> `initiate_delivery` -> `complete_delivery` (lines 317-452) is the whole data plane and its terminal status is a constant.

Tests: tests/all/delivery_executions.rs (header lines 14-20) PINS the current failure state as the emulator's deliberate choice, so the divergence from the real service is asserted rather than covered — no test exercises a successful delivery or an object landing in the S3 emulator.

## Steps to Reproduce

1. CreateExport with a destination bucket that exists in the S3 emulator (e.g. S3Bucket="cur-exports", S3Prefix="cur2")
2. Let the stored delivery instant fall due (DELIVERY_INITIATION_DELAY + DELIVERY_SETTLES_AFTER), then call ListExecutions for the export ARN and GetExecution for the reported ExecutionId
3. Observe ExecutionStatus StatusCode=QUERY_FAILURE / StatusReason=INTERNAL_FAILURE instead of a DELIVERY_SUCCESS progression
4. ListObjectsV2 on the destination bucket: no export object was written under the configured prefix
5. GetExport the ARN: ExportStatus.LastRefreshedAt is absent even though a delivery has completed

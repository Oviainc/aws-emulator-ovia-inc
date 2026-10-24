Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 82d0a94f39949549fd2daeecac19e7039c2ed673

# Refuse StartDICOMImportJob when the target data store is not ACTIVE

_Filed: 2026-10-24T01:38:49Z_
_Operation: StartDICOMImportJob | Class: validation_

> _Note: this issue was identified against the code base at commit `82d0a94f39949549fd2daeecac19e7039c2ed673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler only checks that the data store EXISTS; it never looks at `datastoreStatus`. A data store that is still `CREATING` (the status `CreateDatastore` answers with, held for `DATASTORE_ACTIVATION_DELAY`) or already `DELETING` therefore accepts an import job and answers 200 with `jobStatus: SUBMITTED`, and the job is persisted and driven to `COMPLETED` by the clock.

Expected: AWS HealthImaging imports only into an ACTIVE data store — submitting an import job against a store that is still being created, or that is being deleted, is refused (the operation models both `ValidationException` HTTP 400 and `ConflictException` HTTP 409 for exactly this inconsistent-state case) and no job is created. Actual: HTTP 200 with a brand-new job id, which a later `GetDICOMImportJob` reports as `IN_PROGRESS`/`COMPLETED` against a store that never finished creating (or that disappears mid-job when its deletion falls due).

## Evidence

Spec: `StartDICOMImportJob` declares `ConflictException` (HTTP 409, documented "Updating or deleting a resource can cause an inconsistent state") and `ValidationException` (HTTP 400) (smithy_query operation=StartDICOMImportJob); `DatastoreStatus` models CREATING/DELETING as non-ACTIVE states, and HealthImaging's documented workflow is create data store -> wait for ACTIVE -> start import job. Handler: src/ops/start_dicom_import_job.rs:170-179 checks only `store.datastores.contains_key(...)`, and lines 225-258 insert the job regardless of `datastore.datastore_status`. Tests: tests/all/datastore_lifecycle.rs always calls `active_datastore(..)` (which sleeps out the activation span) before starting a job, and the refusal table only covers an UNKNOWN datastore id (404) — no case drives an import against a CREATING or DELETING store.

## Steps to Reproduce

1. POST /datastore with a fresh clientToken; the response is `CREATING`.
2. Immediately (within DATASTORE_ACTIVATION_DELAY) POST /startDICOMImportJob/datastore/{datastoreId} with a valid dataAccessRoleArn, inputS3Uri and outputS3Uri.
3. Observe HTTP 200 with `jobStatus: SUBMITTED` instead of a refusal for a data store that is not ACTIVE.
4. Alternatively DELETE /datastore/{datastoreId} (status becomes DELETING) and start an import job in the same window — also accepted.

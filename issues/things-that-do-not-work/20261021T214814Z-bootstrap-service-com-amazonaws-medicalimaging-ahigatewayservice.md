Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the AWS HealthImaging (medical-imaging) emulator

_Filed: 2026-10-21T21:48:14Z_
_Operation: service-com-amazonaws-medicalimaging-ahigatewayservice | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 18 operations in `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding zero state. Nothing works end-to-end.

Implement a FIRST SLICE of six operations covering the data store lifecycle plus the DICOM import job loop (the service's primary resource and its core workflow). Protocol is `restJson1`, signing name `medical-imaging`.

1. **CreateDatastore** — `POST /datastore`. Input: `datastoreName` (1..256, `^[A-Za-z0-9._/#-]+$`), `clientToken` (required, @idempotencyToken), optional `tags`, `kmsKeyArn`, `lambdaAuthorizerArn`, `losslessStorageFormat`. Must mint a NEW `datastoreId` matching `^[0-9a-z]{32}$`, persist the datastore record (name, tags, kms/lambda arns, storage format, createdAt/updatedAt, ARN `arn:aws:medical-imaging:<region>:<account>:datastore/<datastoreId>`) and return `{datastoreId, datastoreStatus}` with status `ACTIVE` (or `CREATING` then `ACTIVE`). `clientToken` must be honoured for idempotency: a repeat call with the same token returns the SAME `datastoreId` rather than creating a second store.
2. **GetDatastore** — `GET /datastore/{datastoreId}`. Must read the persisted record and return `datastoreProperties` with `datastoreId`, `datastoreName`, `datastoreStatus`, `datastoreArn`, `createdAt`, `updatedAt`, plus `kmsKeyArn`/`lambdaAuthorizerArn`/`losslessStorageFormat` exactly as supplied at create time. Unknown id → `ResourceNotFoundException` (HTTP 404); id not matching `^[0-9a-z]{32}$` → `ValidationException` (HTTP 400).
3. **ListDatastores** — `GET /datastore`. Must enumerate persisted datastores as `datastoreSummaries` (`datastoreId`, `datastoreName`, `datastoreStatus`, `datastoreArn`, `createdAt`, `updatedAt`), filter by the `datastoreStatus` query param when present, and honour `maxResults`/`nextToken` with a real opaque continuation token (@paginated inputToken=nextToken, outputToken=nextToken, items=datastoreSummaries). A created store must appear here; a deleted one must not.
4. **StartDICOMImportJob** — `POST /startDICOMImportJob/datastore/{datastoreId}`. Input: required `dataAccessRoleArn`, `clientToken`, `inputS3Uri`, `outputS3Uri`, optional `jobName`, `inputOwnerAccountId`, `importConfiguration`. Must verify the datastore exists (else 404), mint a `jobId` matching `^[0-9a-z]{1,32}$`, persist the job against that datastore with all inputs plus `submittedAt`, and return `{datastoreId, jobId, jobStatus, submittedAt}` with `jobStatus` = `SUBMITTED`. `clientToken` is idempotent: same token → same `jobId`.
5. **GetDICOMImportJob** — `GET /getDICOMImportJob/datastore/{datastoreId}/job/{jobId}`. Must return `jobProperties` built from the persisted job: `jobId`, `jobName`, `jobStatus`, `datastoreId`, `dataAccessRoleArn`, `inputS3Uri`, `outputS3Uri`, `submittedAt`, and `endedAt` once terminal. The emulator should advance the job to `COMPLETED` (setting `endedAt`) so a poll loop terminates. Unknown `jobId` or `datastoreId` → `ResourceNotFoundException` (404).
6. **DeleteDatastore** — `DELETE /datastore/{datastoreId}`. Must remove (or mark `DELETED`) the persisted datastore and return `{datastoreId, datastoreStatus}`. A subsequent `GetDatastore` must fail with `ResourceNotFoundException` (404) and the store must be gone from `ListDatastores`. Unknown id → 404.

NEVER STUBS: each of these six must compute its response from the shared store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the request back, no minting an id that is never stored. A create followed by a read must return what was created; a delete followed by a read must 404.

## State Model

In `src/state.rs` (`ComAmazonawsMedicalimagingAhigatewayserviceState`):

- `datastores: HashMap<String /* datastoreId (32 lowercase alnum) */, Datastore>`
- `Datastore { datastore_id: String, datastore_name: Option<String>, datastore_status: DatastoreStatus /* ACTIVE|CREATING|DELETING|DELETED|CREATE_FAILED */, datastore_arn: String, kms_key_arn: Option<String>, lambda_authorizer_arn: Option<String>, lossless_storage_format: Option<LosslessStorageFormat>, tags: HashMap<String,String>, created_at: SystemTime, updated_at: SystemTime, import_jobs: HashMap<String /* jobId */, ImportJob>, image_sets: HashMap<String /* imageSetId */, ImageSet> }`
- `ImportJob { job_id: String, job_name: Option<String>, job_status: JobStatus /* SUBMITTED|IN_PROGRESS|COMPLETED|FAILED */, datastore_id: String, data_access_role_arn: String, input_s3_uri: String, output_s3_uri: String, input_owner_account_id: Option<String>, import_configuration: Option<ImportConfiguration>, submitted_at: SystemTime, ended_at: Option<SystemTime>, message: Option<String> }`
- `ImageSet { image_set_id: String, versions: Vec<ImageSetVersion>, state: ImageSetState, workflow_status: ImageSetWorkflowStatus, created_at, updated_at, deleted_at: Option<..>, is_primary: bool, arn: String }` — reserved for the later image-set slice; the bootstrap only needs the map to exist so import jobs can populate it.
- `client_tokens: HashMap<String /* clientToken */, String /* minted datastoreId or jobId */>` to make `@idempotencyToken` on CreateDatastore and StartDICOMImportJob real.
- `next_id` / RNG source for deterministic 32-char lowercase-alnum id minting.

Out of scope for this first slice (they keep answering 501 and are covered by later per-operation gap issues): SearchImageSets, GetImageSet, GetImageSetMetadata, GetImageFrame, ListImageSetVersions, CopyImageSet, UpdateImageSetMetadata, DeleteImageSet, ListDICOMImportJobs, TagResource, UntagResource, ListTagsForResource.

## Evidence

OPERATIONS.md (crate root) lists all 18 operations with routes, input/output members and errors: CreateDatastore `POST /datastore` → `{datastoreId, datastoreStatus}` with required @idempotencyToken `clientToken`; GetDatastore `GET /datastore/{datastoreId}` → required `datastoreProperties`; ListDatastores `GET /datastore` is @paginated(inputToken=nextToken, outputToken=nextToken, items=datastoreSummaries); StartDICOMImportJob `POST /startDICOMImportJob/datastore/{datastoreId}` → required `{datastoreId, jobId, jobStatus, submittedAt}`; GetDICOMImportJob `GET /getDICOMImportJob/datastore/{datastoreId}/job/{jobId}` → required `jobProperties`; DeleteDatastore `DELETE /datastore/{datastoreId}` → `{datastoreId, datastoreStatus}`. All declare `ResourceNotFoundException (HTTP 404)` and `ValidationException (HTTP 400)`. The Smithy model shapes DatastoreProperties, DatastoreSummary and DICOMImportJobProperties give the exact required member sets quoted above. Census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 18 handler files, so every operation currently returns HTTP 501 and no state exists. No tests cover any behaviour beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI medical-imaging client at it.
2. CreateDatastore with datastoreName='bootstrap-store' and a clientToken; expect 200 and a 32-char lowercase-alnum datastoreId with datastoreStatus ACTIVE (today: HTTP 501 NotImplemented).
3. Repeat the same CreateDatastore call with the SAME clientToken; expect the same datastoreId back, not a second data store.
4. GetDatastore with that datastoreId; expect datastoreProperties.datastoreName == 'bootstrap-store' plus datastoreArn/createdAt populated.
5. ListDatastores; expect the new store present in datastoreSummaries, and verify datastoreStatus=ACTIVE filtering and maxResults=1 + nextToken paging work.
6. StartDICOMImportJob on that datastoreId with dataAccessRoleArn, inputS3Uri='s3://bucket/in/', outputS3Uri='s3://bucket/out/' and a clientToken; expect 200 with a jobId, jobStatus SUBMITTED and submittedAt.
7. GetDICOMImportJob with that datastoreId+jobId; expect jobProperties echoing the persisted inputS3Uri/outputS3Uri/dataAccessRoleArn and eventually jobStatus COMPLETED with endedAt.
8. GetDICOMImportJob with a bogus jobId; expect ResourceNotFoundException (HTTP 404).
9. DeleteDatastore with that datastoreId; expect 200, then GetDatastore returns ResourceNotFoundException (404) and ListDatastores no longer lists it.

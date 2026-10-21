Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the SageMaker Geospatial emulator

_Filed: 2026-10-21T21:28:10Z_
_Operation: service-com-amazonaws-sagemakergeospatial-sagemakergeospatial | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 19 operations in `src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` with zero stored state, so no client workflow works at all. Implement a FIRST SMALL SLICE: the Earth Observation Job lifecycle, statefully, on the restJson1 wire protocol (sigv4 signing name `sagemaker-geospatial`).

### Happy path to implement (in order)

1. **StartEarthObservationJob** — `POST /earth-observation-jobs`. Input: `Name` (required), `InputConfig` (InputConfigInput, required), `JobConfig` (JobConfigInput, required), `ExecutionRoleArn` (required, must match `^arn:(aws[a-z-]*):iam::([0-9]{12}):role/...$`), optional `ClientToken` (@idempotencyToken), `KmsKeyId`, `Tags`. Must MINT and PERSIST a new job keyed by a generated Arn of the modeled form `arn:aws:sagemaker-geospatial:<region>:<account>:earth-observation-job/<12+ lowercase alnum>`, record `CreationTime` (now), `DurationInSeconds`, and `Status` starting at `INITIALIZING`. Response echoes `Name`, `Arn`, `CreationTime`, `DurationInSeconds`, `Status`, `JobConfig`, `ExecutionRoleArn`, `InputConfig` (as InputConfigOutput), `KmsKeyId`, `Tags` — all read back out of the stored record, not from a canned literal.
2. **GetEarthObservationJob** — `GET /earth-observation-jobs/{Arn}`. Must look the Arn up in the store and return the job as created: `Arn`, `Name`, `CreationTime`, `DurationInSeconds`, `Status`, `InputConfig`, `JobConfig`, `ExecutionRoleArn`, `Tags` (plus `ErrorDetails`/`ExportStatus` when set). An unknown or deleted Arn must return `ResourceNotFoundException` (HTTP 404); an Arn that does not match the modeled pattern must return `ValidationException` (HTTP 400). Job status should progress from `INITIALIZING` to `COMPLETED` on read (or after a short simulated delay) so a poll loop terminates.
3. **ListEarthObservationJobs** — `POST /list-earth-observation-jobs`. Must return one `ListEarthObservationJobOutputConfig` entry per stored job (`Arn`, `Name`, `CreationTime`, `Status`, `DurationInSeconds`, `OperationType`, `Tags`), honoring `StatusEquals` as a filter, `SortBy`/`SortOrder` for ordering, and `MaxResults`/`NextToken` for real pagination (truncate at MaxResults and return an opaque `NextToken` that resumes exactly where the page stopped; omit `NextToken` on the last page).
4. **StopEarthObservationJob** — `POST /earth-observation-jobs/stop`. Must mutate the stored job's `Status` to `STOPPING`/`STOPPED` so a following GetEarthObservationJob observes the change. Unknown Arn -> 404; stopping an already terminal job -> `ConflictException` (HTTP 409).
5. **DeleteEarthObservationJob** — `DELETE /earth-observation-jobs/{Arn}`. Must remove the job from the store (empty body, 200). A subsequent GetEarthObservationJob on the same Arn must return `ResourceNotFoundException` 404, and the job must disappear from ListEarthObservationJobs. Deleting a job that is still `IN_PROGRESS` -> `ConflictException` 409; deleting an unknown Arn -> 404.
6. **ListTagsForResource / TagResource** (small, shares the same store) — `GET /tags/{ResourceArn}` must return exactly the tag map persisted by StartEarthObservationJob or by `PUT /tags/{ResourceArn}`; tagging an unknown Arn -> 404.

No canned or echoed responses: each operation must compute its response from the persisted state, so a create is observable from a later read, a stop is observable from a later get, and a delete is observable as a 404. Minting an Arn without storing a record, or echoing the request back without persisting, counts as a failure of this issue.

## State Model

- `GeospatialStore` (behind the service's shared state lock):
  - `earth_observation_jobs: Map<String /* Arn */, EarthObservationJob>`
  - `vector_enrichment_jobs: Map<String /* Arn */, VectorEnrichmentJob>` (declared now, populated by a later slice)
  - `raster_data_collections: Map<String /* Arn */, RasterDataCollection>` (declared now; seeded read-only catalog in a later slice)
  - `idempotency: Map<String /* ClientToken */, String /* Arn */>` so a repeated StartEarthObservationJob with the same `ClientToken` returns the first job instead of creating a second
- `EarthObservationJob { arn: String, name: String, creation_time: Timestamp, duration_in_seconds: i32, status: EarthObservationJobStatus /* INITIALIZING|IN_PROGRESS|COMPLETED|STOPPING|STOPPED|DELETING|DELETED|FAILED */, kms_key_id: Option<String>, input_config: InputConfigInput, job_config: JobConfigInput, output_bands: Vec<OutputBand>, execution_role_arn: String, error_details: Option<EarthObservationJobErrorDetails>, export_status: Option<ExportStatus>, export_error_details: Option<ExportErrorDetails>, tags: Map<String,String> }`
- Arn generator: `arn:aws:sagemaker-geospatial:{region}:{account_id}:earth-observation-job/{12 lowercase alphanumeric chars}` so the value round-trips through the `@httpLabel` pattern on GetEarthObservationJob.

### Done when
A client script can StartEarthObservationJob -> poll GetEarthObservationJob until `COMPLETED` -> see the job in ListEarthObservationJobs (with MaxResults/NextToken paging) -> read its tags via ListTagsForResource -> StopEarthObservationJob (or DeleteEarthObservationJob) -> GetEarthObservationJob returns 404, all with field values matching what was submitted.

Operations outside this slice (ExportEarthObservationJob, GetTile, SearchRasterDataCollection, GetRasterDataCollection, ListRasterDataCollections, the whole VectorEnrichmentJob family, UntagResource) stay 501 for now and will be filed as individual gap issues.

## Evidence

Spec: `services/service-com-amazonaws-sagemakergeospatial-sagemakergeospatial/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-sagemakergeospatial-sagemakergeospatial.json`) documents protocol `restJson1` and the routes/members cited above: StartEarthObservationJob `POST /earth-observation-jobs` (required Name/InputConfig/JobConfig/ExecutionRoleArn, output Arn/CreationTime/DurationInSeconds/Status), GetEarthObservationJob `GET /earth-observation-jobs/{Arn}` (@readonly, ResourceNotFoundException 404, ValidationException 400), ListEarthObservationJobs `POST /list-earth-observation-jobs` (@paginated inputToken=NextToken outputToken=NextToken items=EarthObservationJobSummaries), StopEarthObservationJob `POST /earth-observation-jobs/stop` (ConflictException 409), DeleteEarthObservationJob `DELETE /earth-observation-jobs/{Arn}` (@idempotent, ConflictException 409), ListTagsForResource/TagResource on `/tags/{ResourceArn}`.

Implementation: all 19 files under `services/service-com-amazonaws-sagemakergeospatial-sagemakergeospatial/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and return `crate::scaffolded::Scaffolded::not_implemented(..)` (HTTP 501) — verified by grep across the ops directory (start_earth_observation_job.rs, get_earth_observation_job.rs, list_earth_observation_jobs.rs, stop_earth_observation_job.rs, delete_earth_observation_job.rs, list_tags_for_resource.rs, tag_resource.rs, ...). No state module and no tests exercise any job lifecycle.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `sagemaker-geospatial` at it.
2. Call StartEarthObservationJob with Name="demo-job", a valid ExecutionRoleArn, an InputConfig referencing a raster data collection query, and a JobConfig (e.g. BandMathConfig) — expect HTTP 200 with a newly minted Arn, CreationTime and Status=INITIALIZING; observed: HTTP 501 NotImplemented.
3. Call GetEarthObservationJob with the returned Arn — expect the same Name/InputConfig/JobConfig/ExecutionRoleArn/Tags back and Status progressing to COMPLETED; observed: HTTP 501.
4. Call ListEarthObservationJobs with MaxResults=1 — expect the job in EarthObservationJobSummaries plus a NextToken when more jobs exist; observed: HTTP 501.
5. Call ListTagsForResource with the job Arn — expect the tag map supplied at Start; observed: HTTP 501.
6. Call StopEarthObservationJob with the Arn, then GetEarthObservationJob — expect Status STOPPING/STOPPED; observed: HTTP 501.
7. Call DeleteEarthObservationJob with the Arn, then GetEarthObservationJob with the same Arn — expect ResourceNotFoundException (HTTP 404) and the job absent from ListEarthObservationJobs; observed: HTTP 501 for both.

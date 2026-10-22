Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the Amazon Lookout for Equipment emulator

_Filed: 2026-10-21T23:13:03Z_
_Operation: service-com-amazonaws-lookoutequipment-awslookoutequipmentfrontendservice | Class: missing_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 49 operations bound by `AWSLookoutEquipmentFrontendService` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can get past its first call. This issue scopes the FIRST buildable slice: the dataset lifecycle, which is the primary resource of this service and the prerequisite for every model/scheduler workflow.

## Happy path to implement (in order)

1. **CreateDataset** — input `DatasetName` (required, `^[0-9a-zA-Z_-]{1,200}$`), `DatasetSchema`, `ServerSideKmsKeyId`, `ClientToken` (required, @idempotencyToken), `Tags`. Must allocate and persist a new dataset record and return `DatasetName`, a generated `DatasetArn` matching `arn:aws:lookoutequipment:<region>:<account>:dataset/<DatasetName>/<uuid>`, and `Status` = `CREATED`. Creating a name that already exists must fail with `ConflictException` (HTTP 409) unless the same `ClientToken` is replayed, in which case the original record's values are returned.
2. **DescribeDataset** — input `DatasetName` (required). Must look the dataset up in state and return the stored `DatasetName`, `DatasetArn`, `CreatedAt`, `LastUpdatedAt`, `Status`, `Schema` (the JSON string that was supplied as `DatasetSchema.InlineDataSchema` at create time), `ServerSideKmsKeyId`, and — once an ingestion job has run — `IngestionInputConfiguration`, `RoleArn`, `DataStartTime`, `DataEndTime`. An unknown name must fail with `ResourceNotFoundException` (HTTP 404), not an empty 200.
3. **StartDataIngestionJob** — input `DatasetName` (required), `IngestionInputConfiguration` (required), `RoleArn` (required), `ClientToken` (required). Must verify the dataset exists (else `ResourceNotFoundException`), persist a new job record keyed by a generated 32-hex-char `JobId` (`^[A-Fa-f0-9]{0,32}$`), and return that `JobId` plus `Status` = `IN_PROGRESS`. Side effect on the parent dataset: its `Status` moves to `INGESTION_IN_PROGRESS` and then `ACTIVE`, and its `IngestionInputConfiguration` / `RoleArn` are set from this call — both observable through DescribeDataset.
4. **DescribeDataIngestionJob** — input `JobId` (required). Must return the persisted job: `JobId`, `DatasetArn` of the parent dataset, the `IngestionInputConfiguration` and `RoleArn` exactly as submitted, `CreatedAt`, and a terminal `Status` (`SUCCESS`). Unknown `JobId` → `ResourceNotFoundException` (404).
5. **ListDatasets** — input `NextToken`, `MaxResults` (1..500), `DatasetNameBeginsWith`. Must enumerate the datasets actually in state as `DatasetSummaries` (each with `DatasetName`, `DatasetArn`, `CreatedAt`, `Status`), honour the `DatasetNameBeginsWith` prefix filter, cap the page at `MaxResults`, and emit a `NextToken` only when more datasets remain — a token that resumes where the previous page ended.
6. **DeleteDataset** — input `DatasetName` (required). Must remove the dataset (and its ingestion jobs) from state, return an empty 200 body, and make the subsequent DescribeDataset answer `ResourceNotFoundException` (404) and ListDatasets no longer include it. Deleting an unknown name → `ResourceNotFoundException`.

Every one of these six must compute its response from persisted state. No canned bodies, no echoing the request back, no generating an ARN or `JobId` that is never stored: a create followed by a describe must return what was created, and a delete followed by a describe must 404.

## State Model

A single service-level store behind the existing state lock:

- `datasets: Map<DatasetName, Dataset>` where `Dataset { name: String, arn: String, created_at: Timestamp, last_updated_at: Timestamp, status: DatasetStatus (CREATED|INGESTION_IN_PROGRESS|IMPORT_IN_PROGRESS|ACTIVE), schema: Option<String> /* inline JSON from DatasetSchema */, server_side_kms_key_id: Option<String>, ingestion_input_configuration: Option<IngestionInputConfiguration>, role_arn: Option<String>, data_start_time: Option<Timestamp>, data_end_time: Option<Timestamp>, source_dataset_arn: Option<String>, tags: Vec<Tag>, client_token: String }`
- `ingestion_jobs: Map<JobId, IngestionJob>` where `IngestionJob { job_id: String /* 32 hex chars */, dataset_name: String, dataset_arn: String, ingestion_input_configuration: IngestionInputConfiguration, role_arn: String, created_at: Timestamp, status: IngestionJobStatus, failed_reason: Option<String>, status_detail: Option<String>, ingested_data_size: Option<i64>, data_start_time: Option<Timestamp>, data_end_time: Option<Timestamp>, client_token: String }` plus a per-dataset index so ListDataIngestionJobs/DescribeDataset can find a dataset's jobs.
- `idempotency: Map<(OperationName, ClientToken), ResourceId>` so a replayed `ClientToken` returns the first result instead of creating a duplicate.
- ARNs built from the request's region and a stable 12-digit account id so the modeled ARN patterns hold.

## Out of scope for this slice

Models (CreateModel/DescribeModel/ListModels/DeleteModel/UpdateModel/model versions), inference and retraining schedulers, labels and label groups, ImportDataset/ImportModelVersion, ListSensorStatistics, resource policies, and tagging remain 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md at the crate root documents all 49 operations of `AWSLookoutEquipmentFrontendService` (awsJson1_0, `POST /` with `X-Amz-Target: AWSLookoutEquipmentFrontendService.<Operation>`, signing name `lookoutequipment`). The field names, requiredness, constraint patterns, enum value sets and error/status mappings quoted above are taken verbatim from its CreateDataset (line 27), DescribeDataset (line 111), DescribeDataIngestionJob (line 105), ListDatasets (line 178), StartDataIngestionJob (line 260) and DeleteDataset (line 63) sections — including `DatasetStatus` = ACTIVE|CREATED|IMPORT_IN_PROGRESS|INGESTION_IN_PROGRESS, `IngestionJobStatus` = FAILED|IMPORT_IN_PROGRESS|IN_PROGRESS|SUCCESS, the dataset/job ARN and JobId patterns, `MaxResults` range 1..500, and `ResourceNotFoundException` (404) / `ConflictException` (409) / `ValidationException` (400).

Actual: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-lookoutequipment-awslookoutequipmentfrontendservice/src/ops/` matches all 49 handler files, including `create_dataset.rs`, `describe_dataset.rs`, `start_data_ingestion_job.rs`, `describe_data_ingestion_job.rs`, `list_datasets.rs` and `delete_dataset.rs`; each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, and the crate holds no state structures at all.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `lookoutequipment` client at it.
2. CreateDataset with DatasetName="demo-dataset", a ClientToken, and DatasetSchema containing an inline component/sensor schema → expect 200 with DatasetName="demo-dataset", a dataset ARN, and Status="CREATED" (actual today: HTTP 501 NotImplemented).
3. DescribeDataset with DatasetName="demo-dataset" → expect 200 echoing the stored ARN, CreatedAt, Status and the Schema string supplied at create time.
4. Repeat CreateDataset with the same DatasetName and a NEW ClientToken → expect ConflictException (HTTP 409); repeat with the SAME ClientToken → expect the original 200 response, with still exactly one dataset in state.
5. StartDataIngestionJob with DatasetName="demo-dataset", an IngestionInputConfiguration (S3 bucket/prefix) and a RoleArn → expect 200 with a 32-hex-char JobId and Status="IN_PROGRESS".
6. DescribeDataIngestionJob with that JobId → expect 200 with the parent DatasetArn and the exact IngestionInputConfiguration and RoleArn submitted, not placeholders.
7. DescribeDataset again → expect the dataset's IngestionInputConfiguration/RoleArn now populated and Status advanced to "ACTIVE".
8. Create two more datasets, then ListDatasets with MaxResults=2 → expect 2 DatasetSummaries and a NextToken; pass that NextToken back → expect the remaining dataset(s) with no duplicates and no NextToken on the final page.
9. ListDatasets with DatasetNameBeginsWith="demo" → expect only the matching datasets.
10. DeleteDataset with DatasetName="demo-dataset" → expect 200 empty body; then DescribeDataset for it → expect ResourceNotFoundException (HTTP 404), and ListDatasets no longer returns it.
11. DescribeDataIngestionJob / DescribeDataset / DeleteDataset for a never-created name or JobId → expect ResourceNotFoundException (HTTP 404), not 501 and not an empty 200.

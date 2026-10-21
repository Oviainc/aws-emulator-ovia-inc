Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the machinelearning (Amazon ML) emulator

_Filed: 2026-10-21T21:08:43Z_
_Operation: service-com-amazonaws-machinelearning-amazonml-20141212 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 28 operations in `services/service-com-amazonaws-machinelearning-amazonml-20141212/src/ops/` is an unmodified scaffold carrying `//! SCAFFOLDED, NOT IMPLEMENTED:` and answering HTTP 501 `NotImplemented`, holding zero state. Implement a FIRST, small, stateful slice: the core Amazon ML datasource -> model lifecycle, over the `awsJson1_1` wire protocol (`POST /`, `X-Amz-Target: AmazonML_20141212.<Operation>`).

Happy path to implement, in order (fields per OPERATIONS.md):

1. **CreateDataSourceFromS3** — input `DataSourceId` (required, 1..64, `^[a-zA-Z0-9_.-]+$`), `DataSourceName`, `DataSpec` (S3DataSpec, required: `DataLocationS3`, optional `DataRearrangement`, `DataSchema`, `DataSchemaLocationS3`), `ComputeStatistics` (default false). Output: `DataSourceId`. Must persist a DataSource record; re-using an existing `DataSourceId` with different parameters must raise `IdempotentParameterMismatchException` (400), and a malformed id / non-`s3://` location must raise `InvalidInputException` (400).
2. **GetDataSource** — input `DataSourceId` (required), `Verbose` (default false). Output must be computed from the stored record: `DataSourceId`, `DataLocationS3`, `DataRearrangement`, `Name`, `Status` (EntityStatus; `COMPLETED` once settled), `CreatedAt`, `LastUpdatedAt`, `CreatedByIamUser`, `ComputeStatistics`, `DataSizeInBytes`, `NumberOfFiles`, `StartedAt`/`FinishedAt`/`ComputeTime`, and `DataSourceSchema` only when `Verbose=true`. Unknown id -> `ResourceNotFoundException` (404).
3. **CreateMLModel** — input `MLModelId` (required), `MLModelName`, `MLModelType` (BINARY|MULTICLASS|REGRESSION, required), `TrainingDataSourceId` (required), optional `Parameters`, `Recipe`, `RecipeUri`. Output: `MLModelId`. Must persist the model and reject an unknown/deleted `TrainingDataSourceId` with `InvalidInputException`; duplicate id with differing args -> `IdempotentParameterMismatchException`.
4. **GetMLModel** — input `MLModelId` (required), `Verbose` (default false). Output from stored state: `MLModelId`, `TrainingDataSourceId`, `Name`, `Status`, `MLModelType`, `TrainingParameters` (the stored `Parameters`), `InputDataLocationS3` (resolved from the training datasource's `DataLocationS3`), `ScoreThreshold` + `ScoreThresholdLastUpdatedAt`, `SizeInBytes`, `CreatedAt`/`LastUpdatedAt`/`CreatedByIamUser`, `EndpointInfo`, and `Recipe`/`Schema` only when `Verbose=true`. Unknown id -> `ResourceNotFoundException` (404).
5. **DescribeMLModels** — must list the models actually created, honouring `Limit` (1..100), `NextToken` (opaque continuation that resumes where the previous page stopped), `SortOrder` (asc|dsc on `CreatedAt`), and at minimum `FilterVariable` = `Name`/`Status`/`TrainingDataSourceId` with `EQ`/`Prefix`. Output `Results` (list<MLModel>) + `NextToken` only when more remain.
6. **DeleteMLModel** — input `MLModelId` (required). Output: `MLModelId`. Must mark the stored model `DELETED` (Amazon ML soft-deletes: subsequent `GetMLModel` returns Status=DELETED, and the model drops out of `DescribeMLModels` default results); unknown id -> `ResourceNotFoundException` (404).

Every one of these must compute its response from persisted state — no canned bodies, no echoing the input back, no generating an id without storing a record. A create followed by a get/describe must return exactly what was created, and after delete the status must reflect the deletion.

## State Model

In `src/state.rs` (`ComAmazonawsMachinelearningAmazonml20141212State`, reached via `crate::state::service_state(&*self.engine)` and its lock guard):

- `data_sources: BTreeMap<String /*DataSourceId*/, DataSource>` where `DataSource { id, name: Option<String>, data_location_s3: String, data_rearrangement: Option<String>, data_schema: Option<String>, compute_statistics: bool, role_arn: Option<String>, status: EntityStatus, created_at, last_updated_at, started_at, finished_at, compute_time_ms, data_size_in_bytes, number_of_files, message: Option<String>, created_by_iam_user, request_fingerprint: String /* for idempotency mismatch detection */ }`
- `ml_models: BTreeMap<String /*MLModelId*/, MlModel>` where `MlModel { id, name: Option<String>, model_type: MLModelType, training_data_source_id: String, training_parameters: BTreeMap<String,String>, recipe: Option<String>, schema: Option<String>, score_threshold: Option<f32>, score_threshold_last_updated_at: Option<Timestamp>, status: EntityStatus, size_in_bytes, created_at, last_updated_at, started_at, finished_at, compute_time_ms, endpoint_info: Option<RealtimeEndpointInfo>, created_by_iam_user, request_fingerprint: String }`
- Reserve (unused by this slice, filled by later issues): `evaluations: BTreeMap<String, Evaluation>`, `batch_predictions: BTreeMap<String, BatchPrediction>`, `tags: BTreeMap<(TaggableResourceType, String), BTreeMap<String,String>>`.

Pagination tokens for `DescribeMLModels` should be derived from the stable `BTreeMap` key ordering (e.g. an opaque encoding of the last returned id) so a token handed back resumes deterministically.

The remaining 22 operations (CreateDataSourceFromRDS/Redshift, CreateEvaluation, GetEvaluation, DescribeEvaluations, UpdateEvaluation, DeleteEvaluation, CreateBatchPrediction, GetBatchPrediction, DescribeBatchPredictions, UpdateBatchPrediction, DeleteBatchPrediction, DescribeDataSources, UpdateDataSource, DeleteDataSource, UpdateMLModel, CreateRealtimeEndpoint, DeleteRealtimeEndpoint, Predict, AddTags, DeleteTags, DescribeTags) stay at 501 and are tracked as individual gap issues later.

## Evidence

OPERATIONS.md at the crate root lists all 28 operations with their input/output members and errors (protocol `awsJson1_1`, target prefix `AmazonML_20141212`); the fields and error names quoted above are taken verbatim from it (CreateDataSourceFromS3 / GetDataSource / CreateMLModel / GetMLModel / DescribeMLModels / DeleteMLModel sections, including the `@paginated(inputToken=NextToken, outputToken=NextToken, items=Results)` trait and `Limit` range 1..100). `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-machinelearning-amazonml-20141212/src/ops/` matches all 28 handler files, proving no operation is implemented and no state exists. Soft-delete semantics (EntityStatus DELETED) and the idempotency-mismatch error are grounded in the modeled `EntityStatus` enum value `DELETED` and the `IdempotentParameterMismatchException` declared on every Create* operation.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `machinelearning` client at it.
2. CreateDataSourceFromS3 with DataSourceId=ds-1, DataSourceName='training data', DataSpec={DataLocationS3:'s3://bucket/train.csv'}, ComputeStatistics=true -> expect 200 with DataSourceId=ds-1 (currently HTTP 501 NotImplemented).
3. GetDataSource DataSourceId=ds-1, Verbose=true -> expect the persisted Name, DataLocationS3, ComputeStatistics=true, Status, CreatedAt; not a zeroed/echoed body.
4. GetDataSource DataSourceId=ds-missing -> expect ResourceNotFoundException (HTTP 404).
5. CreateMLModel with MLModelId=ml-1, MLModelName='demo', MLModelType=BINARY, TrainingDataSourceId=ds-1 -> expect 200 with MLModelId=ml-1.
6. CreateMLModel with TrainingDataSourceId=ds-missing -> expect InvalidInputException (HTTP 400).
7. GetMLModel MLModelId=ml-1, Verbose=false -> expect MLModelType=BINARY, TrainingDataSourceId=ds-1, InputDataLocationS3='s3://bucket/train.csv', Name='demo', and no Recipe/Schema.
8. DescribeMLModels with Limit=1 after creating ml-1 and ml-2 -> expect exactly one result plus a NextToken; passing that NextToken returns the other model and no NextToken.
9. DeleteMLModel MLModelId=ml-1 -> expect 200 with MLModelId=ml-1; then GetMLModel ml-1 shows Status=DELETED and DescribeMLModels no longer lists it.
10. DeleteMLModel MLModelId=ml-missing -> expect ResourceNotFoundException (HTTP 404).

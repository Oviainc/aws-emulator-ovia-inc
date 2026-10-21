Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the MigrationHubStrategy emulator

_Filed: 2026-10-21T22:08:01Z_
_Operation: service-com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 22 operations of this restJson1 service (`migrationhub-strategy`) are unmodified scaffolds: every file in `src/ops/` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and returns `Scaffolded::not_implemented(..)` (HTTP 501), and `src/state.rs` holds `ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState { /* TODO: add service-specific state fields */ }` — zero state. No client workflow can run.

Implement a SMALL first slice: the import-then-assess core loop of the service's primary resources (import file task + assessment), statefully, end to end. Six operations, in this order:

1. **StartImportFileTask** (`POST /start-import-file-task`) — input `name` (required), `S3Bucket` (required), `s3key` (required), optional `dataSourceType`, `groupId`, `s3bucketForReportData`. Must mint a fresh task id and PERSIST a task record; returns `id`.
2. **GetImportFileTask** (`GET /get-import-file-task/{id}`) — must look the id up in state and return the values that were stored by StartImportFileTask: `id`, `status` (an `ImportFileTaskStatus`, e.g. `ImportInProgress` then `ImportSuccess`), `startTime`, `inputS3Bucket` (= `S3Bucket`), `inputS3Key` (= `s3key`), `importName` (= `name`), plus `completionTime`/`numberOfRecordsSuccess`/`numberOfRecordsFailed` once complete. An unknown id must be `ResourceNotFoundException` (HTTP 404), not a 200 with empty fields.
3. **StartAssessment** (`POST /start-assessment`) — optional `s3bucketForAnalysisData`, `s3bucketForReportData`, `assessmentTargets` (list<AssessmentTarget>, max 10), `assessmentDataSourceType`. Must mint and PERSIST an assessment id (matching `[0-9a-z-:]+`, len <=52) and record it as the latest; returns `assessmentId`.
4. **GetAssessment** (`GET /get-assessment/{id}`) — must return the stored assessment: `id`, the `assessmentTargets` exactly as supplied to StartAssessment, and a `dataCollectionDetails` computed from state (`status`, `servers`, `success`, `failed`, `inProgress`, `startTime`, `completionTime`, `statusMessage`). Unknown id => `ResourceNotFoundException` (404).
5. **GetLatestAssessmentId** (`GET /get-latest-assessment-id`) — must return the `id` of the most recently started assessment from state (and a modelled error, not a fabricated id, when none exists).
6. **StopAssessment** (`POST /stop-assessment`) — input `assessmentId` (required). Must mutate the stored assessment's status to a stopped/terminal `AssessmentStatus` so a following GetAssessment observes the change; empty output body. Unknown/invalid id => `ValidationException` (400).

Every one of these must compute its response from the persisted record — no canned bodies, no echoing the request back, no minting an id that is never stored. A `StartImportFileTask` followed by `GetImportFileTask` must return what was created; a `StartAssessment` followed by `GetLatestAssessmentId` + `GetAssessment` + `StopAssessment` + `GetAssessment` must show the status transition.

## State Model

Add to `ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState` (reached via `crate::state::service_state(&*self.engine)`, mutated under `state.lock()`):

- `import_tasks: HashMap<String, ImportFileTask>` keyed by task id, where `ImportFileTask { id: String, import_name: String, input_s3_bucket: String, input_s3_key: String, data_source_type: Option<DataSourceType>, group_id: Vec<Group>, status_report_s3_bucket: Option<String>, status_report_s3_key: Option<String>, status: ImportFileTaskStatus, start_time: SystemTime, completion_time: Option<SystemTime>, number_of_records_success: i32, number_of_records_failed: i32 }` — the single source for both GetImportFileTask and (later) ListImportFileTask.
- `assessments: HashMap<String, Assessment>` keyed by assessment id, where `Assessment { id: String, status: AssessmentStatus, assessment_targets: Vec<AssessmentTarget>, assessment_data_source_type: Option<AssessmentDataSourceType>, s3bucket_for_analysis_data: Option<String>, s3bucket_for_report_data: Option<String>, start_time: SystemTime, completion_time: Option<SystemTime>, status_message: Option<String>, servers: i32, success: i32, failed: i32, in_progress: i32 }` — `dataCollectionDetails` is derived from these counters, never hardcoded.
- `latest_assessment_id: Option<String>` — set by StartAssessment, read by GetLatestAssessmentId.
- `next_id: u64` (or a UUID helper) so ids are unique per call and used as the map keys actually stored.

Done = a client script can run the 6 calls above against the emulator and observe its own data coming back, with 404/400 for unknown ids. The other 16 operations (portfolio preferences/summary, servers, application components, collectors, recommendation report, list/update ops) stay 501 and are filed as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) documents all 22 operations with their routes, inputs, outputs and errors: StartImportFileTask (`POST /start-import-file-task`, required `name`/`S3Bucket`/`s3key` -> `id`), GetImportFileTask (`GET /get-import-file-task/{id}` -> id/status/startTime/inputS3Bucket/inputS3Key/importName/..., `ResourceNotFoundException` 404), StartAssessment (`POST /start-assessment` -> `assessmentId`), GetAssessment (`GET /get-assessment/{id}` -> id/dataCollectionDetails/assessmentTargets, 404), GetLatestAssessmentId (`GET /get-latest-assessment-id` -> `id`), StopAssessment (`POST /stop-assessment`, required `assessmentId`, `ValidationException` 400). Shape detail from the Smithy model: `DataCollectionDetails{status,servers,failed,success,inProgress,startTime,completionTime,statusMessage}`, `ImportFileTaskInformation{id,status,startTime,inputS3Bucket,inputS3Key,statusReportS3Bucket,statusReportS3Key,completionTime,numberOfRecordsSuccess,numberOfRecordsFailed,importName}`, `AssessmentTarget{condition,name,values}` (all required). Proof of the all-stub state: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 22 handler files, and `src/state.rs` line 8 is still `// TODO: add service-specific state fields`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `migrationhub-strategy` at it.
2. Call StartImportFileTask with name='import-1', S3Bucket='my-bucket', s3key='servers.csv' — expect 200 with a non-empty `id`; today: HTTP 501 NotImplemented.
3. Call GetImportFileTask with that id — expect 200 echoing the PERSISTED record: importName='import-1', inputS3Bucket='my-bucket', inputS3Key='servers.csv', a populated `status` and `startTime`.
4. Call GetImportFileTask with id='does-not-exist' — expect ResourceNotFoundException (HTTP 404).
5. Call StartAssessment with assessmentTargets=[{condition:'EQUALS', name:'OS_NAME', values:['Linux']}] — expect 200 with `assessmentId`.
6. Call GetLatestAssessmentId — expect the same id just returned by StartAssessment.
7. Call GetAssessment with that id — expect the stored assessmentTargets returned verbatim plus a dataCollectionDetails computed from state (status/servers/success/failed/inProgress).
8. Call StopAssessment with that assessmentId, then GetAssessment again — expect dataCollectionDetails.status to reflect the stop (terminal/stopped), proving the mutation persisted.
9. Call StopAssessment with an unknown assessmentId — expect ValidationException (HTTP 400).

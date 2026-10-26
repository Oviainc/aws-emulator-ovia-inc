Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: a32383ef1b0ca99666925af11391d8042f80bab1

# Export job never writes the recommendations CSV or its metadata JSON into the destination S3 bucket

_Filed: 2026-10-24T10:53:32Z_
_Operation: ExportIdleRecommendations | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `a32383ef1b0ca99666925af11391d8042f80bab1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `ExportIdleRecommendations` queues an asynchronous job that writes the idle-recommendation export to the destination bucket — 'Exports idle resource recommendations ... as a comma-separated values (CSV) file, and its metadata in a JavaScript Object Notation (JSON) file, to an existing Amazon Simple Storage Service (Amazon S3) bucket that you specify'. Once `DescribeRecommendationExportJobs` reports the job as `Complete`, the two objects named by the job's `s3Destination.key` and `s3Destination.metadataKey` are readable from that bucket with `s3:GetObject`, and the CSV carries one row per exported recommendation with the columns `fieldsToExport` asked for. The destination is also resolved: Compute Optimizer does not create the bucket, so a bucket that does not exist (or that does not grant the service write access) is refused rather than reported as a successful export.

Actual: the handler records a job record in Compute Optimizer's own store and answers `jobId` + `s3Destination`, and nothing else ever happens. It never touches the emulated S3 service (`service-com-amazonaws-s3-amazons3`), so no object is created at either key; the job nevertheless transitions Queued → InProgress → Complete on the clock (`ExportJob::status_at`, `EXPORT_JOB_STEP` = 250 ms) and `DescribeRecommendationExportJobs` reports `status: Complete` with `failureReason` absent. A client that follows the documented workflow (export → poll to Complete → GetObject the returned key) gets 404 NoSuchKey from S3. The bucket is only checked for being non-blank, so an export against a bucket that does not exist in the emulated S3 also reports Complete. `fieldsToExport` and `fileFormat` are dropped entirely, which is unobservable only because no file is produced at all.

## Evidence

Spec grounding: the operation's own modeled documentation (reproduced in `src/ops/export_idle_recommendations.rs` lines 53 and 86-100) states the export is written as a CSV plus a JSON metadata file to an existing S3 bucket the caller specifies, and that 'You must create the destination Amazon S3 bucket ... Compute Optimizer does not create the S3 bucket for you'; the output `s3Destination` is modeled as 'the Amazon S3 bucket name and object keys of a recommendations export file'. The export artifact is the entire point of the operation.

Handler: `src/ops/export_idle_recommendations.rs` `handle()` lines 192-336 — validates accountIds/filters/bucket, mints a job id and keys via `crate::state::export_destination_keys`, inserts `crate::state::ExportJob` and returns; there is no S3 access anywhere. `src/state.rs` lines 3345-3347 states the intent explicitly: 'What the job does NOT hold is the exported file: the destination is a bucket in the CALLER's account, which this emulator does not serve' — but this emulator DOES serve S3, and this very crate already reads other services' state cross-service (`ecs::state::service_state` and `cloudwatch::state::service_state` in `src/ops/get_ecs_service_recommendations.rs` lines 340-351), so the destination is reachable.

Test coverage status: `tests/all/recommendation_export_jobs.rs` asserts only the job record and the derived destination keys (lines 109-180, 460-470); no test asserts an object exists in S3, and none resolves the bucket. Precedent for this gap class in this repo: 20261024T084458Z-bcm-data-exports-createexport-event-propagation.md and 20261024T084525Z-backupsearch-startsearchresultexportjob-event-propagation.md.

## Steps to Reproduce

1. Create a bucket in the emulated S3: `CreateBucket(Bucket="co-exports")`.
2. Opt the account in: `UpdateEnrollmentStatus(status="Active")`.
3. Call `ExportIdleRecommendations` with `{"s3DestinationConfig": {"bucket": "co-exports", "keyPrefix": "quarterly"}, "fileFormat": "Csv"}` and keep the returned `jobId`, `s3Destination.key` and `s3Destination.metadataKey`.
4. Poll `DescribeRecommendationExportJobs(jobIds=[jobId])` until `status` is `Complete`.
5. Call S3 `GetObject(Bucket="co-exports", Key=<s3Destination.key>)` and then for `<metadataKey>`: both answer 404 NoSuchKey, where real AWS serves the exported CSV and its metadata JSON.
6. Repeat the export naming a bucket that was never created (`{"bucket": "no-such-bucket"}`): the call still succeeds and the job still reaches `Complete`, where real AWS refuses the export (or fails the job) because the destination bucket does not exist / grants the service no write access.

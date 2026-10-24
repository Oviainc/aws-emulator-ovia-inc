Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0788135e0bb0704427b37eee4190850f5fa33c6

# A completed import file task never reports statusReportS3Key and writes no status report object to the report bucket

_Filed: 2026-10-24T12:26:25Z_
_Operation: GetImportFileTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0788135e0bb0704427b37eee4190850f5fa33c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetImportFileTask reports where the import's status report was written: statusReportS3Bucket "The S3 bucket name for status report of import task" and statusReportS3Key "The Amazon S3 key name for status report of import task. The report contains details about whether each record imported successfully or why it did not." In AWS, an import started with s3bucketForReportData reaches a terminal ImportFileTaskStatus with BOTH members populated, and the named object exists in that bucket, which is how a caller inspects per-record failures (the companion of numberOfRecordsFailed).

Expected: after StartImportFileTask(..., s3bucketForReportData="migrationhub-strategy-x") settles to ImportSuccess (or ImportPartialSuccess/ImportFailed), GetImportFileTask reports statusReportS3Bucket AND a statusReportS3Key naming the status report object the service wrote, and the same pair is reported by ListImportFileTask's taskInfos entry.

Actual: the task is created with status_report_s3_key: None and nothing ever sets it, so statusReportS3Key is permanently absent for every task, in every status, even when the caller named a report bucket; statusReportS3Bucket is only the echo of the request member. No status report object is written anywhere either, so a client that follows the reported bucket to find the report has nothing to read and no way to see per-record outcomes. This is the import-side counterpart of the already-filed GetRecommendationReportDetails gap (a completed report task reporting no s3Bucket/s3Keys).

## Evidence

Smithy model: GetImportFileTaskResponse$statusReportS3Bucket (importS3Bucket) and $statusReportS3Key (importS3Key, "The Amazon S3 key name for status report of import task. The report contains details about whether each record imported successfully or why it did not."); ImportFileTaskInformation declares the same two members; StartImportFileTaskRequest$s3bucketForReportData "The S3 bucket where Strategy Recommendations uploads import results."
Handler/state: src/ops/start_import_file_task.rs:220-226 sets status_report_s3_bucket from the request and `status_report_s3_key: None` with the comment "The report object itself is never written, so there is no statusReportS3Key to report"; src/state.rs ImportFileTask::settle (lines ~176-186) flips status/completion_time only, and to_output()/to_task_information() (lines ~198-237) pass the stored None straight through, so both the single read and the list read report the member absent forever.
Tests: tests/all/import_and_assess.rs:183,304-308 assert only that statusReportS3Bucket echoes s3bucketForReportData; grep of tests/ finds no assertion about statusReportS3Key, so the permanent absence is not pinned by a test.

## Steps to Reproduce

1. POST /start-import-file-task {"name":"t1","S3Bucket":"migrationhub-strategy-in","s3key":"import.json","s3bucketForReportData":"migrationhub-strategy-rep"} -> note the id.
2. Poll GET /get-import-file-task/{id} until status != "ImportInProgress" (it becomes ImportSuccess).
3. Inspect the response: expected statusReportS3Bucket AND statusReportS3Key naming the status report object the import wrote; emulator reports statusReportS3Bucket only, statusReportS3Key absent.
4. GET /list-import-file-task -> the taskInfos entry for that id shows the same missing statusReportS3Key.
5. s3 ListObjectsV2 on migrationhub-strategy-rep -> expected the status report object; no object was written.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# Every export task ends FAILED instead of progressing to SUCCEEDED, so no export workflow can complete

_Filed: 2026-10-24T09:05:20Z_
_Operation: StartExportTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartExportTask persists a real task record, but the task fate is hardcoded to failure: the record is stored with status Initializing and settles_at = now + EXPORT_TASK_SPAN (500ms), and ExportTask::settle unconditionally moves it to ExportTaskStatus::Failed with statusReason EXPORT_NOT_PERFORMED_REASON. The emulator ExportTaskStatus enum only has Initializing and Failed, so EXPORTING and SUCCEEDED are unreachable. Expected (AWS): an accepted export against an AVAILABLE graph with a well-formed S3 destination, role and KMS key reports INITIALIZING, then EXPORTING, then the terminal SUCCEEDED, with exportTaskDetails carrying a rising progressPercentage (100 at the end) and numVerticesWritten / numEdgesWritten, and no statusReason. Actual: GetExportTask reports INITIALIZING for ~500ms then FAILED forever, progressPercentage fixed at 0, both written-element counts absent, and an emulator-authored statusReason; nothing is written to the co-hosted S3 emulator either. A client that polls an export to completion (the normal use of this API) sees a failed export it never asked for, and any branch on FAILED vs SUCCEEDED takes the error path on a request AWS would have succeeded. The terminal status should be SUCCEEDED (optionally via a transient EXPORTING) with progress and counts computed from stored state.

## Evidence

Spec: the model ExportTaskStatus enum declares CANCELLED, CANCELLING, DELETED, EXPORTING, FAILED, INITIALIZING, SUCCEEDED and ExportTaskDetails declares required progressPercentage plus optional numVerticesWritten/numEdgesWritten, i.e. a successful run is the modelled happy path. Well-established behavior: a Neptune Analytics export task that is accepted ends SUCCEEDED; FAILED is for an export that genuinely could not run. Handler/state: src/ops/start_export_task.rs lines 293-302 (status Initializing, settles_at now+EXPORT_TASK_SPAN); src/state.rs enum ExportTaskStatus lines 277-283 (only Initializing and Failed), ExportTask::settle lines 335-341 (sets Failed + EXPORT_NOT_PERFORMED_REASON, lines 257-268), ExportTask::details lines 356-364 (progress_percentage 0, counts None); src/ops/get_export_task.rs lines 174-180 reports that settled record. Test coverage: tests/all/export_task_lifecycle.rs::an_accepted_task_ends_failed_because_no_export_ran (line 501) asserts the divergent FAILED outcome, so the divergence is locked in by a test; no test asserts an export reaching SUCCEEDED. Existing open neptune-graph issues cover the export-task 404 path, the scaffold stubs, region scoping and kmsKeyIdentifier/buildNumber, none the terminal export status.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName=export-probe, provisionedMemory=16) and poll GetGraph until status AVAILABLE.
3. StartExportTask with that graphIdentifier, roleArn arn:aws:iam::123456789012:role/export-role, format CSV, destination s3://my-bucket/exports/ and a well-formed kmsKeyIdentifier: 201 with status INITIALIZING.
4. Poll GetExportTask with the returned taskId until the status stops changing.
5. Observe status FAILED with an emulator statusReason, exportTaskDetails.progressPercentage 0 and no numVerticesWritten/numEdgesWritten; expected SUCCEEDED (optionally after EXPORTING) with progressPercentage 100, written-element counts and no statusReason.

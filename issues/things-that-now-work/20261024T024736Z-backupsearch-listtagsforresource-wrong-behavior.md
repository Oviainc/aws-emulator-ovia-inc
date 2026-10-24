Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# Tag APIs resolve only search-job ARNs, so an export job's tags are unreadable and untaggable (404)

_Filed: 2026-10-24T02:47:36Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartSearchResultExportJob accepts a `Tags` map and the emulator persists it on the export-job record (`ExportJob::tags`, src/state.rs), but none of the three tag operations can reach an export job: all of them resolve `ResourceArn` exclusively through `state::job_by_arn` / `job_by_arn_mut`, which searches only `search_jobs`.

Expected (real service): an export job is a resource of this service in its own right — `ExportJobArn` carries `@arnReference{service: CryoBackupSearchService}` and `StartSearchResultExportJob` accepts the same `TagMap` that `StartSearchJob` does — so `ListTagsForResource(arn:aws:backup-search:<region>:<acct>:export-job/<id>)` returns the tags the export was started with, `TagResource` on that ARN adds tags, and `UntagResource` removes them.

Actual (emulator): every one of those calls answers 404 ResourceNotFoundException because the ARN resolves to no search job. The consequence is that `Tags` passed to StartSearchResultExportJob is write-only — it is stored but no modelled operation can ever observe it, which from the caller's side is indistinguishable from the member being dropped.

## Evidence

Smithy model: `TagResourceRequest$ResourceArn` is documented generically ("The Amazon Resource Name (ARN) that uniquely identifies the resource. This is the resource that will have the indicated tags") and is an unconstrained `String` — it is not restricted to search jobs; `ExportJobArn` is declared with `@arnReference` to `com.amazonaws.backupsearch#CryoBackupSearchService`; `StartSearchResultExportJob` declares `Tags` (TagMap).

Handlers: src/ops/list_tags_for_resource.rs:154-170 (`state.job_by_arn(&input.resource_arn)` else ResourceNotFoundException), src/ops/tag_resource.rs:154-162 and src/ops/untag_resource.rs:159-167 (both `state.job_by_arn_mut`). The export-job store (`state.export_jobs`, src/state.rs `ExportJob { arn, tags, .. }`) is never consulted, and `state.rs` exposes no `export_job_by_arn` at all.

Test coverage: tests/all/tag_removal.rs and tests/all/search_jobs.rs exercise tags only on search-job ARNs; tests/all/export_jobs.rs asserts a started export job's persisted members but never reads its tags back over the tag APIs. No test asserts the export-ARN behaviour either way.

## Steps to Reproduce

1. StartSearchJob with SearchScope={"BackupResourceTypes":["S3"]}, then StopSearchJob and poll GetSearchJob until Status=STOPPED (an exportable status).
2. StartSearchResultExportJob(SearchJobIdentifier=<id>, ExportSpecification={"s3ExportSpecification":{"DestinationBucket":"bucket"}}, Tags={"env":"prod"}) — note the returned ExportJobArn.
3. ListTagsForResource(ResourceArn=<ExportJobArn>) — emulator answers 404 ResourceNotFoundException; expected 200 with Tags={"env":"prod"}.
4. TagResource(ResourceArn=<ExportJobArn>, Tags={"team":"data"}) and UntagResource(ResourceArn=<ExportJobArn>, TagKeys=["env"]) — both answer 404 instead of succeeding and mutating the export job's tag set.

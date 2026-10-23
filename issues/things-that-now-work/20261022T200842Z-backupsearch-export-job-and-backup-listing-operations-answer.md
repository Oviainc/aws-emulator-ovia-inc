Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backupsearch.list-search-job-backups-not-found backupsearch.list-search-result-export-jobs-lenient-filter-on-unknown-job backupsearch.start-search-result-export-job-immediately-after-start-rejected backupsearch.start-search-result-export-job-invalid-role-arn-rejected backupsearch.start-search-result-export-job-empty-export-specification-rejected
Issue-Shape-Id: com.amazonaws.backupsearch#CryoBackupSearchService
Issue-Key: backupsearch.list-search-job-backups-not-found|run#8|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: backupsearch.list-search-result-export-jobs-lenient-filter-on-unknown-job|run#7|AssertionError|assert code is None, f"expected no error (lenient filter), got code={code}"
Issue-Key: backupsearch.start-search-result-export-job-immediately-after-start-rejected|run#12|AssertionError|assert code == "ValidationException", (
Issue-Key: backupsearch.start-search-result-export-job-invalid-role-arn-rejected|run#12|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got code={code!r}"
Issue-Key: backupsearch.start-search-result-export-job-empty-export-specification-rejected|run#11|AssertionError|assert code == "InternalServerException", f"expected InternalServerException, got code={code!r}"
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# BackupSearch export-job and backup-listing operations answer NotImplemented or the wrong rejection code (5 causes)

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSearchResultExportJob

## Description
Five distinct root causes on the BackupSearch export/listing surface. An issue already exists for the unimplemented StartSearchResultExportJob / UntagResource scaffold stubs (filed automatically from the checkout for the checks the replay marked ALREADY FILED); it does NOT carry the checks or the rejection semantics below, which are separate work items. Each cause must be closed before this issue is done.

=== Cause 1 of 5: ListSearchJobBackups is an unimplemented stub ===
AWS: Smithy ListSearchJobBackups, GET /search-jobs/{SearchJobIdentifier}/backups, errors [ResourceNotFoundException (HTTP 404)]. An identifier that names no search job is rejected 404 ResourceNotFoundException; a real search job's identifier returns the paginated Results/NextToken page.
Emulator: every call answers error code "NotImplemented" ("operation ... is scaffolded but not implemented in this emulator"), for both the unknown-id path and the positive control.
Why: the operation is unusable — a client cannot enumerate the recovery points a search covered, and the 404 it is written to expect for a bad identifier never arrives, so not-found handling cannot be exercised at all.
Fix: implement the ListSearchJobBackups handler against the search-job store (not-found -> modelled ResourceNotFoundException; otherwise a page read from the job record).
Traces: backupsearch.list-search-job-backups-not-found; services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/list_search_job_backups.rs.

=== Cause 2 of 5: ListSearchResultExportJobs is an unimplemented stub, so its lenient-filter semantics are unreachable ===
AWS: Smithy ListSearchResultExportJobs, GET /export-search-jobs, errors [ResourceNotFoundException, ServiceQuotaExceededException]. Recorded behavior: the SearchJobIdentifier query member is treated purely as a FILTER — a syntactically well-formed identifier that names no search job does NOT raise ResourceNotFoundException, it returns HTTP 200 with an empty ExportJobs list, exactly like filtering by a real search job with no export jobs.
Emulator: every call answers error code "NotImplemented".
Why: a caller polling for export jobs gets a hard failure instead of an empty page, and the lenient-filter contract (empty page, no 404) cannot hold.
Fix: implement the ListSearchResultExportJobs handler, with SearchJobIdentifier applied as a filter that never 404s.
Traces: backupsearch.list-search-result-export-jobs-lenient-filter-on-unknown-job; services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/list_search_result_export_jobs.rs.

=== Cause 3 of 5: StartSearchResultExportJob does not gate on the search job's status ===
AWS: Smithy StartSearchResultExportJob, PUT /export-search-jobs, with ValidationException (HTTP 400) among the service-common errors. Recorded behavior: starting an export for a search job that has not reached a terminal state (CREATING/RUNNING, i.e. called back-to-back right after StartSearchJob) is rejected 400 ValidationException whose message names the job's current status and the statuses export permits (COMPLETED or STOPPED); no export job is created.
Emulator: the call answers error code "NotImplemented" — the handler is a scaffold stub, so the status gate does not exist.
Why: a client cannot distinguish "too early, poll and retry" from a transport/implementation fault, and the documented precondition on export (job must be COMPLETED or STOPPED) is unenforced.
Fix: implement the StartSearchResultExportJob handler including the search-job status precondition, answering the modelled ValidationException for a non-terminal job.
Traces: backupsearch.start-search-result-export-job-immediately-after-start-rejected; services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/start_search_result_export_job.rs.

=== Cause 4 of 5: a well-formed but non-assumable RoleArn is rejected with the wrong error class ===
AWS: Smithy StartSearchResultExportJob with AccessDeniedException (HTTP 403, service-common). Recorded behavior: a RoleArn that is a syntactically valid ARN but does not name an assumable IAM role is rejected 403 AccessDeniedException (the service tries to assume it and is denied; example message "Cross-account pass role is not allowed."), independent of the search job's status, and no export job is created (ListSearchResultExportJobs for the job stays empty).
Emulator: the request is rejected 400 ValidationException instead — the pass-role/assume step is never attempted.
Why: the error class is the signal a caller branches on: 403 AccessDeniedException means "fix the role/trust policy" (an IAM problem, not retryable by editing the request), while 400 ValidationException tells the caller the request itself is malformed. Clients catching AccessDeniedException around export starts do not catch the emulator's error.
Fix: the StartSearchResultExportJob handler's RoleArn handling — a well-formed ARN must reach the assume/pass-role check and fail as the modelled AccessDeniedException, not be turned away by input validation.
Traces: backupsearch.start-search-result-export-job-invalid-role-arn-rejected.

=== Cause 5 of 5: an empty ExportSpecification union is answered 400 ValidationException where AWS answers 500 InternalServerException ===
AWS: Smithy ExportSpecification is a union whose only member is s3ExportSpecification; InternalServerException is modelled (httpError 500, errorKind server, retryable). Recorded behavior only (the check records it was reproduced twice against real AWS): an ExportSpecification sent with no member set reaches the service and is rejected with a 500 InternalServerException (example message "Internal Server Error. Please try again") rather than a 400 ValidationException — the input is still hard-rejected and no export job is created.
Emulator: the empty union is rejected 400 ValidationException by the request-validation layer before the handler runs.
Why: 500 is a server fault that SDK retry policies retry and that monitoring classifies as service-side; 400 is terminal and client-side. A caller (or test) that encodes AWS's actual classification for this malformed input sees the opposite class and the opposite retry decision.
Fix: the StartSearchResultExportJob handler/validation seam for ExportSpecification — an empty union must pass request validation and be answered with the modelled InternalServerException (500), matching the recorded contract; the request must create nothing either way.
Traces: backupsearch.start-search-result-export-job-empty-export-specification-rejected.

## Evidence
Cause 1 — expected 404 ResourceNotFoundException from list_search_job_backups for an unknown SearchJobIdentifier; got error code "NotImplemented" (replay: "AssertionError: expected ResourceNotFoundException, got NotImplemented").
Cause 2 — expected HTTP 200 with ExportJobs=[] (no error) from list_search_result_export_jobs filtered by an unknown SearchJobIdentifier; got error code "NotImplemented" (replay: "AssertionError: expected no error (lenient filter), got code=NotImplemented").
Cause 3 — expected 400 ValidationException naming the non-terminal status from start_search_result_export_job called immediately after start_search_job; got error code "NotImplemented" (replay: "AssertionError: expected ValidationException (export gated on COMPLETED/STOPPED status), got code='NotImplemented'"); the stub message is "operation StartSearchResultExportJob is scaffolded but not implemented in this emulator".
Cause 4 — expected 403 AccessDeniedException for a well-formed, non-assumable RoleArn; got code='ValidationException' (replay: "AssertionError: expected AccessDeniedException, got code='ValidationException'").
Cause 5 — expected 500 InternalServerException for ExportSpecification={}; got code='ValidationException' (HTTP 400) (replay: "AssertionError: expected InternalServerException, got code='ValidationException'").

## Steps to Reproduce
Boot the emulator and point a botocore backupsearch client at it, then:
1. (Cause 1) list_search_job_backups(SearchJobIdentifier=<random uuid>) — emulator raises NotImplemented; AWS raises ResourceNotFoundException (404). Repeat with a real job's identifier: AWS returns a page, emulator still raises NotImplemented.
2. (Cause 2) list_search_result_export_jobs(SearchJobIdentifier=<random uuid>) — emulator raises NotImplemented; AWS returns 200 with ExportJobs=[].
3. (Cause 3) start_search_job(Name=..., SearchScope={"BackupResourceTypes":["S3"]}) then immediately start_search_result_export_job(SearchJobIdentifier=<id>, ExportSpecification={"s3ExportSpecification":{"DestinationBucket":<pre-created bucket>}}, RoleArn=<valid role>) — emulator raises NotImplemented; AWS raises ValidationException (400) naming the job's status.
4. (Cause 4) drive a search job to a terminal status (stop_search_job, poll get_search_job until STOPPED), then start_search_result_export_job(..., RoleArn="arn:aws:iam::<acct>:role/does-not-exist") — emulator raises ValidationException (400); AWS raises AccessDeniedException (403) and list_search_result_export_jobs(SearchJobIdentifier=<id>) stays empty.
5. (Cause 5) with the same terminal search job, start_search_result_export_job(SearchJobIdentifier=<id>, ExportSpecification={}, RoleArn=<valid role>) — emulator raises ValidationException (400); AWS raises InternalServerException (500) and creates no export job.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# A syntactically invalid job identifier is answered 404 ResourceNotFoundException instead of 400 ValidationException

_Filed: 2026-10-24T09:35:48Z_
_Operation: GetSearchResultExportJob | Class: validation_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every identifier-taking operation of this service guards the identifier's SYNTAX before any store lookup: a job id that is not a well-formed id (the service mints UUIDs — see src/state.rs `mint_identifier`, which documents "a UUID, which is also the only shape the service's own job-identifier guard accepts") is refused 400 ValidationException (recorded message example "Invalid Job Id"), while a WELL-FORMED id that names nothing is refused 404 ResourceNotFoundException.

The emulator implements only the second half: GetSearchResultExportJob looks the string up in `state.export_jobs` and answers 404 ResourceNotFoundException for anything that misses, so a malformed identifier such as "not-a-valid-uuid" is reported as a missing resource.

The same missing guard affects every other identifier-taking operation EXCEPT GetSearchJob, which is already filed: ListSearchJobResults, ListSearchJobBackups and StopSearchJob (`SearchJobIdentifier` @httpLabel) and StartSearchResultExportJob (`SearchJobIdentifier` in the body) all resolve a malformed identifier to a 404 too. Please close the guard once, service-wide, rather than per handler.

Expected: malformed identifier => 400 ValidationException; well-formed identifier naming nothing => 404 ResourceNotFoundException.
Actual: both => 404 ResourceNotFoundException with the service's fixed not-found text.

## Evidence

Spec: smithy_query(query="operation", operation="GetSearchResultExportJob") — GET /export-search-jobs/{ExportJobIdentifier}, ExportJobIdentifier is an @httpLabel @required GenericId, and the operation declares both ValidationException (httpError 400, service-common) and ResourceNotFoundException (httpError 404). Recorded AWS behavior for this classification is quoted in the open issue 20261022T215802Z-backupsearch-refusal-fidelity-on-the-wire-not-found.md, cause 1 (replay key `backupsearch|GetSearchJob|4xx->4xx|status`: expected 400/ValidationException, message "Invalid Job Id", got 404/ResourceNotFoundException) — that issue's keys and its stated fix ("the GetSearchJob handler's input validation") cover GetSearchJob ONLY. Handler: src/ops/get_search_result_export_job.rs:187-197 — the only check is `state.export_job(&input.export_job_identifier)`, whose miss builds ResourceNotFoundException; no syntax check exists anywhere in the crate (same in src/ops/list_search_job_results.rs:167, src/ops/list_search_job_backups.rs:175, src/ops/stop_search_job.rs:169, src/ops/start_search_result_export_job.rs:260). Test coverage: no test exercises a malformed identifier against these operations; tests/all/export_listing.rs only uses well-formed/minted-shaped ids.

## Steps to Reproduce

1. Boot the emulator and point a botocore backupsearch client at it.
2. get_search_result_export_job(ExportJobIdentifier="not-a-valid-uuid") — emulator answers 404 with x-amzn-errortype ResourceNotFoundException; AWS answers 400 with x-amzn-errortype ValidationException (message example "Invalid Job Id").
3. Positive control: get_search_result_export_job(ExportJobIdentifier="48434c42-40a0-4057-b04d-3b484c610e2b") (well formed, names nothing) must stay 404 ResourceNotFoundException.
4. Repeat step 2 against list_search_job_results, list_search_job_backups and stop_search_job with SearchJobIdentifier="not-a-valid-uuid", and against start_search_result_export_job with SearchJobIdentifier="not-a-valid-uuid" — all answer 404 instead of 400.

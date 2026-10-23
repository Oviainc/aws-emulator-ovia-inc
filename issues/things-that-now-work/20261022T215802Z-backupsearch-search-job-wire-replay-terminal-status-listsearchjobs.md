Issue-Source: trace-sweep-divergence
Issue-Trace: backupsearch-export-job-roundtrip
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backupsearch-export-job-roundtrip backupsearch-list-search-jobs-pagination backupsearch-list-export-jobs-filter backupsearch-stop-search-job-and-conflict backupsearch-tagging-on-search-job
Issue-Shape-Id: com.amazonaws.backupsearch#CryoBackupSearchService
Issue-Key: backupsearch|GetSearchJob|2xx->2xx|$.Status
Issue-Key: backupsearch|GetSearchJob|2xx->2xx|$.StatusMessage
Issue-Key: backupsearch|ListSearchJobs|2xx->2xx|$.SearchJobs[*].SearchJobArn
Issue-Key: backupsearch|ListSearchJobs|2xx->2xx|$.SearchJobs[*].SearchJobIdentifier
Issue-Key: backupsearch|ListSearchJobs|2xx->2xx|$.SearchJobs[*].Status
Issue-Key: backupsearch|ListSearchJobs|2xx->2xx|$.SearchJobs[*].StatusMessage
Issue-Key: backupsearch|ListSearchJobs|2xx->2xx|$.NextToken
Issue-Key: backupsearch|StartSearchJob|2xx->2xx|$.SearchJobArn
Issue-Key: backupsearch|StartSearchJob|2xx->2xx|$.SearchJobIdentifier
Issue-Key: backupsearch|StopSearchJob|2xx->4xx|$
Issue-Key: backupsearch|StopSearchJob|2xx->4xx|status
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# BackupSearch search-job wire replay: terminal status, ListSearchJobs page order, minted identifier format and stop-conflict diverge (4 causes)

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSearchJob

## Description
Four distinct root causes on the BackupSearch (sdkId BackupSearch, restJson1) search-job plane, found by replaying the recorded wire corpus (simparity-wire-aws-v1) against the emulator. Each cause must be closed before this issue is done; each names its own fixture#step below.

NOTE on overlap: the open behavioral issue 20261022T200842Z-backupsearch-search-job-lifecycle-iam-authorization-tag-api.md describes the same emulator behavior as Cause 1 and Cause 4 from the behavioral suite, but it carries NO wire replay key, so the wire fixtures below are not gated by it and are filed here.

=== Cause 1 of 4: a search job whose scope resolves to nothing settles to COMPLETED with no StatusMessage, where AWS settles it to FAILED with a scope-empty StatusMessage ===
AWS: recorded behavior only. Smithy: SearchJobState enum = [RUNNING, COMPLETED, STOPPING, STOPPED, FAILED]; GetSearchJobOutput.Status is @required, GetSearchJobOutput.StatusMessage is an optional String (SearchJobSummary.StatusMessage is documented as "A status message will be returned for ... jobs with issues"). The recording shows that a StartSearchJob whose SearchScope resolves to no recovery points is accepted 200 and then settles to Status=FAILED, carrying a StatusMessage describing the empty scope (recorded example: "Search scope is empty. Revise search scope input and retry search job"). The rule is: an empty/unmatched scope is a terminal FAILURE and the service explains it in StatusMessage.
Emulator: GetSearchJob answers HTTP 200 with Status=COMPLETED and omits StatusMessage entirely; the fixture's terminal-status poll aborted because COMPLETED is in its fail_on set after 2 attempts. ListSearchJobs reports the same wrong Status/absent StatusMessage for the job (see Cause 2's fixtures, whose Status/StatusMessage keys share this root cause).
Why: a client that waits for a terminal status and branches on it takes the success branch (read results, start an export) for a search that in fact found nothing and failed, and any alerting or retry path gated on FAILED never fires. The explanatory StatusMessage a caller would surface to an operator is never produced.
Fix: the search-job settle path in the service's state module together with the StartSearchJob handler — the owed terminal status must be derived from the resolved scope (FAILED, with a StatusMessage, when nothing is in scope) instead of being hard-wired to COMPLETED.
Traces: backupsearch-export-job-roundtrip#task#2 (representative, GetSearchJob: expected Status FAILED + StatusMessage, got COMPLETED + no StatusMessage); backupsearch-list-search-jobs-pagination#setup#3 (cluster also covers setup#4: poll aborted, COMPLETED is in fail_on).

=== Cause 2 of 4: ListSearchJobs returns its pages in the wrong order and omits NextToken on a page that has a successor ===
AWS: Smithy ListSearchJobs, GET /search-jobs, @paginated {inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults, items: SearchJobs}; MaxResults range 1..1000. Recorded behavior for the page order: with two search jobs and a page size of 1, the FIRST page carries the most recently created job (the recording's second-created job) and a NextToken, and the following page carries the older job. The requirement is newest-first ordering plus a NextToken whenever a further page exists.
Emulator: the first page carries the OLDEST job and the later page the newest (the two page bodies are swapped relative to the recording), and the page on which the recording carries a NextToken comes back with NextToken absent.
Why: two separate client breakages. The order inversion makes "the latest search job" (a very common single-page read with MaxResults=1) resolve to the wrong job. The missing NextToken makes a paging loop terminate after the current page, so the caller silently enumerates a truncated set and believes it is complete — no error is raised anywhere.
Fix: the ListSearchJobs handler — order the result set newest-first by creation time, and emit NextToken whenever the store holds more jobs beyond the page just returned.
Traces: backupsearch-list-search-jobs-pagination#task#5 (expected SearchJobs[0] to be the newer job, got the older one); backupsearch-list-search-jobs-pagination#task#6 (expected a NextToken and the older job first, got no NextToken and the newer job). The Status/StatusMessage keys that also diverge in both of these steps belong to Cause 1.

=== Cause 3 of 4: StartSearchJob mints a sequential identifier instead of the UUID-shaped identifier the service mints ===
AWS: recorded behavior only — Smithy GenericId (the target of StartSearchJobOutput.SearchJobIdentifier and of the SearchJobArn resource segment) is an unconstrained string, so the model states no format. The recordings show the service mints a UUID-shaped identifier (recorded example: 48434c42-40a0-4057-b04d-3b484c610e2b) and builds SearchJobArn as arn:aws:backup-search:<region>:<account>:search-job/<uuid>. That the format is load-bearing rather than incidental is corroborated by the service rejecting a non-UUID identifier with 400 ValidationException "Invalid Job Id" (filed as the companion refusal issue for this service). The fixtures assert the SHAPE of the identifier, not a literal recorded value.
Emulator: mints searchjob-000000000001 (a zero-padded per-process counter) and embeds it in the ARN, so both SearchJobIdentifier and SearchJobArn fail the shape assertion.
Why: a caller that validates, parses or stores the identifier as a UUID (a UUID column, a UUID-typed parameter, a client-side pre-flight check) rejects everything the emulator mints, and the emulator cannot reproduce the service's own 400 "Invalid Job Id" guard while the ids it hands out are not of the shape that guard accepts. The counter is also deterministic across runs, so two independently started jobs in different fixtures collide on id.
Fix: the StartSearchJob handler's identifier minting and the ARN it derives from it (the search-job id generator in the service's state module) — mint a UUID.
Traces: backupsearch-export-job-roundtrip#task#1 (cluster also covers backupsearch-list-export-jobs-filter#setup#1; SearchJobIdentifier and SearchJobArn both fail the shape assert); backupsearch-stop-search-job-and-conflict#setup#1 (cluster also covers backupsearch-tagging-on-search-job#setup#1).

=== Cause 4 of 4: StopSearchJob on an already-STOPPED job answers 409 ConflictException where AWS answers an empty 200 ===
AWS: Smithy StopSearchJob, PUT /search-jobs/{SearchJobIdentifier}/actions/cancel, @idempotent, empty output structure, http code 200, with ConflictException (httpError 409) modeled. Recorded behavior: a second StopSearchJob against a job that has already reached STOPPED answers HTTP 200 with an empty JSON body and leaves the job STOPPED — the repeat is a no-op success, not a conflict.
Emulator: answers HTTP 409 with __type ConflictException (recorded emulator body also carries resourceId/resourceType and the message "search job '<id>' is STOPPED and can no longer be stopped"; the status and the error code are the requirement here, the text is an example).
Why: the operation is modeled @idempotent, so SDK retry policies and caller-level retries are expected to re-send it; against the emulator a safe retry turns into a caller-visible hard failure, and idempotent "ensure stopped" logic cannot be written against it.
Fix: the StopSearchJob handler's status gate — STOPPED (and STOPPING) must short-circuit to the empty 200; only the genuinely non-stoppable terminal states should raise the modeled ConflictException.
Traces: backupsearch-stop-search-job-and-conflict#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0; 32 steps, 6 passed).

Cause 1 — backupsearch-export-job-roundtrip#task#2, GetSearchJob, expected 200 / actual 200: "$.Status: value_mismatch (expected 'FAILED', got 'COMPLETED'); $.StatusMessage: missing (expected 'Search scope is empty. Revise search scope input and retry search job', got None); $.Status: poll: 'COMPLETED' is in fail_on after 2 attempt(s)". Same root at backupsearch-list-search-jobs-pagination#setup#3 (+setup#4): "$.Status: poll: 'COMPLETED' is in fail_on after 2 attempt(s) (expected None, got 'COMPLETED')".

Cause 2 — backupsearch-list-search-jobs-pagination#task#5, ListSearchJobs, 200/200: "$.SearchJobs[0].SearchJobIdentifier: value_mismatch (expected 'searchjob-000000000002', got 'searchjob-000000000001')" (and the matching SearchJobArn). backupsearch-list-search-jobs-pagination#task#6, ListSearchJobs, 200/200: "$.NextToken: missing (expected '<shaped-not-asserted>', got None); $.SearchJobs[0].SearchJobIdentifier: value_mismatch (expected 'searchjob-000000000001', got 'searchjob-000000000002')" — i.e. the two pages are transposed and the NextToken is absent.

Cause 3 — backupsearch-export-job-roundtrip#task#1, StartSearchJob, 200/200: "$.SearchJobArn: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:backup-search:us-west-2:000000000000:search-job/searchjob-000000000001'); $.SearchJobIdentifier: value_mismatch (expected '<shaped-not-asserted>', got 'searchjob-000000000001')". Same at backupsearch-stop-search-job-and-conflict#setup#1. The recorded service identifier shape is a UUID (e.g. 48434c42-40a0-4057-b04d-3b484c610e2b, as used by the not-found fixtures).

Cause 4 — backupsearch-stop-search-job-and-conflict#task#4, StopSearchJob: "status: expected 200, got 409; $: value_mismatch (expected {}, got {'__type': 'ConflictException', 'message': \"search job 'searchjob-000000000001' is STOPPED and can no longer be stopped\", 'resourceId': 'searchjob-000000000001', 'resourceType': 'search-job'})".

## Steps to Reproduce
Boot the emulator and point a botocore backupsearch client (restJson1) at it.
1. (Cause 1, fixture backupsearch-export-job-roundtrip step task#2) start_search_job(Name=..., SearchScope={"BackupResourceTypes":["S3"]}) with nothing in scope, then poll get_search_job(SearchJobIdentifier=<id>) until Status is terminal — emulator reports Status=COMPLETED and no StatusMessage; AWS reports Status=FAILED with a StatusMessage explaining the empty scope. Re-run backupsearch-list-search-jobs-pagination#setup#3 for the same poll failure.
2. (Cause 2, fixture backupsearch-list-search-jobs-pagination steps task#5 and task#6) start two search jobs in sequence, then list_search_jobs(MaxResults=1) and follow the token — emulator returns the oldest job on the first page and no NextToken; AWS returns the newest job plus a NextToken, then the older job.
3. (Cause 3, fixtures backupsearch-export-job-roundtrip#task#1 and backupsearch-stop-search-job-and-conflict#setup#1) start_search_job(...) and inspect SearchJobIdentifier / SearchJobArn — emulator returns searchjob-000000000001 and .../search-job/searchjob-000000000001; AWS returns a UUID and .../search-job/<uuid>.
4. (Cause 4, fixture backupsearch-stop-search-job-and-conflict#task#4) start_search_job(...), stop_search_job(SearchJobIdentifier=<id>), poll get_search_job until Status=="STOPPED", then call stop_search_job again — emulator answers 409 ConflictException; AWS answers HTTP 200 with an empty body.

Issue-Source: trace-sweep-divergence
Issue-Trace: backupsearch-stop-search-job-and-conflict
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backupsearch-stop-search-job-and-conflict
Issue-Shape-Id: com.amazonaws.backupsearch#CryoBackupSearchService
Issue-Key: backupsearch|StopSearchJob|2xx->4xx|$
Issue-Key: backupsearch|StopSearchJob|2xx->4xx|status
Issue-Key: backupsearch|GetSearchJob|2xx->2xx|$.Status
Issue-Commit: 521a9a87ca33ad668d554c20273a56e6e89cc063

# BackupSearch stop-driven search-job lifecycle never reaches STOPPED on the wire

> _Note: this issue was identified against the code base at commit `521a9a87ca33ad668d554c20273a56e6e89cc063`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopSearchJob

## Description
Replaying the wire fixture backupsearch-stop-search-job-and-conflict against the booted emulator leaves the single most common BackupSearch lifecycle — start a search job, cancel it, poll it to the terminal status — broken end to end on the wire. There are 2 causes, both in the search-job lifecycle of this crate, and both must be closed before this fixture replays clean.

=== Cause 1 of 2: StopSearchJob refuses a just-started (non-terminal) search job with 409 ConflictException instead of accepting the cancel with an empty 200 ===

AWS: Smithy StopSearchJob is PUT /search-jobs/{SearchJobIdentifier}/actions/cancel, @idempotent, with explicit @http(code: 200) and output shape StopSearchJobOutput that has NO members (so success is an empty 200 body, i.e. {}); its modeled errors are ConflictException (httpError 409), ResourceNotFoundException (404), AccessDeniedException (403), ValidationException (400), ThrottlingException (429), InternalServerException (500). The operation's doc sentence ("Only a search job with a status of RUNNING can be stopped") does not state that a freshly accepted job is refused, and the recording settles it: a StopSearchJob sent straight after StartSearchJob is ACCEPTED with an empty 200, and a SECOND StopSearchJob for the same job is likewise accepted (the @idempotent repeat). The recorded 409 ConflictException on this operation is reserved for a job already in a terminal status this operation does not produce (COMPLETED/FAILED). Hypothesis: on AWS the cancel is recorded against the job record and honoured by the scan whenever it starts, so there is no window in which an accepted job cannot be cancelled.

Emulator: a job minted by StartSearchJob sits in a CREATING setup window before the clock owes it RUNNING, and the StopSearchJob handler gates on membership in a stoppable-status set holding RUNNING only, so every stop inside that window is refused 409 ConflictException with an __type/message body (recorded example message: "search job '<id>' is CREATING; only a RUNNING search job can be stopped", plus resourceId/resourceType) and nothing is written to the job. The replayed step expected status 200 and body {} and got 409 with that ConflictException body; the fixture's later repeat stop (task#4) is refused identically, so the idempotent repeat is never exercised either.

Why: a client doing exactly what AWS supports — start a job and cancel it, or re-send a safe @idempotent cancel on an SDK retry — gets a hard 409 where AWS returns an empty 200. ConflictException is a retryable-with-backoff signal, so a well-behaved client retries a call that stays refused, and any state machine that cancels and then waits for STOPPED never converges. Everything gated on a STOPPED job (notably StartSearchResultExportJob) becomes unreachable through the API.

Fix: the StopSearchJob handler's status gate and the stoppable-status set it reads — a non-terminal job (CREATING included) must accept the stop, move to STOPPING and be owed STOPPED; only COMPLETED/FAILED stay a 409, STOPPING/STOPPED keep their idempotent empty 200, and an unknown identifier keeps its 404.

Traces: backupsearch-stop-search-job-and-conflict#task#2 (representative), with backupsearch-stop-search-job-and-conflict#task#4 in the same cluster. Code: services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/stop_search_job.rs:185-217 (is_stop_satisfied / is_stoppable gate) and src/state.rs (STOPPABLE_SEARCH_JOB_STATES, STOP_SATISFIED_STATES, CREATING_DELAY). The already-open behavioral issue 20261025T052640Z-backupsearch-reopens-backupsearch-stopsearchjob-a-just-started-creating-search.md describes the same handler gate from the behavioral corpus but carries NO wire replay key, so this wire step is not covered by it; 20261022T200842Z-backupsearch-search-job-lifecycle-iam-authorization-tag-api.md and 20261023T110504Z-backupsearch-export-stop-refusals-non-iam-rolearn-misclassified.md cover other parts of this handler (the already-STOPPED repeat, the conflict-message template) and neither covers this gate.

=== Cause 2 of 2: a cancelled search job settles to the terminal status FAILED, so GetSearchJob never reports STOPPED ===

AWS: Smithy GetSearchJob is GET /search-jobs/{SearchJobIdentifier} and its output requires Status (enum SearchJobState) plus SearchScope, ItemFilters, CreationTime, SearchJobIdentifier and SearchJobArn, with optional SearchScopeSummary/CurrentSearchProgress/StatusMessage/CompletionTime. A cancel that AWS accepted is the status the job converges on: the recording polls GetSearchJob after the stop and reads Status="STOPPED" (status 200). A stop REPLACES whatever the scan would otherwise have reached, so FAILED is not an outcome of a cancelled job.

Emulator: the poll of GetSearchJob answers 200 but with $.Status="FAILED" rather than "STOPPED", and the replay's poll gives up because FAILED is a terminal fail-on value (2 attempts). Because the stop of Cause 1 wrote nothing, the job keeps the terminal status its own scan reached, and in this emulator that scan always fails: the search scope resolves against an in-crate recovery-point list nothing populates, so the job settles FAILED (StatusMessage about an empty search scope) instead of ever being STOPPED or COMPLETED. The two causes compound — opening the Cause 1 gate is necessary but not sufficient, because a job must be owed STOPPED and that owed status must survive the scan settling.

Why: a client that cancels a job and polls for the documented terminal status sees FAILED, which it must interpret as the job erroring rather than as its own cancel succeeding; retry/cleanup logic keyed on STOPPED hangs or reports a false failure, and the STOPPED precondition of the export operations is never satisfiable through the API.

Fix: the search-job state machine — the stop transition's owed terminal status must take precedence over the scan's own outcome when the job settles, and the scan outcome itself must not force FAILED on every job (the scope resolution feeding it). This is the state/settle component, not the GetSearchJob serializer, which renders Status faithfully.

Traces: backupsearch-stop-search-job-and-conflict#task#3. Code: services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs (settle, owe_only, scan_outcome, STOPPED) and src/ops/start_search_job.rs (scan outcome selection). The open issue 20261024T024736Z-backupsearch-startsearchjob-event-propagation.md describes the empty-scope FAILED outcome from the static pass and carries no wire replay key, so this step is not covered by it.

## Evidence
Wire replay of trace set simparity-wire-ga-prod-v1 against the booted emulator (emulator_up true, bootstrap_signal false): 13 steps total, 10 passed, 3 mismatched, score 76.9%, 2 divergence clusters — both on service backupsearch, both in fixture backupsearch-stop-search-job-and-conflict.

Cause 1 — fixture backupsearch-stop-search-job-and-conflict, step task#2, operation StopSearchJob (cluster also covers task#4):
  Expected (recorded AWS): status 200, body {}
  Actual (emulator): status 409, body {"__type": "ConflictException", "message": "search job '31e46d81-bdfb-40db-bdd3-75a94bb6a744' is CREATING; only a RUNNING search job can be stopped", "resourceId": "31e46d81-bdfb-40db-bdd3-75a94bb6a744", "resourceType": "search-job"}
  Replay detail: "status: expected 200, got 409 / $: value_mismatch (expected {}, got {...ConflictException...})". Divergence key service=backupsearch|StopSearchJob|status=200->409|keys=$,status.
  The requirement is the 200-with-empty-body vs 409/ConflictException difference; the job id in the message is a per-run value and the message text itself is an example.

Cause 2 — fixture backupsearch-stop-search-job-and-conflict, step task#3, operation GetSearchJob:
  Expected (recorded AWS): status 200 with $.Status == "STOPPED"
  Actual (emulator): status 200 with $.Status == "FAILED"
  Replay detail: "$.Status: value_mismatch (expected 'STOPPED', got 'FAILED') / $.Status: poll: 'FAILED' is in fail_on after 2 attempt(s)". Divergence key service=backupsearch|GetSearchJob|status=200->200|keys=$.Status.

Smithy evidence used: StopSearchJob — PUT /search-jobs/{SearchJobIdentifier}/actions/cancel, @idempotent, @http(code: 200), output StopSearchJobOutput with no members, errors ConflictException(409)/ResourceNotFoundException(404)/ValidationException(400)/AccessDeniedException(403)/ThrottlingException(429)/InternalServerException(500). GetSearchJob — GET /search-jobs/{SearchJobIdentifier}, @readonly, output requires Status (SearchJobState enum).

## Steps to Reproduce
Boot the emulator and replay fixture backupsearch-stop-search-job-and-conflict (steps task#1..task#4), or drive the same sequence with a backupsearch client:
1. StartSearchJob(Name=..., SearchScope={"BackupResourceTypes": ["S3"]}) -> note SearchJobIdentifier (fixture step task#1, passes).
2. Immediately StopSearchJob(SearchJobIdentifier=<id>) — back-to-back, i.e. inside the emulator's CREATING setup window (fixture step task#2).
   AWS/recorded: HTTP 200, empty body ({}).
   Emulator: HTTP 409 with __type="ConflictException" and a "... is CREATING; only a RUNNING search job can be stopped" message.
3. Poll GetSearchJob(SearchJobIdentifier=<id>) until the status is terminal (fixture step task#3).
   AWS/recorded: 200 with Status="STOPPED".
   Emulator: 200 with Status="FAILED".
4. Re-send StopSearchJob for the same job (fixture step task#4) — the @idempotent repeat.
   AWS/recorded: HTTP 200, empty body. Emulator: the same 409 ConflictException.
5. Regressions to keep green: StopSearchJob on a COMPLETED or FAILED job must still answer 409 ConflictException; StopSearchJob on a job already STOPPING/STOPPED must answer 200 with an empty body; StopSearchJob on an unknown identifier must answer 404 ResourceNotFoundException.

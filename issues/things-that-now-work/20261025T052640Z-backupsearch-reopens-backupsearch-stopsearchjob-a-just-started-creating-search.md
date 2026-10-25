Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backupsearch.stop-search-job-transitions-to-stopped backupsearch.stop-search-job-not-found backupsearch.stop-search-job-idempotent-when-already-stopped backupsearch.concurrent-stop-search-job-race-idempotent-convergence backupsearch.get-search-result-export-job-not-found backupsearch.start-search-result-export-job-empty-export-specification-rejected
Issue-Shape-Id: com.amazonaws.backupsearch#CryoBackupSearchService
Issue-Key: backupsearch|StopSearchJob|ConflictException
Issue-Key: backupsearch.concurrent-stop-search-job-race-idempotent-convergence|run#9|AssertionError|assert len(winners) == 2, f"expected both concurrent stop calls to succeed (idempotent), got {outcomes}"
Issue-Commit: 2ff8f028b92e5130e7862c7e8e8ba699176bff42

# REOPENS backupsearch StopSearchJob: a just-started (CREATING) search job is refused 409 ConflictException, so no stop-driven lifecycle works (1 cause)

> _Note: this issue was identified against the code base at commit `2ff8f028b92e5130e7862c7e8e8ba699176bff42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopSearchJob

## Description
REOPENS 20261024T230241Z-backupsearch-stopsearchjob-wrong-behavior.md: its fix left the call refused.

That issue was reasoned from the operation's doc sentence alone and asked the handler to refuse a stop on a job in CREATING. The fix landed (src/state.rs now declares STOPPABLE_SEARCH_JOB_STATES = [RUNNING] and stop_search_job.rs refuses everything else with ConflictException), and it is exactly what now refuses the normal AWS call pattern: a StopSearchJob sent straight after StartSearchJob. Six behavioral checks replayed from real-AWS recordings now fail on that refusal, five of them only as a setup step on the way to something else. This is one root cause.

=== Cause 1 of 1: StopSearchJob refuses a non-terminal search job that has not yet been handed RUNNING, instead of accepting the stop ===

AWS: Smithy StopSearchJob (PUT /search-jobs/{SearchJobIdentifier}/actions/cancel, @idempotent, output StopSearchJobOutput with no members, explicit @http(code: 200)) declares ConflictException (httpError 409) and ResourceNotFoundException (httpError 404); its documentation reads "This operations ends a search job. Only a search job with a status of RUNNING can be stopped." That prose does NOT say a freshly created job is refused, and recorded behavior is what settles it: the recordings show StartSearchJob immediately followed by StopSearchJob is ACCEPTED (empty 200) and the job then converges on the terminal status STOPPED, observable through GetSearchJob — the documented RUNNING -> STOPPING -> STOPPED walk is driven from the caller's point of view by a stop issued at any point before the scan reaches a terminal status. The recordings also show the shape of the refusals this operation does make: a 409 ConflictException only for a job in a terminal status this operation does not produce (example message "SearchJob can not be stopped since it is in FAILED state. Please try again later"), and a 404 ResourceNotFoundException for an identifier naming no job. Hypothesis: on AWS the set-up phase is part of the stoppable window because a cancel is recorded against the job record and honored by the scan when it starts; the service does not expose a window in which a job it has accepted cannot be cancelled.

Emulator: a job minted by StartSearchJob sits in CREATING for CREATING_DELAY (src/state.rs, 250 ms) before the clock owes it RUNNING. Within that window StopSearchJob answers 409 ConflictException (example message "search job 'ed1b4c0f-9d47-4b34-8ab8-06577d67d660' is CREATING; only a RUNNING search job can be stopped") and writes nothing, because job.is_stoppable() tests membership in STOPPABLE_SEARCH_JOB_STATES = [RUNNING]. Since a client issuing the two calls back to back is normally inside 250 ms, the refusal is the common case, not an edge: it hit 6 of 17 checks in this replay. Two concurrent stops on the same just-started job both get the same 409 (so the job never converges on STOPPED at all), and every check that merely needs a search job driven to STOPPED as a precondition (export-job reads, export-specification validation) dies on the setup call.

Why: a client that does what AWS supports — start a search job and cancel it (an "ensure stopped" path, a user pressing cancel, a cleanup handler, an SDK retry of a safe @idempotent call) — gets a hard 409 instead of the empty 200 AWS returns, and the job is left to run to its own terminal status. Worse, ConflictException is documented as retryable with backoff, so a well-behaved client retries a call the emulator will keep refusing until the clock happens to put the job in RUNNING, and any state machine that waits for STOPPED after cancelling never converges. Downstream, nothing that requires a STOPPED job (notably StartSearchResultExportJob, which is gated on COMPLETED/STOPPED) can be reached at all.

Fix: the StopSearchJob handler's status gate and the stoppable-status set it reads — a non-terminal job (CREATING included) must accept the stop, move to STOPPING and be owed STOPPED, so only the terminal statuses this operation does not produce (COMPLETED, FAILED) remain a 409 and STOPPING/STOPPED keep their idempotent empty 200. The accompanying unit row that pins the CREATING refusal moves with it. Note that the already-filed open issue 20261022T200842Z-backupsearch-search-job-lifecycle-iam-authorization-tag-api.md (Cause 2, the already-STOPPED idempotent repeat) and 20261023T110504Z-backupsearch-export-stop-refusals-non-iam-rolearn-misclassified.md (Cause 3, the conflict-message template) cover other parts of this handler and neither covers the CREATING gate; backupsearch.stop-search-job-idempotent-when-already-stopped and backupsearch.concurrent-stop-search-job-race-idempotent-convergence also traverse that idempotency behavior after this gate is opened.

Traces: backupsearch.stop-search-job-transitions-to-stopped, backupsearch.stop-search-job-not-found, backupsearch.stop-search-job-idempotent-when-already-stopped, backupsearch.concurrent-stop-search-job-race-idempotent-convergence, backupsearch.get-search-result-export-job-not-found, backupsearch.start-search-result-export-job-empty-export-specification-rejected; code at services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/stop_search_job.rs:189-206 (the is_stoppable gate) and src/state.rs:141-192 (STOP_SATISFIED_STATES, STOPPABLE_SEARCH_JOB_STATES, CREATING_DELAY).

## Evidence
Behavioral replay against the booted emulator: 11/17 passed, bootstrap_signal false; backupsearch 11/17, all 6 failures on this cause.

Expected (recorded AWS): StopSearchJob issued immediately after StartSearchJob returns HTTP 200 with an empty body, and GetSearchJob then reports Status="STOPPED".
Actual (emulator): HTTP 409, Error.Code="ConflictException".

- backupsearch.stop-search-job-transitions-to-stopped [error]: "botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the StopSearchJob operation: search job '48d95d9b-1a89-402e-97de-135127f745df' is CREATING; only a RUNNING search job can be stopped" — raised by the single stop the check makes, so the RUNNING -> STOPPING -> STOPPED walk is never observed.
- backupsearch.stop-search-job-not-found [error]: the negative half (unknown identifier -> 404 ResourceNotFoundException) passed; the in-file positive control then failed with the same ConflictException on '4b7f8959-59bb-4ef7-950e-5f563d4b31d6'.
- backupsearch.stop-search-job-idempotent-when-already-stopped [error]: the FIRST stop (meant to drive the job to STOPPED) raised ConflictException on '0ed71c69-53a8-4a2a-acfd-8e2e28d0ec67', so the idempotent-repeat assertion was never reached.
- backupsearch.concurrent-stop-search-job-race-idempotent-convergence [mismatch]: "AssertionError: expected both concurrent stop calls to succeed (idempotent), got [('error', 'ConflictException'), ('error', 'ConflictException')]" — both racing stops were refused, so the job converged on no stop at all.
- backupsearch.get-search-result-export-job-not-found [error]: setup stop on 'ed1b4c0f-9d47-4b34-8ab8-06577d67d660' raised ConflictException ("... is CREATING; only a RUNNING search job can be stopped"), so the export-job reads under test never ran.
- backupsearch.start-search-result-export-job-empty-export-specification-rejected [error]: setup stop on 'ed78078c-03be-4562-a0d4-9efdc3cf5b03' raised the same ConflictException.

Identifiers, hosts and timestamps above are per-run examples; the requirement is the 200-vs-409 and the ConflictException code on a stop of a non-terminal job.

## Steps to Reproduce
Boot the emulator and point a botocore backupsearch client at it, then:
1. resp = start_search_job(Name="stop-gate-probe", SearchScope={"BackupResourceTypes": ["S3"]}); job_id = resp["SearchJobIdentifier"].
2. Immediately (within src/state.rs CREATING_DELAY = 250 ms, i.e. what a back-to-back client call does) call stop_search_job(SearchJobIdentifier=job_id).
   - Emulator: ClientError with Error.Code == "ConflictException" (HTTP 409), message "search job '<id>' is CREATING; only a RUNNING search job can be stopped".
   - AWS: HTTP 200 with an empty body.
3. Poll get_search_job(SearchJobIdentifier=job_id) until Status is terminal: on AWS it is "STOPPED"; on the emulator the job was never stopped.
4. Concurrency variant: start a job and fire two stop_search_job calls for it from two threads — AWS answers 200 to both and the job converges on STOPPED; the emulator refuses both with ConflictException.
5. Regressions to keep green while fixing: a stop of a job already STOPPED or STOPPING must still answer 200 (the @idempotent repeat), a stop of a COMPLETED or FAILED job must still answer 409 ConflictException, and a stop of an identifier naming no job must still answer 404 ResourceNotFoundException.

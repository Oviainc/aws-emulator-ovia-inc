Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-serverless.create-application-conflicting-client-token-rejected emr-serverless.create-application-same-client-token-conflict-rejected emr-serverless.delete-application-removes-from-list emr-serverless.create-application-invalid-engine-type-rejected emr-serverless.start-stop-application-lifecycle emr-serverless.delete-application-while-started-rejected emr-serverless.cancel-terminal-job-run-rejected emr-serverless.cancel-job-run-on-terminal-state-rejected emr-serverless.list-job-runs-filters-by-state emr-serverless.get-dashboard-for-job-run-returns-url emr-serverless.auto-stop-configuration-stops-idle-application emr-serverless.create-application-invalid-release-label-rejected emr-serverless.get-job-run-invalid-attempt-not-found
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless.create-application-conflicting-client-token-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: emr-serverless.create-application-same-client-token-conflict-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: emr-serverless.delete-application-removes-from-list|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: emr-serverless.create-application-invalid-engine-type-rejected|run#16|AssertionError|assert ok_id not in live_ids, (
Issue-Key: emr-serverless.start-stop-application-lifecycle|_wait_for_absent_from_list#15|Timeout|time.sleep(interval_s)
Issue-Key: emr-serverless.delete-application-while-started-rejected|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: emr-serverless|CancelJobRun|ValidationException
Issue-Key: emr-serverless.cancel-job-run-on-terminal-state-rejected|run#15|AssertionError|assert message == "Job run is not in a cancellable state", (
Issue-Key: emr-serverless.list-job-runs-filters-by-state|run#17|AssertionError|assert job_b in non_terminal_ids, f"expected non-terminal job {job_b} in the non-terminal-states listing, got {non_terminal_ids}"
Issue-Key: emr-serverless.get-dashboard-for-job-run-returns-url|_wait_for_dashboard_url#11|Timeout|_time.sleep(interval_s)
Issue-Key: emr-serverless.auto-stop-configuration-stops-idle-application|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: emr-serverless.create-application-invalid-release-label-rejected|run#6|AssertionError|assert message == "Type is not supported for the release label or release label does not exist", (
Issue-Key: emr-serverless.get-job-run-invalid-attempt-not-found|run#12|AssertionError|assert message == "Job Run with given attempt doesn't exist", (
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# emr-serverless control plane: 8 divergences in idempotency conflict, list filtering, cancel/auto-stop state machine, dashboard and not-found messages

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication, ListApplications, CancelJobRun, ListJobRuns, GetDashboardForJobRun, GetJobRun

## Description
Eight distinct root causes in the emr-serverless emulator, all grounded in failing behavioral checks of this suite. Each must be closed before submitting.

=== Cause 1 of 8: CreateApplication accepts a clientToken replay whose parameters differ, instead of ConflictException ===
AWS: Smithy: CreateApplication.clientToken is @idempotencyToken @required on an @idempotent operation, and the operation models ConflictException (HTTP 409). Replaying the token with the IDENTICAL request is idempotent (same applicationId); replaying it with DIFFERENT parameters is a conflict and is refused with 409 ConflictException (recorded message example: "Request did not match the original"). No second application is created and the first is untouched.
Emulator: the second CreateApplication, same clientToken but a different name, returns HTTP 200 echoing the FIRST application: {'applicationId': '0000000000000001', 'name': '<name_a>', 'arn': 'arn:aws:emr-serverless:...:/applications/0000000000000001'} — i.e. the token lookup short-circuits without comparing the stored request to the new one.
Why: a client that mutates a parameter and retries under the same token silently gets the old resource back and believes its new parameters were applied; the conflict AWS raises (and that clients branch on) never arrives, so divergent state goes undetected.
Fix: CreateApplication's handler/validation (services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/create_application.rs) must store the request fingerprint alongside the clientToken and answer ConflictException (409) when a replay's parameters differ, 200 with the same applicationId only when they are identical.
Traces: emr-serverless.create-application-conflicting-client-token-rejected, emr-serverless.create-application-same-client-token-conflict-rejected.

=== Cause 2 of 8: ListApplications still returns deleted (TERMINATED) applications ===
AWS: recorded behavior only (the Smithy model states no default state filter for ListApplications). DeleteApplication is a soft delete: GetApplication keeps answering with state TERMINATED, but the id DROPS OUT of ListApplications when no `states` filter is supplied. Four checks in this suite assert exactly that, one of them annotating it as confirmed against real AWS.
Emulator: after DeleteApplication the record is retained in TERMINATED (state.rs ApplicationRecord::terminate) and ListApplications walks every stored application, filtering only when `states` is present, so the deleted id is still listed: "deleted application '0000000000000001' is still present in ListApplications: {'0000000000000001'}". Three further checks poll for list-absence and never see it (each killed by the 60 s check budget while polling).
Why: list-absence is the only way a client can tell a soft-deleted application from a live one; inventory, reconciliation and teardown loops that wait for the id to disappear hang forever, and any "how many applications do I have" reading grows without bound.
Fix: ListApplications' handler (src/ops/list_applications.rs, the `states` filter around lines 183-194) must exclude TERMINATED applications when the request names no `states`, while still returning them when TERMINATED is named explicitly.
Traces: emr-serverless.delete-application-removes-from-list, emr-serverless.create-application-invalid-engine-type-rejected, emr-serverless.start-stop-application-lifecycle, emr-serverless.delete-application-while-started-rejected.

=== Cause 3 of 8: CancelJobRun on an already-CANCELLED job run is refused instead of being an idempotent no-op ===
AWS: Smithy: CancelJobRun is @idempotent and its output requires applicationId + jobRunId; it models ValidationException (400) and ResourceNotFoundException (404). Recorded behavior: cancelling a job run that is ALREADY CANCELLED succeeds (HTTP 200, echoes the same applicationId/jobRunId, state stays CANCELLED); cancelling one in another terminal state (FAILED) is refused with 400 ValidationException (recorded message example: "Job run is not in a cancellable state", which is state-agnostic).
Emulator: a repeat cancel of a CANCELLED run raises ValidationException "Job run is in CANCELLED state and cannot be cancelled" — a blanket terminal-state refusal that includes CANCELLED, with a message that interpolates the state instead of AWS's state-agnostic text.
Why: retry-safe clients (and the SDKs' idempotent-operation retries) call CancelJobRun again after a timeout or on a second pass; AWS returns 200 and they continue, while here they get a 400 they cannot distinguish from a genuine bad-state refusal, so a cancel that already succeeded reads as a failure.
Fix: CancelJobRun's handler (src/ops/cancel_job_run.rs) state-machine branch: treat CANCELLED as the idempotent success path (200 echo, no state change) and keep the 400 refusal for the other terminal states.
Traces: emr-serverless.cancel-terminal-job-run-rejected, emr-serverless.cancel-job-run-on-terminal-state-rejected.

=== Cause 4 of 8: every job run is driven to FAILED ~500 ms after StartJobRun, so no run is ever observable in a non-terminal state ===
AWS: recorded behavior only. Smithy: JobRunState spans SUBMITTED, PENDING, SCHEDULED, QUEUED, RUNNING, SUCCESS, FAILED, CANCELLING, CANCELLED. A freshly submitted run spends real time in the non-terminal states, so ListJobRuns(states=[SUBMITTED,PENDING,SCHEDULED,QUEUED,RUNNING]) returns a run submitted moments earlier.
Emulator: StartJobRun schedules the run to FAILED after state::LIFECYCLE_TRANSITION (500 ms) unconditionally (src/ops/start_job_run.rs, the pending PendingState with state "FAILED"), so by the time any follow-up call lands the run is terminal. ListJobRuns(states=[non-terminal set]) returned the empty set for a run submitted seconds before: "expected non-terminal job 0000000000000002 in the non-terminal-states listing, got set()". The states filter itself works (the CANCELLED assertions in the same check passed) — the lifecycle is what diverges.
Why: any client that submits work and then polls/filters for in-flight runs sees nothing in flight and a terminal FAILED immediately; progress tracking, queue-depth logic and "wait until running" paths cannot be exercised at all.
Fix: the job-run lifecycle in src/ops/start_job_run.rs together with state.rs (JobRunRecord::settle / LIFECYCLE_TRANSITION) must hold a submitted run in the non-terminal states long enough to be observed before settling.
Traces: emr-serverless.list-job-runs-filters-by-state.

=== Cause 5 of 8: GetDashboardForJobRun is an unimplemented scaffold and answers NotImplemented ===
AWS: Smithy: GetDashboardForJobRun GET /applications/{applicationId}/jobruns/{jobRunId}/dashboard returns 200 with `url` (Url, length 1..2048); its modeled errors are InternalServerException (500), ResourceNotFoundException (404) and ValidationException (400) — NotImplemented is not among them. Recorded behavior: for a real job run it returns an https URL.
Emulator: the handler returns crate::scaffolded::Scaffolded::not_implemented("GetDashboardForJobRun") (src/ops/get_dashboard_for_job_run.rs:168), so every call is refused with the scaffold's NotImplemented status; the check retried it for its whole budget and was killed at 60 s.
Why: a client fetching the Spark/Hive UI link for a run gets an error code the API does not model and cannot be handled or retried meaningfully.
Fix: implement the handler in src/ops/get_dashboard_for_job_run.rs — 200 with an https `url` for an existing application+job run, 404 ResourceNotFoundException otherwise. (The same check additionally asserts a freshly minted url per call and that the url resolves; those are secondary to answering the modeled 200/`url` shape at all.)
Traces: emr-serverless.get-dashboard-for-job-run-returns-url.

=== Cause 6 of 8: autoStopConfiguration is persisted but never enforced — an idle application never stops ===
AWS: Smithy/docs cited in the emulator's own state.rs: AutoStopConfig$enabled "Enables the application to automatically stop after a certain amount of time being idle. Defaults to true"; AutoStopConfig$idleTimeoutMinutes "The amount of idle time in minutes after which your application will automatically stop" (minimum 1, per AWS::EMRServerless::Application AutoStopConfiguration). A STARTED application with no job runs or sessions transitions to STOPPED once past that threshold, with no StopApplication call.
Emulator: CreateApplication stores the config (state::adopt_auto_stop) but nothing consumes it: ApplicationRecord has no idle/last-activity field and settle() materialises only an explicitly scheduled PendingState (state.rs ApplicationRecord::settle / terminate; StartApplication only schedules STARTING->STARTED). The application therefore stays STARTED indefinitely, and the check polling for STOPPED never observes it.
Why: a client relying on auto-stop to release capacity sees an application that bills/holds capacity forever and a state that never becomes STOPPED, so cost-control and idle-reaping behavior cannot be reproduced.
Fix: the application state machine (src/state.rs: track last activity and owe a STOPPED transition when autoStopConfiguration.enabled and the idle threshold has elapsed, materialised by settle()), driven by the operations that mark activity (StartJobRun, CancelJobRun, session ops).
Traces: emr-serverless.auto-stop-configuration-stops-idle-application.

=== Cause 7 of 8: CreateApplication's refusal of an unknown release label blames the engine type ===
AWS: Smithy: releaseLabel and type are both @required on CreateApplication, which models ValidationException (400). Recorded behavior distinguishes the two causes: an unsupported engine type is refused with a message interpolating the type ("Type NOT_A_REAL_ENGINE is not supported for release label" — which this emulator already reproduces, that assertion passes), while a well-formed but non-existent release label is refused with the non-interpolated "Type is not supported for the release label or release label does not exist". The requirement is that the refusal identifies which member failed, not the literal text.
Emulator: for releaseLabel="emr-0.0.0-does-not-exist" with type="SPARK" it answers 400 ValidationException (code correct) with 'Type SPARK is not supported for release label' — the unknown-type form, naming a member that is in fact valid.
Why: a client (or an operator reading the error) is pointed at the engine type and will "fix" a correct field while the real culprit, the release label, goes unmentioned.
Fix: CreateApplication's validation (src/ops/create_application.rs) must branch on which member is unrecognised and emit the release-label form when the release label is the unknown one.
Traces: emr-serverless.create-application-invalid-release-label-rejected.

=== Cause 8 of 8: GetJobRun with a never-created attempt reports the job-run-id not-found message ===
AWS: Smithy: GetJobRun.attempt (AttemptNumber, @range min 1, httpQuery) with modeled ResourceNotFoundException (404). Recorded behavior distinguishes the two not-found causes by which part of the request was not found: a missing job run id yields "Job Run with given ID doesn't exist" (asserted and PASSING elsewhere in this suite) and a missing attempt yields "Job Run with given attempt doesn't exist".
Emulator: GetJobRun(attempt=999) on an existing job run answers 404 ResourceNotFoundException (code correct) with "Job Run with given ID doesn't exist" — the single shared constant state.rs:670 JOB_RUN_NOT_FOUND_MESSAGE is used for both cases.
Why: a client diagnosing a retry/attempt lookup is told the job run id does not exist when it does, so it discards a valid id instead of correcting the attempt number.
Fix: GetJobRun's not-found path (src/ops/get_job_run.rs plus a second message constant beside JOB_RUN_NOT_FOUND_MESSAGE in src/state.rs) must distinguish an unknown attempt from an unknown job run id.
Traces: emr-serverless.get-job-run-invalid-attempt-not-found.

## Evidence
Suite: simparity-behavioral-aws-v1, 14/45 passed. All evidence below is verbatim from the replay report.

Cause 1 — expected ConflictException, got HTTP 200 echoing the first application:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'applicationId': '0000000000000001', 'name': 'sim2real-emrs-app-a-27712bfedd6c', 'arn': 'arn:aws:emr-serverless:us-west-2:000000000000:/applications/0000000000000001'}
  (same shape for create-application-same-client-token-conflict-rejected: name 'sim2real-emrs-app-a-f91976f6d52a', applicationId '0000000000000001')
  Expected: 409 ConflictException; expected message example "Request did not match the original".

Cause 2 — expected the deleted id absent from ListApplications:
  AssertionError: deleted application '0000000000000001' is still present in ListApplications: {'0000000000000001'}
  and three checks "Check timed out after 60.0s" while polling for list-absence (delete-application-removes-from-list, start-stop-application-lifecycle via _wait_for_absent_from_list, delete-application-while-started-rejected via the trailing list poll — their earlier CREATED/STARTED/STOPPED/TERMINATED polls are satisfied by the emulator, as proven by other checks that pass them).

Cause 3 — repeat cancel of a CANCELLED run:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the CancelJobRun operation: Job run is in CANCELLED state and cannot be cancelled
  Expected: HTTP 200 echoing {applicationId, jobRunId}, state unchanged at CANCELLED.
  Related, same handler branch (FAILED run): expected 400 ValidationException with the state-agnostic "Job run is not in a cancellable state", got 'Job run is in FAILED state and cannot be cancelled' (code matches, text interpolates the state).

Cause 4 — ListJobRuns with the non-terminal states filter:
  AssertionError: expected non-terminal job 0000000000000002 in the non-terminal-states listing, got set()
  states=["CANCELLED"] assertions in the same check passed, so the filter works; the run was already FAILED (start_job_run.rs schedules FAILED after LIFECYCLE_TRANSITION = 500 ms).

Cause 5 — GetDashboardForJobRun: check timed out after 60.0s retrying it; handler is Err(crate::scaffolded::Scaffolded::not_implemented("GetDashboardForJobRun")) at services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/get_dashboard_for_job_run.rs:168. Expected: 200 with `url` starting "https://".

Cause 6 — auto-stop: check timed out after 60.0s polling GetApplication for STOPPED on an application created with autoStopConfiguration={"enabled": True, "idleTimeoutMinutes": 1}; nothing in src/state.rs tracks idleness or owes a STOPPED transition.

Cause 7 — invalid release label:
  AssertionError: unexpected CreateApplication invalid-release-label message: 'Type SPARK is not supported for release label'
  Expected: 400 ValidationException with "Type is not supported for the release label or release label does not exist" (the type-interpolated form is correct only for an unknown type, which the emulator gets right).

Cause 8 — invalid attempt:
  AssertionError: unexpected GetJobRun not-found message: "Job Run with given ID doesn't exist"
  Expected: 404 ResourceNotFoundException with "Job Run with given attempt doesn't exist"; the id form is produced by the shared constant state.rs:670.

## Steps to Reproduce
Boot the emulator and point an emr-serverless client at it (all steps use the emr-serverless endpoint only; no other service is involved).

Cause 1: CreateApplication(name=A, releaseLabel=emr-7.0.0, type=SPARK, clientToken=T). Then CreateApplication(name=B, releaseLabel=emr-7.0.0, type=SPARK, clientToken=T). Expect 409 ConflictException; observe 200 returning application A.

Cause 2: CreateApplication(...); DeleteApplication(applicationId=id); then ListApplications() with no `states`. Expect `id` absent (GetApplication still reports TERMINATED); observe `id` still listed. Same with the STARTED path: StartApplication, wait STARTED, DeleteApplication (400), StopApplication, wait STOPPED, DeleteApplication, poll ListApplications for absence — it never goes absent.

Cause 3: StartJobRun on a started application; CancelJobRun once (reaches CANCELLED); CancelJobRun again with the same ids. Expect 200 echoing {applicationId, jobRunId}; observe 400 ValidationException "Job run is in CANCELLED state and cannot be cancelled". For the FAILED variant, let a run reach FAILED and CancelJobRun it: expect 400 with the state-agnostic message.

Cause 4: StartJobRun twice on one application; CancelJobRun the first and wait for CANCELLED; then ListJobRuns(applicationId, states=["SUBMITTED","PENDING","SCHEDULED","QUEUED","RUNNING"]). Expect the second run listed; observe an empty list (it is already FAILED).

Cause 5: StartJobRun, wait until the run leaves SUBMITTED, then GetDashboardForJobRun(applicationId, jobRunId). Expect 200 with an https `url`; observe NotImplemented.

Cause 6: CreateApplication(autoStopConfiguration={enabled: true, idleTimeoutMinutes: 1}); StartApplication; wait for STARTED; submit no work; poll GetApplication. Expect it to reach STOPPED on its own; observe STARTED forever.

Cause 7: CreateApplication(name=X, releaseLabel="emr-0.0.0-does-not-exist", type="SPARK", clientToken=T). Expect 400 ValidationException naming the release label; observe 'Type SPARK is not supported for release label'.

Cause 8: StartJobRun, then GetJobRun(applicationId, jobRunId, attempt=999). Expect 404 ResourceNotFoundException "Job Run with given attempt doesn't exist"; observe "Job Run with given ID doesn't exist".

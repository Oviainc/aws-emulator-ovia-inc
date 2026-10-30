Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-serverless.start-job-run-native-spark-execution-succeeds emr-serverless.spark-shuffle-partitioned-parquet-roundtrip emr-serverless.spark-write-outside-execution-role-prefix-is-denied emr-serverless.cancel-job-run-transitions-to-cancelled emr-serverless.list-job-runs-filters-by-state emr-serverless.start-session-without-interactive-configuration-rejected emr-serverless.terminate-already-terminated-session-rejected
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless.start-job-run-native-spark-execution-succeeds|run#14|AssertionError|assert final_state["state"] == "SUCCESS", final_state.get("stateDetails")
Issue-Key: emr-serverless.spark-shuffle-partitioned-parquet-roundtrip|run#17|AssertionError|assert final["state"] == "SUCCESS", (
Issue-Key: emr-serverless.spark-write-outside-execution-role-prefix-is-denied|run#12|AssertionError|assert ok_final["state"] == "SUCCESS", (
Issue-Key: emr-serverless.cancel-job-run-transitions-to-cancelled|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: emr-serverless.list-job-runs-filters-by-state|run#17|AssertionError|assert job_b in non_terminal_ids, f"expected non-terminal job {job_b} in the non-terminal-states listing, got {non_terminal_ids}"
Issue-Key: emr-serverless.start-session-without-interactive-configuration-rejected|run#12|AssertionError|assert message == "Sessions are not enabled on this application", (
Issue-Key: emr-serverless|TerminateSession|ValidationException
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# EMR Serverless job-run lifecycle and interactive-session semantics diverge: runs settle terminal in ~3 s and never RUN, and two session refusals are wrong (4 causes)

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
FOUR causes, all in the emr-serverless service. A fixer must close all four before submitting.

Related open issues, named here so this is not mistaken for a duplicate: 20261022T154846Z-emr-serverless-job-runs-are-recorded-but-never-executed.md, 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md and 20261026T072046Z-emr-serverless-startjobrun-s-accepted-run-terminates-failed.md describe the non-execution gap, but their replay keys are bound to other checks (the athena__emr-serverless__glue__s3 combo check and the autoStart gate check). None of them carries the five emr-serverless checks listed below, so those checks are net-new work items here. Cause 2 is a distinct observable divergence that none of them states at all.

=== Cause 1 of 4: an accepted sparkSubmit job run always settles in terminal FAILED and never reaches SUCCESS ===
AWS: Smithy com.amazonaws.emrserverless#JobRunState = SUBMITTED | PENDING | SCHEDULED | RUNNING | SUCCESS | FAILED | CANCELLING | CANCELLED | QUEUED. GetJobRun (GET /applications/{applicationId}/jobruns/{jobRunId}, 200) returns the required JobRun with required `state` and `stateDetails`. A job run whose jobDriver.sparkSubmit.entryPoint resolves and whose executionRoleArn grants the S3/Glue actions the script needs progresses through the non-terminal states and settles on SUCCESS; FAILED is reserved for a driver that actually ran and exited non-zero, or for a run that could not be placed. Recorded behavior only for the exact order of the intermediate states.
Emulator: GetJobRun reports state 'FAILED' with stateDetails 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.' for every run, roughly 3 s after StartJobRun returned 200 and a jobRunId. No workload side effect appears (no Parquet written to S3, no catalog entry, no driver log).
Why: every client that submits work and gates on the outcome takes its error path forever. An orchestrator (Step Functions .sync, Airflow, a shell poller) marks the step failed and never advances. Worse, FAILED is also AWS's genuine-failure state, so a negative control (a run that must fail because the execution role lacks a permission) is indistinguishable from this blanket failure: emr-serverless.spark-write-outside-execution-role-prefix-is-denied cannot grade its denial because even its ALLOWED-prefix control run ends FAILED.
Fix: the job-run lifecycle in the emr-serverless state layer that StartJobRun/GetJobRun drive — an accepted driver configuration must advance to SUCCESS and produce the run's modelled side effects, not be short-circuited to the modelled failure state.
Traces: emr-serverless.start-job-run-native-spark-execution-succeeds, emr-serverless.spark-shuffle-partitioned-parquet-roundtrip, emr-serverless.spark-write-outside-execution-role-prefix-is-denied (file/line citations in Evidence).

=== Cause 2 of 4: a submitted run leaves the non-terminal states after a fixed ~3 s regardless of the submitted workload, and RUNNING is never reported at all, so CancelJobRun cannot be exercised and ListJobRuns(states=[non-terminal]) returns nothing ===
AWS: a job run stays non-terminal for as long as its workload takes; `RUNNING` is a value of JobRunState and JobRun$startedAt is documented as "the date and time when the job moved to the RUNNING state". CancelJobRun (DELETE /applications/{applicationId}/jobruns/{jobRunId}, @idempotent, 200) on a run that is in flight drives it CANCELLING -> CANCELLED. ListJobRuns (GET /applications/{applicationId}/jobruns) takes `states` (JobRunStateSet, @length 0..8, uniqueItems) as an httpQuery filter and must return every run currently in one of the named states, so a run still in flight appears under states=[SUBMITTED,PENDING,SCHEDULED,QUEUED,RUNNING]. Recorded behavior only for how long each intermediate state lasts; the requirement is that the span follows the workload, not a constant.
Emulator: the run's whole pre-terminal life is a fixed ladder of four 750 ms steps — PENDING, QUEUED, SCHEDULED, then FAILED (state.rs JOB_RUN_STAGES, JOB_RUN_STAGE_TRANSITION = 750 ms) — so a run submitted with a 240-second sleep script is terminal about 3 s after StartJobRun, and RUNNING is never entered. A caller polling at any realistic interval observes SUBMITTED and then a terminal state, nothing in between: emr-serverless.cancel-job-run-transitions-to-cancelled never observes its run in (PENDING, SCHEDULED, RUNNING) and spins until the harness kills it, and in emr-serverless.list-job-runs-filters-by-state the deliberately-left-running job run is absent from the non-terminal listing (the listing came back empty; the states=[CANCELLED] listing in the same check was correct, so the filter itself works — the run had simply already gone terminal).
Why: a client cannot cancel work, because by the time it reacts there is nothing cancellable; CancelJobRun's CANCELLING/CANCELLED path is unreachable for anything but a race. A dashboard or scheduler that lists in-flight runs by `states` always shows zero, and no client can ever observe RUNNING or a meaningful startedAt.
Fix: the job-run state machine in the emr-serverless state layer — the non-terminal span must be tied to the run's execution (and must include RUNNING) rather than to a constant per-stage duration, so a cancel and a non-terminal `states` filter both have a live run to act on.
Traces: emr-serverless.cancel-job-run-transitions-to-cancelled, emr-serverless.list-job-runs-filters-by-state.

=== Cause 3 of 4: StartSession's refusal for an application without interactiveConfiguration.sessionEnabled carries the wrong error message ===
AWS: StartSession (POST /applications/{applicationId}/sessions) documents "The application must be in the STARTED state or have AutoStart enabled, and have interactive sessions enabled" and models ValidationException (HTTP 400, required `message`). For an application created with no interactiveConfiguration, the refusal is HTTP 400 ValidationException with the message 'Sessions are not enabled on this application' — that whole string is fixed text; the message has no variable parts taken from the request or from state.
Emulator: the code is right (ValidationException) but the message is 'Interactive sessions are not enabled on this application: set interactiveConfiguration.sessionEnabled'.
Why: a client that branches on the message (or a test/assertion that matches it) does not recognise the refusal; the emulator's text also leaks an emulator-authored remediation hint that AWS never sends, so message-equality checks and log scrapers diverge.
Fix: the message literal in the StartSession handler's sessionEnabled precondition (services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/start_session.rs, the `!record.session_enabled()` branch).
Traces: emr-serverless.start-session-without-interactive-configuration-rejected.

=== Cause 4 of 4: TerminateSession on an already-TERMINATED session is refused with ValidationException instead of succeeding as the idempotent no-op it is modelled to be ===
AWS: TerminateSession is DELETE /applications/{applicationId}/sessions/{sessionId}, carries the @idempotent trait, and returns 200 with required `applicationId` and `sessionId`. Its documentation describes only the forward transition ("After you terminate a session, it enters the TERMINATING state and then the TERMINATED state"); SessionState's terminal value TERMINATED is where that transition ends. Re-terminating a session that is already TERMINATED therefore returns HTTP 200 echoing the stored applicationId and sessionId (values read from state, not request-echoed text) and changes nothing — an idempotent DELETE does not fail because the work is already done. ValidationException is modelled for input that fails constraints, not for a repeated terminate.
Emulator: HTTP 400 ValidationException with message 'Session is not in a state that can be terminated' (services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/terminate_session.rs, the `!crate::state::is_terminable_session_state(...)` branch, message constant crate::state::SESSION_NOT_TERMINABLE_MESSAGE). The handler already treats TERMINATING as an idempotent no-op; TERMINATED/FAILED are refused instead.
Why: any retry-on-timeout or at-least-once teardown path breaks — a client that re-sends the terminate (SDK retry, orchestrator cleanup, cleanup-on-exception) gets a 400 it must special-case, which is exactly what @idempotent exists to prevent. The failure also hides a second requirement that could not be graded in this run because the check aborted here: TerminateSession for a sessionId that does not exist on the application must answer HTTP 404 ResourceNotFoundException with message 'Session with given ID doesn't exist' (fixed text, no variable parts) — verify that alongside the fix.
Fix: the TerminateSession handler's state precondition in the emr-serverless ops layer — an already-terminal session must return the stored ids with 200 rather than entering the refusal branch.
Traces: emr-serverless.terminate-already-terminated-session-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout: 34/45 passed, 8 mismatched, 3 errored. Per-service: emr-serverless 34/43 (79%), 9 failed. (The two athena__emr-serverless__glue__s3 failures are marked OWNED by existing issues and are not part of this issue; emr-serverless.terminate-session-transitions-to-terminated is ALREADY FILED against the unimplemented ListSessions stub; emr-serverless.auto-stop-configuration-stops-idle-application is not filed here — see the run summary.)

Cause 1 — job runs never reach SUCCESS
[mismatch] emr-serverless.start-job-run-native-spark-execution-succeeds
  check_emr_serverless_start_job_run_native_spark_execution_succeeds.py line 73:
    assert final_state["state"] == "SUCCESS", final_state.get("stateDetails")
  AssertionError: This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.
[mismatch] emr-serverless.spark-shuffle-partitioned-parquet-roundtrip
  check_emr_serverless_spark_shuffle_partitioned_parquet_roundtrip.py line 454:
    assert final["state"] == "SUCCESS"
  AssertionError: Spark job run 0000000000000001 ended 'FAILED': 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.'
[mismatch] emr-serverless.spark-write-outside-execution-role-prefix-is-denied
  check_emr_serverless_spark_write_outside_execution_role_prefix_is_denied.py line 300:
    assert ok_final["state"] == "SUCCESS"
  AssertionError: control run 0000000000000001 writing to the ALLOWED prefix ended 'FAILED': 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.' — the fixture itself is broken, so the denial below would prove nothing
  Expected (AWS): GetJobRun eventually reports jobRun.state == 'SUCCESS'.
  Actual (emulator): jobRun.state == 'FAILED', stateDetails as above; StartJobRun returned 200 and a jobRunId, so nothing in the accepting response signals it.

Cause 2 — no run is ever observable in flight
[mismatch] emr-serverless.cancel-job-run-transitions-to-cancelled
  Error: Check timed out after 60.0s
  The check submits a driver that sleeps 240 s, then polls GetJobRun every 10 s (budget 420 s) for a state in (SCHEDULED, RUNNING, PENDING) before cancelling. Expected (AWS): the run is non-terminal for the duration of the sleep, the poll matches, CancelJobRun drives it CANCELLING -> CANCELLED. Actual (emulator): the run walks PENDING -> QUEUED -> SCHEDULED -> FAILED on a fixed 750 ms-per-stage clock (services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs:132 JOB_RUN_STAGE_TRANSITION = 750 ms; src/state.rs:259 JOB_RUN_STAGES = [PENDING, QUEUED, SCHEDULED, FAILED], with the source comment stating RUNNING and SUCCESS are deliberately absent), so it is terminal ~3 s after StartJobRun and the poll never matches.
[mismatch] emr-serverless.list-job-runs-filters-by-state
  check_emr_serverless_list_job_runs_filters_by_state.py line 63:
    assert job_b in non_terminal_ids
  AssertionError: expected non-terminal job 0000000000000002 in the non-terminal-states listing, got set()
  The preceding assertions in the same check passed: ListJobRuns(states=['CANCELLED']) did contain the cancelled run and did not contain the other one, so the states filter itself is correct. Expected (AWS): ListJobRuns(states=['SUBMITTED','PENDING','SCHEDULED','QUEUED','RUNNING']) contains the run that was left in flight. Actual (emulator): jobRuns is empty, because that run had already settled to FAILED.

Cause 3 — StartSession refusal message
[mismatch] emr-serverless.start-session-without-interactive-configuration-rejected
  check_emr_serverless_start_session_without_interactive_configuration_rejected.py line 65:
    assert message == "Sessions are not enabled on this application"
  AssertionError: unexpected StartSession message: 'Interactive sessions are not enabled on this application: set interactiveConfiguration.sessionEnabled'
  Expected (AWS): HTTP 400, code ValidationException, message 'Sessions are not enabled on this application'.
  Actual (emulator): HTTP 400, code ValidationException (correct), message 'Interactive sessions are not enabled on this application: set interactiveConfiguration.sessionEnabled'. Source: src/ops/start_session.rs lines 219-228.

Cause 4 — TerminateSession is not idempotent on a terminal session
[error] emr-serverless.terminate-already-terminated-session-rejected
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the TerminateSession operation: Session is not in a state that can be terminated
  The check's docstring states the contract: "TerminateSession on an already-TERMINATED session is an idempotent no-op, but TerminateSession on a session id that does not exist on the application is rejected with ResourceNotFoundException."
  Expected (AWS): HTTP 200 with the stored applicationId and sessionId; TerminateSession carries @idempotent in the model.
  Actual (emulator): HTTP 400 ValidationException 'Session is not in a state that can be terminated', raised out of the check, so its later assertions (ResourceNotFoundException with message "Session with given ID doesn't exist" for an unknown sessionId) were never reached. Source: src/ops/terminate_session.rs lines 181-194.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 behavioral suite, or reproduce each cause by hand:

Cause 1 (job runs never reach SUCCESS) — replay emr-serverless.start-job-run-native-spark-execution-succeeds, emr-serverless.spark-shuffle-partitioned-parquet-roundtrip, emr-serverless.spark-write-outside-execution-role-prefix-is-denied. By hand: create an S3 bucket, put a trivial PySpark script at s3://<bucket>/job.py; create an IAM role trusted by emr-serverless.amazonaws.com with s3:GetObject/PutObject/ListBucket on that bucket; emr-serverless CreateApplication (releaseLabel 'emr-7.9.0', type 'SPARK'); StartJobRun with jobDriver.sparkSubmit.entryPoint = s3://<bucket>/job.py; poll GetJobRun until terminal. Observe state FAILED with stateDetails 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.'; expected SUCCESS.

Cause 2 (no in-flight run) — replay emr-serverless.cancel-job-run-transitions-to-cancelled and emr-serverless.list-job-runs-filters-by-state. By hand: put a script whose body is `import time; time.sleep(240)`; StartJobRun as above; immediately poll GetJobRun every second and record the states seen. Observe SUBMITTED -> PENDING -> QUEUED -> SCHEDULED -> FAILED within ~3 s, RUNNING never reported, and the run terminal long before the 240 s script could finish. Then: StartJobRun twice on one application, CancelJobRun the first, wait for CANCELLED, and call ListJobRuns with states=['SUBMITTED','PENDING','SCHEDULED','QUEUED','RUNNING'] — observe an empty jobRuns list; expected the second run. CancelJobRun on the second run after ~5 s cannot produce CANCELLING/CANCELLED at all because the run is already terminal.

Cause 3 (StartSession message) — replay emr-serverless.start-session-without-interactive-configuration-rejected. By hand: CreateApplication with a release that supports sessions and NO interactiveConfiguration, start it, wait for STARTED, then StartSession with a clientToken and an execution role ARN. Observe HTTP 400 ValidationException with message 'Interactive sessions are not enabled on this application: set interactiveConfiguration.sessionEnabled'; expected message 'Sessions are not enabled on this application'.

Cause 4 (TerminateSession idempotency) — replay emr-serverless.terminate-already-terminated-session-rejected. By hand: CreateApplication with interactiveConfiguration.sessionEnabled = true on a session-capable release, start it, StartSession, poll GetSession until a started state, TerminateSession, poll GetSession until state TERMINATED, then call TerminateSession again with the same ids. Observe HTTP 400 ValidationException 'Session is not in a state that can be terminated'; expected HTTP 200 with applicationId and sessionId. Then call TerminateSession with a sessionId that does not exist on the application and confirm HTTP 404 ResourceNotFoundException with message "Session with given ID doesn't exist".

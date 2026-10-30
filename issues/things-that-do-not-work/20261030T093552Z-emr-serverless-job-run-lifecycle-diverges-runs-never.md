Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-serverless.start-job-run-native-spark-execution-succeeds emr-serverless.spark-shuffle-partitioned-parquet-roundtrip emr-serverless.spark-write-outside-execution-role-prefix-is-denied emr-serverless.list-job-runs-filters-by-state emr-serverless.cancel-job-run-transitions-to-cancelled
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless.start-job-run-native-spark-execution-succeeds|run#14|AssertionError|assert final_state["state"] == "SUCCESS", final_state.get("stateDetails")
Issue-Key: emr-serverless.spark-shuffle-partitioned-parquet-roundtrip|run#17|AssertionError|assert final["state"] == "SUCCESS", (
Issue-Key: emr-serverless.spark-write-outside-execution-role-prefix-is-denied|run#12|AssertionError|assert ok_final["state"] == "SUCCESS", (
Issue-Key: emr-serverless.list-job-runs-filters-by-state|run#17|AssertionError|assert job_b in non_terminal_ids, f"expected non-terminal job {job_b} in the non-terminal-states listing, got {non_terminal_ids}"
Issue-Key: emr-serverless.cancel-job-run-transitions-to-cancelled|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: db763654427c553d8b06d764576848aa87eee804

# EMR Serverless job-run lifecycle diverges: runs never execute (always terminal FAILED), never report RUNNING, and leave every non-terminal state within ~3 seconds (3 causes)

> _Note: this issue was identified against the code base at commit `db763654427c553d8b06d764576848aa87eee804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Three net-new root causes in the emr-serverless job-run lifecycle, all grounded in failing behavioral checks of this replay. Bootstrap-gate note: get_bootstrap_status reports crate-style tokens, and the crate `service-com-amazonaws-emrserverless-awstoledowebservice` (EMR Serverless) is in `bootstrapped`; the service answers 35/43 of its own behavioral checks, so these are fidelity findings, not a bootstrap gap.

Adjacent open issues (none of these checks is marked OWNED or ALREADY FILED by the replay report, so they are filed here): 20261022T154846Z-emr-serverless-job-runs-are-recorded-but-never-executed.md, 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md, 20261026T072046Z-emr-serverless-startjobrun-s-accepted-run-terminates-failed.md. Cause 1 below is the same family of defect as those, now reproduced by three further single-service emr-serverless checks whose replay keys none of them carries; causes 2 and 3 are distinct lifecycle defects no open issue describes.

Cause 1 of 3: a SparkSubmit job run accepted by StartJobRun always settles on terminal FAILED and never on SUCCESS, so no submitted workload ever executes.
AWS: Smithy com.amazonaws.emrserverless#JobRunState = SUBMITTED | PENDING | SCHEDULED | RUNNING | SUCCESS | FAILED | CANCELLING | CANCELLED | QUEUED. JobRunSummary/JobRun model `state` and `stateDetails` as required members; JobRun$startedAt is documented as the instant the job moved to the RUNNING state. GetJobRun is readonly, GET /applications/{applicationId}/jobruns/{jobRunId}. A sparkSubmit run whose entryPoint resolves in S3 and whose execution role grants the actions the script needs progresses through the non-terminal states and settles on SUCCESS; FAILED is reserved for a driver that actually ran and exited non-zero or a run that could not be placed. Recorded behavior only for the exact intermediate progression.
Emulator: GetJobRun reports state 'FAILED' with stateDetails 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.' for every accepted run, and no workload side effect (S3 output objects, Glue catalog entries, driver logs) appears. StartJobRun still answers HTTP 200 with a jobRunId, so nothing in the control-plane response signals the refusal. The fixed text of that stateDetails sentence is emulator-authored; AWS carries no such sentence at all for a healthy run.
Why: every client that submits work and gates on the outcome breaks — a poller waiting for SUCCESS (the AWS-documented pattern) sees permanent FAILED and takes its error path, and an orchestrator step never advances. Worse, FAILED is also AWS's genuine-failure state, so a negative control (an IAM-denied or bad-prefix write that AWS fails) becomes indistinguishable from this stub, and `spark-write-outside-execution-role-prefix-is-denied` reports exactly that: its ALLOWED-prefix positive control ended FAILED, so its denial assertion would prove nothing.
Fix: the job-run lifecycle in the emr-serverless state/handler layer that drives the state machine behind StartJobRun and GetJobRun — an accepted run must be able to reach SUCCESS and produce the side effects of a run that executed, instead of being short-circuited to the modelled failure state.
Traces: emr-serverless.start-job-run-native-spark-execution-succeeds (assert final_state["state"] == "SUCCESS"), emr-serverless.spark-shuffle-partitioned-parquet-roundtrip (assert final["state"] == "SUCCESS" on run 0000000000000001), emr-serverless.spark-write-outside-execution-role-prefix-is-denied (control run writing to the ALLOWED prefix ended 'FAILED'). Lifecycle source: services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs lines 234-264 (NOT_EXECUTED_STATE_DETAILS and JOB_RUN_STAGES) and src/ops/start_job_run.rs lines 303-319.

Cause 2 of 3: ListJobRuns with a non-terminal `states` filter reports nothing, because an accepted job run leaves every non-terminal state within ~3 seconds and never passes through RUNNING.
AWS: Smithy ListJobRuns — GET /applications/{applicationId}/jobruns, readonly, paginated (items jobRuns); ListJobRunsRequest$states targets JobRunStateSet (@length max 8, @uniqueItems) and is documented as "An optional filter for job run states. Note that if this filter contains multiple states, the resulting list will be grouped by the state." A run submitted on a real application is admitted, queued, scheduled and then RUNNING for the duration of the workload — minutes for a cold emr-7.x Spark run — so a listing filtered on SUBMITTED/PENDING/SCHEDULED/QUEUED/RUNNING taken shortly after submit reports that run. Recorded behavior only for the exact dwell durations.
Emulator: the run is driven PENDING -> QUEUED -> SCHEDULED -> FAILED at 750 ms per stage (state.rs JOB_RUN_STAGE_TRANSITION = 750 ms, JOB_RUN_STAGES), so roughly 3 s after StartJobRun it is terminal and RUNNING is never produced at all. ListJobRuns(states=[SUBMITTED, PENDING, SCHEDULED, QUEUED, RUNNING]) then answers HTTP 200 with jobRuns = [] for a run the client only just submitted: the check observed `set()` where it expected job run 0000000000000002. The CANCELLED-filtered listing in the same check was correct, so the `states` filter itself works — the state the run occupies is what diverges.
Why: any client that distinguishes in-flight work from finished work by a states-filtered ListJobRuns (a dashboard, a queue-depth gate, a scheduler that refuses to submit while runs are in flight) sees an empty in-flight list and concludes nothing is running, while GetJobRun on the same id reports a run it never saw in flight. RUNNING being absent from the enum values the emulator can ever report also makes it impossible to write any client logic keyed on RUNNING.
Fix: the job-run lifecycle in the emr-serverless state layer that enumerates the stages a submitted run passes through and the span each lasts — RUNNING must be among the states a run occupies, and the non-terminal window must be long enough that a client that submits and then lists observes the run in flight, rather than ~3 s total.
Traces: emr-serverless.list-job-runs-filters-by-state (AssertionError: expected non-terminal job 0000000000000002 in the non-terminal-states listing, got set()). Lifecycle source: services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs lines 124-132 and 246-281.

Cause 3 of 3: CancelJobRun's CANCELLING -> CANCELLED path is unreachable for a run that was still working, because the run is already terminal FAILED before a client can observe it in flight and cancel it.
AWS: Smithy CancelJobRun — DELETE /applications/{applicationId}/jobruns/{jobRunId}, @idempotent, documented "Cancels a job run", input shutdownGracePeriodInSeconds (httpQuery, unbounded), errors ValidationException (400), ResourceNotFoundException (404), InternalServerException (500). JobRunState models CANCELLING and CANCELLED, so cancelling a run that is actively RUNNING drives it CANCELLING -> CANCELLED and releases its capacity before natural completion. Recorded behavior only for how long a real run stays RUNNING.
Emulator: a client that submits a long-running driver (a script that sleeps past the observation window) and polls GetJobRun for a state in {PENDING, SCHEDULED, RUNNING} never sees one: the run is SUBMITTED at t=0 and terminal FAILED by ~3 s, so the poll never matches and the check never reaches its CancelJobRun call (observed as 'Check timed out after 60.0s'). Had it reached it, cancel_job_run.rs lines 191-199 would refuse the terminal run with ValidationException and the emulator-authored JOB_RUN_NOT_CANCELLABLE_MESSAGE instead of accepting the cancel — so the cancel path AWS exercises here cannot be reached by any client at all.
Why: a client that cancels work in flight (a timeout guard, a user-initiated abort, an orchestrator cleaning up a superseded run) can never see CANCELLING, can never see a cancel-induced CANCELLED on a run that was working, and instead gets a 400 ValidationException telling it the run cannot be cancelled — a refusal AWS would only give for a run that had genuinely finished.
Fix: the same job-run lifecycle in the emr-serverless state layer — once a run can occupy RUNNING for an observable span (cause 2), CancelJobRun's existing CANCELLING -> CANCELLED transition becomes reachable; the handler's terminal-state refusal must not be the only outcome a client can observe for a cancel.
Traces: emr-serverless.cancel-job-run-transitions-to-cancelled (Check timed out after 60.0s; the check polls GetJobRun for a state in SCHEDULED/RUNNING/PENDING before calling CancelJobRun with shutdownGracePeriodInSeconds=0 and asserting the run reaches CANCELLED). Sources: services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/cancel_job_run.rs lines 191-215 and src/state.rs lines 246-293.

## Evidence
Replay of simparity-behavioral-aws-v1: 35/45 passed (77.8%), 7 mismatched, 3 errored. emr-serverless 35/43 (81%), 8 failed. Two athena__emr-serverless__glue__s3 checks and emr-serverless.auto-stop-configuration-stops-idle-application were marked OWNED, and emr-serverless.start-session-without-interactive-configuration-rejected ALREADY FILED (ListSessions stub); those are skipped. The five below are the surviving failures.

Cause 1 — runs never execute:
[mismatch] emr-serverless.start-job-run-native-spark-execution-succeeds
  check_emr_serverless_start_job_run_native_spark_execution_succeeds.py:73
  assert final_state["state"] == "SUCCESS", final_state.get("stateDetails")
  AssertionError: This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.
[mismatch] emr-serverless.spark-shuffle-partitioned-parquet-roundtrip
  check_emr_serverless_spark_shuffle_partitioned_parquet_roundtrip.py:454
  AssertionError: Spark job run 0000000000000001 ended 'FAILED': 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.'
[mismatch] emr-serverless.spark-write-outside-execution-role-prefix-is-denied
  check_emr_serverless_spark_write_outside_execution_role_prefix_is_denied.py:300
  AssertionError: control run 0000000000000001 writing to the ALLOWED prefix ended 'FAILED': '...executes no Spark or Hive workload...' — the fixture itself is broken, so the denial below would prove nothing
  Expected (AWS): GetJobRun eventually reports jobRun.state == 'SUCCESS' for an accepted sparkSubmit run whose driver and role resolve.
  Actual (emulator): jobRun.state == 'FAILED' with the emulator-authored stateDetails above; StartJobRun answered 200 with a jobRunId.

Cause 2 — no observable non-terminal window / no RUNNING:
[mismatch] emr-serverless.list-job-runs-filters-by-state
  check_emr_serverless_list_job_runs_filters_by_state.py:63
  assert job_b in non_terminal_ids
  AssertionError: expected non-terminal job 0000000000000002 in the non-terminal-states listing, got set()
  Expected (AWS): ListJobRuns(applicationId=<app>, states=[SUBMITTED, PENDING, SCHEDULED, QUEUED, RUNNING]) includes a run submitted moments earlier and still in flight; the sibling states=[CANCELLED] listing includes only the cancelled run.
  Actual (emulator): the states=[CANCELLED] listing is correct, but the non-terminal listing is HTTP 200 with jobRuns == [] — the run reached terminal FAILED ~3 s after submit (4 stages x 750 ms) and RUNNING is never emitted.

Cause 3 — cancel-while-running unreachable:
[mismatch] emr-serverless.cancel-job-run-transitions-to-cancelled
  Error: Check timed out after 60.0s
  Expected (AWS): GetJobRun reports SCHEDULED/PENDING and then RUNNING for a driver that sleeps 240 s; CancelJobRun(shutdownGracePeriodInSeconds=0) then drives it CANCELLING -> CANCELLED.
  Actual (emulator): GetJobRun never reports any of PENDING/SCHEDULED/RUNNING to a 10 s-interval poller — the run is SUBMITTED at t=0 and terminal FAILED by ~3 s — so the poll never satisfies its predicate and the check never reaches CancelJobRun. A cancel issued after that point is refused HTTP 400 ValidationException with the emulator-authored JOB_RUN_NOT_CANCELLABLE_MESSAGE (cancel_job_run.rs:192-199).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 behavioral suite; or reproduce each cause by hand as below.

Cause 1 (never SUCCESS):
2. Create an S3 bucket and put a PySpark driver at s3://<bucket>/job.py that writes a small DataFrame to sys.argv[1].
3. Create an IAM role trusted by emr-serverless.amazonaws.com granting the S3 object/bucket actions on that bucket.
4. emr-serverless CreateApplication (releaseLabel 'emr-7.9.0', type 'SPARK', a fresh clientToken), then StartJobRun with jobDriver.sparkSubmit.entryPoint = s3://<bucket>/job.py and entryPointArguments = ['s3://<bucket>/output/'].
5. Poll GetJobRun until jobRun.state is in {SUCCESS, FAILED, CANCELLED}. Observe FAILED with stateDetails 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.'; expected SUCCESS, with the output objects present under s3://<bucket>/output/.

Cause 2 (states filter / non-terminal window):
6. On a CREATED application with autoStartConfiguration.enabled=true, StartJobRun twice (job A and job B) with distinct clientTokens.
7. CancelJobRun on job A and wait for it to report CANCELLED.
8. Call ListJobRuns(applicationId=<app>, states=['CANCELLED']) — job A is present (correct) — then ListJobRuns(applicationId=<app>, states=['SUBMITTED','PENDING','SCHEDULED','QUEUED','RUNNING']). Observe jobRuns == [] where job B is expected. Polling GetJobRun on job B from the instant of submit shows it passing PENDING -> QUEUED -> SCHEDULED -> FAILED in about 3 seconds and never reporting RUNNING.

Cause 3 (cancel while running):
9. Put a driver at s3://<bucket>/sleep.py containing 'import time; time.sleep(240)' and StartJobRun on it.
10. Poll GetJobRun for a state in {PENDING, SCHEDULED, RUNNING} at a 10 s interval. Observe the predicate is never satisfied (the run is terminal FAILED within ~3 s), so CancelJobRun is never reached.
11. Call CancelJobRun(applicationId=<app>, jobRunId=<id>, shutdownGracePeriodInSeconds=0) on that now-FAILED run. Observe HTTP 400 ValidationException refusing the cancel, instead of the CANCELLING -> CANCELLED transition AWS performs on a run that is still working.

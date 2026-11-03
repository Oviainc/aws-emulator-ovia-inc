Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.emr-serverless-failing-script-reports-failed-and-writes-no-output emr.emr-serverless-spark-job-run-reaches-success emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.emr-serverless-failing-script-reports-failed-and-writes-no-output|run#18|AssertionError|assert started == "STARTED", (
Issue-Key: emr.emr-serverless-spark-job-run-reaches-success|run#21|AssertionError|assert started == "STARTED", f"application must reach STARTED, got {started!r}"
Issue-Key: emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected|run#28|AssertionError|assert started == "STARTED", (
Issue-Commit: e0cc4f83c3608c610536f43d6e35ff223081a6f4

# EMR Serverless applications auto-stop before STARTED is ever observable: a configured idleTimeoutMinutes of 1 is served as a 5-second idle span measured from the StartApplication call

> _Note: this issue was identified against the code base at commit `e0cc4f83c3608c610536f43d6e35ff223081a6f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartApplication

## Description
One cause, grounded in three failing checks.

Attribution note: the failing check ids lead with the token `emr` and the suite lists them under `### emr` (the files live in behavioral/emr/ and declare SERVICES=['emr']), but their bodies drive the SEPARATE emr-serverless model (com.amazonaws.emrserverless#AwsToledoWebService) through ctx.client('emr-serverless'). The service token is taken from the suite heading as the gate requires. get_bootstrap_status reports both the EMR crate (service-com-amazonaws-emr-elasticmapreduce) and the EMR Serverless crate (service-com-amazonaws-emrserverless-awstoledowebservice) as bootstrapped — only athena and iot are pending — and the emulator did answer every call here (these are mismatches, not NotImplemented), so this is a fidelity finding, not a bootstrap gap.

Related open issues, named rather than duplicated silently: 20261026T072046Z-emr-serverless-startjobrun-s-accepted-run-terminates-failed.md, 20261030T093552Z-emr-serverless-job-run-lifecycle-diverges-runs-never.md, 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md and 20261022T154846Z-emr-serverless-job-runs-are-recorded-but-never-executed.md all describe the JOB-RUN lifecycle. This issue is a different, EARLIER cause in the APPLICATION lifecycle: the three checks below no longer reach any StartJobRun at all, because the application is never observable in STARTED. None of the three replay keys was marked OWNED by any open issue.

Cause 1 of 1: an EMR Serverless application created or started with autoStopConfiguration={'enabled': true, 'idleTimeoutMinutes': 1} never reports the STARTED state to a polling client — the automatic idle stop is already due by the time the STARTING->STARTED transition is materialised, so GetApplication jumps straight from STARTING to STOPPED.
AWS: Smithy com.amazonaws.emrserverless#ApplicationState = CREATING | CREATED | STARTING | STARTED | STOPPING | STOPPED | TERMINATED; AutoStopConfig$idleTimeoutMinutes is @range(min:1,max:10080), documented "The amount of idle time in MINUTES after which your application will automatically stop. Defaults to 15 minutes", and AutoStopConfig$enabled is "Enables the application to automatically stop after a certain amount of time being idle. Defaults to true." So the idle span is counted in minutes and it starts when the application is actually idle in STARTED — not when StartApplication (or CreateApplication with autoStart enabled) was called. On AWS an application with idleTimeoutMinutes=1 settles in STARTED and stays there for a full minute of idleness, so a client polling GetApplication at a 10-second interval always observes STARTED and can submit a job run; StartApplication returns 200 with an empty body and the application transitions CREATED/STOPPED -> STARTING -> STARTED.
Emulator: GetApplication reports STOPPED instead of STARTED. In services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs each configured idle MINUTE is served as IDLE_MINUTE_SPAN = 5 seconds, while the STARTING->STARTED transition is only LIFECYCLE_TRANSITION = 500 ms away, and ApplicationRecord::auto_stop_due() measures the idle span from last_activity_at — which StartApplication sets at the moment of the call (record.mark_activity(now)) rather than at the instant STARTED materialises. ApplicationRecord::settle() therefore materialises STARTING->STARTED (due at call+0.5s) and then, in the SAME settle pass, the auto-stop (due at call+5s) and overwrites the state with STOPPED plus the auto-stopped stateDetails. Any client whose next GetApplication lands more than ~5 s after StartApplication — i.e. every one of these checks, which poll at 10 s — sees the state leave STARTING as 'STOPPED'. All three checks assert on exactly that: 'the application must reach STARTED before any job run; got STOPPED', 'application must reach STARTED, got STOPPED', 'StartApplication must bring the application to STARTED, got STOPPED'.
Why: STARTED is the gate for the whole EMR Serverless data plane. A client that starts an application and polls for STARTED never sees it, so it either times out or, if it proceeds, has its StartJobRun refused by the service's own state gate ('Job run cannot be started as Application is not in STARTED state and auto-start is disabled'). The application also appears to stop itself seconds after being started even though no idle minute has elapsed, which no AWS-conformant client can account for; and because STARTED is skipped entirely, nothing downstream of it (job submission, capacity, auto-stop fidelity itself) can be exercised or graded.
Fix: the application lifecycle in the emr-serverless crate's state layer — ApplicationRecord::auto_stop_due()/settle() and the idle-span constants in services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs, together with StartApplication's activity marking in src/ops/start_application.rs. The idle clock must begin when the application actually reaches STARTED (and be reset by job-run/session activity), and the served idle span must be long enough that a client polling a lifecycle edge observes STARTED before any auto-stop falls due — an auto-stop must never be materialised in the same settle pass that first materialises STARTED.
Traces: emr.emr-serverless-failing-script-reports-failed-and-writes-no-output, emr.emr-serverless-spark-job-run-reaches-success, emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout: 36/275 passed; service emr 33/216.

[mismatch] emr.emr-serverless-failing-script-reports-failed-and-writes-no-output
check_emr_emr_serverless_failing_script_reports_failed_and_writes_no_output.py line 234:
  assert started == "STARTED", (...)
  AssertionError: the application must reach STARTED before any job run; got 'STOPPED'
(application created with autoStartConfiguration={'enabled': True}, autoStopConfiguration={'enabled': True, 'idleTimeoutMinutes': 1}; poll interval 10 s, timeout 300 s)

[mismatch] emr.emr-serverless-spark-job-run-reaches-success
check_emr_emr_serverless_spark_job_run_reaches_success.py line 232:
  assert started == "STARTED", f"application must reach STARTED, got {started!r}"
  AssertionError: application must reach STARTED, got 'STOPPED'

[mismatch] emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected
check_emr_emr_serverless_start_job_run_on_stopped_non_autostart_application_is_rejected.py line 242:
  assert started == "STARTED", (...)
  AssertionError: StartApplication must bring the application to STARTED, got 'STOPPED'

Passed before it in the third check (so these are NOT the divergence): an application created with autoStartConfiguration={'enabled': False} settles in CREATED; GetApplication reports autoStart disabled; StartJobRun is refused 400 ValidationException with the exact message "Job run cannot be started as Application is not in STARTED state and auto-start is disabled"; ListJobRuns returns []; the application state is unchanged by the refusal. The divergence begins at the StartApplication that follows.

Expected (AWS): after StartApplication (or after CreateApplication with autoStart enabled), GetApplication().application.state leaves STARTING as 'STARTED', and stays STARTED for the configured idle minutes of idleness (1 minute here) so a 10-second poller observes it.
Actual (emulator): the state leaves STARTING as 'STOPPED' — the auto-stop, served as 5 s per configured minute and measured from the StartApplication call instead of from the instant STARTED was reached, is already due when STARTED is materialised and overwrites it in the same settle pass.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 behavioral suite, checks emr.emr-serverless-spark-job-run-reaches-success, emr.emr-serverless-failing-script-reports-failed-and-writes-no-output and emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected. By hand:
2. emr-serverless CreateApplication(name=<any>, releaseLabel='emr-6.15.0', type='SPARK', maximumCapacity={'cpu':'8 vCPU','memory':'32 GB'}, autoStartConfiguration={'enabled': False}, autoStopConfiguration={'enabled': True, 'idleTimeoutMinutes': 1}).
3. Poll GetApplication(applicationId=<id>) until state != 'CREATING'. It settles in CREATED (correct).
4. Call StartApplication(applicationId=<id>). It answers 200.
5. Poll GetApplication every 10 seconds until state != 'STARTING'.
6. Observe the state reported is 'STOPPED' (with the auto-stopped stateDetails); AWS reports 'STARTED' and keeps it STARTED for the configured idle minute. Repeat with autoStartConfiguration={'enabled': True} for the other two checks — the application likewise never reports STARTED.
7. Any StartJobRun attempted after step 6 is then refused by the emulator's own not-STARTED state gate, so no job run can be submitted at all.

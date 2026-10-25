Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.emr-serverless-spark-job-run-reaches-success emr.emr-serverless-failing-script-reports-failed-and-writes-no-output
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr.emr-serverless-spark-job-run-reaches-success|run#25|AssertionError|assert final == "SUCCESS", (
Issue-Key: emr.emr-serverless-failing-script-reports-failed-and-writes-no-output|run#28|AssertionError|assert fail_job.get("totalExecutionDurationSeconds", 0) > 0, (
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# EMR Serverless job runs are recorded but never executed, so no run reaches SUCCESS and none reports an execution duration (1 cause)

This is cause 4 of
`20261022T154846Z-emr-enum-filter-refusals-use-unmodeled-validationexception-runjobflow.md`,
split out and left OPEN when that issue's other three causes were fixed: those were
EMR control-plane fidelity gaps in `com.amazonaws.emr#ElasticMapReduce`, while this one
is a data-plane gap in the separate EMR Serverless service
(`com.amazonaws.emrserverless#AwsToledoWebService`, reached as
`ctx.client('emr-serverless')`), and it needs work no part of that fix could do. Its two
checks still fail.

## Operation
StartJobRun

## Description
Cause 1 of 1: EMR Serverless job runs are recorded but never executed, so every run ends FAILED with no execution duration.
AWS: StartJobRun on a STARTED application runs the submitted Spark entry point and the run reaches state SUCCESS, with GetJobRun reporting a non-zero totalExecutionDurationSeconds; a run whose script raises reaches FAILED but still reports a non-zero totalExecutionDurationSeconds, because the driver started, worked and then raised. Recorded behavior only for the duration value's magnitude; the terminal states are the model's jobRunState values.
Emulator: every run goes to FAILED with stateDetails "This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.", and totalExecutionDurationSeconds is absent (None) even on the failure path.
Why: a client cannot observe a successful job run at all — success and failure both report FAILED — so job-orchestration logic that branches on jobRunState or measures run duration is unexercisable and any emulator-driven test of a happy path inverts.
Fix: the job-run state machine and its duration accounting in services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/start_job_run.rs and get_job_run.rs, once there is something that can RUN the submitted entry point (see the blocker below).
Traces: emr.emr-serverless-spark-job-run-reaches-success, emr.emr-serverless-failing-script-reports-failed-and-writes-no-output.

## Known blocker
Both checks submit a real Spark entry point (a script in Amazon S3) and assert on what
running it produced: the terminal state, and a duration measured over work that
happened. Satisfying them faithfully needs a Spark or Hive runtime, and this platform
provides none — no containers and no external processes — and no in-process crate
implements one. Until that changes, reporting SUCCESS or stamping a
totalExecutionDurationSeconds would be a status and a duration for work nothing ran, so
the emulator deliberately ends a submitted run in the service's modelled failure state
with a reason saying exactly that, pinned by
`services/service-com-amazonaws-emrserverless-awstoledowebservice/tests/all/application_lifecycle.rs`'s
`a_submitted_job_run_ends_in_the_modelled_failure_state`. A fixer picking this up should
either bring a real in-process execution engine for the submitted entry point, or close
it with mark_wont_fix(platform_blocker) naming that missing runtime.

## Evidence
Expected final jobRunState SUCCESS; actual AssertionError: "the Spark job run must reach SUCCESS, got 'FAILED' (stateDetails: 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.')". And on the failure path, expected non-zero totalExecutionDurationSeconds; actual AssertionError: "the FAILED run must report a non-zero totalExecutionDurationSeconds ... got None".

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. ems = client('emr-serverless'); create an application and StartApplication it.
3. StartJobRun with a trivial Spark entry point, then poll GetJobRun -> expect SUCCESS with totalExecutionDurationSeconds > 0; observe FAILED with the emulator's stateDetails and no duration.
4. Repeat with an entry point whose script raises -> expect FAILED with a non-zero totalExecutionDurationSeconds; observe no duration.
5. Re-replay emr.emr-serverless-spark-job-run-reaches-success and emr.emr-serverless-failing-script-reports-failed-and-writes-no-output.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run glue__iam__lakeformation__s3.glue-get-job-runs-lists-newest-first-and-error-message-only-on-failed
Issue-Key: glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run|_error#15|AssertionError|raise AssertionError("the call succeeded where AWS refuses it")
Issue-Key: glue__iam__lakeformation__s3.glue-get-job-runs-lists-newest-first-and-error-message-only-on-failed|run#25|AssertionError|assert "Arguments" not in by_id[second["Id"]], (
Issue-Commit: 8339e0b79c5a3f0b6ab10000bb256c51d9f94d77

# Glue job-run surface: 2 net-new causes — StartJobRun never enforces ExecutionProperty.MaxConcurrentRuns, and a run started with no overrides records the job's DefaultArguments as its own Arguments

> _Note: this issue was identified against the code base at commit `8339e0b79c5a3f0b6ab10000bb256c51d9f94d77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Two independent root causes in the Glue job-run surface, both reached by the glue__iam__lakeformation__s3 behavioral checks. They are distinct defects and must both be closed before this issue is submitted.

--- Cause 1 of 2: StartJobRun does not enforce the job's MaxConcurrentRuns admission limit ---

AWS: Smithy (services/service-com-amazonaws-glue-awsglue) — ExecutionProperty.MaxConcurrentRuns: "The maximum number of concurrent runs allowed for the job. The default is 1. An error is returned when this threshold is reached." StartJobRun models the error ConcurrentRunsExceededException (errorKind client, HTTP 400, doc "Too many jobs are being run concurrently."). Recorded behavior: with one non-terminal run of a job whose ExecutionProperty is {"MaxConcurrentRuns": 1}, a second StartJobRun on the same job is refused HTTP 400 with error code ConcurrentRunsExceededException and the fixed message text "Concurrent runs exceeded." The whole message is fixed text for this error class — it carries no request- or state-derived values; the only variable part of the refusal is WHEN it fires, which is governed by the job's current ExecutionProperty.MaxConcurrentRuns (the default 1 when the member is absent), counted over that job's runs that are not in a terminal state (SUCCEEDED/FAILED/STOPPED/TIMEOUT/ERROR). After UpdateJob raises the cap to 2, the same call must be ADMITTED and mint a new run id, and only the start that would make the (N+1)th concurrent run is refused with that same code and message. A refused start must create no job run at all — it must not appear in GetJobRuns.

Emulator: the admission check does not exist. With one active run under MaxConcurrentRuns=1, the second StartJobRun answers 2xx and mints a run id instead of refusing; the check's helper reports "the call succeeded where AWS refuses it". The emulator does store and return ExecutionProperty correctly (GetJob reads {"MaxConcurrentRuns": 1}, and reads 2 after UpdateJob) — only the enforcement at start time is missing, so the stored cap has no effect on behavior.

Why: a client that scales a job out by raising MaxConcurrentRuns, or that relies on the cap of 1 to serialize runs, gets no feedback at all. Orchestrators (and the retry loops in the AWS SDK samples and in Glue triggers/workflows) are written around catching ConcurrentRunsExceededException and backing off; against this emulator that exception never arrives, so unbounded concurrent runs are admitted, the serialization guarantee the cap exists to provide is silently lost, and code paths that only execute on the refusal are never exercised.

Fix: StartJobRun's handler — the admission step that precedes minting the run must count the named job's non-terminal runs, compare against the job's effective ExecutionProperty.MaxConcurrentRuns (defaulting to 1), and return ConcurrentRunsExceededException (400, "Concurrent runs exceeded.") without creating a run record when the count is already at the cap.

Traces: glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run

--- Cause 2 of 2: a job run started without per-run Arguments reports the job's DefaultArguments as its own Arguments ---

AWS: Smithy — JobRun.Arguments (GenericMap): "The job arguments associated with this run. For this job run, they replace the default arguments set in the job definition itself." StartJobRun's input Arguments is optional. A run's Arguments therefore holds only the per-run OVERRIDES supplied to StartJobRun; the job definition's DefaultArguments are not copied into the run record. Recorded behavior: a run started with Arguments={...} reports exactly that map on GetJobRun/GetJobRuns, and a run started with no Arguments at all has NO Arguments member in its JobRun — the member is absent, not an empty map and not a copy of DefaultArguments. This is a per-run property: whichever keys the StartJobRun request carried, and nothing else.

Emulator: a run started with no Arguments comes back from GetJobRuns carrying the job's DefaultArguments as its Arguments — in the recorded replay the job's DefaultArguments map was echoed verbatim onto the run that was started with no overrides, where AWS omits the member. (The run started WITH an override reports that override correctly, so the defect is specifically the defaulting of the member when the request omitted it.)

Why: Arguments on a JobRun is how a client reconstructs what a specific run was actually invoked with, which is the basis for reruns, audit trails and diffing a failed run against a successful one. When the emulator back-fills the job-level defaults, every run looks as though it was explicitly parameterized; a client cannot distinguish "this run overrode the defaults" from "this run used them", so a rerun built from a run's recorded Arguments pins values the original start never requested, and a change to the job's DefaultArguments appears to have been requested by historical runs.

Fix: the StartJobRun handler / job-run state record — store only the Arguments the request supplied, and the GetJobRun/GetJobRuns serializer must omit JobRun.Arguments entirely when the run carried none, rather than resolving it against the job definition's DefaultArguments.

Traces: glue__iam__lakeformation__s3.glue-get-job-runs-lists-newest-first-and-error-message-only-on-failed

## Evidence
Cause 1 — check glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run:
  Expected (AWS): StartJobRun #2 while run #1 is active, job ExecutionProperty={"MaxConcurrentRuns": 1} -> HTTP 400, Error.Code="ConcurrentRunsExceededException", Error.Message="Concurrent runs exceeded." (fixed text), and no new JobRun in GetJobRuns.
  Actual (emulator): HTTP 2xx with a JobRunId; the check's _error() helper raised
    AssertionError: the call succeeded where AWS refuses it
  at check line 57, `got = _error(lambda: glue.start_job_run(JobName=job))`.
  The preceding assertions passed, so the cap is stored and read back correctly: get_job reported ExecutionProperty == {"MaxConcurrentRuns": 1} before UpdateJob. Only the start-time admission check is absent.
  Smithy: ExecutionProperty.MaxConcurrentRuns "The default is 1. An error is returned when this threshold is reached."; StartJobRun errors include ConcurrentRunsExceededException (client, httpError 400).

Cause 2 — check glue__iam__lakeformation__s3.glue-get-job-runs-lists-newest-first-and-error-message-only-on-failed:
  Setup: CreateJob with DefaultArguments={"--outcome": "ok"}; run A started with Arguments={"--outcome": "boom"}; run B started with NO Arguments.
  Expected (AWS): GetJobRuns -> run A has Arguments == {"--outcome": "boom"}; run B has NO "Arguments" key at all.
  Actual (emulator): run B carries Arguments == {'--outcome': 'ok'} — the job's DefaultArguments echoed onto the run:
    AssertionError: a run started without overrides records none, got {'--outcome': 'ok'}
  at check line 72. (The map value shown is this recording's filler for the template: the requirement is that the member is ABSENT whenever StartJobRun supplied no Arguments, whatever the job's defaults are.)
  Smithy: JobRun.Arguments "The job arguments associated with this run. For this job run, they replace the default arguments set in the job definition itself."

## Steps to Reproduce
Cause 1 (MaxConcurrentRuns not enforced):
1. Create an S3 bucket and upload a pythonshell script that sleeps for the value of a required argument (e.g. --hold_s), so a run stays non-terminal.
2. iam:CreateRole a role assumable by this account/glue, then glue:CreateJob with Command={Name: pythonshell, PythonVersion: 3.9, ScriptLocation: s3://<bucket>/scripts/ps.py}, MaxCapacity=0.0625, DefaultArguments={"--hold_s": "180"} and no ExecutionProperty.
3. glue:GetJob -> ExecutionProperty is {"MaxConcurrentRuns": 1} (the default).
4. glue:StartJobRun(JobName=job) and wait until that run is non-terminal (not SUCCEEDED/FAILED/STOPPED/TIMEOUT/ERROR).
5. glue:StartJobRun(JobName=job) again. EXPECT HTTP 400 ConcurrentRunsExceededException "Concurrent runs exceeded."; OBSERVED a 2xx with a fresh JobRunId.
6. glue:UpdateJob with JobUpdate.ExecutionProperty={"MaxConcurrentRuns": 2}; a second start must now be admitted and only a third refused with the same code/message. GetJobRuns must contain no run for any refused start.

Cause 2 (DefaultArguments leaked into a run's Arguments):
1. glue:CreateJob with DefaultArguments={"--outcome": "ok"} (any non-empty default map).
2. glue:StartJobRun(JobName=job, Arguments={"--outcome": "boom"}) and let it reach a terminal state.
3. glue:StartJobRun(JobName=job) with NO Arguments and let it reach a terminal state.
4. glue:GetJobRuns(JobName=job) (and glue:GetJobRun for each id). EXPECT the overridden run to report Arguments == {"--outcome": "boom"} and the plain run to have no Arguments member; OBSERVED the plain run reports Arguments == {"--outcome": "ok"}, the job's DefaultArguments.

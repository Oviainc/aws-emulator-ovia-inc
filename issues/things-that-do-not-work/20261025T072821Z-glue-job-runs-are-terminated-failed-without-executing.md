Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.eventbridge-rule-filters-glue-job-name-positive
Issue-Key: athena__glue__s3.eventbridge-rule-filters-glue-job-name-positive|_run_job_to_success#18|AssertionError|assert state == "SUCCEEDED", f"job {job_name} did not SUCCEED after retries: {detail}"
Issue-Commit: c03b32b2dcc5227aba431e790e0b1e2397cf2279

# Glue job runs are terminated FAILED without executing the script, so a pythonshell job never reaches SUCCEEDED and no Glue Job State Change event can follow

> _Note: this issue was identified against the code base at commit `c03b32b2dcc5227aba431e790e0b1e2397cf2279`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One root cause, grounded in one failing behavioral check of suite simparity-behavioral-ga-prod-v1.

Cause 1 of 1 — a started Glue job run is recorded and immediately terminated FAILED without the job script ever being executed, so no run can reach SUCCEEDED.

AWS: Smithy com.amazonaws.glue#StartJobRun takes JobName (required, NameString @length 1..255) and returns JobRunId; its modeled errors are ConcurrentRunsExceededException, EntityNotFoundException, InvalidInputException, ResourceNumberLimitExceededException, OperationTimeoutException (client, HTTP 400) and InternalServiceException (server, HTTP 500). com.amazonaws.glue#GetJobRun (JobName + RunId required) returns JobRun, whose JobRunState walks through the run's lifecycle. Recorded behavior: a pythonshell job whose ScriptLocation points at a trivial script (`print('ok')`) is actually executed, and GetJobRun settles on JobRunState == 'SUCCEEDED'; the terminal transition is what then produces the 'Glue Job State Change' event on the default EventBridge bus (source 'aws.glue', detail.jobName/jobRunId/state) that a matching rule delivers to its targets.

Emulator: StartJobRun accepts the call and mints a run id, but the run settles on JobRunState 'FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' No script is run, so no run ever becomes SUCCEEDED and no successful-run side effects or events exist. The check's bounded retry loop around StartJobRun/GetJobRun re-runs the job and sees the same terminal FAILED every time.

Why: a client cannot execute any ETL work, and cannot distinguish a genuinely failing script from a run that was never attempted — the ErrorMessage is emulator prose, not an AWS job failure. Everything keyed on a successful run is unreachable: downstream catalog/S3 side effects, job bookmarks, workflow/trigger progression, and in this check the 'Glue Job State Change' SUCCEEDED event that an EventBridge rule filtering detail.jobName is supposed to deliver to its SQS target (the check never got as far as asserting that delivery, so only the non-execution is graded here; the EventBridge leg of the same check is unverified, not shown correct).

Fix: the glue job-run execution/state machine in services/service-com-amazonaws-glue-awsglue — the StartJobRun handler and the job-run state that terminates the run, plus the terminal-state event emission it should drive. Related open issues describe the same non-execution for other checks — 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md (cause 1) and 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md — but neither carries this check's replay key, so this check id is recorded here to be re-replayed with that fix.

Traces: athena__glue__s3.eventbridge-rule-filters-glue-job-name-positive.

Service-attribution note: the check's leading id token is the composite 'athena__glue__s3' (SERVICES = ['athena','glue','s3']). It failed on Glue's own job-run behavior, before any Athena call, so it is filed under 'glue', which is bootstrapped. The Athena legs of this suite are blocked separately by Athena not being bootstrapped and are not part of this issue.

## Evidence
Expected: after glue.create_job(Name=<job>, Role=<glue-trust role>, Command={'Name':'pythonshell','ScriptLocation':'s3://<bucket>/scripts/<script>.py'}) with script body `print('ok')`, glue.start_job_run(JobName=<job>) followed by polling glue.get_job_run(JobName=<job>, RunId=<id>)['JobRun']['JobRunState'] settles on 'SUCCEEDED'.

Actual: AssertionError: job sim2real-filter-job-<id> did not SUCCEED after retries: FAILED: This emulator records job runs but executes no job script, so the job run ended without running.

(The run id and the generated job/bucket names differ per environment; the requirement is the terminal JobRunState 'SUCCEEDED' for a script that exits 0, not any particular name or message text.)

## Steps to Reproduce
1. s3.create_bucket on a fresh bucket; put_object the script body `print('ok')` at scripts/<name>.py.
2. iam.create_role with a trust policy for glue.amazonaws.com, attach arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole, and put_role_policy granting s3:GetObject/PutObject/ListBucket on that bucket.
3. glue.create_job(Name=<job>, Role=<roleArn>, Command={'Name':'pythonshell','ScriptLocation':'s3://<bucket>/scripts/<name>.py'}).
4. glue.start_job_run(JobName=<job>), then poll glue.get_job_run(JobName=<job>, RunId=<runId>) until terminal.
5. Observe JobRunState 'FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.'; expect 'SUCCEEDED'.
6. Re-replay check athena__glue__s3.eventbridge-rule-filters-glue-job-name-positive, which additionally asserts that the resulting 'Glue Job State Change' SUCCEEDED event is delivered to the SQS target of an EventBridge rule whose pattern filters detail.jobName to that job.

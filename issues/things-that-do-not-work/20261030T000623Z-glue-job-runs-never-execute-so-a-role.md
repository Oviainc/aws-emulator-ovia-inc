Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.glue-job-role-missing-trust-policy-negative
Issue-Key: athena__glue__s3.glue-job-role-missing-trust-policy-negative|_start_and_wait_with_retry#22|Timeout|time.sleep(delay)
Issue-Commit: c63965f11451d24ea2dc1fce2d159a07ab797af8

# Glue job runs never execute, so a role whose trust policy omits glue.amazonaws.com is indistinguishable from a healthy run and no positive job run can ever reach SUCCEEDED (1 cause)

> _Note: this issue was identified against the code base at commit `c63965f11451d24ea2dc1fce2d159a07ab797af8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
1 cause, grounded in 1 failing behavioral check whose replay key is carried by NO open issue.

Related open issue, named for the fixer: 20261029T185330Z-glue-job-runs-are-recorded-but-never-executed-e42472.md describes the same root cause, but its Issue-Trace-Ids list does NOT contain athena__glue__s3.glue-job-role-missing-trust-policy-negative, so re-replaying that issue's trace list will not exercise this check. Other open issues on the same area: 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md, 20261025T072821Z-glue-job-runs-are-terminated-failed-without-executing.md, 20261029T175551Z-glue-job-runs-reached-through-the-emr-glue.md. The cause is restated here so this check is covered; a fixer must make the check in trace_ids pass.

Cause 1 of 1: StartJobRun records a run and then settles it into a constant emulator-authored FAILED state instead of executing the job's Command under the job's Role, so the IAM trust-policy boundary on the execution role is never a boundary and no run can ever succeed.
Failing check id: athena__glue__s3.glue-job-role-missing-trust-policy-negative.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): StartJobRun takes JobName (NameString, @required, length 1..255) and returns JobRunId; its modeled errors are ConcurrentRunsExceededException, EntityNotFoundException, InvalidInputException, OperationTimeoutException, ResourceNumberLimitExceededException (all HTTP 400) and InternalServiceException (HTTP 500). GetJobRun reports a JobRun whose JobRunState targets the JobRunState enum with values STARTING | RUNNING | STOPPING | STOPPED | SUCCEEDED | FAILED | TIMEOUT | ERROR | WAITING | EXPIRED, alongside ExecutionTime (integer seconds, default 0), StartedOn, CompletedOn and ErrorMessage ("An error message associated with this job run"). On real AWS a started run is executed by Glue after assuming the job's Role: a job whose role trusts glue.amazonaws.com runs its Command.ScriptLocation, the run passes through RUNNING, reaches SUCCEEDED with ErrorMessage absent and ExecutionTime > 0, and the script's side effects land (here, the destination S3 object holding the uppercased source bytes). A job whose role carries the same permissions but a trust policy naming a different service principal (for example s3.amazonaws.com) cannot be assumed by Glue at all: that run reaches FAILED and its ErrorMessage names the assume-role rejection for that role, not a generic sentence — the fixed text is Glue's/STS's own refusal wording, with the role ARN and the service principal taken from the job definition and the role's trust document. No output object is written in that case. The two arms therefore differ in both the terminal state and the ErrorMessage text.
Emulator: StartJobRun (services/service-com-amazonaws-glue-awsglue/src/ops/start_job_run.rs) validates Timeout/MaxCapacity/ExecutionClass, mints a JobRunId, records the run in STARTING and hands the terminal state to a clock span (crate::state::JOB_RUN_SETTLES_AFTER). It never reads or evaluates the job Role's trust policy and never executes Command.ScriptLocation. The run then settles into the constant modeled failure carrying crate::state::JOB_RUN_NOT_EXECUTED_MESSAGE — the fixed emulator sentence "This emulator records job runs but executes no job script, so the job run ended without running." — with ExecutionTime 0 and CompletedOn ~500ms after StartedOn, for every job regardless of role trust. RUNNING is never observable. Consequently the check's negative arm (mistrusted role) and its in-file positive control (an otherwise identical job whose only changed variable is a role trusting glue.amazonaws.com) both end FAILED with the identical sentence; the harness retries the positive control (3 starts, 15s apart, each polling GetJobRun to a terminal state) waiting for SUCCEEDED, which can never occur, and the check is reported as "Check timed out after 60.0s".
Why: a client cannot distinguish a real IAM defect from a healthy job. A trust policy that omits glue.amazonaws.com — a misconfiguration real AWS reports explicitly — produces byte-identical output to a correctly trusted role, so code and CI that assert on the failure reason (or that assert a job succeeded and its output object exists) are both wrong against the emulator: positive flows hang or fail forever, and negative flows pass for the wrong reason. The destination object the script was supposed to write never exists, so anything downstream of the job run (catalog tables, Glue Job State Change events, workflow/trigger chaining, bookmark advancement) is inert.
Fix: the job-run execution component of the glue service crate — the StartJobRun handler and the job-run state machine that owns run transitions. It must assume the job's Role (refusing, with the assume-role refusal text, a role whose trust policy does not name glue.amazonaws.com), execute the job's Command so script side effects and IAM refusals are real, drive JobRunState through RUNNING to SUCCEEDED/FAILED with ExecutionTime and CompletedOn set, leave ErrorMessage absent on success, and carry the underlying refusal verbatim on failure. The constant emulator sentence must not be produced at all. File and line citations are under Traces.
Traces: athena__glue__s3.glue-job-role-missing-trust-policy-negative (suite report: "Error: Check timed out after 60.0s"). Handler: services/service-com-amazonaws-glue-awsglue/src/ops/start_job_run.rs lines 3-22 (module doc states "Because this emulator runs no job script, the state the clock owes is the modelled failure state carrying crate::state::JOB_RUN_NOT_EXECUTED_MESSAGE rather than a result nothing computed") and lines 350-372 (the run is recorded and the handler returns without executing anything).

## Evidence
Check: athena__glue__s3.glue-job-role-missing-trust-policy-negative (verdict: mismatch)
Reported error: Check timed out after 60.0s

Negative arm (role trust policy names s3.amazonaws.com instead of glue.amazonaws.com, same S3/Glue permissions):
  expected: GetJobRun -> JobRunState 'FAILED' whose ErrorMessage names the assume-role rejection for that execution role (Glue cannot assume a role that does not trust glue.amazonaws.com); destination object absent.
  actual:   JobRunState 'FAILED' with the constant emulator ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' — the failure is not attributed to the trust policy.

Positive control (identical job, only the execution role swapped for one trusting glue.amazonaws.com):
  expected: the run observable in RUNNING, then JobRunState 'SUCCEEDED' with ExecutionTime > 0 and no ErrorMessage; s3:GetObject on the destination key returns the source bytes upper-cased.
  actual:   JobRunState 'FAILED' with the same constant sentence, ExecutionTime 0, CompletedOn ~500ms after StartedOn; the destination object is never created. The harness re-starts the run twice more (15s apart) waiting for SUCCEEDED and the runner's 60s budget expires -> 'Check timed out after 60.0s'.

Both arms are byte-identical apart from per-run ids and timestamps (which are per-run values, not part of the requirement). The requirement is that the two arms differ: SUCCEEDED with no ErrorMessage versus FAILED naming the assume-role refusal.

## Steps to Reproduce
1. s3:CreateBucket B; s3:PutObject B/scripts/job.py with a pythonshell script that reads --src_bucket/--src_key and writes the upper-cased bytes to --dst_bucket/--dst_key; s3:PutObject B/input/source.txt with a known body.
2. iam:CreateRole R_bad with AssumeRolePolicyDocument {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"s3.amazonaws.com"},"Action":"sts:AssumeRole"}]}; attach arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole and an inline policy allowing s3:GetObject/s3:PutObject/s3:ListBucket on B.
3. iam:CreateRole R_good — identical except the trust policy names glue.amazonaws.com.
4. glue:CreateJob(Name=J, Role=R_bad, Command={"Name":"pythonshell","ScriptLocation":"s3://B/scripts/job.py","PythonVersion":"3.9"}, MaxCapacity=0.0625, Timeout=10).
5. glue:StartJobRun(JobName=J, Arguments={...}); poll glue:GetJobRun(JobName=J, RunId=<id>) to a terminal JobRunState. Expect FAILED whose ErrorMessage names the assume-role rejection for R_bad. Today: FAILED with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.'
6. s3:HeadObject(B, output/result.txt) must 404 (nothing ran). This part holds today, but for the wrong reason.
7. glue:UpdateJob(JobName=J, JobUpdate={"Role":R_good, "Command":{...same...}}); glue:StartJobRun again and poll glue:GetJobRun. Expect the run observable in RUNNING and then SUCCEEDED with ExecutionTime > 0 and no ErrorMessage. Today it is FAILED within ~500ms with the same constant sentence, so no number of retries ever yields SUCCEEDED.
8. s3:GetObject(B, output/result.txt) must return the source body upper-cased. Today the object does not exist.

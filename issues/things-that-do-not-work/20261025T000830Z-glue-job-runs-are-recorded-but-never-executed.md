Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive
Issue-Key: emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive|run#13|AssertionError|assert run1["JobRunState"] == "SUCCEEDED", run1.get("ErrorMessage")
Issue-Key: emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Glue job runs are recorded but never executed: a started run never reaches SUCCEEDED, produces no script side effects and never advances a job bookmark (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One root cause, grounded in two failing behavioral checks of suite simparity-behavioral-ga-prod-v1.

Cause 1 of 1: Glue's job-run plane records a run but never executes it, so a run never reaches a successful terminal state and none of its observable effects happen.

AWS: Smithy com.amazonaws.glue#StartJobRun takes JobName (+Arguments/WorkerType/NumberOfWorkers/MaxCapacity/Timeout) and answers HTTP 200 with a JobRunId; its modeled errors are the client/throttling family (InvalidInputException, EntityNotFoundException, ConcurrentRunsExceededException, ResourceNumberLimitExceededException, OperationTimeoutException, InternalServiceException). GetJobRun answers 200 with JobRun, whose JobRunState is enum com.amazonaws.glue#JobRunState = STARTING|RUNNING|STOPPING|STOPPED|SUCCEEDED|FAILED|TIMEOUT|ERROR|WAITING|EXPIRED, alongside StartedOn/CompletedOn/ExecutionTime/ErrorMessage. A run of a job whose Command.ScriptLocation points at an S3 script executes that script under the job's Role: the run walks STARTING -> RUNNING -> SUCCEEDED and the script's own side effects are observable afterwards (an S3 PutObject issued from inside the script exists). With DefaultArguments {"--job-bookmark-option": "job-bookmark-enable"} a SUCCEEDED run commits a bookmark for the script's transformation_ctx, so the next run reads only source keys added since that run (the model exposes this state through GetJobBookmark/ResetJobBookmark).

Emulator: StartJobRun now answers 200 with a JobRunId (the earlier 501 scaffold was closed by the fixed issue 20261024T015202Z-glue-startjobrun-answers-notimplemented-501-scaffold-so-no.md, which carried these same two check ids) — this is therefore NOT a refusal and not a reopen of that issue: the call is served, but nothing runs. The run is only recorded. In emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive, GetJobRun reports a non-SUCCEEDED JobRunState whose ErrorMessage is the emulator's own admission, example text: 'This emulator records job runs but executes no job script, so the job run ended without running.' Consequently s3://<bucket>/output/run1/count.txt is never written, and the bookmark half of the check (run 2 seeing only the newly added file) is unreachable. In emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive the same path never yields a SUCCEEDED run, so the check spends its whole budget in the terminal-state poll plus its two documented role-propagation restarts and is killed at the suite's 60 s wall clock before reaching the UpdateJob/GetJob connection-association and GetConnection assertions; nothing ahead of the run is slow — s3.create_bucket/put_object, iam.create_role/attach_role_policy, glue.create_connection and glue.create_job all answer 200 immediately. An emulator that settles a run promptly completes that check well inside the budget, so re-replay it to confirm.

Why: the entire Glue execution surface is unusable for a client. Code that starts a job and waits for SUCCEEDED either loops to its own deadline or sees a failed run with an emulator-authored ErrorMessage instead of AWS's run states; the role the job assumes is never actually used, so no IAM-trust or S3-permission wiring can be validated; and job bookmarks never advance, so incremental-ETL logic (reprocess vs. skip already-processed keys) silently behaves as if every run were the first.

Fix: the Glue job-run state machine and its execution hook — services/service-com-amazonaws-glue-awsglue/src/ops/start_job_run.rs together with the job-run state that get_job_run.rs serializes, plus the bookmark state read by get_job_bookmark/reset_job_bookmark. A run must progress to SUCCEEDED, reproduce the script's observable S3 effects, and commit a bookmark on success. The open issue 20261023T123230Z-glue-startjobrun-is-an-unimplemented-501-scaffold-and.md names the same operation but describes the 501 scaffold and does not carry these two checks' replay keys, so this cause is recorded here.

Traces: emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive, emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive.

Service-attribution note: the leading token of both check ids is the composite 'emr__glue__s3' (the checks declare SERVICES = ['emr','glue','s3']), which is not a single service token. The failing operation is Glue's StartJobRun/GetJobRun and the state lives in the Glue crate, so this issue is filed under service 'glue' (emr, glue and s3 are all bootstrapped).

## Evidence
Expected (AWS): glue.start_job_run(JobName=j) -> 200 + JobRunId; glue.get_job_run(JobName=j, RunId=r)['JobRun']['JobRunState'] -> 'SUCCEEDED'; the script's S3 side effect present (s3://<bucket>/output/run1/count.txt containing the row count '3' for the bookmark check, s3://<bucket>/markers/conn-job.txt non-empty for the connection check); a second run with --job-bookmark-option=job-bookmark-enable sees only the file added after the first successful run (count 2, not 5).

Actual, emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive: AssertionError at check_emr__glue__s3_glue_job_bookmark_persists_processed_s3_keys_positive.py:89 — 'assert run1["JobRunState"] == "SUCCEEDED", run1.get("ErrorMessage")' failed with the run's own ErrorMessage: 'This emulator records job runs but executes no job script, so the job run ended without running.' (job created with Command {'Name':'glueetl','ScriptLocation':'s3://<bucket>/scripts/bookmark.py','PythonVersion':'3'}, GlueVersion='4.0', WorkerType='G.1X', NumberOfWorkers=2, DefaultArguments {'--job-bookmark-option':'job-bookmark-enable'}).

Actual, emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive: 'Check timed out after 60.0s'. The check polls get_job_run every 10 s until a terminal state (360 s budget) and, on a non-SUCCEEDED result, sleeps 15 s and restarts the run up to twice; it never observed SUCCEEDED and never reached its update_job/get_job connection assertions. Job created with Command {'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://<bucket>/scripts/conn-job.py'}, MaxCapacity=0.0625, Timeout=5.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=b); s3.put_object(Bucket=b, Key='scripts/job.py', Body=<script that does s3.put_object of a marker into b>).
3. iam.create_role(RoleName=r, AssumeRolePolicyDocument=<trust for glue.amazonaws.com>) and grant it s3:GetObject/PutObject/ListBucket on b.
4. glue.create_job(Name=j, Role=<role arn>, Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://b/scripts/job.py'}, MaxCapacity=0.0625, Timeout=5) -> 200.
5. rid = glue.start_job_run(JobName=j)['JobRunId'] -> 200. Poll glue.get_job_run(JobName=j, RunId=rid): expect JobRunState to reach 'SUCCEEDED'; observe a non-SUCCEEDED terminal state with ErrorMessage 'This emulator records job runs but executes no job script...'.
6. s3.head_object(Bucket=b, Key=<marker the script writes>) -> expect it to exist; observe it missing.
7. Repeat with Command {'Name':'glueetl'}, GlueVersion='4.0' and DefaultArguments {'--job-bookmark-option':'job-bookmark-enable'}; add a second source file after the first successful run and confirm the second run processes only the new file.
8. Re-replay the two check ids in trace_ids.

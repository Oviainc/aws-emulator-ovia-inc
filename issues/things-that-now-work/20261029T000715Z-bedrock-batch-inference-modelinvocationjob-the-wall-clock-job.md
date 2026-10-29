Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.model-invocation-job-lifecycle bedrock.e2e-batch-inference-job-produces-output
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.model-invocation-job-lifecycle|run#16|AssertionError|assert started_running["status"] == "InProgress", (
Issue-Key: bedrock.e2e-batch-inference-job-produces-output|_wait_job_status#14|Timeout|time.sleep(interval_s)
Issue-Commit: c441cdb97fe099c25286b36cfc473da57b1ed5b6

# bedrock batch inference (ModelInvocationJob): the wall-clock job progression makes InProgress unobservable to any realistic poller, so StopModelInvocationJob can never reach a running job and a completion-and-output wait never settles

> _Note: this issue was identified against the code base at commit `c441cdb97fe099c25286b36cfc473da57b1ed5b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModelInvocationJob

## Description
Two root causes in the ModelInvocationJob (batch inference) state machine of the bedrock control plane. Both are grounded in failing behavioral checks; neither is marked ALREADY FILED or OWNED, and no open issue covers them (the closest, 20261024T094806Z-bedrock-runtime-async-invoke-startasyncinvoke-over-validates-modelinput.md, is bedrock-runtime StartAsyncInvoke, a different service and operation).

Cause 1 of 2: a submitted job never dwells in InProgress long enough for a polling client to observe it, so the Stop-a-running-job path is unreachable.
AWS: Smithy ModelInvocationJobStatus (com.amazonaws.bedrock#ModelInvocationJobStatus) models the statuses Submitted | Validating | Scheduled | InProgress | Stopping | Stopped | Completed | PartiallyCompleted | Failed | Expired, and GetModelInvocationJob returns that status plus totalRecordCount/processedRecordCount/successRecordCount/errorRecordCount. InProgress means the job has begun running its records; on AWS a batch inference job of any real size runs for minutes-to-hours, so a client polling GetModelInvocationJob at ordinary intervals (seconds to tens of seconds) reliably observes Submitted/Validating/Scheduled, then InProgress, and StopModelInvocationJob (POST /model-invocation-job/{jobIdentifier}/stop) applied to that running job drives it Stopping -> Stopped. StopModelInvocationJob models ConflictException (HTTP 400) for a job that is no longer stoppable, so whether Stop is usable at all is decided by how long the job stays non-terminal.
Emulator: the whole progression is a fixed wall clock armed at submission, and it is ~4.5 seconds end to end: VALIDATES_AFTER 500ms, SCHEDULES_AFTER 500ms, BEGINS_AFTER 500ms, COMPLETE_AFTER 3s. InProgress therefore exists only in the window t+1.5s .. t+4.5s after CreateModelInvocationJob. A client polling every 10s sees Submitted on its first read and Completed on its second; the documented InProgress status is never returned to it, and by the time it calls StopModelInvocationJob the job is already terminal. The replay reports exactly that: "AssertionError: job reached 'Completed' before Stop, expected InProgress". Because the status a caller observes depends on that caller's poll interval rather than on the job's work, the Stopping/Stopped half of the modelled enum is unreachable in practice.
Why: a client that implements the documented control flow — submit, poll until the job is running, then cancel it — can never cancel a job on this emulator, and can never observe a non-terminal running job or its advancing processedRecordCount. Code written against the emulator will look correct and then diverge on AWS, where the job is InProgress for a long time; conversely a cancellation path exercised against the emulator is never exercised at all. Any state-machine assertion over Submitted -> ... -> InProgress -> Stopped is untestable.
Fix: the job's status progression in the control plane's state — the clock that CreateModelInvocationJob arms and that GetModelInvocationJob/ListModelInvocationJobs materialise. Progression should be observable to a poller rather than decided by fixed short wall-clock spans: advance at most one modelled stage per observation of a job (or hold the job in InProgress until the run over its records is accounted for) so that Submitted, Validating, Scheduled and InProgress are each readable, and a Stop issued against the job it just read as InProgress lands on a non-terminal job and yields Stopping then Stopped.
Traces: bedrock.model-invocation-job-lifecycle.

Cause 2 of 2: a job's terminal status and its written output are not observable within a single completion wait, so a submit-then-wait-for-Completed-and-output flow never settles.
AWS: GetModelInvocationJob reports the terminal status (Completed / PartiallyCompleted / Failed / Stopped / Expired) together with successRecordCount, and a Completed job has written its result records under outputDataConfig.s3OutputDataConfig.s3Uri, so a client can list that prefix and find non-empty objects. recorded behavior only for the exact timing: the recording shows the end-to-end flow (create bucket, upload a 100-record .jsonl input, create the batch role, CreateModelInvocationJob, poll GetModelInvocationJob to a terminal status, then ListObjectsV2 on the output prefix) completing and asserting status == Completed, successRecordCount == the submitted record count, and at least one output object of non-zero size.
Emulator: the check never gets that far — it is killed with "Check timed out after 60.0s" while waiting on GetModelInvocationJob. The emulator's job is still non-terminal on the first read (the clock above leaves it Submitted for the first 500ms and non-terminal for 4.5s) and the lifecycle is materialised only lazily, on a later call that touches the record, so a client whose wait loop re-reads only after a long interval has no terminal status, no successRecordCount and no output objects to find inside its budget. Hypothesis: the same wall-clock-vs-observation coupling as Cause 1 — the job's advance is tied to elapsed wall time between calls rather than to the reads that observe it, so the terminal status and the output write land in a window the client is not reading in.
Why: the observable consequence a batch job exists for — output records actually written under the output prefix, status Completed with successRecordCount matching the input — cannot be confirmed by a client following the documented submit-and-wait flow. A client gets a job that is neither reportably running nor reportably finished, which on AWS cannot happen.
Fix: the same component as Cause 1 — the ModelInvocationJob clock/settle path in the control plane's state and the run that writes result records under the output prefix. The terminal status, the record counts and the output objects must become observable to a client that polls GetModelInvocationJob, independently of how long that client waits between polls.
Traces: bedrock.e2e-batch-inference-job-produces-output.

## Evidence
Cause 1 — check bedrock.model-invocation-job-lifecycle (verdict: mismatch):
  AssertionError: job reached 'Completed' before Stop, expected InProgress
  (check_bedrock_model_invocation_job_lifecycle.py line 60; the check polls GetModelInvocationJob every 10s, timeout 600s, until status is not one of Submitted/Validating/Scheduled, then asserts InProgress and calls StopModelInvocationJob expecting a terminal Stopped.)
Expected: the first status after the pre-start stages is "InProgress"; StopModelInvocationJob on it then yields "Stopping" and finally "Stopped" (all values of the modelled ModelInvocationJobStatus enum).
Actual: "Completed". The pre-terminal stages are consumed by a fixed wall clock armed at submission:
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:1997 MODEL_INVOCATION_JOB_STOPPED_AFTER = 500ms
  .../src/state.rs:2007 MODEL_INVOCATION_JOB_VALIDATES_AFTER = 500ms
  .../src/state.rs:2014 MODEL_INVOCATION_JOB_SCHEDULES_AFTER = 500ms
  .../src/state.rs:2020 MODEL_INVOCATION_JOB_BEGINS_AFTER = 500ms
  .../src/state.rs:2039 MODEL_INVOCATION_JOB_COMPLETE_AFTER = 3s
  .../src/state.rs:2066 ModelInvocationJobClock::armed_at computes validating/scheduled/in_progress/completed instants from submit_time
  .../src/ops/create_model_invocation_job.rs:329 arms that clock on submission
  .../src/ops/get_model_invocation_job.rs:239-266 materialises whatever the clock has already made due at read time
  => InProgress exists only for t+1.5s..t+4.5s; a 10s poller cannot observe it, and Stopping/Stopped are unreachable.

Cause 2 — check bedrock.e2e-batch-inference-job-produces-output (verdict: mismatch):
  Error: Check timed out after 60.0s
  (the check waits on GetModelInvocationJob for a terminal status, then asserts status == "Completed", successRecordCount == 100, and that ListObjectsV2 on the output/ prefix returns at least one object with Size > 0.)
Expected: a terminal "Completed" job with successRecordCount equal to the submitted record count, and non-empty result objects under outputDataConfig.s3OutputDataConfig.s3Uri.
Actual: no terminal status is ever observed by the waiting client; the check is killed before any of those assertions can run.

Smithy used: com.amazonaws.bedrock#ModelInvocationJobStatus enum = [Submitted, InProgress, Completed, Failed, Stopping, Stopped, PartiallyCompleted, Expired, Validating, Scheduled]; GetModelInvocationJob GET /model-invocation-job/{jobIdentifier} -> status, message, totalRecordCount, processedRecordCount, successRecordCount, errorRecordCount; StopModelInvocationJob POST /model-invocation-job/{jobIdentifier}/stop, errors incl. ConflictException (HTTP 400); CreateModelInvocationJob POST /model-invocation-job -> jobArn.

## Steps to Reproduce
Cause 1:
1. Create an S3 bucket and upload a 100-record .jsonl batch input; create an IAM role trusted by bedrock.amazonaws.com with s3:GetObject/PutObject/ListBucket on the bucket.
2. CreateModelInvocationJob with that roleArn, a batch-capable text modelId, inputDataConfig.s3InputDataConfig.s3Uri pointing at the uploaded object and outputDataConfig.s3OutputDataConfig.s3Uri pointing at an output prefix.
3. Poll GetModelInvocationJob(jobIdentifier=jobArn) every 10 seconds until status is not one of Submitted/Validating/Scheduled.
4. Observe the first such status is "Completed" (expected "InProgress"), so there is no running job left for StopModelInvocationJob to drive to "Stopped".

Cause 2:
1. Same setup as steps 1-2 above.
2. Poll GetModelInvocationJob at a long interval (e.g. every 60 seconds) waiting for any of Completed/PartiallyCompleted/Failed/Stopped/Expired.
3. Observe the wait never settles inside the client's budget: no terminal status is returned to the poller, so status == "Completed", successRecordCount == 100 and ListObjectsV2 on the output prefix returning non-empty objects can never be asserted.

Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.e2e-batch-inference-job-produces-output
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.e2e-batch-inference-job-produces-output|_wait_job_status#14|Timeout|time.sleep(interval_s)
Issue-Commit: f0a30a453d4da98b65a93cfb3097ca05ddb2711e

# bedrock batch inference job: the InProgress dwell is a fixed 60-second wall-clock span, so CreateModelInvocationJob never reaches a terminal status and no output is ever observable

> _Note: this issue was identified against the code base at commit `f0a30a453d4da98b65a93cfb3097ca05ddb2711e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetModelInvocationJob

## Description
ONE CAUSE reached through the `bedrock` behavioral checks. Smithy model read: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json.

Cause 1 of 1 - a submitted batch inference job cannot be observed to finish, because its move out of `InProgress` is gated behind a fixed 60-second wall-clock span rather than on the work the job has to do.

AWS: Smithy `CreateModelInvocationJob` -> POST /model-invocation-job, input {jobName, roleArn, modelId, inputDataConfig, outputDataConfig required}, output {jobArn required}; `GetModelInvocationJob` reports `status` from the ModelInvocationJobStatus enum (Submitted | Validating | Scheduled | InProgress | PartiallyCompleted | Completed | Failed | Stopping | Stopped | Expired) together with `totalRecordCount`, `processedRecordCount`, `successRecordCount` and `errorRecordCount`. The modeled documentation of that member ties each status to real work: `InProgress` is "this job has begun. You can start viewing the results in the output S3 location" and the terminal `Completed` is the point at which "view the output files in the output S3 location" holds. The time a real job spends running is a function of its input (here 100 one-line .jsonl records with max_gen_len 16) and of queue depth - it is not a constant, and a tiny input finishes quickly. The observable contract a client depends on is: poll GetModelInvocationJob until `status` is terminal, then find one result record per input record under the `outputDataConfig` prefix.

Emulator: the submission arms the whole progression from fixed constants (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs: MODEL_INVOCATION_JOB_VALIDATES_AFTER = 500ms, MODEL_INVOCATION_JOB_SCHEDULES_AFTER = 500ms, MODEL_INVOCATION_JOB_BEGINS_AFTER = 500ms, MODEL_INVOCATION_JOB_COMPLETE_AFTER = **60 seconds**; ModelInvocationJobClock::armed_at at state.rs:2054). The terminal status is therefore first due at submit_time + 61.5s REGARDLESS of how many records the input holds, and the run that writes the output records is only materialised once the clock has carried the job into `InProgress` (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/get_model_invocation_job.rs, `job.due_run()` / `run_batch_inference`). The check submits the job and then polls GetModelInvocationJob; it is killed with `Check timed out after 60.0s` before the 61.5s terminal instant can ever come due, so `status` is still one of Submitted/Validating/Scheduled/InProgress, `successRecordCount` is still 0, and nothing has been written under the `output/` prefix. Every other job-like resource in this same state module uses sub-second spans (500ms); this one dwell is three orders of magnitude larger and is the only thing standing between the submission and the observable result.

Why: a client cannot observe the one thing a batch inference job exists to produce. Any caller - a test, a CI step, a workflow with a step timeout, or the emulator's own consumers - that submits a job and waits for a terminal status must block for over a minute of pure wall clock even for a 100-record input, and anything with a budget at or under that minute concludes the job hung: it reads a non-terminal status forever, finds an empty output prefix, and cannot distinguish "still running" from "the emulator will never finish this". The emulator's purpose as a stand-in for the long real job is defeated exactly where it matters, and the whole downstream contract (terminal status, successRecordCount == totalRecordCount, one output object per input object) is unreachable.

Fix: the batch inference job clock in the Bedrock control plane's state - MODEL_INVOCATION_JOB_COMPLETE_AFTER in services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs, and the `due_run`/`record_run` settle path it drives that ops/get_model_invocation_job.rs and ops/list_model_invocation_jobs.rs read through. The dwell must be of a size with the other spans of that same clock (sub-second, as MODEL_INVOCATION_JOB_VALIDATES_AFTER / SCHEDULES_AFTER / BEGINS_AFTER already are) so each documented status still dwells long enough to be read, while the terminal status and the output records become reachable in a single polling pass. Note the constant's own doc comment is stale on the point it justifies itself with - it claims "this emulator reads no input and runs no inference", but state/model_invocation_run.rs does both now, so the span is not standing in for work that is actually performed.

Traces: bedrock.e2e-batch-inference-job-produces-output.

Related but NOT this: 20261028T021122Z-bedrock-an-evaluation-job-reports-completed-but-performs.md (evaluation jobs reach Completed but write nothing) and 20261024T094806Z-bedrock-runtime-async-invoke-startasyncinvoke-over-validates-modelinput.md (async invoke jobs never reach Completed) are different resources and different root causes; this one is the model-invocation (batch inference) job's clock.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, bedrock 56/75 passed. The emulator was serving (200s and modeled 4xx observed throughout the run), so this is a fidelity finding and not a bootstrap gap.

Failing check bedrock.e2e-batch-inference-job-produces-output, verdict mismatch:
  Error: Check timed out after 60.0s

What the check does before it is killed (from the replayed source): creates a bucket, puts a 100-record .jsonl input, creates the batch role, then
  bedrock.create_model_invocation_job(jobName=..., roleArn=..., modelId='meta.llama3-1-8b-instruct-v1:0', inputDataConfig={'s3InputDataConfig': {'s3Uri': 's3://<bucket>/batch-input.jsonl'}}, outputDataConfig={'s3OutputDataConfig': {'s3Uri': 's3://<bucket>/output/'}})
and then polls get_model_invocation_job(jobIdentifier=<jobArn>) waiting for one of {Completed, PartiallyCompleted, Failed, Stopped, Expired}, asserting afterwards
  assert job['status'] == 'Completed'
  assert job.get('successRecordCount', 0) == 100
  assert [o for o in s3.list_objects_v2(Bucket=bucket, Prefix='output/')['Contents'] if o['Size'] > 0]
None of those three assertions is ever reached.

Emulator code evidence (why the terminal status cannot come due inside the budget):
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:1991  MODEL_INVOCATION_JOB_STOPPED_AFTER   = Duration::from_millis(500)
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:2001  MODEL_INVOCATION_JOB_VALIDATES_AFTER = Duration::from_millis(500)
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:2008  MODEL_INVOCATION_JOB_SCHEDULES_AFTER = Duration::from_millis(500)
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:2014  MODEL_INVOCATION_JOB_BEGINS_AFTER    = Duration::from_millis(500)
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:2027  MODEL_INVOCATION_JOB_COMPLETE_AFTER  = Duration::from_secs(60)
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:2054  ModelInvocationJobClock::armed_at - completed_at = submit + 500ms + 500ms + 500ms + 60s
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_model_invocation_job.rs:329  let clock = ModelInvocationJobClock::armed_at(now)
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/get_model_invocation_job.rs:254-266  the output-writing run is materialised only from job.due_run(), i.e. only once the clock has reached InProgress

Model evidence: smithy_query on the bedrock control-plane model for CreateModelInvocationJob (required inputDataConfig/jobName/modelId/outputDataConfig/roleArn, output jobArn, errors AccessDenied 403 / Conflict 400 / InternalServer 500 / ResourceNotFound 404 / ServiceQuotaExceeded 400 / Throttling 429 / Validation 400) and GetModelInvocationJob (status -> ModelInvocationJobStatus with the ten documented values, plus totalRecordCount / processedRecordCount / successRecordCount / errorRecordCount).

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay bedrock.e2e-batch-inference-job-produces-output.
3. By hand: create a bucket, PutObject a 100-line .jsonl input (each line {"recordId": ..., "modelInput": {...}}), create an IAM role trusted by bedrock.amazonaws.com, then call CreateModelInvocationJob naming that input and an output prefix.
4. Immediately poll GetModelInvocationJob(jobIdentifier=<jobArn>) in a tight loop and record the wall-clock time at which `status` first becomes terminal. Expected: a terminal status within a single short polling pass for a 100-record input, then one result record per input record under the output prefix with successRecordCount == totalRecordCount == 100. Observed: the status stays Submitted/Validating/Scheduled/InProgress and the terminal status is not due until submit_time + 61.5s, so a caller with a 60s budget never sees it and ListObjectsV2 on the output prefix stays empty.
5. Confirm the gate is the constant, not the work: the input read and the per-record inference are already performed by state/model_invocation_run.rs, and the only thing deferring them is MODEL_INVOCATION_JOB_COMPLETE_AFTER in state.rs.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.e2e-evaluation-job-with-guardrail-results-teardown
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.e2e-evaluation-job-with-guardrail-results-teardown|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# An accepted automated evaluation job is pinned InProgress forever: GetEvaluationJob never reports Completed/Failed/Stopped

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEvaluationJob

## Description
Cause 1 of 1: an evaluation job minted by CreateEvaluationJob never leaves the non-terminal `InProgress` status, so every client that polls GetEvaluationJob for a terminal status loops until it gives up.

AWS: Smithy model com.amazonaws.bedrock — `EvaluationJobStatus` is an enum with values InProgress | Completed | Failed | Stopping | Stopped | Deleting, and `GetEvaluationJob.status` is a required member documented as "The current status of the evaluation job". CreateEvaluationJob is documented as creating a job that runs asynchronously (HTTP 202), i.e. the status is expected to advance off InProgress to a terminal value (Completed, or Failed with `failureMessages`, or Stopped after StopEvaluationJob). `GetEvaluationJob.failureMessages` is modeled (ErrorMessages, len ..20) precisely so a job that ends badly can report why. Recorded behavior: the check polls GetEvaluationJob for a status in {Completed, Failed, Stopped} and this is the normal terminal progression for a Builtin automated job.

Emulator: CreateEvaluationJob stores the record with `status: crate::state::EVALUATION_JOB_STATUS_IN_PROGRESS` and the handler's own doc comment states this "is the only status this operation mints: see crate::state::EVALUATION_JOB_STATUS_IN_PROGRESS for why no clock transition moves it". GetEvaluationJob is a pure read whose doc comment states "an evaluation job has no transition the clock owes ... so this handler writes nothing and two reads of one job answer alike", and it hard-codes `failure_messages: None` with the comment "this emulator mints no failed job". The result in replay: the check's bounded poll for a terminal status never succeeds and the check is killed by the harness ('Check timed out after 60.0s') while every GetEvaluationJob response still says InProgress. Note this is a different resource from the ones covered by the open issues 20261024T094806Z-bedrock-runtime-async-invoke-startasyncinvoke-over-validates-modelinput.md (bedrock-runtime async invoke jobs) and 20261024T175106Z-bedrock-the-account-enforced-guardrail-configuration-is-stored.md (enforced guardrail not applied); neither covers the evaluation-job lifecycle.

Why: a client that creates an evaluation job and waits for it — the only supported way to consume one, since the results are published only when the job ends — hangs indefinitely against the emulator. Every downstream step of the lifecycle becomes unreachable: reading the job's output from the configured outputDataConfig S3 prefix, asserting a Failed job's failureMessages, and the delete/stop legs that are only valid once the job is terminal. Code written and tested against the emulator will deadlock the first time it runs, and conversely no test can ever observe the emulator's terminal-state handling because that state is unreachable.

Fix: the evaluation-job state in the control-plane crate's state module — the record needs a clock-owed transition instant (the same pattern the crate already uses for model import jobs' move to Completed and for marketplace endpoints' move to InService), and GetEvaluationJob/ListEvaluationJobs must settle that transition on read so a created job advances InProgress -> Completed (and Stopping -> Stopped after StopEvaluationJob) instead of being pinned.

Traces: bedrock.e2e-evaluation-job-with-guardrail-results-teardown. Handler evidence: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_evaluation_job.rs lines 7-10 and 302-306 (status set to EVALUATION_JOB_STATUS_IN_PROGRESS, "no clock transition moves it"); .../src/ops/get_evaluation_job.rs lines 7-10 and 222-224 (pure read, failure_messages hard-coded None).

## Evidence
Expected (AWS): CreateEvaluationJob accepts the job (202) and a subsequent GetEvaluationJob eventually reports `status` as one of the terminal EvaluationJobStatus values Completed | Failed | Stopped, with failureMessages populated for a Failed job.

Actual (emulator): GetEvaluationJob reports `status: "InProgress"` on every poll, indefinitely, and `failureMessages` is always absent. The replayed check bounded-polls GetEvaluationJob for a terminal status and never sees one; the suite reports it as 'Error: Check timed out after 60.0s'.

Source evidence in the checkout: create_evaluation_job.rs sets `status: crate::state::EVALUATION_JOB_STATUS_IN_PROGRESS` with the comment that this is the only status the operation mints and "no clock transition moves it"; get_evaluation_job.rs documents "an evaluation job has no transition the clock owes ... two reads of one job answer alike" and returns `failure_messages: None` with the comment "this emulator mints no failed job".

## Steps to Reproduce
1. Create an S3 bucket and an IAM role Bedrock can assume for evaluation output.
2. Call CreateEvaluationJob with an automated evaluationConfig (e.g. datasetMetricConfigs with taskType=Generation, dataset Builtin.RealToxicityPrompts, metricNames=[Builtin.Toxicity]), inferenceConfig naming one foundation-model ARN, and outputDataConfig.s3Uri pointing at that bucket. Note the returned jobArn.
3. Poll GetEvaluationJob(jobIdentifier=jobArn) repeatedly.
4. Observe `status` is "InProgress" on every response and never becomes Completed, Failed or Stopped, so any wait-for-terminal loop never exits (the replayed check is killed at its 60s budget).

Replay: bedrock.e2e-evaluation-job-with-guardrail-results-teardown

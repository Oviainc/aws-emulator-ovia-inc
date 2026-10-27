Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.put-action-revision-concurrency-limit-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.put-action-revision-concurrency-limit-rejected|run#11|AssertionError|assert limit_code == "ConcurrentPipelineExecutionsLimitExceededException", (
Issue-Commit: ae1f47ba1ed091886b5d17d179a2bc0dadf4c0eb

# CodePipeline: PutActionRevision never enforces the per-pipeline active-execution ceiling — a new revision past 50 active executions is accepted instead of ConcurrentPipelineExecutionsLimitExceededException

> _Note: this issue was identified against the code base at commit `ae1f47ba1ed091886b5d17d179a2bc0dadf4c0eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutActionRevision

## Description
One net-new root cause found in the codepipeline behavioral sweep. The failing check it is grounded in is NOT marked ALREADY FILED and is NOT marked OWNED by the replay report. Related open issue: 20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md — its Cause 10 covers the same missing accounting reached through StartPipelineExecution (trace ids codepipeline.concurrent-pipeline-executions-limit-exceeded and codepipeline.start-pipeline-execution-concurrency-limit-exceeded). That issue does NOT carry the check id below and does not mention PutActionRevision at all, so the revision-triggered entry point into the execution-creation path is left unverified by it; re-replay this check as well.

=== Cause 1 of 1: the active-execution ceiling is not enforced on the PutActionRevision execution-creation path ===
AWS: Smithy: PutActionRevision takes pipelineName, stageName, actionName and actionRevision (all required) and returns {newRevision (boolean, default false), pipelineExecutionId}. Its modeled error set includes ConcurrentPipelineExecutionsLimitExceededException (errorKind client, httpError 400, documented "The pipeline has reached the limit for concurrent pipeline executions."), alongside ActionNotFoundException, PipelineNotFoundException, StageNotFoundException and ValidationException. Posting a NEW action revision to a source action starts a new pipeline execution, so it consumes the same per-pipeline active-execution budget that StartPipelineExecution does; for a PARALLEL-executionMode V2 pipeline that budget is the non-adjustable "Active executions per pipeline" quota of 50 (AWS General Reference, CodePipeline quotas). Once the pipeline already holds that many active executions, the next PutActionRevision carrying a fresh revisionId is REFUSED: HTTP 400, error code ConcurrentPipelineExecutionsLimitExceededException, message as AWS words it for this error class — the fixed text "The pipeline has reached the limit for concurrent pipeline executions." with no variable part; where a pipeline identifier appears it is the pipelineName taken from the request, never a literal from one run. No execution is created by the refused call.
Emulator: every PutActionRevision succeeds. The check submits a positive-control revision and then up to 70 further fresh revisions sequentially (with ThrottlingException/TooManyRequestsException backoff), and the ceiling is never reached: "never hit the concurrency ceiling after 71 executions; last error code=None". Each call returns HTTP 200 with newRevision=true and a distinct pipelineExecutionId, so 71 distinct executions accumulate on one PARALLEL pipeline and ConcurrentPipelineExecutionsLimitExceededException is never produced for this operation. Because the ceiling never trips, the check's follow-on assertions — that ListPipelineExecutions reports at most 50 active executions, and that the refused call creates no extra execution (before_count == after_count) — are never reached either.
Why: a client that handles the documented ceiling (back off, queue the revision, surface the quota to the operator) can never exercise that path against the emulator, so a load-shedding or quota-handling regression ships unnoticed. Worse, the emulator reports unbounded successful executions where AWS stops at 50, so capacity-planning and fan-out tests draw the opposite conclusion from the same code, and a negative test asserting the 400 passes against AWS and fails here.
Fix: concurrency accounting in the PutActionRevision handler and the pipeline-execution state it creates into — the active-execution count must be read and the ceiling applied on the revision-triggered execution-start path, not only on StartPipelineExecution, and the refused call must leave the execution set unchanged. This is the same accounting the related issue's Cause 10 names for StartPipelineExecution; both entry points must consult it.
Traces: codepipeline.put-action-revision-concurrency-limit-rejected.

## Evidence
Check codepipeline.put-action-revision-concurrency-limit-rejected [mismatch].

Expected (AWS): after the pipeline's active-execution ceiling is reached, PutActionRevision with a fresh revisionId is refused with HTTP 400, error code ConcurrentPipelineExecutionsLimitExceededException (Smithy: PutActionRevision.errors, errorKind client, httpError 400) and message "The pipeline has reached the limit for concurrent pipeline executions." — that text is fixed, with no per-run values; any pipeline identifier in it comes from the request's pipelineName. ListPipelineExecutions then reports <= 50 active executions and the refused call adds none.

Actual (emulator): all 71 sequential PutActionRevision calls returned HTTP 200 with newRevision=true and a fresh pipelineExecutionId. Verbatim assertion:
  File "check_codepipeline_put_action_revision_concurrency_limit_rejected.py", line 63, in run
    assert limit_code == "ConcurrentPipelineExecutionsLimitExceededException", (
  AssertionError: never hit the concurrency ceiling after 71 executions; last error code=None

'last error code=None' means no ClientError was raised at all — not a wrong error code, but no refusal. The downstream assertions (before_count <= 50 for the PARALLEL ceiling, and after_count == before_count proving the rejected call creates no execution) were therefore never evaluated.

## Steps to Reproduce
1. Boot the emulator and replay codepipeline.put-action-revision-concurrency-limit-rejected.
2. Create a versioned S3 source bucket with an object key (e.g. source.zip) and an IAM role trusted by codepipeline.amazonaws.com with s3:* on that bucket.
3. CreatePipeline with pipelineType="V2", executionMode="PARALLEL", artifactStore {type: S3, location: <bucket>}, a Source stage with an S3 Source action (PollForSourceChanges="false") producing outputArtifacts, and a Manual Approval stage.
4. Call PutActionRevision(pipelineName=<name>, stageName="Source", actionName="SourceAction", actionRevision={revisionId: <fresh value>, revisionChangeId: <fresh value>, created: now}) repeatedly with a DISTINCT revisionId each time, retrying ThrottlingException/TooManyRequestsException with backoff.
5. Observe: every call returns HTTP 200 with newRevision=true and a new pipelineExecutionId; after 71 calls no ConcurrentPipelineExecutionsLimitExceededException has been raised. AWS refuses the call once the pipeline holds 50 active executions (HTTP 400, that error code).
6. Confirm the second symptom: ListPipelineExecutions(pipelineName=<name>) reports more than 50 distinct pipelineExecutionIds, where AWS caps the active set at 50.

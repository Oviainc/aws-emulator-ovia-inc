Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.disabled-transition-withhold-is-bounded-by-a-concurrent-enabled-twin
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.disabled-transition-withhold-is-bounded-by-a-concurrent-enabled-twin|_poll#27|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: 004a13bc6b62a404600c0a09686b1d7dd87c7440

# CodePipeline execution plane: an explicitly started execution of a pipeline with NO transition hold never delivers its S3 Deploy artifact, so the transition-twin control exhausts its budget

> _Note: this issue was identified against the code base at commit `004a13bc6b62a404600c0a09686b1d7dd87c7440`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
ONE net-new root cause (Cause 1 of 1). A fixer must close it before submitting.

Related but NOT covering this failure: the open issue 20261025T032228Z-codepipeline-execution-engine-still-stalls-an-action-never.md lists this same check id under its Cause 1, but it carries the check's PREVIOUS replay key (codepipeline.disabled-transition-withhold-is-bounded-by-a-concurrent-enabled-twin|_poll#27|Timeout|time.sleep(interval_s)) and this replay marks the check neither ALREADY FILED nor OWNED, so no open issue carries its current key. The failure point has also moved: the emulator now gets through pipeline creation, DisableStageTransition, abandoning the auto-started executions to a terminal status, and both GetPipelineState inboundTransitionState assertions, and fails later, on the CONTROL pipeline's delivery.

Cause 1 of 1: a pipeline execution started by StartPipelineExecution on a pipeline whose stage transitions are all ENABLED never writes its Deploy stage's output object to the artifact bucket.

AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models StartPipelineExecution(name) -> pipelineExecutionId (pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$, errors ConcurrentPipelineExecutionsLimitExceededException, ConflictException, PipelineNotFoundException, ValidationException) and GetPipelineState -> stageStates[].inboundTransitionState; EnableStageTransition/DisableStageTransition toggle that state over StageTransitionType Inbound|Outbound. Beyond the model this is recorded behavior only: after StartPipelineExecution returns, the service DRIVES the execution — the S3 Source action reads the configured S3Bucket/S3ObjectKey, the artifact is written to the artifactStore, the Deploy stage's S3 action is entered as soon as its inbound transition is enabled, and the deploy object appears under the configured ObjectKey; GetPipelineExecution then reports the terminal status Succeeded. A pipeline with NO disabled transition has nothing withholding it, so its artifact appears without client intervention.
Emulator: the check builds two identical two-stage (S3 Source -> S3 Deploy) pipelines, disables only the HELD pipeline's Deploy inbound transition, and starts both in the same instant. The control (twin) pipeline — whose Deploy inbound transition GetPipelineState reports as enabled, asserted before the graded executions start — never produces its deploy object: polling S3 GetObject for the twin's deploy key returns NoSuchKey for the whole remaining budget and the check dies with 'TimeoutError: the transition-ENABLED twin <pipeline> to deliver <deploy key>; if this times out the control is broken and the withhold below would prove nothing: the run budget ended first; not graded' (the pipeline name and key in that text are generated per run; the requirement is that the object exists, not its name). Everything before that point now succeeds, so the stall is specifically in advancing an unheld execution from a started state to the Deploy delivery.
Why: a client that starts a pipeline and waits for the deployed artifact waits for ever, with no error and no failed execution to inspect — the normal CodePipeline contract (start, then observe the artifact and a terminal status) is unusable. It also makes the DisableStageTransition/EnableStageTransition semantics unverifiable: with no working unheld baseline, a withheld artifact cannot be distinguished from an engine that simply never delivers, so every gated-deploy workflow is both broken and undiagnosable.
Fix: the codepipeline crate's execution plane — the driver behind StartPipelineExecution that advances a started execution through its stages, the S3 Deploy action handler that writes the output object into the artifactStore bucket, and the execution/stage status it publishes to GetPipelineState and GetPipelineExecution.
Traces: codepipeline.disabled-transition-withhold-is-bounded-by-a-concurrent-enabled-twin.

## Evidence
Verbatim from the replay report (the pipeline name is generated per run and is only an example):
  #### [error] codepipeline.disabled-transition-withhold-is-bounded-by-a-concurrent-enabled-twin
  Error: File "check_codepipeline_disabled_transition_withhold_is_bounded_by_a_concurrent_enabled_twin.py", line 92, in run
  twin_body = _poll(ctx, lambda: _get_object(s3, bucket, twin_key), lambda b: b is not None,
  File "check_codepipeline_disabled_transition_withhold_is_bounded_by_a_concurrent_enabled_twin.py", line 191, in _poll
  raise TimeoutError(f"{what}: the run budget ended first; not graded")
  TimeoutError: the transition-ENABLED twin 'sim2real-behav-cptwin-fea3adcb8901' to deliver 'deployed/twin.zip'; if this times out the control is broken and the withhold below would prove nothing: the run budget ended first; not graded

Expected: for the pipeline with every stage transition ENABLED, S3 GetObject on the Deploy action's configured object key returns the deployed artifact within seconds of StartPipelineExecution (byte-identical to the zipped source payload), and GetPipelineExecution reports Succeeded.
Actual: the object never exists; GetObject keeps answering NoSuchKey until the budget is exhausted. Steps that DID pass in this run and bound the cause: CreatePipeline for both pipelines, DisableStageTransition on the held pipeline, every auto-started execution reaching a terminal status, both deploy keys confirmed absent, GetPipelineState reporting Deploy.inboundTransitionState.enabled == False for the held pipeline and not False for the twin, and both StartPipelineExecution calls returning a pipelineExecutionId.
Per-service pass rate for codepipeline in this run: 55/83 (66%), 28 failed.

## Steps to Reproduce
Boot the emulator and point botocore at it; cp = client('codepipeline'), s3 = client('s3'), iam = client('iam').
1. Create an artifact bucket and upload a zip to key 'source.zip'; create a pipeline role trusted by codepipeline.amazonaws.com with access to the bucket.
2. cp.create_pipeline twice (names P_held and P_twin), each with two stages: Source = {category Source, owner AWS, provider S3, version '1'}, configuration S3Bucket=<bucket>, S3ObjectKey='source.zip', PollForSourceChanges='false', outputArtifacts=[{'name':'SourceOutput'}]; Deploy = {category Deploy, owner AWS, provider S3, version '1'} with inputArtifacts=[{'name':'SourceOutput'}] and configuration BucketName=<bucket>, ObjectKey='deployed/held.zip' resp. 'deployed/twin.zip', Extract='false'.
3. cp.disable_stage_transition(pipelineName=P_held, stageName='Deploy', transitionType='Inbound', reason=...). Leave P_twin untouched.
4. Abandon any auto-started executions of both pipelines and delete both deploy keys, so nothing is already past the boundary.
5. Confirm cp.get_pipeline_state(name=P_twin) reports Deploy.inboundTransitionState.enabled is not False.
6. cp.start_pipeline_execution(name=P_held) and cp.start_pipeline_execution(name=P_twin).
7. Poll s3.get_object(Bucket=<bucket>, Key='deployed/twin.zip') -> expect the deployed artifact within seconds for the UNHELD twin; observe NoSuchKey indefinitely while the check's budget drains.
8. Re-replay codepipeline.disabled-transition-withhold-is-bounded-by-a-concurrent-enabled-twin.

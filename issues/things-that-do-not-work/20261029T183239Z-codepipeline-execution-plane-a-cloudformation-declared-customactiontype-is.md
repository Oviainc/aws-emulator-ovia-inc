Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.deploy-codepipeline-cloudformation-executes codepipeline.action-execution-sequence-matches-stage-order codepipeline.approval-gate-withholds-the-artifact-until-it-is-approved codepipeline.list-action-executions-reflects-stage-sequence codepipeline.override-stage-condition-not-overridable-rejected codepipeline.put-job-failure-invalid-state-rejected codepipeline.stop-completed-execution-rejected codepipeline.stop-pipeline-execution-not-stoppable-rejected codepipeline.superseded-execution-mode-drops-the-middle-queued-execution
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|PollForJobs|ActionTypeNotFoundException
Issue-Key: codepipeline.action-execution-sequence-matches-stage-order|_find_build_job_for_execution#9|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.approval-gate-withholds-the-artifact-until-it-is-approved|run#31|Timeout|time.sleep(3)
Issue-Key: codepipeline.list-action-executions-reflects-stage-sequence|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.override-stage-condition-not-overridable-rejected|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.put-job-failure-invalid-state-rejected|_poll_for_job#7|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.stop-completed-execution-rejected|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.stop-pipeline-execution-not-stoppable-rejected|_poll_for_job#8|Timeout|time.sleep(3)
Issue-Key: codepipeline.superseded-execution-mode-drops-the-middle-queued-execution|_poll#24|Timeout|time.sleep(interval_s)
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# CodePipeline execution plane: a CloudFormation-declared CustomActionType is never registered, and 8 further checks exhaust the whole poll budget waiting for execution progression (2 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
TWO net-new root causes in the codepipeline execution plane. A fixer must close BOTH before submitting; the two causes have DIFFERENT failing-check lists, named per cause. None of these checks is marked ALREADY FILED or OWNED in this replay. Related but NOT covering: the open issue 20261025T032228Z-codepipeline-execution-engine-still-stalls-an-action-never.md records the same stall FAMILY but carries a different set of check ids (18 others, listed in its own Issue-Trace-Ids) — the eight ids under Cause 2 appear in none of them; 20261023T080113Z-codebuild-codepipeline-ecr-cloudformation-stack-reports-completion-but-provisions-no-codepipeline.md covers a CFN stack provisioning no AWS::CodePipeline::Pipeline, not the CustomActionType resource of Cause 1.

=== Cause 1 of 2: a CloudFormation stack reports CREATE_COMPLETE but the AWS::CodePipeline::CustomActionType it declares is not registered, so PollForJobs refuses the action type ===
AWS: Smithy models CreateCustomActionType/PollForJobs; a registered custom action type is pollable by a job worker and PollForJobs answers ActionTypeNotFoundException only for a type that is NOT registered in the account. A CloudFormation AWS::CodePipeline::CustomActionType resource that reaches CREATE_COMPLETE has performed that registration, so a worker polling for Category='Build', Owner='Custom', Provider=<the stack's provider>, Version='1' is served (an empty jobs list until a job is due), never refused.
Emulator: after the stack reaches completion, cp.poll_for_jobs for exactly that action type is refused ActionTypeNotFoundException: `ActionType (Category='Build', Owner='Custom', Provider='<provider>', Version='1') is not registered in account '<accountId>'` (HTTP 400). Hypothesis (neither the model nor the recording states the mechanism): the CloudFormation resource handler for AWS::CodePipeline::CustomActionType reports success without calling the CreateCustomActionType path, so no action type exists in codepipeline state.
Why: an infrastructure-as-code deployment of a pipeline with a custom action succeeds and is then unusable — the declared pipeline can never have its Build action claimed by a worker, and the failure surfaces far from the stack that caused it.
Fix: the CloudFormation resource handler for AWS::CodePipeline::CustomActionType (it must create the action type in codepipeline state), and/or the account-scoped action-type registry PollForJobs reads.
Traces: codepipeline.deploy-codepipeline-cloudformation-executes.

=== Cause 2 of 2: an execution never reaches the next state these eight checks wait for, so each exhausts the whole per-check budget ===
AWS: Smithy models StartPipelineExecution -> pipelineExecutionId and the service then DRIVES the execution: actions run in stage order and in runOrder, GetPipelineState/ListActionExecutions report each action's progression, a Manual approval action reaches InProgress and issues the token PutApprovalResult consumes (and the downstream stage receives the artifact only after approval), a custom-action job reaches the state PutJobFailureResult's InvalidJobStateException path needs, StopPipelineExecution moves a running execution to Stopping/Stopped and a stop of a terminal execution is refused, a stage condition resolves so OverrideStageCondition has something to refuse, and with executionMode SUPERSEDED a queued execution is superseded by a newer one. Every one of these states is reported through GetPipelineExecution/GetPipelineState/ListActionExecutions.
Emulator: each of the eight checks below stops at the suite's 60 s per-check ceiling ('Error: Check timed out after 60.0s') while polling for that next state; against an emulator none of these waits should take seconds. The checks span four different waits — stage/action progression, an approval token and the artifact it gates, a job state for a Put*Result refusal, and a stop/supersede state transition — and all of them stop before their first graded assertion, so no refusal or ordering assertion in them was ever reached.
Why: the core CodePipeline workflow is unusable for a client: starting a pipeline and waiting for it hangs, approvals never become actionable, and because the states the negative classes act on are never reached the refusals (PipelineExecutionNotStoppableException, InvalidJobStateException, ConditionNotOverridableException) are unreachable too.
Fix: the codepipeline crate's execution driver behind StartPipelineExecution and the state it publishes to GetPipelineState/GetPipelineExecution/ListActionExecutions, plus the approval-token, job and stop/supersede lifecycles the PutApprovalResult/PutJobFailureResult/StopPipelineExecution handlers read.
Traces: codepipeline.action-execution-sequence-matches-stage-order, codepipeline.approval-gate-withholds-the-artifact-until-it-is-approved, codepipeline.list-action-executions-reflects-stage-sequence, codepipeline.override-stage-condition-not-overridable-rejected, codepipeline.put-job-failure-invalid-state-rejected, codepipeline.stop-completed-execution-rejected, codepipeline.stop-pipeline-execution-not-stoppable-rejected, codepipeline.superseded-execution-mode-drops-the-middle-queued-execution.

## Evidence
Cause 1 — codepipeline.deploy-codepipeline-cloudformation-executes [error], verbatim from the replay report (provider name and account are per-run values):
  botocore.errorfactory.ActionTypeNotFoundException: An error occurred (ActionTypeNotFoundException) when calling the PollForJobs operation: ActionType (Category='Build', Owner='Custom', Provider='sim2realcfn-cp-cfn-71dea960c811', Version='1') is not registered in account '000000000000'
The check creates a stack whose template declares `CustomBuildActionType: Type: AWS::CodePipeline::CustomActionType` with Category: Build, Provider: !Ref ProviderName, Version: '1', InputArtifactDetails {MaximumCount: 1, MinimumCount: 0}, OutputArtifactDetails {MaximumCount: 1, MinimumCount: 0} alongside an AWS::CodePipeline::Pipeline that uses it, waits for the stack, starts an execution and then polls cp.poll_for_jobs(actionTypeId={'category':'Build','owner':'Custom','provider':<provider>,'version':'1'}, maxBatchSize=10). Expected: the poll is served (jobs: [] until the Build action is due). Actual: HTTP 400 ActionTypeNotFoundException, so the stack's own action type does not exist.

Cause 2 — all eight checks report, identically, 'Error: Check timed out after 60.0s' with no state observed and no graded assertion reached. For context in the same run the comparable non-timeout evidence is the Source action of a started execution sitting at status 'InProgress' with lastUpdateTime == startTime for the whole poll (seen in codepipeline.e2e-pipeline-create-execute-approve-observe-teardown, which is OWNED by the open stall issue). Per-service pass rate for codepipeline in this run: 81/162 (50%), 81 failed.

## Steps to Reproduce
Boot the emulator and point botocore at it: cp = boto3.client('codepipeline'), cfn = boto3.client('cloudformation'), s3 = boto3.client('s3'), iam = boto3.client('iam').

Cause 1:
1. cfn.create_stack with a template containing an S3 artifact bucket, a pipeline role, an AWS::CodePipeline::CustomActionType (Category Build, Provider <P>, Version '1', Input/OutputArtifactDetails {MinimumCount 0, MaximumCount 1}) and an AWS::CodePipeline::Pipeline whose Build stage uses {Category: Build, Owner: Custom, Provider: <P>, Version: '1'}; Capabilities=['CAPABILITY_IAM'].
2. Wait for CREATE_COMPLETE.
3. cp.poll_for_jobs(actionTypeId={'category':'Build','owner':'Custom','provider':'<P>','version':'1'}, maxBatchSize=10) -> expect a served response (jobs: [] or a job for the started execution); observe HTTP 400 ActionTypeNotFoundException "... is not registered in account '<acct>'".
4. Cross-check cp.list_action_types(actionOwnerFilter='Custom') -> the stack's provider is absent.

Cause 2: for each check id under Cause 2, re-replay it. Minimal repro of the shared shape:
5. Create an artifact bucket (versioned) with a source zip, a pipeline role, and a pipeline with stages Source (AWS/S3), Approve (AWS/Manual) and Deploy (AWS/S3).
6. exec_id = cp.start_pipeline_execution(name=P)['pipelineExecutionId'], then poll cp.get_pipeline_state(name=P) for the approval action's latestExecution.token and cp.list_action_executions(pipelineName=P, filter={'pipelineExecutionId': exec_id}) for the Source action to reach Succeeded -> expect both within seconds; observe neither within 60 s and no deploy artifact written.
7. With a Custom Build action: after PollForJobs/AcknowledgeJob/PutJobSuccessResult the execution must advance so a second PutJobFailureResult hits InvalidJobStateException; observe the state never arrives.
8. cp.stop_pipeline_execution on a running execution -> expect Stopping/Stopped and a stop of a terminal execution refused; observe no state change to assert on.
9. With executionMode SUPERSEDED, start three executions in quick succession -> expect the middle queued execution to be superseded and reported as such; observe no progression.

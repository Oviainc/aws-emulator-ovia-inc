Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-gpn-traj-creation-run-failed-at-source-read-path
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-gpn-traj-creation-run-failed-at-source-read-path codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListPipelineExecutions|2xx->2xx|$.pipelineExecutionSummaries[*].trigger.triggerDetail
Issue-Key: codepipeline|GetPipelineExecution|2xx->2xx|$.pipelineExecution.artifactRevisions
Issue-Key: codepipeline|GetPipelineExecution|2xx->2xx|$.pipelineExecution.trigger.triggerDetail
Issue-Key: codepipeline|ListActionExecutions|2xx->2xx|$.actionExecutionDetails[*].input.actionTypeId.__type
Issue-Key: codepipeline|ListActionExecutions|2xx->2xx|$.actionExecutionDetails[*].input.inputArtifacts
Issue-Key: codepipeline|ListActionExecutions|2xx->2xx|$.actionExecutionDetails[*].output.executionResult.externalExecutionSummary
Issue-Key: codepipeline|ListActionExecutions|2xx->2xx|$.actionExecutionDetails[*].output.outputVariables
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.stageStates[*].actionStates[*].entityUrl
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.stageStates[*].actionStates[*].latestExecution.errorDetails.message
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.stageStates[*].inboundExecutions
Issue-Key: codepipeline|GetPipelineState|2xx->2xx|$.stageStates[*].inboundTransitionState
Issue-Key: codepipeline|ListActionExecutions|2xx->2xx|$.actionExecutionDetails[*]
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# CodePipeline execution read path: 5 net-new root causes (triggerDetail, artifactRevisions, raw SDK error text, omitted empty members, retry attempt not recorded)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipelineExecutions

## Description
Five independent defects in the CodePipeline execution read path (ListPipelineExecutions, GetPipelineExecution, ListActionExecutions, GetPipelineState). All five must be closed before this issue is submitted.

Cause 1 of 5: ListPipelineExecutions omits ExecutionTrigger.triggerDetail.
  AWS: Smithy: ListPipelineExecutions -> PipelineExecutionSummaryList; PipelineExecutionSummary.trigger -> ExecutionTrigger with members triggerType and triggerDetail (@length 0..1024), documented as "Detail related to the event that started a pipeline execution, such as the webhook ARN ... or the user ARN for a user-initiated start-pipeline-execution CLI command". For an execution started by an API/CLI caller AWS populates triggerDetail with the ARN of the calling principal (recorded value shape: arn:aws:iam::<account-id>:user/<user-name>); the account id and principal name come from the caller's identity, not from any fixed string.
  Emulator: the summary carries trigger.triggerType but triggerDetail is absent (replay: "$.pipelineExecutionSummaries[0].trigger.triggerDetail: missing (expected 'arn:aws:iam::000000000000:user/emulator', got None)"); after a retry both summaries omit it.
  Why: a client that audits or displays who/what started an execution (console-style "triggered by") reads trigger.triggerDetail; with the member absent it cannot attribute an execution to its initiator at all, and SDK code that unwraps the field fails.
  Fix: the ListPipelineExecutions response serializer / the execution-start state that records the trigger in the codepipeline service crate -- the calling principal's ARN must be stored on the execution when StartPipelineExecution (and the retry/rollback variants) create it, and emitted on every summary.
  Traces: codepipeline-gpn-traj-creation-run-failed-at-source-read-path#task#7 (representative); also codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source#task#15 (both summaries after a retry).

Cause 2 of 5: GetPipelineExecution omits artifactRevisions and trigger.triggerDetail.
  AWS: Smithy: GetPipelineExecution -> PipelineExecution, whose members include artifactRevisions (ArtifactRevisionList, "A list of ArtifactRevision objects included in a pipeline execution") and trigger (ExecutionTrigger). AWS returns artifactRevisions as a present list -- empty when the execution produced no resolved source revision -- and trigger.triggerDetail as the initiating principal's ARN.
  Emulator: both members are absent from pipelineExecution (replay: "$.pipelineExecution.artifactRevisions: missing (expected [], got None)" and "$.pipelineExecution.trigger.triggerDetail: missing (expected 'arn:aws:iam::000000000000:user/emulator', got None)").
  Why: present-but-empty and absent are different on the wire. A client that iterates pipelineExecution.artifactRevisions to report which source revision an execution ran gets a null where AWS gives it a list, so the natural loop/length check breaks instead of reporting "no revisions"; the missing triggerDetail loses initiator attribution as in Cause 1.
  Fix: the GetPipelineExecution response serializer in the codepipeline service crate -- always serialize artifactRevisions (empty list when nothing was resolved) and carry the recorded trigger detail through.
  Traces: codepipeline-gpn-traj-creation-run-failed-at-source-read-path#task#8.

Cause 3 of 5: ListActionExecutions reports internal Rust SDK debug text as the action provider's externalExecutionSummary, and omits empty ActionExecutionInput/Output members and the nested ActionTypeId discriminator.
  AWS: Smithy: ListActionExecutions -> ActionExecutionDetailList; ActionExecutionOutput.executionResult -> ActionExecutionResult.externalExecutionSummary is "the action provider's summary for the action execution". For a CodeCommit source action whose configured branch does not exist, the provider summary is AWS's ConfigurationError sentence: fixed text "The action failed because no branch named " + <BranchName from the action configuration> + " was found in the selected AWS CodeCommit repository " + <RepositoryName from the action configuration> + ". Make sure you are using the correct branch name, and then try again. Error: null" -- only the branch name and repository name are request-derived; the rest is fixed. The accompanying errorDetails.code is "ConfigurationError". AWS also returns input.inputArtifacts and output.outputVariables as present-but-empty when the action has none, and the JSON ActionTypeId object carries its __type discriminator.
  Emulator: externalExecutionSummary is the emulator's internal client error, Rust Debug-formatted: "the branch main of the repository <repo> could not be resolved: service error: BranchDoesNotExistException: BranchDoesNotExistException (ServiceError(ServiceError { source: BranchDoesNotExistException(BranchDoesNotExistException { message: None, meta: ErrorMetadata { code: Some(\"BranchDoesNotExistException\") ... } }), raw: Response { status: StatusCode(400), ... } }))" -- including a nested HTTP response dump. In the same response "$.actionExecutionDetails[0].input.inputArtifacts: missing (expected [], got None)", "$.actionExecutionDetails[0].output.outputVariables: missing (expected {}, got None)" and "$.actionExecutionDetails[0].input.actionTypeId.__type: missing (expected 'ActionTypeId', got None)".
  Why: the summary is user-facing failure text that pipelines surface in consoles, notifications and CI gating logic; a client matching on AWS's wording (or merely showing it) gets emulator-internal Rust struct dumps that leak implementation types and never match. The absent empty list/map members break clients that iterate them unconditionally.
  Fix: the action-execution recording for the CodeCommit source provider (the failure is translated from the internal SDK error instead of being mapped to a ConfigurationError provider summary) plus the ListActionExecutions response serializer in the codepipeline service crate.
  Traces: codepipeline-gpn-traj-creation-run-failed-at-source-read-path#task#9.

Cause 4 of 5: GetPipelineState carries the same leaked Rust debug text as errorDetails.message and omits entityUrl, inboundExecutions and inboundTransitionState.
  AWS: Smithy: GetPipelineState -> StageStateList; StageState has inboundExecutions (StageExecutionList) and inboundTransitionState (TransitionState, "the state of the inbound transition, which is either enabled or disabled"), and ActionState has entityUrl ("A URL link for more information about the state of the action"). AWS returns inboundExecutions as a present list (empty when there is no inbound execution), inboundTransitionState with enabled=true for a transition nobody disabled, and entityUrl for provider actions that have a console target. The failed action's latestExecution.errorDetails.message is the same ConfigurationError sentence templated in Cause 3.
  Emulator: "$.stageStates[0].actionStates[0].latestExecution.errorDetails.message" is the Rust Debug dump quoted in Cause 3 instead of the AWS sentence, and "$.stageStates[0].actionStates[0].entityUrl", "$.stageStates[0].inboundExecutions" (expected []), "$.stageStates[0].inboundTransitionState" (expected {'enabled': True}) and "$.stageStates[1].inboundExecutions" are all absent.
  Why: GetPipelineState is the primary polling API for CI tooling. Without inboundTransitionState a client cannot tell whether a stage transition is enabled, without inboundExecutions it cannot iterate pending inbound executions, and the error message it shows operators is emulator-internal text rather than AWS's actionable sentence.
  Fix: the GetPipelineState response serializer and the stage/action state projection in the codepipeline service crate (same error-translation defect as Cause 3 for the message).
  Traces: codepipeline-gpn-traj-creation-run-failed-at-source-read-path#task#10.

Cause 5 of 5: after RetryStageExecution the retried action's new attempt is never recorded, so ListActionExecutions returns one detail where AWS returns two.
  AWS: recorded behavior only (the model does not state attempt accumulation): retrying a failed stage runs the action again and appends a NEW ActionExecutionDetail for that attempt, so the history holds one entry per attempt (the recording expects actionExecutionDetails[1] for the second 'Source' attempt, with its own actionExecutionId, lastUpdateTime, input.configuration/resolvedConfiguration and output.executionResult).
  Emulator: only actionExecutionDetails[0] exists -- "$.actionExecutionDetails[1]: missing" -- and that single entry still carries the Cause 3 defects (leaked Rust summary, absent inputArtifacts/outputVariables/__type).
  Why: a client that counts attempts or reads the newest attempt to decide whether a retry happened sees no evidence of the retry at all, so retry-aware CI logic concludes the stage was never re-run.
  Fix: the RetryStageExecution handler / action-execution history in the codepipeline service crate -- a retry must append a new action execution record rather than mutate or discard the previous one.
  Traces: codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source#task#12.

Related open issues that do NOT cover these causes: 20261029T202908Z-codepipeline-pipelinedeclaration-responses-omit-modeled-defaults-and-nested.md (PipelineDeclaration shapes only) and 20261028T112857Z-codepipeline-execution-plane-5-net-new-root-causes.md (execution-plane behavior, not these read-path records).

## Evidence
Replay of trace set simparity-wire-aws-v1 (58 fixtures, 312 steps, score 71.2%).

Cause 1 (ListPipelineExecutions, codepipeline-gpn-traj-creation-run-failed-at-source-read-path#task#7, status 200 vs 200):
  $.pipelineExecutionSummaries[0].trigger.triggerDetail: missing (expected 'arn:aws:iam::000000000000:user/emulator', got None)
  same at codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source#task#15 for summaries [0] and [1]

Cause 2 (GetPipelineExecution, codepipeline-gpn-traj-creation-run-failed-at-source-read-path#task#8, 200 vs 200):
  $.pipelineExecution.artifactRevisions: missing (expected [], got None)
  $.pipelineExecution.trigger.triggerDetail: missing (expected 'arn:aws:iam::000000000000:user/emulator', got None)

Cause 3 (ListActionExecutions, codepipeline-gpn-traj-creation-run-failed-at-source-read-path#task#9, 200 vs 200):
  $.actionExecutionDetails[0].output.executionResult.externalExecutionSummary: value_mismatch
    expected: 'The action failed because no branch named main was found in the selected AWS CodeCommit repository <repo>. Make sure you are using the correct branch name, and then try again. Error: null'
    actual:   'the branch main of the repository <repo> could not be resolved: service error: BranchDoesNotExistException: BranchDoesNotExistException (ServiceError(ServiceError { source: BranchDoesNotExistException(...), raw: Response { status: StatusCode(400), headers: {}, body: SdkBody { inner: Once(Some(b"{\"__type\":\"BranchDoesNotExistException\"}")), retryable: true }, ... } }))'
  $.actionExecutionDetails[0].input.inputArtifacts: missing (expected [], got None)
  $.actionExecutionDetails[0].output.outputVariables: missing (expected {}, got None)
  $.actionExecutionDetails[0].input.actionTypeId.__type: missing (expected 'ActionTypeId', got None)

Cause 4 (GetPipelineState, codepipeline-gpn-traj-creation-run-failed-at-source-read-path#task#10, 200 vs 200):
  $.stageStates[0].actionStates[0].latestExecution.errorDetails.message: value_mismatch (same expected AWS sentence vs the same Rust Debug dump)
  $.stageStates[0].actionStates[0].entityUrl: missing
  $.stageStates[0].inboundExecutions: missing (expected [], got None)
  $.stageStates[0].inboundTransitionState: missing (expected {'enabled': True}, got None)
  $.stageStates[1].actionStates[0].entityUrl: missing; $.stageStates[1].inboundExecutions: missing

Cause 5 (ListActionExecutions, codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source#task#12, 200 vs 200):
  $.actionExecutionDetails[1]: missing (expected the second 'Source' attempt detail with its own actionExecutionId, input.configuration {BranchName, PollForSourceChanges, RepositoryName}, resolvedConfiguration and output.executionResult.errorDetails.code 'ConfigurationError')
  plus the Cause 3 defects on actionExecutionDetails[0]

Smithy citations (com-amazonaws-codepipeline-codepipeline-20150709): ExecutionTrigger.triggerDetail @length 0..1024; PipelineExecution.artifactRevisions -> ArtifactRevisionList; ActionExecutionInput.inputArtifacts -> ArtifactDetailList; ActionExecutionOutput.outputVariables -> OutputVariablesMap; ActionExecutionResult.externalExecutionSummary; ActionState.entityUrl -> Url @length 1..2048; StageState.inboundExecutions -> StageExecutionList; StageState.inboundTransitionState -> TransitionState.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture codepipeline-gpn-traj-creation-run-failed-at-source-read-path and inspect step task#7 (ListPipelineExecutions) -- trigger.triggerDetail is absent from the summary. Repeat with codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source step task#15.
3. Cause 2: same fixture, step task#8 (GetPipelineExecution) -- pipelineExecution has neither artifactRevisions nor trigger.triggerDetail.
4. Cause 3: same fixture, step task#9 (ListActionExecutions) on the pipeline whose CodeCommit source points at a non-existent branch -- output.executionResult.externalExecutionSummary is Rust Debug text and input.inputArtifacts / output.outputVariables / input.actionTypeId.__type are absent.
5. Cause 4: same fixture, step task#10 (GetPipelineState) -- actionStates[0].latestExecution.errorDetails.message is the same Rust Debug text and entityUrl / inboundExecutions / inboundTransitionState are absent.
6. Cause 5: replay fixture codepipeline-gpn-traj-retry-stop-and-restart-the-failed-source through step task#12 (RetryStageExecution then ListActionExecutions) -- only one actionExecutionDetails entry is returned instead of one per attempt.

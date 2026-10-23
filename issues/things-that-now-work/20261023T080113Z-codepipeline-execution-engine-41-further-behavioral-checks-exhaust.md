Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.action-execution-sequence-matches-stage-order codepipeline.approval-already-completed-optimistic-lock codepipeline.deploy-artifact-bytes-are-identical-to-the-source-object codepipeline.disable-enable-stage-transition-controls-progression codepipeline.disable-enable-stage-transition-persists-state codepipeline.disabled-transition-withhold-is-bounded-by-a-concurrent-enabled-twin codepipeline.duplicate-stop-request-rejected codepipeline.e2e-pipeline-create-run-approve-teardown codepipeline.e2e-pipeline-create-start-approve-complete-teardown codepipeline.e2e-pipeline-manual-approval-flow codepipeline.job-output-variables-precision-preserved codepipeline.list-action-executions-chronological-order codepipeline.list-action-executions-order-matches-stage-sequence codepipeline.list-action-executions-reflects-stage-sequence codepipeline.list-rule-executions-matches-condition-evaluation-order codepipeline.list-rule-executions-order-matches-invocation-sequence codepipeline.override-stage-condition-not-overridable-rejected codepipeline.pipeline-execution-lifecycle-in-progress-to-succeeded codepipeline.put-approval-result-already-completed-rejected codepipeline.put-approval-result-nonexistent-action-rejected codepipeline.put-approval-result-second-attempt-rejected codepipeline.put-approval-result-second-submission-rejected codepipeline.put-approval-result-token-invalidated-after-completion codepipeline.put-approval-result-token-semantics codepipeline.put-job-failure-invalid-job-state-rejected codepipeline.put-job-failure-on-completed-job-rejected codepipeline.put-job-failure-result-wrong-job-state-rejected codepipeline.put-job-success-output-variables-size-exceeded-rejected codepipeline.put-job-success-result-completed-job-rejected codepipeline.put-job-success-result-wrong-job-state-rejected codepipeline.retry-stage-execution-creates-new-execution codepipeline.retry-stage-execution-resumes-failed-stage codepipeline.rollback-stage-restores-previous-execution codepipeline.rollback-stage-restores-previous-state codepipeline.rollback-stage-restores-the-previous-artifact-bytes codepipeline.server-enforces-disabled-stage-transition-blocks-progress codepipeline.server-side-approval-gate-blocks-progress codepipeline.source-output-variable-resolves-into-the-downstream-object-key codepipeline.source-revision-override-delivers-the-pinned-object-version codepipeline.stage-transition-disable-blocks-execution codepipeline.stop-pipeline-execution-terminal-not-stoppable
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.action-execution-sequence-matches-stage-order|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.approval-already-completed-optimistic-lock|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.deploy-artifact-bytes-are-identical-to-the-source-object|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.disable-enable-stage-transition-controls-progression|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.disable-enable-stage-transition-persists-state|_wait_for_approval_token#2|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.disabled-transition-withhold-is-bounded-by-a-concurrent-enabled-twin|_poll#27|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.duplicate-stop-request-rejected|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.e2e-pipeline-create-run-approve-teardown|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.e2e-pipeline-create-start-approve-complete-teardown|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.e2e-pipeline-manual-approval-flow|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.job-output-variables-precision-preserved|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.list-action-executions-chronological-order|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.list-action-executions-order-matches-stage-sequence|_wait_for_action_state#8|Timeout|time.sleep(3)
Issue-Key: codepipeline.list-action-executions-reflects-stage-sequence|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.list-rule-executions-matches-condition-evaluation-order|_poll_for_execution_id#12|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.list-rule-executions-order-matches-invocation-sequence|_wait_for_action_state#10|Timeout|time.sleep(3)
Issue-Key: codepipeline.override-stage-condition-not-overridable-rejected|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.pipeline-execution-lifecycle-in-progress-to-succeeded|_wait_for_approval_token#2|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.put-approval-result-already-completed-rejected|_wait_for_approval_token#9|Timeout|time.sleep(4)
Issue-Key: codepipeline.put-approval-result-nonexistent-action-rejected|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.put-approval-result-second-attempt-rejected|_wait_for_approval_token#2|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.put-approval-result-second-submission-rejected|_wait_for_action_state#8|Timeout|time.sleep(3)
Issue-Key: codepipeline.put-approval-result-token-invalidated-after-completion|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.put-approval-result-token-semantics|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.put-job-failure-invalid-job-state-rejected|_drain_initial_execution#9|Timeout|time.sleep(3)
Issue-Key: codepipeline.put-job-failure-on-completed-job-rejected|_drain_initial_execution#6|Timeout|time.sleep(3)
Issue-Key: codepipeline.put-job-failure-result-wrong-job-state-rejected|_wait_for_execution#6|Timeout|time.sleep(2)
Issue-Key: codepipeline.put-job-success-output-variables-size-exceeded-rejected|_wait_for_execution#6|Timeout|time.sleep(2)
Issue-Key: codepipeline.put-job-success-result-completed-job-rejected|_run_pipeline_execution#9|Timeout|time.sleep(2)
Issue-Key: codepipeline.put-job-success-result-wrong-job-state-rejected|_wait_for_execution#6|Timeout|time.sleep(2)
Issue-Key: codepipeline.retry-stage-execution-creates-new-execution|_wait_for_approval_token#2|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.retry-stage-execution-resumes-failed-stage|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.rollback-stage-restores-previous-execution|_wait_for_approval_token#2|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.rollback-stage-restores-previous-state|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.rollback-stage-restores-the-previous-artifact-bytes|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.server-enforces-disabled-stage-transition-blocks-progress|_wait_for_stage_status#10|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.server-side-approval-gate-blocks-progress|_drain_pending_approvals#10|Timeout|time.sleep(3)
Issue-Key: codepipeline.source-output-variable-resolves-into-the-downstream-object-key|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.source-revision-override-delivers-the-pinned-object-version|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.stage-transition-disable-blocks-execution|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: codepipeline.stop-pipeline-execution-terminal-not-stoppable|_poll#9|Timeout|time.sleep(interval_s)
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# CodePipeline execution engine: 41 further behavioral checks exhaust their poll budget because nothing downstream of StartPipelineExecution ever happens

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
One root cause, recorded here because 41 failing checks are grounded in it and none of them is marked ALREADY FILED or OWNED by the replay report — so no open issue carries their replay keys, and a fixer working only from the existing issue would close it without ever re-running them.

=== Cause 1 of 1: no execution engine — stage/action/job/approval state never advances, so every check that waits for a consequence of an execution exhausts its budget ===
AWS: Smithy: StartPipelineExecution returns pipelineExecutionId (pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$) and the service then DRIVES the execution: each stage's actions run in runOrder, artifacts are written to the artifactStore, custom-action jobs become pollable and are completed through PutJobSuccessResult / PutJobFailureResult, manual-approval actions reach InProgress and issue an approval token for PutApprovalResult, stage transitions withhold progress while disabled, and the execution reaches a terminal PipelineExecutionStatus that GetPipelineExecution / ListPipelineExecutions / ListActionExecutions / ListRuleExecutions report. GetPipelineState publishes the per-stage and per-action state throughout.
Emulator: nothing downstream of the accepted start is ever observable. All 41 checks below stop at the suite's 60 s per-check ceiling ("Error: Check timed out after 60.0s") while polling for the next state — an approval token, a pollable job, an action-execution record, a rule-execution record, a terminal status, a held transition, a retry/rollback result or a delivered artifact. Against a working emulator none of these waits should take seconds, let alone exceed the ceiling.
Why: every CodePipeline workflow is unusable: a client that starts a pipeline and waits hangs; approval, job-result, retry, rollback, stop and stage-transition APIs have no state to act on, so even their rejection classes (InvalidApprovalTokenException, InvalidJobStateException, ApprovalAlreadyCompletedException, PipelineExecutionNotStoppableException, StageNotRetryableException) can never be reached; and ordering guarantees (ListActionExecutions / ListRuleExecutions chronology) are unverifiable.
Fix: the pipeline execution engine / state machine behind StartPipelineExecution in the codepipeline crate (services/service-com-amazonaws-codepipeline-codepipeline-20150709/src) and the state it must publish to GetPipelineState, GetPipelineExecution, ListPipelineExecutions, ListActionExecutions and ListRuleExecutions, plus the job and approval-token lifecycles the Put*Result / PutApprovalResult handlers read.
This is the same engine gap as Cause 1 of the open issue 20261022T173520Z-codepipeline-executions-never-progress-structural-validation-absent-error.md (which records last='InProgress' and 16 sibling timeouts). That issue's Issue-Trace-Ids list contains NONE of the 41 check ids below, so this issue exists to carry them: closing the engine gap must be confirmed by re-replaying this list as well.
Traces: the 41 check ids in trace_ids.

## Evidence
Expected: within seconds of StartPipelineExecution (or of CreatePipeline's own execution), the polled next state appears — a manual-approval action at InProgress with an approval token, a pollable custom-action job, an action/rule-execution record, a disabled transition withholding progress, or a terminal PipelineExecutionStatus.
Actual: all 41 checks report, verbatim and identically, "Error: Check timed out after 60.0s" with no state observed. Corroborating non-timeout evidence for the same engine gap, from checks in the sibling issues: "condition not met within 45.0s; last='InProgress'" (status never leaves InProgress) and "condition not met within 30.0s; last=None".

## Steps to Reproduce
1. Boot the emulator and replay the 41 codepipeline behavioral checks listed in trace_ids.
2. Minimal repro: create an S3-source + manual-approval + S3-deploy pipeline, call StartPipelineExecution, then poll GetPipelineState for the approval action's status and GetPipelineExecution for the execution status — neither ever leaves its initial value, so no approval token is ever issued and PutApprovalResult cannot be called.
3. Same with a Custom-owner build action: PollForJobs/GetPipelineState never surface a job, so PutJobSuccessResult / PutJobFailureResult have no job to complete and their InvalidJobStateException paths are unreachable.
4. Same with DisableStageTransition before a start: no execution ever arrives at the disabled transition, so the withhold cannot be observed.

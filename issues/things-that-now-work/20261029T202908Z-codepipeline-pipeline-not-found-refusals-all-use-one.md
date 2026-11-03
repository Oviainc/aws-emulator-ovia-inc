Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-action-revision-and-approval-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-action-revision-and-approval-error-paths codepipeline-put-approval-result-error-paths codepipeline-put-approval-result-malformed-token-and-name codepipeline-put-approval-result-missing-pipeline codepipeline-execution-read-apis-not-found-errors codepipeline-get-and-list-pipeline-executions-pagination-and-validation codepipeline-operate-on-nonexistent-pipeline codepipeline-override-stage-condition-not-found codepipeline-pipeline-execution-full-workflow-against-missing-pipeline codepipeline-start-check-list-on-missing-pipeline codepipeline-list-action-executions-pipeline-not-found codepipeline-missing-pipeline-error-paths codepipeline-list-pipeline-executions-edge-cases codepipeline-list-rule-executions-pipeline-not-found codepipeline-start-pipeline-execution-not-found-error codepipeline-start-pipeline-execution-variables-and-validation codepipeline-stop-and-retry-missing-pipeline codepipeline-stop-pipeline-execution-reason-boundaries-and-validation codepipeline-retry-stage-execution-modes-and-validation codepipeline-rollback-stage-missing-pipeline codepipeline-rollback-stage-targets-and-validation
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|PutApprovalResult|4xx->4xx|$.message
Issue-Key: codepipeline|GetPipelineExecution|4xx->4xx|$.message
Issue-Key: codepipeline|ListActionExecutions|4xx->4xx|$.message
Issue-Key: codepipeline|ListPipelineExecutions|4xx->4xx|$.message
Issue-Key: codepipeline|ListRuleExecutions|4xx->4xx|$.message
Issue-Key: codepipeline|StartPipelineExecution|4xx->4xx|$.message
Issue-Key: codepipeline|StopPipelineExecution|4xx->4xx|$.message
Issue-Key: codepipeline|RetryStageExecution|4xx->4xx|$.message
Issue-Key: codepipeline|RollbackStage|4xx->4xx|$.message
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodePipeline pipeline-not-found refusals all use one generic message instead of the per-operation PipelineNotFoundException wording

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipelineExecution

## Description
Every CodePipeline operation that takes a pipeline name models PipelineNotFoundException (Smithy: GetPipeline/StartPipelineExecution/RollbackStage/PutWebhook etc. all declare error PipelineNotFoundException, errorKind=client, httpError 400). On AWS the HTTP status (400) and the error code (PipelineNotFoundException) are shared, but the MESSAGE text is NOT: it is worded per operation family. The emulator emits a single hard-coded template for all of them: "Account '<accountId>' does not have a pipeline with name '<pipelineName>'". Status and __type match on every cause below; only the message diverges, and the message is part of the contract. In every template below the fixed text is the requirement and the only variable parts are the caller's pipeline name and the caller's 12-digit account id taken from the request/credentials — the concrete probe names and the all-zero account id in the recordings are fill-ins, not requirements.

Cause 1 of 9 — PutApprovalResult (also PutActionRevision in the same cluster): AWS answers 400 PipelineNotFoundException with "<pipelineName> was not found in account <accountId>. Make sure you are using the correct name for the pipeline, and then try again." Emulator: the generic "Account '<accountId>' does not have a pipeline with name '<pipelineName>'". Why: an approval client that matches on AWS's guidance text (or logs it for an operator) never sees it. Fix: the message builder behind PutApprovalResult's pipeline lookup in the codepipeline handler. Traces: codepipeline-action-revision-and-approval-error-paths#task#2 (plus 5 more steps in the cluster, incl. codepipeline-put-approval-result-missing-pipeline#task#2).

Cause 2 of 9 — GetPipelineExecution: AWS answers 400 PipelineNotFoundException with "The account with id '<accountId>' does not include a pipeline with the name '<pipelineName>'" — note this family says "with id" and "with the name". Emulator: generic template. Fix: GetPipelineExecution's lookup refusal. Traces: codepipeline-execution-read-apis-not-found-errors#task#2.

Cause 3 of 9 — ListActionExecutions: AWS answers 400 PipelineNotFoundException with "The account with AWS account ID '<accountId>' does not include a pipeline named '<pipelineName>'". Emulator: generic template. Fix: ListActionExecutions' lookup refusal. Traces: codepipeline-execution-read-apis-not-found-errors#task#4.

Cause 4 of 9 — ListPipelineExecutions: same AWS wording as Cause 3 ("The account with AWS account ID '<accountId>' does not include a pipeline named '<pipelineName>'"). Emulator: generic template. Fix: ListPipelineExecutions' lookup refusal. Traces: codepipeline-execution-read-apis-not-found-errors#task#6.

Cause 5 of 9 — ListRuleExecutions: same AWS wording as Cause 3. Emulator: generic template. Fix: ListRuleExecutions' lookup refusal. Traces: codepipeline-execution-read-apis-not-found-errors#task#7.

Cause 6 of 9 — StartPipelineExecution: AWS answers 400 PipelineNotFoundException with "Pipeline with name <pipelineName> does not exist in account <accountId>." (unquoted name, trailing period). Emulator: generic template. Fix: StartPipelineExecution's lookup refusal. Traces: codepipeline-operate-on-nonexistent-pipeline#task#3.

Cause 7 of 9 — StopPipelineExecution: same AWS wording as Cause 6. Emulator: generic template. Fix: StopPipelineExecution's lookup refusal. Traces: codepipeline-operate-on-nonexistent-pipeline#task#4.

Cause 8 of 9 — RetryStageExecution: same AWS wording as Cause 6. Emulator: generic template. Fix: RetryStageExecution's lookup refusal. Traces: codepipeline-operate-on-nonexistent-pipeline#task#5.

Cause 9 of 9 — RollbackStage: AWS answers 400 PipelineNotFoundException with "Pipeline '<pipelineName>' does not exist." — no account id at all. Emulator: generic template that injects an account id. Fix: RollbackStage's lookup refusal. Traces: codepipeline-operate-on-nonexistent-pipeline#task#6.

Related but not covering: the open issue 20261029T183239Z-codepipeline-refusal-classification-and-wording-10-net-new.md discusses refusal wording generally; none of the nine clusters above is owned by it (the replay reports owned_by=[] for each), so each still diverges.

## Evidence
All nine clusters: expected_status 400 == actual_status 400, __type matches, only $.message diverges.
PutApprovalResult: expected 'sim2real-probe-codepipeline-cp-ghost-8dc1a5ca was not found in account 000000000000. Make sure you are using the correct name for the pipeline, and then try again.' / got "Account '000000000000' does not have a pipeline with name 'sim2real-probe-codepipeline-cp-ghost-8dc1a5ca'" (6 affected steps).
GetPipelineExecution: expected "The account with id '000000000000' does not include a pipeline with the name '<name>'" / got the generic string (6 affected steps).
ListActionExecutions / ListPipelineExecutions / ListRuleExecutions: expected "The account with AWS account ID '000000000000' does not include a pipeline named '<name>'" / got the generic string (5, 9 and 3 affected steps respectively).
StartPipelineExecution / StopPipelineExecution / RetryStageExecution: expected 'Pipeline with name <name> does not exist in account 000000000000.' / got the generic string (6, 5 and 6 affected steps).
RollbackStage: expected "Pipeline '<name>' does not exist." / got the generic string (5 affected steps).

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Call each of PutApprovalResult, GetPipelineExecution, ListActionExecutions, ListPipelineExecutions, ListRuleExecutions, StartPipelineExecution, StopPipelineExecution, RetryStageExecution and RollbackStage naming a pipeline that was never created.
3. Each answers 400 PipelineNotFoundException but with the single generic message; compare $.message against the per-operation templates above.
4. Re-replay codepipeline-action-revision-and-approval-error-paths#task#2, codepipeline-execution-read-apis-not-found-errors#task#2/#task#4/#task#6/#task#7 and codepipeline-operate-on-nonexistent-pipeline#task#3/#task#4/#task#5/#task#6.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline codepipeline.approval-already-completed-optimistic-lock codepipeline.disable-enable-stage-transition-persists-state codepipeline.disable-stage-transition-unknown-stage-not-permissive codepipeline.e2e-pipeline-create-run-approve-teardown codepipeline.e2e-pipeline-create-start-approve-complete-teardown codepipeline.e2e-pipeline-manual-approval-flow codepipeline.job-output-variables-precision-preserved codepipeline.list-rule-executions-order-matches-invocation-sequence codepipeline.override-stage-condition-not-overridable-rejected codepipeline.pipeline-execution-lifecycle-in-progress-to-succeeded codepipeline.pipeline-lifecycle-create-active-delete codepipeline.put-approval-result-already-completed-rejected codepipeline.put-approval-result-nonexistent-action-rejected codepipeline.put-approval-result-second-attempt-rejected codepipeline.put-approval-result-second-submission-rejected codepipeline.put-approval-result-token-invalidated-after-completion codepipeline.put-approval-result-token-semantics codepipeline.retry-stage-execution-creates-new-execution codepipeline.rollback-stage-restores-previous-execution codepipeline.server-side-approval-gate-blocks-progress codepipeline.stop-pipeline-execution-terminal-not-stoppable
Issue-Key: codepipeline|GetPipelineState|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codepipeline GetPipelineState: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipelineState

## Description
`GetPipelineState` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/get_pipeline_state.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

22 recorded unit(s) fail on their `GetPipelineState` call, so each one unblocks only when `GetPipelineState` works: `cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline`, `codepipeline.approval-already-completed-optimistic-lock`, `codepipeline.disable-enable-stage-transition-persists-state`, `codepipeline.disable-stage-transition-unknown-stage-not-permissive`, `codepipeline.e2e-pipeline-create-run-approve-teardown`, `codepipeline.e2e-pipeline-create-start-approve-complete-teardown`, `codepipeline.e2e-pipeline-manual-approval-flow`, `codepipeline.job-output-variables-precision-preserved`, `codepipeline.list-rule-executions-order-matches-invocation-sequence`, `codepipeline.override-stage-condition-not-overridable-rejected`, `codepipeline.pipeline-execution-lifecycle-in-progress-to-succeeded`, `codepipeline.pipeline-lifecycle-create-active-delete`, `codepipeline.put-approval-result-already-completed-rejected`, `codepipeline.put-approval-result-nonexistent-action-rejected`, `codepipeline.put-approval-result-second-attempt-rejected`, `codepipeline.put-approval-result-second-submission-rejected`, `codepipeline.put-approval-result-token-invalidated-after-completion`, `codepipeline.put-approval-result-token-semantics`, `codepipeline.retry-stage-execution-creates-new-execution`, `codepipeline.rollback-stage-restores-previous-execution` (+2 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/get_pipeline_state.rs`. Replaying the corpus, the emulator refused the `GetPipelineState` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetPipelineState operation: operation GetPipelineState is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codepipeline emulator
Call GetPipelineState and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

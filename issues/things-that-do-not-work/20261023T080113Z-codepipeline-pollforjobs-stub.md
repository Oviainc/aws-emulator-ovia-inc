Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.acknowledge-job-not-found codepipeline.approve-already-completed-rejected codepipeline.condition-not-overridable-rejected codepipeline.e2e-pipeline-create-run-approve-complete-teardown codepipeline.get-job-details-not-found codepipeline.get-pipeline-state-reflects-latest-execution codepipeline.job-claim-nonce-single-use-after-acknowledgement-rejected codepipeline.job-claim-withholds-job-from-poll-until-completed codepipeline.job-output-variables-value-round-trip codepipeline.job-poll-visibility-timeout-requeue codepipeline.job-success-output-variables-round-trip codepipeline.list-action-executions-invalid-next-token-rejected codepipeline.list-pipeline-executions-ordering-most-recent-first codepipeline.list-rule-executions-ordering codepipeline.output-variables-size-exceeded-rejected codepipeline.override-stage-condition-conflict-detection codepipeline.pipeline-execution-lifecycle-transitions codepipeline.pipeline-execution-timestamp-precision-preserved codepipeline.poll-for-jobs-lease-expires-and-job-becomes-available-again codepipeline.poll-for-jobs-unknown-action-type-rejected codepipeline.put-approval-result-invalid-token-rejected codepipeline.put-approval-result-replay-rejected codepipeline.put-job-failure-invalid-state-rejected codepipeline.put-job-success-invalid-job-state-rejected codepipeline.put-job-success-on-already-completed-job-not-permissive codepipeline.put-job-success-result-wrong-state-rejected codepipeline.stop-pipeline-execution-duplicate-request-rejected codepipeline.stop-pipeline-execution-not-stoppable-rejected codepipeline.stop-pipeline-execution-transitions-to-stopped codepipeline.third-party-job-details-reflect-polled-job codepipeline.third-party-job-nonce-single-use-visibility
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|PollForJobs|NotImplemented
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# Implement codepipeline PollForJobs: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PollForJobs

## Description
`PollForJobs` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/poll_for_jobs.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

31 recorded unit(s) fail on their `PollForJobs` call, so each one unblocks only when `PollForJobs` works: `codepipeline.acknowledge-job-not-found`, `codepipeline.approve-already-completed-rejected`, `codepipeline.condition-not-overridable-rejected`, `codepipeline.e2e-pipeline-create-run-approve-complete-teardown`, `codepipeline.get-job-details-not-found`, `codepipeline.get-pipeline-state-reflects-latest-execution`, `codepipeline.job-claim-nonce-single-use-after-acknowledgement-rejected`, `codepipeline.job-claim-withholds-job-from-poll-until-completed`, `codepipeline.job-output-variables-value-round-trip`, `codepipeline.job-poll-visibility-timeout-requeue`, `codepipeline.job-success-output-variables-round-trip`, `codepipeline.list-action-executions-invalid-next-token-rejected`, `codepipeline.list-pipeline-executions-ordering-most-recent-first`, `codepipeline.list-rule-executions-ordering`, `codepipeline.output-variables-size-exceeded-rejected`, `codepipeline.override-stage-condition-conflict-detection`, `codepipeline.pipeline-execution-lifecycle-transitions`, `codepipeline.pipeline-execution-timestamp-precision-preserved`, `codepipeline.poll-for-jobs-lease-expires-and-job-becomes-available-again`, `codepipeline.poll-for-jobs-unknown-action-type-rejected` (+11 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/poll_for_jobs.rs`. Replaying the corpus, the emulator refused the `PollForJobs` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PollForJobs operation: operation PollForJobs is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codepipeline emulator
Call PollForJobs and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

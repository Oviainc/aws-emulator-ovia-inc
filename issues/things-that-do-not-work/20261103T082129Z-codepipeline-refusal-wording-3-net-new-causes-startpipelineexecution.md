Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.pipeline-lifecycle-create-active-delete-404 codepipeline.start-pipeline-execution-concurrent-limit-exceeded codepipeline.put-action-revision-concurrency-limit-exceeded codepipeline.put-action-revision-concurrency-limit-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.pipeline-lifecycle-create-active-delete-404|_assert_pipeline_not_found#29|AssertionError|assert message == expected, f"{what}: expected message {expected!r}, got {message!r}"
Issue-Key: codepipeline.start-pipeline-execution-concurrent-limit-exceeded|run#19|AssertionError|assert err.get("Message") == expected, (err.get("Message"), expected)
Issue-Key: codepipeline.put-action-revision-concurrency-limit-exceeded|run#11|AssertionError|assert limit_message == (
Issue-Key: codepipeline.put-action-revision-concurrency-limit-rejected|run#13|AssertionError|assert limit_message == _LIMIT_MESSAGE, limit_message
Issue-Commit: 1c0413a0db44f6903ae94b4a2eeb83f1af34b87e

# CodePipeline refusal wording: 3 net-new causes — StartPipelineExecution PipelineNotFoundException text, and ConcurrentPipelineExecutionsLimitExceededException text on StartPipelineExecution and (distinctly) on PutActionRevision

> _Note: this issue was identified against the code base at commit `1c0413a0db44f6903ae94b4a2eeb83f1af34b87e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutActionRevision

## Description
THREE net-new root causes in the codepipeline refusal-message paths. A fixer must close ALL THREE before submitting; each cause names its own failing check ids. None of the four checks below is marked ALREADY FILED or OWNED in this replay.

Related but NOT covering (name them when fixing, do not assume they close these checks): the open issue 20261029T183239Z-codepipeline-refusal-classification-and-wording-10-net-new.md discusses the same two refusal families in its Cause 7 and Cause 8 but carries OTHER check ids (codepipeline.pipeline-lifecycle-delete-then-404, codepipeline.start-pipeline-execution-concurrency-limit-exceeded/-rejected) and does NOT carry any of the four ids listed here; the open wire issue 20261029T202908Z-codepipeline-pipeline-not-found-refusals-all-use-one.md covers the same per-operation PipelineNotFoundException wording from the wire corpus. Cause 3 below is a text neither of them states at all.

=== Cause 1 of 3: StartPipelineExecution answers the GetPipeline PipelineNotFoundException message ===
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models PipelineNotFoundException on StartPipelineExecution (errorKind=client, httpError 400), the same code GetPipeline models. Status (400) and code are shared across the family, but the MESSAGE text is worded per operation. Recorded AWS behavior: GetPipeline -> "Account '<accountId>' does not have a pipeline with name '<pipelineName>'"; StartPipelineExecution -> "Pipeline with name <pipelineName> does not exist in account <accountId>." (pipeline name unquoted, account id unquoted, trailing period). In that template the fixed text is the requirement; the only variable parts are the pipeline name from the request and the caller's 12-digit account id.
Emulator: after DeletePipeline, GetPipeline is refused with AWS's exact GetPipeline text (that assertion passes), and StartPipelineExecution against the same dead name is then refused 400 PipelineNotFoundException with that SAME GetPipeline string "Account '<accountId>' does not have a pipeline with name '<pipelineName>'" instead of its own wording.
Why: deployment tooling that distinguishes a deleted pipeline on a read from one on a mutation matches the documented per-operation text and never matches; the two refusals are also indistinguishable from each other.
Fix: per-operation PipelineNotFoundException message construction in the StartPipelineExecution handler, rather than the one shared not-found string used by the codepipeline crate's pipeline lookup.
Traces: codepipeline.pipeline-lifecycle-create-active-delete-404.

=== Cause 2 of 3: StartPipelineExecution's ConcurrentPipelineExecutionsLimitExceededException message is the emulator's own wording ===
AWS: Smithy models ConcurrentPipelineExecutionsLimitExceededException on StartPipelineExecution (errorKind=client, httpError 400). Recorded AWS message is a constant with no variable parts: "The limit for concurrent pipeline executions has been exceeded. Please try again later."
Emulator: the code and the 400 status are right; the message is "The pipeline has reached the limit for concurrent pipeline executions." — entirely different fixed text.
Why: a client that fills a PARALLEL-mode pipeline to its active-execution ceiling and matches this throttling-class message before backing off sees an unknown string.
Fix: the concurrency-limit refusal message in the StartPipelineExecution handler.
Traces: codepipeline.start-pipeline-execution-concurrent-limit-exceeded.

=== Cause 3 of 3: PutActionRevision's ConcurrentPipelineExecutionsLimitExceededException has its OWN AWS text, and the emulator shares one string ===
AWS: Smithy models ConcurrentPipelineExecutionsLimitExceededException on PutActionRevision too (errorKind=client, httpError 400) — PutActionRevision with a new revision starts an execution, so it hits the same active-execution ceiling. But the recorded AWS message for this operation is NOT StartPipelineExecution's: it is "The limit for concurrent pipeline executions has been exceeded. Wait for concurrent executions to complete, and then try again." — the first sentence is shared, the second differs ("Wait for concurrent executions to complete, and then try again." vs StartPipelineExecution's "Please try again later."). The whole string is fixed text with no variable parts.
Emulator: answers the right code and 400 but the single shared string "The pipeline has reached the limit for concurrent pipeline executions.", so fixing Cause 2 alone still leaves PutActionRevision diverging. The code, the 400 status and the effect-absent behaviour (no extra execution created) are correct; only the message text diverges.
Why: a source-revision driver using PutActionRevision to trigger executions cannot match the documented guidance text, and the emulator's one-string approach makes the two operations' distinct AWS messages unreachable.
Fix: a per-operation message for the concurrency-limit refusal in the PutActionRevision handler's revision/execution-start path, not a shared constant in the codepipeline crate.
Traces: codepipeline.put-action-revision-concurrency-limit-exceeded, codepipeline.put-action-revision-concurrency-limit-rejected.

## Evidence
All quotes are verbatim from this replay's failing-check details. Pipeline names, account ids and execution ids are per-run values that fill the message templates and are not themselves requirements.

Cause 1 — codepipeline.pipeline-lifecycle-create-active-delete-404 [mismatch]:
  AssertionError: StartPipelineExecution against the deleted pipeline: expected message 'Pipeline with name sim2real-cp-1ae116e2b930 does not exist in account 000000000000.', got "Account '000000000000' does not have a pipeline with name 'sim2real-cp-1ae116e2b930'"
  The same check's earlier assertions pass: code == PipelineNotFoundException, status == 400, and the GetPipeline-after-delete message matched AWS exactly. Only StartPipelineExecution's text diverges.

Cause 2 — codepipeline.start-pipeline-execution-concurrent-limit-exceeded [mismatch]:
  AssertionError: ('The pipeline has reached the limit for concurrent pipeline executions.', 'The limit for concurrent pipeline executions has been exceeded. Please try again later.')
  (actual, expected). The error Code assertion ConcurrentPipelineExecutionsLimitExceededException passed for the 51st active execution.

Cause 3 — codepipeline.put-action-revision-concurrency-limit-exceeded [mismatch]:
  AssertionError: The pipeline has reached the limit for concurrent pipeline executions.
  raised by `assert limit_message == ("The limit for concurrent pipeline executions has been exceeded. " "Wait for concurrent executions to complete, and then try again.")`.
  codepipeline.put-action-revision-concurrency-limit-rejected [mismatch]: identical actual message, raised by `assert limit_message == _LIMIT_MESSAGE` with the same expected text. In both checks the preceding assertion limit_code == 'ConcurrentPipelineExecutionsLimitExceededException' passed, and so did newRevision=True on the positive-control calls.

## Steps to Reproduce
Boot the emulator and point botocore at it: cp = boto3.client('codepipeline'), s3 = boto3.client('s3'), iam = boto3.client('iam').

Cause 1: create an artifact bucket and a pipeline role, CreateCustomActionType for a Source and a Build provider, CreatePipeline P with those two stages, GetPipeline/UpdatePipeline to confirm it is live, then cp.delete_pipeline(name=P). Now (a) cp.get_pipeline(name=P) -> 400 PipelineNotFoundException "Account '<acct>' does not have a pipeline with name 'P'" (already correct) and (b) cp.start_pipeline_execution(name=P) -> expect 400 PipelineNotFoundException with 'Pipeline with name P does not exist in account <acct>.'; observe the message from (a).

Cause 2: create a PARALLEL-mode V2 pipeline whose second stage is a manual approval that is never approved, so executions stay InProgress. Start executions sequentially (pace them ~1s apart so the request-rate throttle is not what trips) until 50 are active, then issue one more cp.start_pipeline_execution(name=P) -> expect ConcurrentPipelineExecutionsLimitExceededException with message exactly 'The limit for concurrent pipeline executions has been exceeded. Please try again later.'

Cause 3: create a PARALLEL-mode V2 pipeline with a Source action. Call cp.put_action_revision(pipelineName=P, stageName='Source', actionName='SourceAction', actionRevision={'revisionId': f'rev-{i}', 'revisionChangeId': f'chg-{i}', 'created': now}) with a fresh revision each time, sequentially, until the call is refused -> expect ConcurrentPipelineExecutionsLimitExceededException with message exactly 'The limit for concurrent pipeline executions has been exceeded. Wait for concurrent executions to complete, and then try again.' (note: NOT StartPipelineExecution's 'Please try again later.' wording).

Then re-replay the four check ids listed in trace_ids.

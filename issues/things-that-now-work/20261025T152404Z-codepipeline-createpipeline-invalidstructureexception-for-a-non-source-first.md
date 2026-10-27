Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.create-pipeline-first-stage-not-source-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.create-pipeline-first-stage-not-source-rejected|run#13|AssertionError|assert message == "Pipeline should start with a stage that only contains source actions", (
Issue-Commit: f808b2c8e13d7f3dcd6135e455847d306dbb5fc7

# CodePipeline CreatePipeline: InvalidStructureException for a non-source first stage carries a locally-invented message instead of AWS's fixed wording

> _Note: this issue was identified against the code base at commit `f808b2c8e13d7f3dcd6135e455847d306dbb5fc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipeline

## Description
One net-new root cause found in this codepipeline behavioral sweep. Every other failing codepipeline check in this run was marked ALREADY FILED or OWNED by an open issue and is skipped here.

=== Cause 1 of 1: the InvalidStructureException raised when the first stage contains no Source action has the wrong message text ===

AWS: Smithy: CreatePipeline.errors includes InvalidStructureException (errorKind client, httpError 400) — models/codepipeline/2015-07-09, operation CreatePipeline, input member `pipeline` -> PipelineDeclaration. CodePipeline enforces a structural law that the Smithy model does not express: the first stage of a pipeline must contain only source actions, and source actions may appear only in the first stage. A declaration whose stage 1 is an Approval stage and whose S3 Source action sits in stage 2 is refused at request time with HTTP 400, error code `InvalidStructureException`, and the message

    Pipeline should start with a stage that only contains source actions

That message is recorded behavior (captured 2026-09-16, us-west-2) and is entirely FIXED TEXT: it is a constant sentence for this error class with NO variable parts — no stage name, action name, pipeline name, count or index is interpolated into it. The recording also confirms the code is InvalidStructureException and not the plausible alternative InvalidStageDeclarationException.

Emulator: the emulator correctly refuses the call with error code `InvalidStructureException`, but replaces AWS's fixed sentence with its own diagnostic prose that additionally interpolates request-derived values:

    the first stage of a pipeline must contain at least one Source action, but stage Gate contains none

(the trailing `Gate` is the first stage's name taken from the request; the rest is invented wording). The check reports: AssertionError: expected the exact InvalidStructureException message AWS returns, got 'the first stage of a pipeline must contain at least one Source action, but stage Gate contains none'.

Note the semantics also differ, not only the prose: AWS's rule is "a stage that ONLY contains source actions", while the emulator's message states the weaker "at least one Source action" — so a first stage mixing a Source action with a non-source action is described (and, on the evidence of this wording, likely validated) as acceptable where AWS refuses it.

Why: the error message is part of the operation's contract. A client that matches on, logs, surfaces or asserts against the message AWS returns for a mis-ordered pipeline sees different text here, so message-based branching and negative tests that pass against AWS fail against the emulator (and vice versa). Because the emulator's wording is also a weaker statement of the rule, an operator reading it is told a first stage merely needs one source action, which is not the invariant AWS enforces.

Fix: the message construction for the structural first-stage rule in the shared pipeline-declaration validator used by the CreatePipeline handler (the same validator that must raise InvalidStructureException). Emit the fixed sentence for this error class and interpolate nothing; keep the existing HTTP 400 / InvalidStructureException classification, which is already correct. The related open issue 20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md covers MISSING declaration-validation classes in the same validator (unknown providers, approval outputArtifacts, duplicate stage names) but says nothing about this message, and the fixed issue 20261023T080113Z-codepipeline-error-messages-are-not-built-by-aws.md covered PipelineNotFound / pipeline-version / PipelineExecutionNotFound / StageNotFound messages only — not InvalidStructureException.

Traces: codepipeline.create-pipeline-first-stage-not-source-rejected.

## Evidence
Cause 1 — CreatePipeline with stages [Gate (Approval/AWS/Manual), Source (Source/AWS/S3)] in that order, pipelineType V2, a genuinely assumable role and a versioned S3 artifact store.

Expected (recorded from real AWS): HTTP 400, Error.Code == "InvalidStructureException", Error.Message == "Pipeline should start with a stage that only contains source actions" (fixed text, no variable parts), and the rejected pipeline absent from ListPipelines.

Actual (emulator): Error.Code == "InvalidStructureException" (correct), Error.Message == "the first stage of a pipeline must contain at least one Source action, but stage Gate contains none" — invented wording, with the first stage's name ("Gate") interpolated from the request where AWS interpolates nothing.

Check assertion that failed, verbatim:
  File "check_codepipeline_create_pipeline_first_stage_not_source_rejected.py", line 212, in run
    assert message == "Pipeline should start with a stage that only contains source actions"
  AssertionError: expected the exact InvalidStructureException message AWS returns, got 'the first stage of a pipeline must contain at least one Source action, but stage Gate contains none'

The control half of the check passed: the same two stages declared in the correct order (Source then Gate) are accepted and the stage order is preserved, so this is purely the refusal message of the inverted declaration.

## Steps to Reproduce
1. Boot the emulator and replay codepipeline.create-pipeline-first-stage-not-source-rejected.
2. Create a versioned S3 bucket to serve as the artifact store, and an IAM role with a codepipeline.amazonaws.com trust policy.
3. CreatePipeline with pipelineType V2, that roleArn and artifactStore, and stages in this order:
   - stage "Gate": one action "ApproveAction", actionTypeId {category: Approval, owner: AWS, provider: Manual, version: "1"}, runOrder 1.
   - stage "Source": one action "SourceAction", actionTypeId {category: Source, owner: AWS, provider: S3, version: "1"}, runOrder 1, configuration {S3Bucket, S3ObjectKey, PollForSourceChanges: "false"}, outputArtifacts [{name: SourceOutput}].
4. Observe the ClientError: the code is InvalidStructureException (correct) but Error.Message is "the first stage of a pipeline must contain at least one Source action, but stage <firstStageName> contains none" instead of AWS's fixed "Pipeline should start with a stage that only contains source actions".
5. Re-run step 3 with the stages reversed (Source first) and confirm it is accepted — isolating the mutated knob to stage order.

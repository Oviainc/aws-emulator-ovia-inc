Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__codepipeline__ecr.ecr-push-to-untracked-repository-does-not-trigger-pipeline
Issue-Key: codebuild__codepipeline__ecr.ecr-push-to-untracked-repository-does-not-trigger-pipeline|run#17|AssertionError|assert now < deadline, f"no new execution on pipeline {pipeline_name} within {timeout_s:.0f}s of pushes to the tracked repository"
Issue-Commit: ed1d21260dddf47059c212135af7f1fff621c450

# CodeBuild/CodePipeline/ECR: an ECR image push never starts the pipeline its EventBridge rule targets, so the positive control of the untracked-repository check never fires

> _Note: this issue was identified against the code base at commit `ed1d21260dddf47059c212135af7f1fff621c450`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
ONE net-new root cause: EventBridge delivery from an ECR image push to a CodePipeline target never happens, so no pipeline execution is ever started by a push.

Related open issue that describes the same family but does NOT carry this check's replay key, so it does not cover this work: 20261029T174654Z-codebuild-codepipeline-ecr-build-actions-are-never-executed.md (its keys are the build-action and CFN checks, not ecr-push-to-untracked-repository-does-not-trigger-pipeline).

Cause 1 of 1: pushing an image to an ECR repository that an EventBridge rule references, with a rule target of the pipeline and a role permitting codepipeline:StartPipelineExecution, starts no pipeline execution.
AWS: recorded behavior only for the end-to-end delivery (the hop is not described by any one service's Smithy model). On a push, ECR publishes an 'ECR Image Action' event; an EventBridge rule whose pattern matches that repository invokes its target via the supplied role, which calls CodePipeline StartPipelineExecution; a NEW execution id then appears in ListPipelineExecutions for that pipeline, beyond the ids present before the push. A push to a repository that NO rule references starts nothing — that negative is the check's subject, and it is only meaningful once the positive control is observed on the same wiring.
Emulator: after repeated PutImage pushes to the tracked repository (re-pushed every 20s for the whole budget), ListPipelineExecutions for the pipeline returns no execution id beyond the baseline — the check fails with "no new execution on pipeline <pipeline> within 55s of pushes to the tracked repository". The emulator never delivers the push event to the rule's CodePipeline target.
Why: event-driven CI is the whole point of this topology. A client that wires ECR -> EventBridge -> CodePipeline sees a silent no-op: pushes never build, nothing appears in ListPipelineExecutions, and there is no error anywhere to diagnose. It also makes the negative case unprovable — an emulator that delivers nothing at all trivially 'passes' the no-trigger assertion while being wrong, so the absence of pipeline executions can never be attributed to the missing rule.
Fix: the ECR image-push event publication and the EventBridge rule dispatch to a CodePipeline target (the ECR PutImage path that emits the 'ECR Image Action' event, and the events crate's target invocation that calls StartPipelineExecution with the rule's role).
Traces: codebuild__codepipeline__ecr.ecr-push-to-untracked-repository-does-not-trigger-pipeline.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: 72/190 passed (37.9%); codebuild__codepipeline__ecr 1/29 with 28 failures. This is the only failure under that heading that is neither blocked by an unimplemented scaffold stub nor owned by an open issue's replay key.

codebuild__codepipeline__ecr.ecr-push-to-untracked-repository-does-not-trigger-pipeline: the check creates an artifact bucket, two ECR repositories (one tracked, one untracked), a codepipeline.amazonaws.com role, a pipeline with an ECR source on the tracked repository (tag 'latest'), an events.amazonaws.com role permitting codepipeline:StartPipelineExecution on the pipeline ARN, and an EventBridge rule matching pushes to the TRACKED repository with the pipeline as target. It then re-pushes the tracked image every 20s and polls ListPipelineExecutions for an id beyond the baseline. Failure: AssertionError "no new execution on pipeline sim2real-pipeline-<suffix> within 55s of pushes to the tracked repository" (the pipeline name and the 55s window are per-run values — the window is derived from the remaining run budget, not a fixed requirement). The negative half of the check (a push to the untracked repository must start nothing, within an absence window of >=3x the measured control latency) was never reached, because that latency can only be measured once the control fires.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Create an S3 artifact bucket, and two ECR repositories (call them TRACKED and UNTRACKED).
3. Create an IAM role trusted by codepipeline.amazonaws.com with s3:PutObject/GetObject/GetObjectVersion on the bucket and ecr:DescribeImages, and create a pipeline whose source action is ECR on TRACKED with imageTag 'latest'.
4. Snapshot the pipeline's execution ids via ListPipelineExecutions.
5. Create an IAM role trusted by events.amazonaws.com with codepipeline:StartPipelineExecution on the pipeline ARN, and an EventBridge rule whose event pattern matches an ECR image push to TRACKED with that pipeline as the target (using that role).
6. PutImage a 'latest' image into TRACKED and poll ListPipelineExecutions — expect a NEW execution id beyond the snapshot; observe that no new execution ever appears, even with the push repeated every 20s for the whole budget.
7. Then PutImage into UNTRACKED and confirm no new execution appears (this negative assertion is only meaningful once step 6 fires).

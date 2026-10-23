Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__codepipeline__ecr.deploy-stack-outputs-correct-codebuild-project-arn-in-pipeline-stage
Issue-Key: codebuild__codepipeline__ecr|GetPipeline|PipelineNotFoundException
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# CloudFormation stack reports completion but provisions no CodePipeline pipeline: GetPipeline on the stack's declared pipeline answers PipelineNotFoundException

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipeline

## Description
One root cause. This failing check is not marked ALREADY FILED or OWNED by the replay report; its sibling checks in the same service ARE owned by 20261022T172625Z-codebuild-codepipeline-ecr-ci-chain-pipeline-executions-never.md (which covers the CI chain, the FAULTing builds and the ECR UploadLayerPart stub) and the CodePipeline-only form of this provisioning gap is Cause 8 of 20261022T173520Z-codepipeline-executions-never-progress-structural-validation-absent-error.md. Neither of those issues carries this check id, so it is recorded here.

=== Cause 1 of 1: a CreateStack whose template declares AWS::CodePipeline::Pipeline reaches completion without creating the pipeline ===
AWS: a CloudFormation stack reaches CREATE_COMPLETE only once every declared resource exists and is addressable by its physical id, and DescribeStacks Outputs carry an OutputValue per declared Output. A stack declaring an AWS::CodePipeline::Pipeline (with an AWS::CodeBuild::Project referenced from its Build stage) therefore leaves a pipeline that GetPipeline resolves — Smithy: GetPipeline.errors includes PipelineNotFoundException (errorKind client, httpError 400), raised only when no such pipeline exists in the account.
Emulator: the stack reports completion, but the declared pipeline does not exist. The check's first post-deploy call fails with PipelineNotFoundException from GetPipeline for the stack-named pipeline, so the stage/action assertions on the CodeBuild project ARN are never reached. The status and code are exactly those of a genuinely absent resource, i.e. nothing was provisioned — the divergence is the provisioning, not the serializer.
Why: every infrastructure-as-code path through CloudFormation is broken: a client deploys a stack, is told it succeeded, and then every reference to a stack resource 404s. Because the stack reports success, the failure surfaces far from its cause, and a template regression is indistinguishable from a working deployment.
Fix: the CloudFormation stack-resource provisioner — the resource-type handler that must create the real underlying resource (and populate the stack's Outputs) for AWS::CodePipeline::Pipeline, alongside the CodeBuild/IAM/ECR types this template also declares.
Traces: codebuild__codepipeline__ecr.deploy-stack-outputs-correct-codebuild-project-arn-in-pipeline-stage.

## Evidence
Expected: after the stack completes, GetPipeline(name=<stack's pipeline>) returns the pipeline declaration whose Build stage action configuration carries the stack's CodeBuild project ARN (and DescribeStacks Outputs carry an OutputValue for it).
Actual: "botocore.errorfactory.PipelineNotFoundException: An error occurred (PipelineNotFoundException) when calling the GetPipeline operation: pipeline sim2real-xsvc-stk-558b45cec588-Pipeline-A5B74C481269 does not exist". Sibling checks in the same stack show the same provisioning gap for other types (GetRole -> NoSuchEntity for the stack's CodeBuildRole/PipelineRole, StartBuild -> ResourceNotFoundException for the stack's project, DescribeStacks -> KeyError 'OutputValue').

## Steps to Reproduce
1. Boot the emulator and replay codebuild__codepipeline__ecr.deploy-stack-outputs-correct-codebuild-project-arn-in-pipeline-stage.
2. CreateStack with a template declaring an AWS::ECR::Repository, an AWS::CodeBuild::Project, the two AWS::IAM::Roles and an AWS::CodePipeline::Pipeline whose Build stage references the project, plus an Output for the project ARN.
3. Wait for the stack to report completion, then call GetPipeline with the stack's pipeline name — it answers 400 PipelineNotFoundException instead of returning the declaration.

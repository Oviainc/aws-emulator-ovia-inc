Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__codepipeline__ecr.cli-verify-ecr-image-pushed-after-pipeline-execution-succeeds codebuild__codepipeline__ecr.codebuild-environment-variables-injected-from-pipeline-action codebuild__codepipeline__ecr.codebuild-project-arn-resolves-correctly-in-pipeline-action-config codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected codebuild__codepipeline__ecr.deploy-stack-outputs-correct-codebuild-project-arn-in-pipeline-stage
Issue-Key: codebuild__codepipeline__ecr.cli-verify-ecr-image-pushed-after-pipeline-execution-succeeds|run#8|AssertionError|assert status == "Succeeded", f"pipeline execution ended with status={status}"
Issue-Key: codebuild__codepipeline__ecr.codebuild-environment-variables-injected-from-pipeline-action|run#16|AssertionError|assert status == "Succeeded", f"pipeline execution ended with status={status}"
Issue-Key: codebuild__codepipeline__ecr.codebuild-project-arn-resolves-correctly-in-pipeline-action-config|run#23|AssertionError|assert status == "Succeeded", f"pipeline execution ended with status={status}"
Issue-Key: codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration|run#13|AssertionError|assert final_status == "Succeeded", f"pipeline execution ended in {final_status}"
Issue-Key: codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result|_get_action_execution#16|AssertionError|raise AssertionError(f"no action execution found for {stage_name}/{action_name}")
Issue-Key: codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected|_get_action_execution#20|AssertionError|raise AssertionError(f"no action execution found for {stage_name}/{action_name}")
Issue-Key: codebuild__codepipeline__ecr.deploy-stack-outputs-correct-codebuild-project-arn-in-pipeline-stage|run#14|IndexError|project = codebuild.batch_get_projects(names=[configured_project_name])["projects"][0]
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# CodePipeline build stages fail and record no action executions, and a CFN-declared CodeBuild project is never created (3 causes)

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
Three causes. Related open issues that do NOT cover these checks: 20261022T172625Z-codebuild-codepipeline-ecr-ci-chain-pipeline-executions-never.md (owns four other checks in this suite), 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md, 20261023T080113Z-codebuild-codepipeline-ecr-cloudformation-stack-reports-completion-but-provisions-no-codepipeline.md.

Cause 1 of 3: every CodeBuild build FAULTs at PROVISIONING, so a pipeline whose Build stage invokes CodeBuild always terminates `Failed`.
AWS: recorded behavior only. StartBuild runs the buildspec; BatchGetBuilds reports buildStatus `SUCCEEDED` (or `FAILED` when the buildspec fails), and a CodePipeline execution whose only non-source stage is that build reaches pipelineExecutionStatus `Succeeded`.
Emulator: BatchGetBuilds reports currentPhase `COMPLETED`, buildStatus `FAULT`, with a PROVISIONING phase whose context message is `this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed`; GetPipelineExecution then reports `Failed`. No image is ever pushed to the destination ECR repository and no buildspec side effect occurs.
Why: a client cannot distinguish a build that genuinely failed from one that was never run; every CI assertion downstream of a build (artifact pushed, image tag present, environment variable honoured, base image pulled) is unreachable, and `FAULT` is not the status a caller polling for SUCCEEDED/FAILED expects to terminate on.
Fix: the CodeBuild StartBuild/build-execution state machine (the build runner/phase model) and the CodePipeline action worker that consumes its terminal status.
Traces: codebuild__codepipeline__ecr.cli-verify-ecr-image-pushed-after-pipeline-execution-succeeds, codebuild__codepipeline__ecr.codebuild-environment-variables-injected-from-pipeline-action, codebuild__codepipeline__ecr.codebuild-project-arn-resolves-correctly-in-pipeline-action-config, codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration.

Cause 2 of 3: ListActionExecutions returns no entry for a stage/action that a started pipeline execution ran.
AWS: recorded behavior only. After StartPipelineExecution, ListActionExecutions (filtered by pipelineExecutionId) returns one actionExecutionDetails entry per stage/action — including `Build`/`Build` — carrying its status and output.
Emulator: no action execution is ever found for `Build`/`Build`, so a caller cannot read the per-action outcome at all (the check raises `no action execution found for Build/Build`). This is independent of cause 1: whatever terminal status the action takes, the execution record is absent.
Why: a client that reads per-action status (the normal way to attribute a pipeline failure to the action that caused it, and to assert that a deleted build project makes the Build action fail) sees an empty history and cannot tell that the action ran at all.
Fix: the CodePipeline action-execution history state (what StartPipelineExecution records per stage/action) and the ListActionExecutions handler.
Traces: codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result, codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected.

Cause 3 of 3: a CloudFormation stack that declares an AWS::CodeBuild::Project reaches completion but no project exists.
AWS: recorded behavior only. After the stack completes, the project named by the stack output exists and BatchGetProjects(names=[<name>]) returns exactly one project whose arn matches the stack's output.
Emulator: BatchGetProjects returns an empty `projects` list for the stack's own declared project name, so indexing `[0]` raises IndexError — the resource was reported as provisioned but nothing was registered in CodeBuild.
Why: every client that discovers resources through stack outputs (CDK, CI templates) gets names that resolve to nothing, while the stack claims success.
Fix: the CloudFormation resource provider for AWS::CodeBuild::Project (resource create must call through to the CodeBuild state), not the CodeBuild read path.
Traces: codebuild__codepipeline__ecr.deploy-stack-outputs-correct-codebuild-project-arn-in-pipeline-stage.

## Evidence
Cause 1: AssertionError: pipeline execution ended with status=Failed; BatchGetBuilds shows {'currentPhase': 'COMPLETED', 'buildStatus': 'FAULT', 'phases': [... {'phaseType': 'PROVISIONING', 'phaseStatus': 'FAULT', 'contexts': [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}]} ...]}. Expected pipelineExecutionStatus 'Succeeded' and buildStatus 'SUCCEEDED' (or 'FAILED' for the negative leg, where the check got 'FAULT').
Cause 2: AssertionError: no action execution found for Build/Build — ListActionExecutions returned no entry for the executed stage/action.
Cause 3: IndexError: list index out of range at `codebuild.batch_get_projects(names=[configured_project_name])['projects'][0]` — empty projects list for the project the completed stack declared.

## Steps to Reproduce
Cause 1: create a CodePipeline with a Source stage and a Build stage invoking a CodeBuild project, call codepipeline.start_pipeline_execution, poll get_pipeline_execution; observe 'Failed', and codebuild.batch_get_builds on the build id shows buildStatus 'FAULT' with a FAULT PROVISIONING phase.
Cause 2: after that execution, call codepipeline.list_action_executions(pipelineName=..., filter={'pipelineExecutionId': <id>}); observe no entry for stage 'Build' / action 'Build'.
Cause 3: create a CFN stack declaring AWS::CodeBuild::Project (plus a pipeline referencing it), wait for completion, read the project name from the stack outputs and call codebuild.batch_get_projects(names=[name]); observe projects == [].

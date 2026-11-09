Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration
Issue-Key: codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result|run#18|KeyError|build_id = result["externalExecutionId"]
Issue-Key: codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected|run#34|AssertionError|assert status_2 == "Succeeded", f"expected Succeeded once pointed at an existing project, got {status_2}"
Issue-Key: codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6

# CodeBuild-in-CodePipeline: 3 net-new root causes — a Build action's executionResult carries no externalExecutionId, a valid existing project still fails the Build action, and the CFN/CodeBuild/ECR integration never settles

> _Note: this issue was identified against the code base at commit `aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Resolution note: causes 1 and 2 are fixed (a build now runs its buildspec's commands and its status, and the containing pipeline action's, is their outcome; the action reports the build it ran as `externalExecutionId`). Cause 3's own symptom — the check exhausting its ceiling with no settled state — is fixed too: `AWS::ECR::Repository` is now really created in Amazon ECR, the stack reaches `CREATE_COMPLETE` and the execution reaches a terminal status. What remains for that check is its stack's buildspec needing programs a build container provides, which no in-process build environment has; that leg is `assert final_status == "Succeeded"`, carried as Cause 1 of `issues/things-that-do-not-work/20261108T090606Z-codebuild-codepipeline-ecr-pipeline-driven-builds-never-execute.md`, which owns it._

## Operation
ListActionExecutions

## Description
THREE net-new root causes in the CodeBuild-backed CodePipeline Build action. A fixer must close ALL THREE before submitting; each cause names its own failing check ids below.

None of these three checks is marked ALREADY FILED or OWNED in this replay. Related but NOT covering: 20261022T172625Z-codebuild-codepipeline-ecr-ci-chain-pipeline-executions-never.md (builds FAULT at PROVISIONING) and 20261024T014332Z-codebuild-codepipeline-ecr-codepipeline-build-stages-fail-and-record-no-action.md (build stages record no action executions) own the sibling checks in this service; these three carry replay keys neither of them holds.

=== Cause 1 of 3: a CodeBuild action execution's output.executionResult has no externalExecutionId ===
AWS: Smithy (com.amazonaws.codepipeline#CodePipeline_20150709) models ActionExecutionOutput$executionResult -> ActionExecutionResult, whose externalExecutionId is documented as "The action provider's external ID for the action execution". For a CodeBuild Build action that is the CodeBuild build ID ("<projectName>:<buildUuid>"), and the sibling member ActionExecution$externalExecutionId carries it in GetPipelineState too; it is how a client joins a pipeline action to the build that ran it.
Emulator: ListActionExecutions returns the Build action's output.executionResult WITHOUT an externalExecutionId member at all — result["externalExecutionId"] raises KeyError: 'externalExecutionId'.
Why: a client cannot correlate a pipeline action with the CodeBuild build it launched, so BatchGetBuilds on the action's build, log retrieval and build-status propagation are all impossible — the documented join key between the two services is missing.
Fix: the codepipeline crate's action-execution record for provider-backed actions and its ListActionExecutions/GetPipelineState serializers — the launched build's id must be stored and emitted as ActionExecutionResult$externalExecutionId (and ActionExecution$externalExecutionId).
Traces: codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result.

=== Cause 2 of 3: a Build action pointed at an existing, valid CodeBuild project still fails the execution ===
AWS: recorded behavior only for the positive half of this check: a CodePipeline Build action whose configuration ProjectName names an EXISTING CodeBuild project launches that project and the pipeline execution reaches 'Succeeded' (the negative half — a DELETED project — fails the action, which is what the check is about). Smithy models no error for this, so the requirement is the recorded terminal status 'Succeeded'.
Emulator: after the pipeline is re-pointed at an existing project, the execution still ends 'Failed'. The sibling checks in this service show why the build cannot succeed: BatchGetBuilds reports buildStatus 'FAULT' with the PROVISIONING phase context message "this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed" — an emulator-authored sentence AWS never returns, and a buildStatus AWS does not use for a well-formed build.
Why: the check's negative assertion becomes vacuous — a client cannot distinguish "the Build action failed because the project is deleted" from "every Build action fails here" — and no CodeBuild-backed pipeline can ever succeed, so the whole CI shape is unusable.
Fix: the CodeBuild build execution path (it must take a well-formed build to buildStatus 'SUCCEEDED' with the modelled phases rather than FAULT at PROVISIONING) and the codepipeline Build-action handler that maps the build's outcome onto the action and execution status; no phase context may describe the emulator.
Traces: codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected.

=== Cause 3 of 3: the CloudFormation-deployed pipeline + CodeBuild + ECR integration never settles ===
AWS: a CloudFormation stack declaring an ECR repository, a CodeBuild project and a CodePipeline pipeline reaches CREATE_COMPLETE with all three resources created, the pipeline then runs its Source and Build stages to a terminal status within seconds-to-minutes, and the image the build pushes is listed by DescribeImages on the repository.
Emulator: the check stops at the suite's 60 s per-check ceiling ('Error: Check timed out after 60.0s') while waiting for that settled state; no terminal state is observed to assert on. Hypothesis (neither the model nor the recording states the mechanism): the pipeline execution driven from the CFN-created resources never advances, consistent with causes 1-2 above and with the sibling issues named at the top.
Why: the end-to-end, infrastructure-as-code CI shape — the way these services are actually consumed — hangs with no error, so a client gets neither a result nor a diagnosis.
Fix: the CloudFormation provisioning of AWS::ECR::Repository / AWS::CodeBuild::Project / AWS::CodePipeline::Pipeline and the codepipeline execution driver behind the pipeline those resources form.
Traces: codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration.

## Evidence
Quoted verbatim from the replay report; project names, build uuids and bucket names are generated per run and are examples.

Cause 1 — codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result [error]:
  build_id = result["externalExecutionId"]
  KeyError: 'externalExecutionId'
(result is the Build action's output.executionResult from ListActionExecutions.)

Cause 2 — codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected [mismatch]:
  AssertionError: expected Succeeded once pointed at an existing project, got Failed
The underlying build answer, from the sibling checks in this same service run:
  'buildStatus': 'FAULT', 'phases': [... {'phaseType': 'PROVISIONING', 'phaseStatus': 'FAULT', ..., 'contexts': [{'message': 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'}]}, {'phaseType': 'COMPLETED', ...}]

Cause 3 — codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration [mismatch]:
  Error: Check timed out after 60.0s
Per-service pass rate for codebuild__codepipeline__ecr in this run: 0/11 (0%).

## Steps to Reproduce
Boot the emulator and point botocore at it; cp = client('codepipeline'), codebuild = client('codebuild'), ecr = client('ecr'), cfn = client('cloudformation'), s3 = client('s3'), iam = client('iam').

Cause 1: create an artifact bucket with a source object, a pipeline role, and a CodeBuild project; cp.create_pipeline with an S3 Source stage and a Build stage {category Build, owner AWS, provider CodeBuild, version '1'} configured with ProjectName=<project>; cp.start_pipeline_execution; poll cp.list_action_executions(pipelineName=P, filter={'pipelineExecutionId': E}) for the Build action -> expect its output.executionResult.externalExecutionId to be the CodeBuild build id '<projectName>:<uuid>'; observe the member absent.
Cause 2: with the same pipeline, cp.update_pipeline so the Build action's ProjectName names a DELETED project and start an execution -> expect the execution to fail; then cp.update_pipeline back to the EXISTING project and start another execution -> expect 'Succeeded'; observe 'Failed', and codebuild.batch_get_builds on the launched build reporting buildStatus 'FAULT' with the PROVISIONING context message quoted above.
Cause 3: cfn.create_stack with a template declaring AWS::ECR::Repository, AWS::CodeBuild::Project and AWS::CodePipeline::Pipeline wired together; wait for CREATE_COMPLETE, then start the pipeline and poll for a terminal execution status and for ecr.describe_images to list the pushed image -> observe the check exhausting the 60 s ceiling with no settled state.
Then re-replay the three check ids in trace_ids.

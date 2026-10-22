Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result codebuild__codepipeline__ecr.codebuild-environment-variables-injected-from-pipeline-action codebuild__codepipeline__ecr.codebuild-project-arn-resolves-correctly-in-pipeline-action-config codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected codebuild__codepipeline__ecr.codepipeline-execution-includes-ecr-image-digest-as-source-revision codebuild__codepipeline__ecr.ecr-image-push-triggers-codepipeline-eventbridge-execution codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger codebuild__codepipeline__ecr.ecr-push-to-untracked-repository-does-not-trigger-pipeline codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation codebuild__codepipeline__ecr.pipeline-invokes-codebuild-project-on-build-stage codebuild__codepipeline__ecr.pipeline-passes-source-artifact-location-to-codebuild codebuild__codepipeline__ecr.pipeline-role-missing-codebuild-startbuild-permission-blocks-invocation-rejected codebuild__codepipeline__ecr.codebuild-authenticates-to-ecr-via-get-authorization-token codebuild__codepipeline__ecr.codebuild-missing-ecr-login-fails-docker-push codebuild__codepipeline__ecr.codebuild-pulls-base-image-from-ecr-private-registry codebuild__codepipeline__ecr.codebuild-pushes-docker-image-to-ecr-repository codebuild__codepipeline__ecr.codebuild-role-missing-ecr-push-permission-blocks-image-upload codebuild__codepipeline__ecr.ecr-image-tag-matches-codebuild-generated-tag codebuild__codepipeline__ecr.ecr-repository-policy-denies-codebuild-role-blocks-pull codebuild__codepipeline__ecr.cli-create-pipeline-with-codebuild-and-ecr-source-full-flow codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions codebuild__codepipeline__ecr.cli-start-pipeline-execution-and-verify-codebuild-log-output codebuild__codepipeline__ecr.cli-verify-ecr-image-pushed-after-pipeline-execution-succeeds codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration codebuild__codepipeline__ecr.deploy-stack-outputs-correct-codebuild-project-arn-in-pipeline-stage codebuild__codepipeline__ecr.deploy-stack-codebuild-role-trust-policy-allows-codebuild-service codebuild__codepipeline__ecr.deploy-stack-pipeline-role-trust-policy-allows-codepipeline-service codebuild__codepipeline__ecr.deploy-stack-missing-ecr-repository-policy-blocks-codebuild-push codebuild__codepipeline__ecr.codebuild-role-missing-trust-policy-blocks-assume-role-during-build-rejected
Issue-Key: codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result|_wait_for_pipeline_execution#14|Timeout|time.sleep(5)
Issue-Key: codebuild__codepipeline__ecr.codebuild-environment-variables-injected-from-pipeline-action|_wait_for_pipeline_execution#15|Timeout|time.sleep(5)
Issue-Key: codebuild__codepipeline__ecr.codebuild-project-arn-resolves-correctly-in-pipeline-action-config|_wait_for_pipeline_execution#22|Timeout|time.sleep(5)
Issue-Key: codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected|_wait_for_pipeline_execution#18|Timeout|time.sleep(5)
Issue-Key: codebuild__codepipeline__ecr.codepipeline-execution-includes-ecr-image-digest-as-source-revision|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: codebuild__codepipeline__ecr.ecr-image-push-triggers-codepipeline-eventbridge-execution|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: codebuild__codepipeline__ecr.ecr-push-to-untracked-repository-does-not-trigger-pipeline|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: codebuild__codepipeline__ecr.pipeline-invokes-codebuild-project-on-build-stage|_wait_for_pipeline_execution#14|Timeout|time.sleep(5)
Issue-Key: codebuild__codepipeline__ecr.pipeline-passes-source-artifact-location-to-codebuild|_wait_for_pipeline_execution#15|Timeout|time.sleep(5)
Issue-Key: codebuild__codepipeline__ecr.pipeline-role-missing-codebuild-startbuild-permission-blocks-invocation-rejected|_wait_for_pipeline_execution#15|Timeout|time.sleep(5)
Issue-Key: codebuild__codepipeline__ecr.codebuild-authenticates-to-ecr-via-get-authorization-token|run#9|AssertionError|assert build["buildStatus"] == "SUCCEEDED", f"build did not succeed: {build}"
Issue-Key: codebuild__codepipeline__ecr.codebuild-missing-ecr-login-fails-docker-push|run#7|AssertionError|assert no_login_build["buildStatus"] == "FAILED", f"expected FAILED, got {no_login_build}"
Issue-Key: codebuild__codepipeline__ecr.codebuild-pulls-base-image-from-ecr-private-registry|run#10|AssertionError|assert setup_build["buildStatus"] == "SUCCEEDED", f"base push failed: {setup_build}"
Issue-Key: codebuild__codepipeline__ecr.codebuild-pushes-docker-image-to-ecr-repository|run#9|AssertionError|assert build["buildStatus"] == "SUCCEEDED", f"build did not succeed: {build}"
Issue-Key: codebuild__codepipeline__ecr.codebuild-role-missing-ecr-push-permission-blocks-image-upload|run#9|AssertionError|assert blocked_build["buildStatus"] == "FAILED", f"expected FAILED, got {blocked_build}"
Issue-Key: codebuild__codepipeline__ecr.ecr-image-tag-matches-codebuild-generated-tag|run#8|AssertionError|assert build["buildStatus"] == "SUCCEEDED", f"build did not succeed: {build}"
Issue-Key: codebuild__codepipeline__ecr.ecr-repository-policy-denies-codebuild-role-blocks-pull|run#8|AssertionError|assert setup_build["buildStatus"] == "SUCCEEDED", f"seed push failed: {setup_build}"
Issue-Key: codebuild__codepipeline__ecr|UploadLayerPart|NotImplemented
Issue-Key: codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration|_deploy_cfn#4|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: codebuild__codepipeline__ecr.deploy-stack-outputs-correct-codebuild-project-arn-in-pipeline-stage|_deploy_cfn#4|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: iam|GetRole|NoSuchEntity
Issue-Key: codebuild__codepipeline__ecr.codebuild-role-missing-trust-policy-blocks-assume-role-during-build-rejected|_client_error#6|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Related: 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeBuild/CodePipeline/ECR CI chain: pipeline executions never reach a terminal state, builds FAULT at PROVISIONING, ECR UploadLayerPart is a stub and CFN leaves roles/projects/Outputs unmaterialized

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
Seven distinct root causes found in the `codebuild__codepipeline__ecr` cross-service suite (all 29 checks of the suite failed; none is marked ALREADY FILED or OWNED).

Cause 1 of 7: CodePipeline executions never reach a terminal status, so every pipeline-level check exhausts its budget.
AWS: StartPipelineExecution returns a pipelineExecutionId; the execution then progresses through its stages and reaches a terminal status (Succeeded or Failed) that GetPipelineExecution/ListActionExecutions report, with the Build action's executionResult carrying externalExecutionId (the CodeBuild build id) and errorDetails on failure. A pipeline whose Build action references a deleted project, or whose role lacks codebuild:StartBuild, ends Failed rather than never ending.
Emulator: the bounded poll for a terminal execution status never observes one and each check dies at 'Check timed out after 60.0s' (12 checks). Nothing in the replay shows the execution advancing past its initial status.
Why: a client that starts a pipeline and waits for it, which is the only way to use CodePipeline, hangs forever; both the success and the failure branches are unreachable.
Fix: the pipeline-execution engine behind StartPipelineExecution in the CodePipeline service crate (stage/action transition driver and the execution status it exposes to GetPipelineExecution / ListActionExecutions / GetPipelineState), including its invocation of CodeBuild for a CodeBuild action.
Traces: codebuild__codepipeline__ecr.codebuild-build-status-propagates-to-pipeline-action-result, codebuild__codepipeline__ecr.codebuild-environment-variables-injected-from-pipeline-action, codebuild__codepipeline__ecr.codebuild-project-arn-resolves-correctly-in-pipeline-action-config, codebuild__codepipeline__ecr.codebuild-project-deleted-fails-pipeline-build-action-rejected, codebuild__codepipeline__ecr.codepipeline-execution-includes-ecr-image-digest-as-source-revision, codebuild__codepipeline__ecr.ecr-image-push-triggers-codepipeline-eventbridge-execution, codebuild__codepipeline__ecr.ecr-image-tag-filter-in-eventbridge-rule-controls-pipeline-trigger, codebuild__codepipeline__ecr.ecr-push-to-untracked-repository-does-not-trigger-pipeline, codebuild__codepipeline__ecr.missing-eventbridge-target-permission-blocks-pipeline-invocation, codebuild__codepipeline__ecr.pipeline-invokes-codebuild-project-on-build-stage, codebuild__codepipeline__ecr.pipeline-passes-source-artifact-location-to-codebuild, codebuild__codepipeline__ecr.pipeline-role-missing-codebuild-startbuild-permission-blocks-invocation-rejected.

Cause 2 of 7: CodeBuild builds never execute — every build terminates FAULT at PROVISIONING.
AWS: a build walks SUBMITTED/QUEUED/PROVISIONING/DOWNLOAD_SOURCE/INSTALL/PRE_BUILD/BUILD/POST_BUILD/UPLOAD_ARTIFACTS/FINALIZING/COMPLETED (Smithy BuildPhaseType) and ends SUCCEEDED on exit 0 or FAILED on a non-zero command (Smithy StatusType); the docker build/login/push commands in the buildspec actually run.
Emulator: buildStatus FAULT with phases [SUBMITTED SUCCEEDED, QUEUED SUCCEEDED, PROVISIONING FAULT, COMPLETED] and context 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'.
Why: the whole image-build half of the chain is unobservable — a caller cannot tell a successful push from a failed one, and FAULT (an infrastructure fault) is reported where AWS reports SUCCEEDED or FAILED.
Fix: the CodeBuild build state machine behind StartBuild (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs and the phase state batch_get_builds.rs reads back). Same underlying defect as the codebuild-service issue filed in this sweep.
Traces: codebuild__codepipeline__ecr.codebuild-authenticates-to-ecr-via-get-authorization-token, codebuild__codepipeline__ecr.codebuild-missing-ecr-login-fails-docker-push, codebuild__codepipeline__ecr.codebuild-pulls-base-image-from-ecr-private-registry, codebuild__codepipeline__ecr.codebuild-pushes-docker-image-to-ecr-repository, codebuild__codepipeline__ecr.codebuild-role-missing-ecr-push-permission-blocks-image-upload, codebuild__codepipeline__ecr.ecr-image-tag-matches-codebuild-generated-tag, codebuild__codepipeline__ecr.ecr-repository-policy-denies-codebuild-role-blocks-pull.

Cause 3 of 7: ECR UploadLayerPart is an unimplemented scaffold stub, so no image can be pushed.
AWS: UploadLayerPart accepts the layer part bytes for an initiated upload and returns the uploadId/lastByteReceived; it is a required step of the docker push flow (InitiateLayerUpload -> UploadLayerPart -> CompleteLayerUpload -> PutImage).
Emulator: answers error code 'NotImplemented', message 'operation UploadLayerPart is scaffolded but not implemented in this emulator'. An open issue covers this operation for another suite (20261022T154613Z-ecr-eks-logs-ecr-uploadlayerpart-is-a-501-stub-and-a.md); it does not carry these four check ids.
Why: every CLI-driven push in this chain aborts before an image exists, so nothing downstream of the registry can be exercised.
Fix: implement the UploadLayerPart handler in the ECR service crate (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/upload_layer_part.rs) and the layer-upload state it belongs to.
Traces: codebuild__codepipeline__ecr.cli-create-pipeline-with-codebuild-and-ecr-source-full-flow, codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions, codebuild__codepipeline__ecr.cli-start-pipeline-execution-and-verify-codebuild-log-output, codebuild__codepipeline__ecr.cli-verify-ecr-image-pushed-after-pipeline-execution-succeeds.

Cause 4 of 7: CloudFormation stack Outputs carry no OutputValue.
AWS: each DescribeStacks Outputs entry has OutputKey and OutputValue, the latter holding the resolved physical id/ARN (e.g. the CodeBuild project ARN referenced by a pipeline stage).
Emulator: the Outputs entries lack an OutputValue member entirely (KeyError 'OutputValue'). Related open issues exist for other services' stacks (20261022T154846Z-ec2-emr-cloudformation-stack-outputs-return-synthesized-logical-names-instead.md); they do not carry these check ids.
Why: a stack's consumers read Outputs to find what it created; with no value they cannot address any provisioned resource.
Fix: Outputs resolution in the CloudFormation stack engine (the Fn::GetAtt/Ref resolution that fills OutputValue at stack completion).
Traces: codebuild__codepipeline__ecr.deploy-cfn-stack-pipeline-codebuild-ecr-full-integration, codebuild__codepipeline__ecr.deploy-stack-outputs-correct-codebuild-project-arn-in-pipeline-stage.

Cause 5 of 7: CFN-declared AWS::IAM::Role resources are not created in IAM.
AWS: once the stack completes, IAM GetRole on the generated role name returns the role with its AssumeRolePolicyDocument (trust policy for codebuild.amazonaws.com / codepipeline.amazonaws.com).
Emulator: GetRole answers NoSuchEntity (HTTP 400), 'The role with name <stack>-CodeBuildRole-<suffix> cannot be found.' / '...-PipelineRole-<suffix> cannot be found.'
Why: the stack claims success while the identities it declared do not exist, so nothing can assume them and trust-policy assertions are impossible.
Fix: the CloudFormation resource provider for AWS::IAM::Role must create the role in the IAM service state.
Traces: codebuild__codepipeline__ecr.deploy-stack-codebuild-role-trust-policy-allows-codebuild-service, codebuild__codepipeline__ecr.deploy-stack-pipeline-role-trust-policy-allows-codepipeline-service.

Cause 6 of 7: CFN-declared AWS::CodeBuild::Project resources are not created in CodeBuild.
AWS: after the stack completes, StartBuild on the stack's project starts a build.
Emulator: StartBuild answers ResourceNotFoundException (HTTP 400) 'Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/<stack>-CodeBuildProject-<suffix>'.
Why: same as cause 5 — provisioning reports success and the resource is absent.
Fix: the CloudFormation resource provider for AWS::CodeBuild::Project must create the project in the CodeBuild service state.
Traces: codebuild__codepipeline__ecr.deploy-stack-missing-ecr-repository-policy-blocks-codebuild-push.

Cause 7 of 7: a role whose trust policy does not allow codebuild.amazonaws.com is accepted.
AWS: using a service role that CodeBuild cannot assume is rejected (InvalidInputException, HTTP 400) rather than accepted.
Emulator: 'expected the call to be rejected, but it succeeded'.
Why: the most common real CI misconfiguration silently succeeds, so the emulator can never reproduce an assume-role failure.
Fix: assume-role/trust-policy validation in the CodeBuild CreateProject and StartBuild handlers, cross-checking the IAM role's AssumeRolePolicyDocument.
Traces: codebuild__codepipeline__ecr.codebuild-role-missing-trust-policy-blocks-assume-role-during-build-rejected.

## Evidence
Cause 1: 'Error: Check timed out after 60.0s' on 12 checks, each inside a bounded poll for a terminal pipeline execution status (e.g. _wait_for_pipeline_execution then 'expected pipeline execution to fail, got <status>').
Cause 2: buildStatus 'FAULT', phases [('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)], context 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'.
Cause 3: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UploadLayerPart operation: operation UploadLayerPart is scaffolded but not implemented in this emulator.
Cause 4: KeyError: 'OutputValue'.
Cause 5: botocore.errorfactory.NoSuchEntityException: An error occurred (NoSuchEntity) when calling the GetRole operation: The role with name sim2real-xsvc-stk-...-CodeBuildRole-... cannot be found.
Cause 6: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the StartBuild operation: Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/sim2real-xsvc-stk-...-CodeBuildProject-...
Cause 7: AssertionError: expected the call to be rejected, but it succeeded.

## Steps to Reproduce
Boot the emulator from this checkout and replay the codebuild__codepipeline__ecr suite. Individually: (1) create an S3 artifact bucket, a CodeBuild project and a 2-stage pipeline, call StartPipelineExecution and poll GetPipelineExecution for a terminal status; (2) StartBuild on that project and poll BatchGetBuilds to terminal; (3) InitiateLayerUpload then UploadLayerPart against an ECR repository; (4) deploy a stack with Outputs referencing the project ARN and read DescribeStacks Outputs; (5) deploy a stack declaring AWS::IAM::Role and call IAM GetRole on the created name; (6) deploy a stack declaring AWS::CodeBuild::Project and call StartBuild on it; (7) CreateProject/StartBuild with a role whose trust policy omits codebuild.amazonaws.com.

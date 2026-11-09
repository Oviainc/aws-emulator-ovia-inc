Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions codebuild__codepipeline__ecr.codebuild-missing-ecr-login-fails-docker-push codebuild__codepipeline__ecr.codebuild-role-missing-ecr-push-permission-blocks-image-upload codebuild__codepipeline__ecr.pipeline-role-missing-codebuild-startbuild-permission-blocks-invocation-rejected
Issue-Key: codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions|run#14|KeyError|log_info = bad_build["logs"]
Issue-Key: codebuild__codepipeline__ecr.codebuild-missing-ecr-login-fails-docker-push|run#13|AssertionError|assert login_build["buildStatus"] == "SUCCEEDED", f"control build failed: {login_build}"
Issue-Key: codebuild__codepipeline__ecr.codebuild-role-missing-ecr-push-permission-blocks-image-upload|run#16|AssertionError|assert restored_build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__codepipeline__ecr.pipeline-role-missing-codebuild-startbuild-permission-blocks-invocation-rejected|run#16|AssertionError|assert status == "Failed", f"expected Failed without codebuild:StartBuild, got {status}"
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# codebuild__codepipeline__ecr: 3 net-new causes — BatchGetBuilds omits Build.logs, a directly started build always fails at PRE_BUILD, and a pipeline role without codebuild:StartBuild still succeeds

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetBuilds

## Description
Three root causes in the CodeBuild/CodePipeline surface. All three are in the codebuild__codepipeline__ecr service and must each be closed.

Cause 1 of 3: BatchGetBuilds/StartBuild responses carry no `logs` member.
AWS: a Build carries `logs` (a LogsLocation) describing where the build's output went; CodeBuild creates the CloudWatch Logs group/stream during PROVISIONING, so a build read back after it has started has `logs` with groupName/streamName (and cloudWatchLogsArn/deepLink). Smithy (CodeBuild): Build$logs targets LogsLocation { groupName, streamName, deepLink, s3DeepLink, cloudWatchLogsArn, s3LogsArn, cloudWatchLogs, s3Logs }.
Emulator: the build dict returned for a build has no 'logs' key at all — a client indexing build['logs'] gets a KeyError, so there is no way to find the build's log group/stream.
Why: the documented way to fetch a build's output (read logs.groupName/streamName and then GetLogEvents) is unavailable; any diagnosis that explains WHY a build failed is impossible.
Fix: the CodeBuild Build serializer/state (populate LogsLocation when the build is provisioned).
Traces: codebuild__codepipeline__ecr.cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions.

Cause 2 of 3: a build started directly with StartBuild always ends FAILED at phase PRE_BUILD, even in the control case whose buildspec must succeed.
AWS: a build whose role carries the needed ECR permissions runs its buildspec phases and ends buildStatus SUCCEEDED; PRE_BUILD only fails when the commands in it fail. The paired checks use a build that must succeed as the control for a permission-denial scenario.
Emulator: every such build ends currentPhase COMPLETED with buildStatus FAILED, with the phases list showing SUBMITTED/QUEUED/PROVISIONING/DOWNLOAD_SOURCE SUCCEEDED and PRE_BUILD FAILED — including after the role is granted ECR push permissions, so the positive leg can never pass.
Why: no CodeBuild build can be used for anything: the control of every permission scenario fails, and a client cannot distinguish 'my buildspec is wrong' from 'builds never run'.
Fix: the CodeBuild build executor's PRE_BUILD phase (buildspec command execution, including the ECR login/docker commands) and the phase-status recorder.
Traces: codebuild__codepipeline__ecr.codebuild-missing-ecr-login-fails-docker-push, codebuild__codepipeline__ecr.codebuild-role-missing-ecr-push-permission-blocks-image-upload.

Cause 3 of 3: a pipeline whose role lacks codebuild:StartBuild still succeeds.
AWS: when the pipeline's service role has no codebuild:StartBuild permission, the Build action cannot be invoked and the pipeline execution ends Failed.
Emulator: StartPipelineExecution ends with status Succeeded — the authorization of the pipeline role against codebuild:StartBuild is never evaluated.
Why: a client gets a green pipeline for a deployment that AWS would refuse, so a real IAM misconfiguration is invisible until production. (The open issue 20261108T090606Z-codebuild-codepipeline-ecr-pipeline-driven-builds-never-execute.md names a similar StartBuild-grant symptom but does not carry this check's replay key; this failure is net-new here.)
Fix: CodePipeline's action invocation path (evaluate the pipeline role's identity policy for codebuild:StartBuild and fail the action/execution when denied).
Traces: codebuild__codepipeline__ecr.pipeline-role-missing-codebuild-startbuild-permission-blocks-invocation-rejected.

## Evidence
1) cli-pipeline-execution-fails-when-codebuild-role-lacks-ecr-permissions — KeyError: 'logs' at `log_info = bad_build["logs"]` (the Build returned by BatchGetBuilds has no logs member)
2) codebuild-missing-ecr-login-fails-docker-push — AssertionError: control build failed: {... 'currentPhase': 'COMPLETED', 'buildStatus': 'FAILED', 'phases': [... {'phaseType': 'PRE_BUILD', 'phaseStatus': 'FAILED', ...}]}
3) codebuild-role-missing-ecr-push-permission-blocks-image-upload — AssertionError: the identical build must succeed once the role gains ECR push permissions, got {... 'buildStatus': 'FAILED' ... PRE_BUILD FAILED ...}
4) pipeline-role-missing-codebuild-startbuild-permission-blocks-invocation-rejected — AssertionError: expected Failed without codebuild:StartBuild, got Succeeded

Expected: Build.logs present with groupName/streamName; buildStatus SUCCEEDED for the control/after-grant builds; pipeline execution status Failed when the pipeline role lacks codebuild:StartBuild.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay the checks in trace_ids (service codebuild__codepipeline__ecr).
3. Or: (1) StartBuild then BatchGetBuilds and read builds[0]['logs']; (2) StartBuild a project whose role has ECR push permissions and poll BatchGetBuilds for buildStatus; (3) create a pipeline whose role policy omits codebuild:StartBuild, StartPipelineExecution and read the execution status.

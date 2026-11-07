Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.lambda-deployment-all-at-once-shifts-alias codedeploy.deployment-rollback-restores-alias-and-records-both-deployments codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved codedeploy.s3-revision-etag-mismatch-rejected-and-alias-unmoved codedeploy.s3-revision-object-contents-decide-alias-target
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.lambda-deployment-all-at-once-shifts-alias|run#19|AssertionError|assert after["FunctionVersion"] == v2, (
Issue-Key: codedeploy.deployment-rollback-restores-alias-and-records-both-deployments|run#11|AssertionError|assert moved["FunctionVersion"] == v2, (
Issue-Key: codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created|run#18|AssertionError|assert at_v2["FunctionVersion"] == v2, (
Issue-Key: codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved|run#15|AssertionError|assert at_v2["FunctionVersion"] == v2, (
Issue-Key: codedeploy.s3-revision-etag-mismatch-rejected-and-alias-unmoved|run#14|AssertionError|assert at_v2["FunctionVersion"] == v2, (
Issue-Key: codedeploy.s3-revision-object-contents-decide-alias-target|run#14|AssertionError|assert alias_1["FunctionVersion"] == v2, (
Issue-Commit: 4a3d8b19a967ebf1ce116fc7179d038536b0e95c

# CodeDeploy Lambda deployments reach Succeeded without shifting the alias, so six more checks fail in their positive control (1 cause)

> _Note: this issue was identified against the code base at commit `4a3d8b19a967ebf1ce116fc7179d038536b0e95c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Related open issues: 20261029T064006Z-codedeploy-lambda-deployment-reports-succeeded-while-the-function.md and 20261024T114701Z-codedeploy-deployments-report-success-without-executing-the-lambda.md describe the same family of symptoms, but neither carries the replay keys of the six checks listed here and the suite did not mark these failures as owned by them. A fixer closing either of those issues must re-replay the checks in trace_ids and may choose to merge these items.

Cause 1 of 1: a Lambda-platform CodeDeploy deployment is advanced to the terminal status 'Succeeded' without performing the traffic shift described by the revision — the named function's alias still points at the AppSpec CurrentVersion instead of its TargetVersion. This holds for both revision types the checks use (inline AppSpecContent and an S3Location object), so with an S3 revision the stored object's AppSpec is evidently never read and applied either. Failing checks for this cause: codedeploy.lambda-deployment-all-at-once-shifts-alias, codedeploy.deployment-rollback-restores-alias-and-records-both-deployments, codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created, codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved, codedeploy.s3-revision-etag-mismatch-rejected-and-alias-unmoved, codedeploy.s3-revision-object-contents-decide-alias-target.

AWS: Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json, com.amazonaws.codedeploy#CodeDeploy_20141006): CreateDeployment(applicationName required, deploymentGroupName, revision: RevisionLocation, deploymentConfigName) -> {deploymentId}; GetDeployment(deploymentId required) -> {deploymentInfo: DeploymentInfo} whose status is the DeploymentStatus enum (Created | Queued | InProgress | Baking | Succeeded | Failed | Stopped | Ready). For an application whose computePlatform is Lambda the revision's AppSpec names the function, its alias, a CurrentVersion and a TargetVersion; the AppSpec body itself is not modeled, so this part is recorded behavior only. The contract carried by the terminal value 'Succeeded' is that the shift actually happened: under CodeDeployDefault.LambdaAllAtOnce the whole shift is applied at once, so once GetDeployment reports 'Succeeded' a Lambda GetAlias on that function/alias reports FunctionVersion equal to the AppSpec TargetVersion and carries no residual RoutingConfig.AdditionalVersionWeights. For an S3Location revision the AppSpec that is applied is the content of the S3 object named by bucket/key (and eTag when supplied), so re-deploying the same key after its contents change moves the alias to the version the new object names. Every one of the six checks asserts this as its positive control, before measuring anything negative.

Emulator: the deployment does reach 'Succeeded' (the status assertions pass), but the alias is never mutated; Lambda GetAlias returns the pre-deployment version. Replay output, one line per check: "deployment reported Succeeded but alias live still points at '1', expected TargetVersion 2"; "the forward deployment succeeded but the alias is at '1', expected '2'"; "positive control: the real-alias deployment did not reach '2', the alias is at '1'"; "positive control: the permissioned-role deployment did not reach '2', the alias is at '1'"; "positive control: the correct-eTag deployment did not reach '2', the alias is at '1'"; "the bundled AppSpec named TargetVersion '2' but the alias is at '1'". The '1'/'2' are this run's published version numbers and are not part of the requirement — the requirement is FunctionVersion == the AppSpec TargetVersion after a Succeeded deployment. No HTTP status or error code diverges here: the divergence is a terminal success with the mutation absent.

Why: a client cannot exercise any Lambda deployment flow against this emulator. GetDeployment says the rollout succeeded while the alias every caller and invoker reads still serves the old version, so a pipeline that gates on deployment status proceeds on a rollout that never happened, a smoke test invoking the alias silently tests the previous code, and a reverse (rollback) deployment has nothing to undo. It also makes negative semantics unfalsifiable: 'the alias did not move' is indistinguishable from 'the alias never moves', so the refusal classes these checks exist to pin — Failed/INVALID_LAMBDA_FUNCTION for an AppSpec naming a nonexistent alias, Failed/IAM_ROLE_PERMISSIONS naming AWSLambda for a service role with no Lambda actions, Failed/INVALID_REVISION with the 'The AppSpec file cannot be located in the specified S3 bucket...' message for a mismatched S3 eTag — cannot be verified at all, because each check aborts in its positive control before reaching them.

Fix: the deployment state machine of the codedeploy service — the CreateDeployment handler and the code that drives a deployment to a terminal status in services/service-com-amazonaws-codedeploy-codedeploy-20141006. Before marking a Lambda deployment 'Succeeded' it must resolve the revision (inline appSpecContent, or fetch the S3 object named by the S3Location and use its contents), resolve the named function/alias, and apply the shift by updating the Lambda alias to the AppSpec TargetVersion and clearing any weighted routing it set — instead of advancing status only.

Traces: the six failing checks listed above; each fails on its first alias assertion after a 'Succeeded' deployment.

## Evidence
Suite: simparity-behavioral-aws-v1. Total 93/135 passed. Per-service: codedeploy 93/131 (71%), 38 failed (32 of them already owned by open issues).

[mismatch] codedeploy.lambda-deployment-all-at-once-shifts-alias
  check_codedeploy_lambda_deployment_all_at_once_shifts_alias.py line 72: assert after["FunctionVersion"] == v2
  AssertionError: deployment reported Succeeded but alias live still points at '1', expected TargetVersion 2

[mismatch] codedeploy.deployment-rollback-restores-alias-and-records-both-deployments
  check_codedeploy_deployment_rollback_restores_alias_and_records_both_deployments.py line 48: assert moved["FunctionVersion"] == v2
  AssertionError: the forward deployment succeeded but the alias is at '1', expected '2'

[mismatch] codedeploy.appspec-naming-nonexistent-alias-rejected-and-alias-not-created
  check_codedeploy_appspec_naming_nonexistent_alias_rejected_and_alias_not_created.py line 62: assert at_v2["FunctionVersion"] == v2
  AssertionError: positive control: the real-alias deployment did not reach '2', the alias is at '1'

[mismatch] codedeploy.deployment-service-role-without-lambda-permission-denied-and-alias-unmoved
  check_codedeploy_deployment_service_role_without_lambda_permission_denied_and_alias_unmoved.py line 72: assert at_v2["FunctionVersion"] == v2
  AssertionError: positive control: the permissioned-role deployment did not reach '2', the alias is at '1'

[mismatch] codedeploy.s3-revision-etag-mismatch-rejected-and-alias-unmoved
  check_codedeploy_s3_revision_etag_mismatch_rejected_and_alias_unmoved.py line 62: assert at_v2["FunctionVersion"] == v2
  AssertionError: positive control: the correct-eTag deployment did not reach '2', the alias is at '1'

[mismatch] codedeploy.s3-revision-object-contents-decide-alias-target
  check_codedeploy_s3_revision_object_contents_decide_alias_target.py line 54: assert alias_1["FunctionVersion"] == v2
  AssertionError: the bundled AppSpec named TargetVersion '2' but the alias is at '1'

Expected (AWS): after CreateDeployment on a Lambda application with deploymentConfigName 'CodeDeployDefault.LambdaAllAtOnce' and a revision whose AppSpec names function F, alias A, CurrentVersion V1, TargetVersion V2, GetDeployment eventually reports deploymentInfo.status 'Succeeded' AND lambda GetAlias(FunctionName=F, Name=A).FunctionVersion == V2 with no RoutingConfig.AdditionalVersionWeights left. With an S3Location revision the AppSpec applied is the content of the named object, so two deployments of the same key whose contents changed move the alias to different versions.

Observed (emulator): deploymentInfo.status becomes 'Succeeded' (status assertions pass) while GetAlias(FunctionName=F, Name=A).FunctionVersion is unchanged at V1, for both inline AppSpecContent and S3Location revisions. Because of this the negative legs of four of the six checks (Failed/INVALID_LAMBDA_FUNCTION, Failed/IAM_ROLE_PERMISSIONS naming AWSLambda, Failed/INVALID_REVISION with the S3 'AppSpec file cannot be located' message, and the reverse-deployment/rollbackInfo records) are never reached and remain unverified.

## Steps to Reproduce
1. Create a Lambda function, publish versions V1/V2/V3, and create alias A pointing at V1.
2. codedeploy CreateApplication(computePlatform='Lambda'); CreateDeploymentGroup on it with a service role and deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce'.
3. Inline-revision leg: CreateDeployment(applicationName=<app>, deploymentGroupName=<group>, deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce', revision={revisionType:'AppSpecContent', appSpecContent:{content: AppSpec naming function F, alias A, CurrentVersion V1, TargetVersion V2}}).
4. Poll GetDeployment(deploymentId=...) until deploymentInfo.status is terminal. Emulator: 'Succeeded'.
5. lambda GetAlias(FunctionName=F, Name=A). AWS: FunctionVersion == V2, no RoutingConfig.AdditionalVersionWeights. Emulator: FunctionVersion is still V1.
6. S3-revision leg: PutObject an appspec.yaml naming F/A with CurrentVersion V1, TargetVersion V2 into a bucket, then CreateDeployment with revision={revisionType:'S3',s3Location:{bucket,key:'appspec.yaml',bundleType:'YAML',eTag:<object eTag>}}. Poll to terminal ('Succeeded') and GetAlias again. AWS: FunctionVersion == V2. Emulator: still V1, so the object's contents were never applied.
7. Reverse leg: CreateDeployment with an AppSpec naming CurrentVersion V2, TargetVersion V1 and observe that no alias move ever occurs in either direction while both deployments report 'Succeeded'.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment|run#18|AssertionError|assert at_v2["FunctionVersion"] == strict_vs[1], (
Issue-Commit: 004a13bc6b62a404600c0a09686b1d7dd87c7440

# CodeDeploy Lambda deployment reports Succeeded while the function alias is never moved to the AppSpec TargetVersion

> _Note: this issue was identified against the code base at commit `004a13bc6b62a404600c0a09686b1d7dd87c7440`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Related open issue: 20261024T114701Z-codedeploy-deployments-report-success-without-executing-the-lambda.md covers an overlapping family of symptoms, but it does not carry this check's replay key and the suite did not mark this failure as owned by it; a fixer closing that issue should re-replay this check too, and may choose to merge the two.

Cause 1 of 1: a Lambda CodeDeploy deployment is advanced to the terminal status 'Succeeded' without executing the revision at all — the deployed function's alias still points at the AppSpec's CurrentVersion instead of its TargetVersion.

AWS: Smithy (com.amazonaws.codedeploy#CodeDeploy_20141006): CreateDeployment takes `revision` (RevisionLocation); for an application whose computePlatform is Lambda the revision's appSpecContent describes the target function with its Name, Alias, CurrentVersion and TargetVersion, and GetDeployment returns `deploymentInfo` (DeploymentInfo) whose `status` is a DeploymentStatus enum (Created | Queued | InProgress | Baking | Succeeded | Failed | Stopped | Ready). The contract carried by the terminal value 'Succeeded' is that the traffic shift described by the revision actually happened: for CodeDeployDefault.LambdaAllAtOnce the whole shift is done at once, so after the deployment reaches 'Succeeded' Lambda GetAlias on the named function/alias reports FunctionVersion == the AppSpec's TargetVersion and carries no residual AdditionalVersionWeights. Recorded behavior (the check's own measurement header, us-west-2): with the service role granted cloudwatch:DescribeAlarms, the AllAtOnce deployment ends 'Succeeded' and the alias reads the target version; this is the check's positive control, asserted before anything negative is measured.

Emulator: the deployment does reach 'Succeeded' (that assertion passes), but the alias was never mutated. Lambda GetAlias on the deployed function reports the pre-deployment version: `AssertionError: positive control (strict): the alias is at '1' after a Succeeded deployment, expected '2'`. The '1'/'2' are this run's published version numbers and are not part of the requirement — the requirement is that after a Succeeded Lambda deployment the alias's FunctionVersion equals the AppSpec TargetVersion. So the emulator reports a successful deployment whose only observable effect, the alias move, did not occur; the whole deployment is a bookkeeping transition with no execution behind it.

Why: a client cannot use this emulator to exercise any Lambda deployment flow. GetDeployment says the rollout succeeded while the alias — the thing every caller and every downstream invoker reads — still serves the old version, so a pipeline that gates on deployment status proceeds on a rollout that never happened, a smoke test invoking the alias silently tests the previous code, and a rollback has nothing to undo. Worse for test suites, every negative assertion downstream of such a deployment is unfalsifiable: 'the alias did not move' is indistinguishable from 'the alias never moves', so refusal/permission/alarm semantics layered on top of deployments cannot be verified at all (this check never reaches its own negative legs for exactly that reason).

Fix: the deployment state machine of the codedeploy service (services/service-com-amazonaws-codedeploy-codedeploy-20141006 — the CreateDeployment handler and the deployment transition code in src/state.rs that drives a deployment to a terminal status). Before marking a Lambda deployment 'Succeeded' it must parse the revision's appSpecContent, resolve the named function/alias, and apply the traffic shift by updating the Lambda alias to the AppSpec TargetVersion (and clear any weighted routing it set), rather than advancing status only.

Traces: codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment (the failure is in that check's positive control, before any alarm/IAM behavior is measured).

## Evidence
Suite: simparity-behavioral-ga-prod-v1. Per-service: codedeploy 64/82 (78%), 18 failed.

[mismatch] codedeploy.ignore-poll-alarm-failure-decides-whether-an-unpollable-alarm-stops-the-deployment
  File "check_codedeploy_ignore_poll_alarm_failure_decides_whether_an_unpollable_alarm_stops_the_deployment.py", line 114, in run
    assert at_v2["FunctionVersion"] == strict_vs[1], (
  AssertionError: positive control (strict): the alias is at '1' after a Succeeded deployment, expected '2'

Expected (AWS): CreateDeployment(applicationName=<Lambda app>, deploymentGroupName=<group>, deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce', revision=<AppSpecContent naming function F, alias A, CurrentVersion=V1, TargetVersion=V2>) -> GetDeployment reports deploymentInfo.status 'Succeeded' AND lambda GetAlias(FunctionName=F, Name=A).FunctionVersion == V2, with no RoutingConfig.AdditionalVersionWeights left on the alias.

Observed (emulator): GetDeployment reports deploymentInfo.status 'Succeeded' (the preceding assertion on status passed, so the status side is right), while GetAlias(FunctionName=F, Name=A).FunctionVersion is still V1 — the alias was never updated. The deployment therefore produced no effect at its destination.

No status code or error code diverges here: the divergence is a terminal success with the mutation absent.

## Steps to Reproduce
1. Create a Lambda function and publish three versions V1, V2, V3; create an alias A pointing at V1.
2. CreateApplication(computePlatform='Lambda').
3. CreateDeploymentGroup on that application with a service role, deploymentConfigName of a Lambda config, and an alarmConfiguration enabled on an existing CloudWatch alarm that reads OK.
4. CreateDeployment(applicationName=<app>, deploymentGroupName=<group>, deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce', revision={revisionType:'AppSpecContent', appSpecContent:{content: AppSpec naming function F, alias A, CurrentVersion V1, TargetVersion V2}}).
5. Poll GetDeployment(deploymentId=...) until deploymentInfo.status is terminal. Emulator: 'Succeeded'.
6. Call lambda GetAlias(FunctionName=F, Name=A). AWS: FunctionVersion == V2. Emulator: FunctionVersion is still V1, and no traffic-shift state was ever written.

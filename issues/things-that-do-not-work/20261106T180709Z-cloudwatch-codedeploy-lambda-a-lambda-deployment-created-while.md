Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm
Issue-Key: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm|_poll#16|Timeout|time.sleep(interval_s)
Issue-Commit: f50d8ce9e37ffa71976cdbe00198995770403bcc

# cloudwatch__codedeploy__lambda: a Lambda deployment created while its alarm is already in ALARM never reaches Stopped/ALARM_ACTIVE and no codeDeployRollback deployment appears

> _Note: this issue was identified against the code base at commit `f50d8ce9e37ffa71976cdbe00198995770403bcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeployment

## Description
One net-new root cause.

Cause 1 of 1: CreateDeployment against a deployment group whose alarm configuration names a CloudWatch alarm that is ALREADY in ALARM never settles into a terminal Stopped deployment, and no rollback deployment is recorded, so the check polls to the harness's 60 s cap and is killed. Failing check id: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm.

AWS: an alarm already in ALARM state when a Lambda (compute platform Lambda) deployment is created stops that deployment before an AllAtOnce configuration moves the alias. GetDeployment then reports deploymentInfo.status = "Stopped" (Smithy: codedeploy DeploymentStatus enum = Created|Queued|InProgress|Baking|Succeeded|Failed|Stopped|Ready) with deploymentInfo.errorInformation.code = "ALARM_ACTIVE" (Smithy: codedeploy ErrorCode enum includes ALARM_ACTIVE), the function alias is NOT moved to the AppSpec TargetVersion, and CodeDeploy additionally creates a rollback deployment whose deploymentInfo.creator is "codeDeployRollback" (Smithy: codedeploy DeploymentCreator enum includes codeDeployRollback, documented as "a rollback process created the deployment"). The control arm — the same deployment with the alarm in OK — reaches status "Succeeded" and the alias does move.

Emulator: the check never completes. The replay reports only `Error: Check timed out after 60.0s`; the check polls GetDeployment for a terminal status in (Succeeded, Failed, Stopped) and is still polling when the harness budget expires, so no terminal status, no errorInformation.code and no second deployment attributed to codeDeployRollback is ever observed.

Why: a client that gates a release on the alarm — create the deployment, read the terminal status, branch on Stopped/ALARM_ACTIVE and on the rollback deployment — can neither observe the stop nor the rollback. A deployment that real AWS refuses to perform is left indefinitely non-terminal here, so a safety alarm that should block a bad version is invisible to the caller and the alias state is unverifiable.

Fix: CodeDeploy's deployment lifecycle for the Lambda compute platform — the CreateDeployment handler's alarm-configuration evaluation, the deployment state machine that must drive the deployment to a terminal Stopped with errorInformation.code = ALARM_ACTIVE, and the rollback-deployment creation that records creator = codeDeployRollback; GetDeployment's serializer must surface status, errorInformation and creator. The open issue 20261025T013642Z-codedeploy-time-based-traffic-shifting-and-alarm-driven.md describes alarm-driven rollback being stored but never executed for the codedeploy service, but it does not carry this check's replay key.

Traces: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm (verdict mismatch; detail `Error: Check timed out after 60.0s`).

## Evidence
Expected: GetDeployment on the deployment created while the alarm is in ALARM reaches a terminal deploymentInfo.status == "Stopped" with deploymentInfo.errorInformation.code == "ALARM_ACTIVE", the alias still points at the pre-deployment version, and ListDeployments shows a second deployment with deploymentInfo.creator == "codeDeployRollback"; the OK-alarm control deployment reaches "Succeeded" and moves the alias.
Actual: the deployment never reaches any terminal status (Succeeded|Failed|Stopped); the check is terminated by the harness with `Error: Check timed out after 60.0s`, so neither the ALARM_ACTIVE stop nor the codeDeployRollback deployment is ever produced.

## Steps to Reproduce
1. Create a Lambda function and publish two versions plus an alias.
2. Create a CloudWatch alarm and force it into ALARM (SetAlarmState).
3. Create a CodeDeploy application and deployment group with computePlatform Lambda, an AllAtOnce deployment configuration and an alarm configuration naming that alarm.
4. Call CreateDeployment with an AppSpec moving the alias to the second version, then poll GetDeployment for a terminal status.
5. Observe that no terminal status is ever reported, no errorInformation.code ALARM_ACTIVE appears, and no rollback deployment with creator codeDeployRollback is created.

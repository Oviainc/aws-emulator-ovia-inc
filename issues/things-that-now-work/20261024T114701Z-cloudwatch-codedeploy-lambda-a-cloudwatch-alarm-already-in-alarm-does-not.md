Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm
Issue-Key: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm|_poll#16|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# A CloudWatch alarm already in ALARM does not stop a Lambda CodeDeploy deployment: no Stopped/ALARM_ACTIVE outcome and no rollback deployment record

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a deployment group's alarmConfiguration is never evaluated, so an alarm that is already in ALARM state when CreateDeployment is called does not stop the deployment and no rollback deployment is recorded.
AWS: Smithy CreateDeploymentGroup/UpdateDeploymentGroup take alarmConfiguration (AlarmConfiguration {enabled, alarms, ignorePollAlarmFailure}) and autoRollbackConfiguration (events including DEPLOYMENT_STOP_ON_ALARM); DeploymentInfo exposes status (enum DeploymentStatus incl. 'Stopped'), errorInformation (ErrorInformation whose code enum includes ALARM_ACTIVE), rollbackInfo (RollbackInfo {rollbackDeploymentId, rollbackTriggeringDeploymentId, rollbackMessage}) and creator (enum DeploymentCreator incl. 'codeDeployRollback'). Recorded behavior (us-west-2): with the alarm at OK the AllAtOnce Lambda deployment Succeeds and moves the alias — the positive control. With the SAME function, alias, deployment group and config but the alarm already in ALARM when CreateDeployment is called, CodeDeploy accepts the request (the CreateDeployment response is byte-for-byte the same acceptance carrying a deploymentId) and then stops the deployment out of band in ~2.4s: GetDeployment reports status 'Stopped' with errorInformation.code 'ALARM_ACTIVE', the alias is NEVER moved, and CodeDeploy spawns a second deployment record attributed to creator 'codeDeployRollback' even though there was nothing to undo.
Emulator: the check never completes within the suite's 60s per-check cap. No deployment with status 'Stopped' / errorInformation.code 'ALARM_ACTIVE' and no codeDeployRollback-attributed deployment record is produced, so the polls for that outcome cannot be satisfied. Hypothesis: the alarmConfiguration is stored but never polled against the cloudwatch service state and the deployment is reported terminal on its own — the same root as the codedeploy issue "CodeDeploy deployments report success without executing..." (the alias not moving and deployments terminating immediately are directly asserted there). Both legs of this check use CodeDeployDefault.LambdaAllAtOnce, which the check measured at a few seconds per deployment on real AWS plus a ~2.4s stop, i.e. well inside the cap.
Why: the alarm gate is the safety mechanism customers rely on to prevent a bad deployment from reaching traffic. An emulator that ignores it reports success (and moves traffic) in exactly the situation AWS refuses to deploy at all, so a monitored deployment pipeline tested against the emulator has no coverage of its own guardrail, and clients that read rollbackInfo/creator to reconcile 'which deployment undid which' find neither record.
Fix: the deployment execution path of the codedeploy service — evaluate the deployment group's (and CreateDeployment's overrideAlarmConfiguration) alarms against the cloudwatch alarm state before and during the shift, stop the deployment with status 'Stopped' + errorInformation.code 'ALARM_ACTIVE' when any is in ALARM, and when autoRollbackConfiguration includes DEPLOYMENT_STOP_ON_ALARM create the companion deployment with creator 'codeDeployRollback' and populate rollbackInfo on both records.
Traces: cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout (50/91 passed; bootstrap_signal false — the emulator was serving). Service group cloudwatch__codedeploy__lambda: 0/1 (0%), 1 failed.

cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm — "Error: Check timed out after 60.0s".

Expected (measured on real AWS us-west-2 by the check, stated in its own source): leg 1 (alarm forced to OK via SetAlarmState) the AllAtOnce deployment ends 'Succeeded' and the alias moves; leg 2 (same wiring, alarm forced to ALARM before CreateDeployment) CreateDeployment is ACCEPTED, then GetDeployment reports status 'Stopped' with errorInformation.code 'ALARM_ACTIVE' naming the alarm, GetAlias shows the alias never moved, and a second deployment record exists with creator 'codeDeployRollback'. The alarm is created on a namespace that receives no datapoints with TreatMissingData=notBreaching and is driven purely by SetAlarmState, so nothing here depends on a real metric breach or on live-account data.
Actual (emulator): the check does not reach a verdict inside the 60s cap; none of the Stopped / ALARM_ACTIVE / codeDeployRollback states it polls for is produced.

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): DeploymentStatus includes 'Stopped'; ErrorCode includes 'ALARM_ACTIVE'; DeploymentCreator includes 'codeDeployRollback'; RollbackInfo = {rollbackDeploymentId, rollbackTriggeringDeploymentId, rollbackMessage}; CreateDeploymentGroup takes alarmConfiguration and autoRollbackConfiguration. Function, alias and alarm names and generated deployment ids are environment-specific; the requirements are the statuses, the error code and the existence/attribution of the rollback deployment record.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite); the check reproduces.
2. Create a Lambda execution role and a 128MB function; publish versions 1, 2 and 3 and point an alias at version 1.
3. cloudwatch PutMetricAlarm on a namespace that receives no datapoints with TreatMissingData='notBreaching'.
4. CreateApplication(computePlatform='Lambda'); CreateDeploymentGroup(deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce', alarmConfiguration={enabled:true, alarms:[{name:<alarm>}]}, autoRollbackConfiguration={enabled:true, events:['DEPLOYMENT_STOP_ON_ALARM']}).
5. Positive control: SetAlarmState(StateValue='OK'); CreateDeployment with an AppSpecContent revision 1 -> 2; poll GetDeployment -> expect 'Succeeded' and GetAlias at version 2.
6. SetAlarmState(StateValue='ALARM'); CreateDeployment with revision 2 -> 3 -> expect the call ACCEPTED with a deploymentId, then GetDeployment -> status 'Stopped' with errorInformation.code 'ALARM_ACTIVE', GetAlias still at version 2, and ListDeployments/GetDeployment showing a companion deployment whose creator is 'codeDeployRollback' (with rollbackInfo linking the two).
7. Observe instead: no Stopped/ALARM_ACTIVE deployment and no rollback record ever appear; the check is killed at the 60s cap.

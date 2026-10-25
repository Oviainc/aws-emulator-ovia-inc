Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.lambda-deployment-canary-holds-partial-alias-weight codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.lambda-deployment-canary-holds-partial-alias-weight|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias|_poll#19|Timeout|time.sleep(interval_s)
Issue-Commit: d6e9bd85505b2fcf81ff15dd47c3206271b87270

# CodeDeploy time-based traffic shifting and alarm-driven rollback are stored but never executed (2 causes)

> _Note: this issue was identified against the code base at commit `d6e9bd85505b2fcf81ff15dd47c3206271b87270`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Two net-new root causes in the CodeDeploy deployment clock. Both are the same shape of gap — a configuration the emulator validates, stores and echoes faithfully, but that nothing ever ACTS on — and both are distinct from the open issue 20261024T114701Z-codedeploy-deployments-report-success-without-executing-the-lambda.md (which covers the alias never moving at all and targets being absent) and from 20261024T114701Z-cloudwatch-codedeploy-lambda-a-cloudwatch-alarm-already-in-alarm-does-not.md (which covers an alarm that is ALREADY in ALARM at CreateDeployment time, not one that breaches mid-shift). Neither of the two checks below is reported as owned by those issues.

Cause 1 of 2: a TimeBasedCanary deployment configuration is echoed but never executed, so no intermediate traffic weight ever exists and the deployment terminates immediately.
AWS: Smithy: DeploymentConfig.trafficRoutingConfig / TrafficRoutingType = TimeBasedCanary | TimeBasedLinear | AllAtOnce; TimeBasedCanary.canaryPercentage is documented as "The percentage of traffic to shift in the first increment of a TimeBasedCanary deployment" and TimeBasedCanary.canaryInterval as "The number of minutes between the first and second traffic shifts of a TimeBasedCanary deployment". For a Lambda deployment this means the service performs TWO traffic shifts: it first parks canaryPercentage of traffic on the TargetVersion — observable on the Lambda alias as FunctionVersion == CurrentVersion with RoutingConfig.AdditionalVersionWeights == {TargetVersion: canaryPercentage/100} — holds that for canaryInterval, and only then shifts the remainder and clears the weighted routing. The deployment is therefore NOT terminal while the canary window is open.
Emulator: GetDeploymentConfig round-trips timeBasedCanary correctly, but the deployment clock ignores it entirely. services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/state.rs fixes DEPLOYMENT_START_MILLIS = 250 and DEPLOYMENT_COMPLETION_MILLIS = 500 for every deployment regardless of trafficRoutingConfig, and there is no state at all representing an in-flight traffic increment or a partial alias weight. The check never observes any AdditionalVersionWeights, so its canary-window poll never satisfies its predicate and the check is killed at the suite's budget.
Why: a client that uses canary/linear deployments to validate a new version under a fraction of live traffic gets no canary at all — the window it would use to watch metrics, run validation hooks or abort never exists, and any client that reads the alias to confirm the canary weight (the documented way to see the first increment) sees nothing. Rollback-on-canary logic built against this emulator is never exercised.
Fix: the deployment clock / state machine in the codedeploy service state (deployment status progression and the traffic-shift it owes the alias), which must derive the shift schedule from the deployment's resolved trafficRoutingConfig instead of a single fixed completion delay.
Traces: codedeploy.lambda-deployment-canary-holds-partial-alias-weight (uses a custom TimeBasedCanary config, canaryPercentage=10, canaryInterval=2).

Cause 2 of 2: a CloudWatch alarm that transitions to ALARM while a deployment is in flight does not stop it — no Stopped/ALARM_ACTIVE outcome, no rollback deployment record, no alias restoration.
AWS: Smithy: DeploymentGroup/CreateDeployment carry alarmConfiguration (AlarmConfiguration, up to 10 alarms) and autoRollbackConfiguration with AutoRollbackEvent = DEPLOYMENT_FAILURE | DEPLOYMENT_STOP_ON_ALARM | DEPLOYMENT_STOP_ON_REQUEST; ErrorCode models ALARM_ACTIVE; RollbackInfo models rollbackDeploymentId ("The ID of the deployment rollback") and rollbackTriggeringDeploymentId ("The deployment ID of the deployment that was underway and triggered a rollback deployment because it failed or was stopped"). Recorded behavior (us-west-2, per the check's measurement header): driving the associated alarm to ALARM while the deployment is InProgress moves it to a terminal state; the deployment ends status "Stopped" (not "Failed") with errorInformation.code "ALARM_ACTIVE" naming the activated alarm, CodeDeploy creates a SECOND deployment record with creator "codeDeployRollback", the stopped deployment's rollbackInfo.rollbackDeploymentId names that rollback and the rollback's rollbackTriggeringDeploymentId points back, and the alias is returned to the version the deployment started from. The errorInformation.message for this case is quoted as an example only.
Emulator: alarmConfiguration and autoRollbackConfiguration are validated and echoed by GetDeploymentGroup (state.rs validate_alarm_configuration / DeploymentGroup.alarm_configuration / auto_rollback_configuration) but nothing ever consults the alarm's state: there is no CloudWatch poll, no transition to Stopped, no ALARM_ACTIVE errorInformation, and no code path that mints a second deployment with creator codeDeployRollback or populates RollbackInfo. The deployment reaches Succeeded ~500 ms after creation and stays there, so the check's wait for a terminal-by-breach state and for rollbackInfo never resolves and it is killed at the suite's budget.
Why: a client relying on alarm-gated deployments believes a breached alarm aborts the rollout and restores the previous version. Against this emulator every deployment succeeds, the rollback deployment record a client reconciles against (ListDeployments/BatchGetDeployments with creator codeDeployRollback, and the two deployments' mutual rollbackInfo linkage) is never created, and safe-deployment tooling cannot be tested at all.
Fix: the deployment clock / state machine in the codedeploy service state — it must evaluate the deployment group's enabled alarmConfiguration against CloudWatch alarm state while the deployment is non-terminal, and when autoRollbackConfiguration includes DEPLOYMENT_STOP_ON_ALARM, terminate the deployment as Stopped with errorInformation.code ALARM_ACTIVE and create the attributed rollback deployment with the RollbackInfo linkage in both directions.
Traces: codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias (control leg with the alarm at OK, then SetAlarmState to ALARM mid-canary).

## Evidence
Suite: simparity-behavioral-ga-prod-v1, codedeploy 53/86 (62%).

[mismatch] codedeploy.lambda-deployment-canary-holds-partial-alias-weight
  Error: Check timed out after 60.0s
  The check's first graded observation is a poll of lam.get_alias for RoutingConfig.AdditionalVersionWeights (its own timeout 180s). Expected: {TargetVersion: 0.1} with FunctionVersion still == CurrentVersion, then after canaryInterval FunctionVersion == TargetVersion and the weights cleared. Observed: no AdditionalVersionWeights is ever produced; GetDeploymentConfig does echo timeBasedCanary {canaryPercentage: 10, canaryInterval: 2} correctly, so the configuration is stored and only its execution is missing.

[mismatch] codedeploy.alarm-breach-auto-rolls-back-deployment-and-restores-alias
  Error: Check timed out after 60.0s
  The deployment group echoes alarmConfiguration == [the alarm] and autoRollbackConfiguration.events == ["DEPLOYMENT_STOP_ON_ALARM"] (those assertions precede the hang). Expected after SetAlarmState(ALARM) on an InProgress deployment: status "Stopped", errorInformation.code "ALARM_ACTIVE" (message example: "One or more alarms have been activated according to the Amazon CloudWatch metrics you selected, and the affected deployments have been stopped. Activated alarms: <name>"), rollbackInfo.rollbackDeploymentId populated, a second deployment with creator "codeDeployRollback" whose rollbackTriggeringDeploymentId names the stopped one, and the alias back on its starting version. Observed: the deployment reaches Succeeded on the fixed 500 ms clock and no rollback record or rollbackInfo ever appears, so both polls run to the kill.

Source corroboration (why both timeouts are the emulator's and not a slow response): services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/state.rs declares DEPLOYMENT_START_MILLIS = 250 and DEPLOYMENT_COMPLETION_MILLIS = 500 as the whole of a deployment's progression; trafficRoutingConfig and alarmConfiguration appear only in storage/echo paths (deployment_config_info, deployment_group_info, validate_alarm_configuration) and in no transition.

## Steps to Reproduce
Cause 1 (canary):
1. CreateApplication(computePlatform=Lambda); CreateDeploymentConfig with trafficRoutingConfig {type: TimeBasedCanary, timeBasedCanary: {canaryPercentage: 10, canaryInterval: 2}}; CreateDeploymentGroup using it.
2. Publish two Lambda versions and an alias pointing at v1.
3. CreateDeployment with an AppSpecContent revision whose CurrentVersion=v1 and TargetVersion=v2.
4. Poll lambda GetAlias. AWS: FunctionVersion==v1 with RoutingConfig.AdditionalVersionWeights=={v2: 0.1} for ~2 minutes, then FunctionVersion==v2 with no weights. Emulator: no AdditionalVersionWeights ever, and GetDeployment reports Succeeded ~0.5 s after creation.

Cause 2 (alarm breach mid-deployment):
1. PutMetricAlarm on a namespace with no datapoints, TreatMissingData=notBreaching.
2. CreateDeploymentGroup (Lambda platform, a TimeBasedCanary config) with alarmConfiguration {enabled: true, alarms: [{name: alarm}]} and autoRollbackConfiguration {enabled: true, events: [DEPLOYMENT_STOP_ON_ALARM]}.
3. SetAlarmState(alarm, OK); CreateDeployment; confirm it is InProgress.
4. SetAlarmState(alarm, ALARM) while it is InProgress.
5. Poll GetDeployment. AWS: status Stopped, errorInformation.code ALARM_ACTIVE, rollbackInfo.rollbackDeploymentId set, and a second deployment with creator codeDeployRollback; the alias is restored. Emulator: status Succeeded, no errorInformation, no rollbackInfo, no rollback deployment.

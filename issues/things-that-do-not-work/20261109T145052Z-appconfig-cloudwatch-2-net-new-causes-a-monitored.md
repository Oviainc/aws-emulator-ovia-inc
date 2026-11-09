Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__cloudwatch.cloudwatch-alarm-rollback-restores-previous-version appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected
Issue-Key: appconfig__cloudwatch.cloudwatch-alarm-rollback-restores-previous-version|run#52|AssertionError|assert final["State"] == "ROLLED_BACK", (
Issue-Key: appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected|run#37|AssertionError|assert broken_final["State"] == "ROLLED_BACK", (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# appconfig__cloudwatch: 2 net-new causes — a monitored deployment completes although its CloudWatch alarm is in ALARM, and one whose monitor role cannot be assumed completes instead of rolling back

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
Two net-new root causes grounded in two replayed behavioral checks. BOTH are separate defects and EVERY ONE must be closed before this issue is submitted; re-replay every check id in trace_ids to confirm. Neither check was marked ALREADY FILED and neither was marked OWNED by an open issue in this replay.

Both causes are the same family — the environment's Monitors are never consulted by the deployment state machine — but they are separate mechanisms with separate observable outcomes (alarm-state evaluation vs. role assumability) and separate event-log requirements, so each must be closed on its own.

=== Cause 1 of 2: a deployment whose environment Monitors name an alarm that goes into ALARM during the bake window completes instead of rolling back ===
AWS: Smithy (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json) models Monitor = structure{AlarmArn @required -> StringWithLengthBetween1And2048, AlarmRoleArn -> RoleArn pattern ^((arn):(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov|aws-eusc):(iam)::\d{12}:role[/].*)$} documented "Amazon CloudWatch alarms to monitor during the deployment process", and DeploymentEventType includes ROLLBACK_STARTED / ROLLBACK_COMPLETED with TriggeredBy including CLOUDWATCH_ALARM. The rollback outcome itself is recorded behavior: when a monitored alarm enters ALARM during the deployment's bake window, AppConfig rolls the deployment back — GetDeployment settles on State ROLLED_BACK (not COMPLETE), the EventLog records a ROLLBACK_COMPLETED event, and the environment serves the PREVIOUS configuration version again.
Emulator: the deployment reaches terminal state COMPLETE. The monitor is never consulted, so the alarm's ALARM state has no effect, no rollback is recorded, and the new configuration stays served.
Why: this is the over-permissive direction. Every safe-deployment flow built on AppConfig monitors — bake, auto-rollback, restore-previous-version — silently ships a release the alarm was configured to block. A client testing its rollback path against the emulator sees a green deployment and concludes its guardrail works, when against AWS the same release is reverted.
Fix: the deployment state machine in the appconfig StartDeployment handler and its deployment/environment state — during the bake window, evaluate each environment/deployment Monitor's AlarmArn against CloudWatch's alarm state and, on ALARM, drive the deployment to ROLLED_BACK (appending ROLLBACK_STARTED/ROLLBACK_COMPLETED with TriggeredBy CLOUDWATCH_ALARM) and restore the previous configuration version to the data plane.
Traces: appconfig__cloudwatch.cloudwatch-alarm-rollback-restores-previous-version.

=== Cause 2 of 2: a deployment whose environment monitor role AppConfig cannot assume completes instead of rolling back, and no role-specific ROLLBACK_COMPLETED event is recorded ===
AWS: with Monitor.AlarmRoleArn naming an IAM role whose trust policy does NOT allow appconfig.amazonaws.com to assume it, AppConfig cannot read the alarm and does not ship unmonitored: the deployment terminates ROLLED_BACK. Recorded behavior for the log and the aftermath: the EventLog carries a ROLLBACK_COMPLETED event whose TriggeredBy is NOT CLOUDWATCH_ALARM (the alarm itself stayed OK) and whose Description is 'Unable to assume environment monitor role: <roleArn>' — fixed text 'Unable to assume environment monitor role: ', the single variable part being the AlarmRoleArn taken from the environment's Monitors; DEPLOYMENT_COMPLETED is never logged for that deployment; GetEnvironment settles on the CamelCase State 'RolledBack'; and because no deployment ever succeeded in that environment, appconfigdata.StartConfigurationSession for it is refused with ResourceNotFoundException (HTTP 404) and the message 'Deployment not found' (entirely fixed text).
Emulator: the deployment terminates COMPLETE. The unassumable role is never exercised, so there is no ROLLBACK_COMPLETED event, no role-naming Description, DEPLOYMENT_COMPLETED is logged, and the environment does not settle on 'RolledBack'. (A correctly-trusted sibling environment in the same check did complete, so this is not IAM propagation timing — the check's positive control passed.)
Why: an IAM misconfiguration that AWS treats as a hard stop is silently ignored, and the release ships with no monitoring at all. An operator debugging why a deployment reverted gets no event naming the role that could not be assumed, so the trust-policy break is indistinguishable from an alarm firing or from success.
Fix: the same deployment state machine plus its event-log recording in the appconfig service — make monitor-role assumability a precondition of monitoring, drive the deployment to ROLLED_BACK when the role cannot be assumed, append the role-naming ROLLBACK_COMPLETED event with a non-CLOUDWATCH_ALARM TriggeredBy, suppress DEPLOYMENT_COMPLETED, and leave the environment in 'RolledBack' so the appconfigdata data plane still refuses sessions with 'Deployment not found'.
Traces: appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected.

Related open issue that does NOT carry these checks' replay keys: 20261025T020208Z-appconfig-cloudwatch-appconfig-deployments-with-cloudwatch-alarm-monitors-never-reach.md recorded an EARLIER symptom of the same area — monitored deployments never reached a terminal state at all (both checks timed out). That part has progressed: both deployments now settle, but they settle on COMPLETE, so the monitor is still never consulted. Its replay keys are the old timeout keys and this replay reports both checks as unowned. Close this issue and that one deliberately rather than assuming one covers the other.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the booted emulator: 207/246 passed (84.1%), 30 mismatched, 9 errored, 0 skipped, 0 unloadable; bootstrap_signal false. Per-service: appconfig__cloudwatch 0/2 (0%), 2 failed — neither marked ALREADY FILED nor OWNED. appconfig (crate service-com-amazonaws-appconfig-amazonappconfig) and cloudwatch (crate service-com-amazonaws-cloudwatch-graniteserviceversion20100801) are both bootstrapped.

Cause 1 — appconfig__cloudwatch.cloudwatch-alarm-rollback-restores-previous-version, verdict [mismatch]:
  AssertionError: a monitored deployment whose alarm goes into ALARM during bake must terminate ROLLED_BACK, got 'COMPLETE' — COMPLETE means the monitor was never consulted, which is exactly the failure mode a recorded frame cannot catch
  The check builds an application/environment with Monitors=[{AlarmArn, AlarmRoleArn}] on a role trusted by appconfig.amazonaws.com with cloudwatch:DescribeAlarms, a PutMetricAlarm in namespace 'sim2real/AppConfigRollback', a hosted configuration profile with two versions, and a strategy with DeploymentDurationInMinutes 0 / GrowthFactor 100 / FinalBakeTimeInMinutes 1. It drives the alarm into ALARM during the second deployment's bake and polls GetDeployment for a terminal state (_TERMINAL = COMPLETE/ROLLED_BACK/REVERTED) with a 600s budget.
  Expected: State 'ROLLED_BACK', a ROLLBACK_COMPLETED EventLog entry, and the previous configuration version served again. Observed: State 'COMPLETE'.

Cause 2 — appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected, verdict [mismatch]:
  AssertionError: a deployment whose environment monitor role AppConfig cannot assume must terminate ROLLED_BACK, got 'COMPLETE' — COMPLETE means the emulator ignored Monitors entirely and shipped a release with no monitoring at all, which is the over-permissive failure this check exists for
  The check creates TWO roles differing only in which service their trust policy names (one appconfig.amazonaws.com, one not), both with cloudwatch:DescribeAlarms inline, and TWO environments monitoring the SAME alarm (which stays OK throughout) through those two roles. It first proves the correctly-trusted environment deploys to COMPLETE and serves the bytes (positive control PASSED, ruling out IAM propagation timing), then starts the same deployment on the broken environment with FinalBakeTimeInMinutes 1 and polls for a terminal state with a 480s budget.
  Expected: State 'ROLLED_BACK'; an EventLog ROLLBACK_COMPLETED entry with TriggeredBy != 'CLOUDWATCH_ALARM' and Description 'Unable to assume environment monitor role: <AlarmRoleArn>'; no DEPLOYMENT_COMPLETED entry; GetEnvironment State 'RolledBack'; and appconfigdata.StartConfigurationSession on that environment refused with ResourceNotFoundException / 'Deployment not found'. Observed: State 'COMPLETE', so none of those assertions is reached.
  The role ARN in the expected Description is a per-run value filling the template; 'Unable to assume environment monitor role: ' is the fixed text.

Smithy consulted (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json): StartDeployment — POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, success 201, output Deployment{State -> DeploymentState, PercentageComplete, EventLog -> DeploymentEvents, FinalBakeTimeInMinutes -> MinutesBetween0And24Hours @range 0..1440, CompletedAt}, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500. Monitor = structure{AlarmArn @required @length 1..2048, AlarmRoleArn -> RoleArn @length 20..2048 pattern ^((arn):(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov|aws-eusc):(iam)::\d{12}:role[/].*)$}, doc 'Amazon CloudWatch alarms to monitor during the deployment process'. DeploymentEventType values [PERCENTAGE_UPDATED, ROLLBACK_STARTED, ROLLBACK_COMPLETED, BAKE_TIME_STARTED, DEPLOYMENT_STARTED, DEPLOYMENT_COMPLETED, REVERT_COMPLETED]; TriggeredBy values [USER, APPCONFIG, CLOUDWATCH_ALARM, INTERNAL_ERROR]; DeploymentEvent.Description is documented to include 'The Amazon Web Services account or the Amazon CloudWatch alarm ARN that initiated a rollback'. The rollback decision and the event Description text are not modeled, so those rest on recorded behavior.

## Steps to Reproduce
Boot the emulator and point appconfig, appconfigdata, cloudwatch and iam clients at its data listener.

Cause 1 — replay appconfig__cloudwatch.cloudwatch-alarm-rollback-restores-previous-version:
1. iam.create_role with a trust policy allowing Service 'appconfig.amazonaws.com' to sts:AssumeRole, plus an inline policy granting cloudwatch:DescribeAlarms on '*'.
2. cloudwatch.put_metric_alarm on a metric in a private namespace.
3. CreateApplication -> CreateEnvironment(Monitors=[{'AlarmArn': <alarm arn>, 'AlarmRoleArn': <role arn>}]) -> CreateConfigurationProfile(LocationUri='hosted') -> CreateHostedConfigurationVersion twice (v1 then v2).
4. CreateDeploymentStrategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=1, ReplicateTo='NONE'); deploy v1 to COMPLETE.
5. StartDeployment of v2, then drive the alarm into ALARM (put_metric_data / set_alarm_state) during the bake window and poll GetDeployment for a terminal State.
   EXPECTED: State 'ROLLED_BACK', a ROLLBACK_COMPLETED EventLog entry, and StartConfigurationSession/GetLatestConfiguration serving v1's bytes again.
   OBSERVED: State 'COMPLETE'; v2 stays served.

Cause 2 — replay appconfig__cloudwatch.alarm-monitor-role-not-assumable-deployment-rejected:
1. Create TWO roles identical except for the Principal.Service in their trust policy: one 'appconfig.amazonaws.com', one a service that is not AppConfig. Give both cloudwatch:DescribeAlarms.
2. put_metric_alarm once; leave the alarm OK for the whole run.
3. CreateApplication, then TWO environments monitoring that same alarm — one with the trusted role's ARN, one with the untrusted role's ARN.
4. Hosted profile + one version, strategy with DeploymentDurationInMinutes 0 / GrowthFactor 100 / FinalBakeTimeInMinutes 1.
5. Deploy to the trusted environment and confirm it reaches COMPLETE and serves the bytes (it does).
6. StartDeployment the same configuration to the UNTRUSTED environment and poll GetDeployment for a terminal State.
   EXPECTED: State 'ROLLED_BACK'; EventLog contains ROLLBACK_COMPLETED with TriggeredBy != 'CLOUDWATCH_ALARM' and Description 'Unable to assume environment monitor role: <that role ARN>'; no DEPLOYMENT_COMPLETED entry; GetEnvironment State 'RolledBack'; appconfigdata.start_configuration_session on that environment refused HTTP 404 ResourceNotFoundException 'Deployment not found'.
   OBSERVED: State 'COMPLETE', no rollback events, and the configuration is reachable in that environment.

Re-replay both check ids in trace_ids to confirm both causes are closed.

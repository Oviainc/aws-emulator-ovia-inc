Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-catalog-list-extensions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-catalog-list-extensions appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|ListExtensions|2xx->2xx|$.Items[Arn="arn
Issue-Key: appconfig|StopDeployment|2xx->2xx|$.EventLog[*].Description
Issue-Key: appconfig|StopDeployment|2xx->2xx|$.EventLog[*].EventType
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.EventLog[*]
Issue-Related: 20261103T021132Z-appconfig-deployment-lifecycle-advances-instantly-through-deploy-bake.md
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# AppConfig synthesized response content diverges: deployment EventLog entries and the predefined extension catalog

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeployment

## Description
This issue bundles 3 net-new causes, all in appconfig, that share one root cause family: the emulator synthesizes response content (the AWS-authored extension catalog, and the deployment event log plus the rollout percentage that travels with it) with its own wording and its own set of entries instead of the content AWS serves. Every step below is reported by the replay as NOT owned by any open issue.

Related open issues (none of them covers the steps below, per the replay's ownership report): 20261103T021132Z-appconfig-deployment-lifecycle-advances-instantly-through-deploy-bake.md describes the deploy/bake phase jump for task#10 and the 409 wording for task#12 of the same fixture, and its prose mentions task#13/task#14 — but its replay pointer (Issue-Trace-Ids) lists only the bare fixture id with no '#step' half, so those two steps are not bound to it and are reported unowned here; Causes 2 and 3 below carry them with a complete fixture#step binding. 20261030T085534Z-appconfig-serves-deployment-state-as-the-model-s.md covers Deployment.State casing, which is a different member.

Cause 1 of 3: ListExtensions returns the wrong Description text for AWS-authored predefined extensions, and omits it entirely for others.
  AWS: Smithy: ListExtensions GET /extensions, http code 200, output Extensions{Items -> ExtensionSummaries of ExtensionSummary{Id, Name, VersionNumber, Arn, Description}}; ExtensionSummary.Description -> Description (@length 0..1024), documented as "Information about the extension". The model does not enumerate the predefined catalog, so for the catalog content this is recorded behavior only: AWS ships a fixed set of AWS-authored extensions (ARNs of the form arn:aws:appconfig:<region>::extension/<Name>/<VersionNumber>) and each one carries a fixed, AWS-authored Description string on every ListExtensions page. The recording shows, for example, AWS.AppConfig.FeatureFlags/1 = 'Validates AppConfig feature flag data automatically against a JSON schema that includes structure and constraints. Also transforms feature flag data prior to sending to the client. This extension is automatically associated to configuration profiles with type "AWS.AppConfig.FeatureFlags".', AWS.AppConfig.DeploymentNotificationsToEventBridge/1 = 'Sends events to Amazon EventBridge when a deployment of configuration data in AppConfig is started, completed, or rolled back. Can be associated to the following resources in AppConfig: Application, Environment, Configuration Profile.', and non-empty Descriptions for AWS.AppConfig.JiraIntegration/1 and AWS.Evidently.ProjectImporter/1. These strings are entirely fixed text keyed by extension name and version number — there is no part of them taken from the request or from account state, so the catalog must carry AWS's string verbatim per extension rather than a paraphrase.
  Emulator: serves the same extension ARNs but with its own shorter paraphrases where AWS has a longer fixed string (FeatureFlags/1 comes back as 'Validates AppConfig feature flag data automatically against a JSON schema that includes the feature flag data format. Also transforms feature flag data prior to sending to the client.'; DeploymentNotificationsToEventBridge/1 as 'Sends AppConfig deployment events to the default Amazon EventBridge event bus'), and omits Description altogether (null) for other catalog entries including AWS.AppConfig.JiraIntegration/1, AWS.Evidently.ProjectImporter/1 and the deployment-notifications-to-SQS entry. Status matches at 200 on both sides.
  Why: Description is what a console or a client-side extension picker shows a user to choose an extension, and tooling that asserts on the catalog (or that requires the member to be present) sees either different prose or a null where AWS always returns a string. A client cannot distinguish "this emulator paraphrases AWS" from "this account has a different extension".
  Fix: the appconfig predefined/AWS-authored extension catalog seed data that ListExtensions (and GetExtension) reads, in the extension state/catalog component — every predefined entry must carry AWS's verbatim Description.
  Traces: appconfig-catalog-list-extensions#task#1.

Cause 2 of 3: StopDeployment's returned event log has no PERCENTAGE_UPDATED event, mis-words the bake event, and the response still reports PercentageComplete 100.0 after the rollback.
  AWS: Smithy: StopDeployment DELETE /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, http code 202, output Deployment{State -> DeploymentState, PercentageComplete -> Percentage, EventLog -> DeploymentEvents of DeploymentEvent{EventType -> DeploymentEventType [PERCENTAGE_UPDATED, ROLLBACK_STARTED, ROLLBACK_COMPLETED, BAKE_TIME_STARTED, DEPLOYMENT_STARTED, DEPLOYMENT_COMPLETED, REVERT_COMPLETED], TriggeredBy -> TriggeredBy [USER, APPCONFIG, CLOUDWATCH_ALARM, INTERNAL_ERROR], Description, ActionInvocations, OccurredAt}}. For the event descriptions the model says only "A description of the deployment event", so recorded behavior: AWS records one event per lifecycle transition, newest first, and for a rollout that reached full availability the log contains an APPCONFIG-triggered PERCENTAGE_UPDATED event whose Description is 'Configuration available to <pct>% of clients' — fixed text 'Configuration available to ' + the rollout percentage formatted with two decimal places + '% of clients', the percentage being the value from the deployment's own state — immediately below the APPCONFIG-triggered BAKE_TIME_STARTED event, whose Description is exactly 'Deployment bake time started' (entirely fixed text; AWS does not name the bake duration in it), with the USER-triggered DEPLOYMENT_STARTED event 'Deployment started' at the bottom. After the stop/revert the response's PercentageComplete reports the reverted rollout, 0.0, not the pre-stop percentage.
  Emulator: the 202 status matches, but the log is one entry short and shifted: it never records a PERCENTAGE_UPDATED event at all, so where AWS has 'Configuration available to 100.00% of clients' (PERCENTAGE_UPDATED / APPCONFIG) the emulator has the DEPLOYMENT_STARTED / USER / 'Deployment started' event, and the slot AWS fills with DEPLOYMENT_STARTED is absent entirely; the bake event's Description carries a non-AWS suffix, 'Deployment bake time started for 10 minute(s)' instead of 'Deployment bake time started' (the ' for N minute(s)' tail is invented — AWS's text has no variable part there); and PercentageComplete is still 100.0 instead of 0.0.
  Why: a client that reads the event log to audit a rollout can never attribute the moment the configuration became available to clients, because the event AWS uses for that does not exist; one that matches on event Description or indexes the log positionally reads the wrong event for the wrong phase. PercentageComplete 100.0 on the stop response reports a reverted configuration as fully deployed, which is the opposite of the state the caller just asked for.
  Fix: the deployment state machine's event-log recording and the StopDeployment handler's response projection in the appconfig deployment state — append an APPCONFIG-triggered PERCENTAGE_UPDATED event when the rollout reaches a percentage, emit AWS's fixed bake-time description without the duration suffix, and recompute PercentageComplete for the reverted deployment.
  Traces: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#13.

Cause 3 of 3: GetDeployment omits DeploymentEvent.ActionInvocations on every event (null instead of an empty list), and repeats Cause 2's log and percentage divergences on the read path.
  AWS: Smithy: GetDeployment GET /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, http code 200, output Deployment; DeploymentEvent.ActionInvocations -> ActionInvocations (list), documented "The list of extensions that were invoked as part of the deployment". Recorded behavior: every event in the returned log carries ActionInvocations — an EMPTY list when no extension action fired for that event, never an absent member — and the log reads newest-first with the same four events as Cause 2 (ROLLBACK_COMPLETED, BAKE_TIME_STARTED 'Deployment bake time started', PERCENTAGE_UPDATED 'Configuration available to 100.00% of clients', DEPLOYMENT_STARTED 'Deployment started'), with PercentageComplete 0.0 after the revert.
  Emulator: the 200 status matches, but ActionInvocations is omitted on every event (null), the PERCENTAGE_UPDATED event is again missing so the DEPLOYMENT_STARTED / USER event occupies its slot and the final slot is absent, the bake description again carries the ' for 10 minute(s)' suffix, and PercentageComplete is 100.0.
  Why: ActionInvocations is a modeled list; serving null where AWS guarantees a list is a deserialization / null-dereference hazard for generated SDK clients and silently breaks any code that iterates extension invocations per event. Combined with the missing percentage event, a client reconstructing the rollout timeline from a read of the deployment gets a different history than AWS recorded.
  Fix: the Deployment/DeploymentEvent serializer (always emit ActionInvocations, defaulting to an empty list) plus the same event-log recording and PercentageComplete computation named in Cause 2.
  Traces: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#14.

## Evidence
Replay of trace set simparity-wire-aws-v1 (48 fixtures, 580 steps, 565 passed, 15 mismatched, score 97.4%); the three clusters below are reported with stub_path empty and owned_by empty.

Cause 1 — appconfig-catalog-list-extensions#task#1, ListExtensions, 200 expected / 200 actual (mismatch):
  $.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.FeatureFlags/1"].Description: value_mismatch — expected 'Validates AppConfig feature flag data automatically against a JSON schema that includes structure and constraints. Also transforms feature flag data prior to sending to the client. This extension is automatically associated to configuration profiles with type "AWS.AppConfig.FeatureFlags".', got 'Validates AppConfig feature flag data automatically against a JSON schema that includes the feature flag data format. Also transforms feature flag data prior to sending to the client.'
  $.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.JiraIntegration/1"].Description: missing — expected 'Exports feature flag data from AWS AppConfig into Jira. The lifecycle of each feature flag in AppConfig is tracked in Jira as an individual issue. Customers can see in Jira when flags are updated, turned on or off. Works in conjunction with the AppConfig app in the Atlassian Marketplace and is automatically associated to configuration profiles configured within that app.', got None
  $.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.Evidently.ProjectImporter/1"].Description: missing — expected 'Imports Evidently Project data for use with its client-side evaluation feature, powered by AppConfig. This extension is automatically associated to ConfigurationProfiles created by Evidently.', got None
  $.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentNotificationsToEventBridge/1"].Description: value_mismatch — expected 'Sends events to Amazon EventBridge when a deployment of configuration data in AppConfig is started, completed, or rolled back. Can be associated to the following resources in AppConfig: Application, Environment, Configuration Profile.', got 'Sends AppConfig deployment events to the default Amazon EventBridge event bus'
  $.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentNotificationsToSqs/1"]: further Description divergence reported in the same cluster (the detail is elided by the replay report at this entry).
  (The ARN region/partition segment is environment-specific and is not part of the requirement; the per-extension Description text is.)

Cause 2 — appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#13, StopDeployment, 202 expected / 202 actual (mismatch):
  $.EventLog[1].Description: expected 'Deployment bake time started', got 'Deployment bake time started for 10 minute(s)'
  $.EventLog[2].EventType: expected 'PERCENTAGE_UPDATED', got 'DEPLOYMENT_STARTED'
  $.EventLog[2].TriggeredBy: expected 'APPCONFIG', got 'USER'
  $.EventLog[2].Description: expected 'Configuration available to 100.00% of clients', got 'Deployment started'
  $.EventLog[3]: missing — expected {Description: 'Deployment started', EventType: 'DEPLOYMENT_STARTED', TriggeredBy: 'USER', OccurredAt: <shaped-not-asserted>}, got None
  $.PercentageComplete: expected 0.0, got 100.0

Cause 3 — appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#14, GetDeployment, 200 expected / 200 actual (mismatch):
  $.EventLog[0].ActionInvocations: missing — expected [], got None
  $.EventLog[1].ActionInvocations: missing — expected [], got None
  $.EventLog[2].ActionInvocations: missing — expected [], got None
  $.EventLog[1].Description: expected 'Deployment bake time started', got 'Deployment bake time started for 10 minute(s)'
  $.EventLog[2].EventType: expected 'PERCENTAGE_UPDATED', got 'DEPLOYMENT_STARTED'; $.EventLog[2].TriggeredBy: expected 'APPCONFIG', got 'USER'; $.EventLog[2].Description: expected 'Configuration available to 100.00% of clients', got 'Deployment started'
  $.EventLog[3]: missing — expected {ActionInvocations: [], Description: 'Deployment started', EventType: 'DEPLOYMENT_STARTED', TriggeredBy: 'USER', OccurredAt: <shaped-not-asserted>}, got None
  $.PercentageComplete: expected 0.0, got 100.0
  (OccurredAt values and generated ids are per-run and are not asserted.)

## Steps to Reproduce
Cause 1:
1. Call ListExtensions with no filters against a fresh emulator and read $.Items.
2. For each AWS-authored entry (ARN arn:aws:appconfig:<region>::extension/<Name>/<Version>) compare Description with AWS's fixed catalog text: AWS.AppConfig.FeatureFlags/1, AWS.AppConfig.DeploymentNotificationsToEventBridge/1 and AWS.AppConfig.DeploymentNotificationsToSqs/1 come back paraphrased, and AWS.AppConfig.JiraIntegration/1 and AWS.Evidently.ProjectImporter/1 come back with no Description at all.
3. Replay appconfig-catalog-list-extensions (task#1).

Causes 2 and 3:
1. Create an application, an environment, a hosted configuration profile with one version, and a deployment strategy with DeploymentDurationInMinutes 0, GrowthType LINEAR, GrowthFactor 100 and FinalBakeTimeInMinutes 10.
2. StartDeployment with that strategy, then call StopDeployment on the in-flight deployment and inspect the 202 body's EventLog: AWS's log is four events newest-first — ROLLBACK_COMPLETED, BAKE_TIME_STARTED 'Deployment bake time started', PERCENTAGE_UPDATED / APPCONFIG 'Configuration available to 100.00% of clients', DEPLOYMENT_STARTED / USER 'Deployment started' — and PercentageComplete is 0.0. The emulator returns three events, no PERCENTAGE_UPDATED, 'Deployment bake time started for 10 minute(s)', and PercentageComplete 100.0.
3. Call GetDeployment on the same deployment number: AWS returns ActionInvocations: [] on every event; the emulator omits the member on all of them and repeats the log/percentage divergences from step 2.
4. Replay appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value (task#13 for StopDeployment, task#14 for GetDeployment).

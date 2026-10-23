Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-end-to-end-deployment-workflow
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-end-to-end-deployment-workflow appconfig-experiment-run-full-lifecycle appconfig-experiment-run-lifecycle appconfig-experiment-run-multiple-runs appconfig-experiment-run-treatment-overrides appconfig-deployment-stop-rollback
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.AppliedExtensions
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.CompletedAt
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.EventLog[*].Description
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.EventLog[*].EventType
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.EventLog[*].OccurredAt
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.StartedAt
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.State
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.Type
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.EventLog[*]
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.EventLog[*].TriggeredBy
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.PercentageComplete
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.CompletedAt
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.EventLog[*]
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.StartedAt
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.Type
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# AppConfig Deployment documents use epoch timestamps, omit modeled members and log the wrong events

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
Every Deployment document the emulator serves diverges from AWS in the same ways: date-time members are serialized as epoch numbers, modeled/recorded members are missing, the EventLog is populated with the wrong events, and an instant (0-minute, 100% growth) deployment is reported as still DEPLOYING. Three replayed clusters. Numbered; close ALL THREE before submitting.

=== Cause 1 of 3: StartDeployment on an instant strategy — epoch timestamps, missing members, wrong EventLog, wrong State ===
AWS: StartDeployment answers 201 with a Deployment. StartedAt and CompletedAt target Iso8601DateTime with timestampFormat date-time, and DeploymentEvent.OccurredAt likewise, so all three are ISO-8601 strings on the wire (Smithy: Deployment.StartedAt/CompletedAt Iso8601DateTime timestampFormat=date-time; DeploymentEvent{EventType,TriggeredBy,Description,ActionInvocations,OccurredAt}; EventLog doc: 'the most recent events are displayed first'; State target DeploymentState with COMPLETE among its values; AppliedExtensions is a modeled member). Recorded behavior only for the rest: a strategy with DeploymentDurationInMinutes 0 and GrowthFactor 100 completes immediately, so the response carries State='COMPLETE', a CompletedAt, AppliedExtensions=[] and a two-entry EventLog whose newest entry is EventType='DEPLOYMENT_COMPLETED' (Description example 'Deployment completed instantly'), plus Type='USER' (a member the recorded service returns and the on-disk model does not declare).
Emulator: 201 but State='DEPLOYING', no CompletedAt, no AppliedExtensions, no Type, StartedAt/OccurredAt emitted as epoch floats (e.g. 1792766831.7267702) instead of ISO-8601 strings, and EventLog[0] is EventType='PERCENTAGE_UPDATED' ('The configuration reached 100% of deployment targets.') where AWS's newest event is DEPLOYMENT_COMPLETED.
Why: an SDK that parses StartedAt/CompletedAt/OccurredAt as date-time either fails or yields a nonsense instant; a client waiting for State='COMPLETE' on an instant deployment never finishes; code reading AppliedExtensions/Type gets a missing key; event-driven logic keyed on DEPLOYMENT_COMPLETED misses the completion.
Fix: the appconfig deployment serializer must render Iso8601DateTime members in date-time form and emit AppliedExtensions and Type; the StartDeployment handler must settle a 0-duration/100%-growth deployment to COMPLETE with a CompletedAt and append the DEPLOYMENT_STARTED/DEPLOYMENT_COMPLETED events AWS records (newest first) instead of a PERCENTAGE_UPDATED entry.
Traces: appconfig-end-to-end-deployment-workflow#task#6 (cluster of 5 fixtures).

=== Cause 2 of 3: StartDeployment on a gradual strategy — PercentageComplete and EventLog jump ahead, timestamps epoch, Type/AppliedExtensions missing ===
AWS: recorded behavior only — the 201 for a just-started gradual deployment reports PercentageComplete=0.0 with a single EventLog entry EventType='DEPLOYMENT_STARTED', TriggeredBy='USER', plus Type='USER', AppliedExtensions=[] and an ISO-8601 StartedAt/OccurredAt (Smithy as cause 1).
Emulator: PercentageComplete=10.0 and EventLog[0] is already EventType='PERCENTAGE_UPDATED'/TriggeredBy='APPCONFIG' with the DEPLOYMENT_STARTED entry pushed to index 1; Type and AppliedExtensions absent; StartedAt/OccurredAt epoch floats.
Why: the response describes progress that has not happened, so a client reading PercentageComplete off StartDeployment mis-reports rollout progress, and the first-interval event is fabricated at creation time.
Fix: StartDeployment must return the deployment at its initial state (PercentageComplete 0, only the DEPLOYMENT_STARTED event) and advance percentage/events only as the deployment progresses; same serializer fixes as cause 1.
Traces: appconfig-deployment-stop-rollback#task#6.

=== Cause 3 of 3: GetDeployment repeats the same defects on the read path ===
AWS: GetDeployment answers 200 with the Deployment document — newest-first EventLog whose entries are DEPLOYMENT_COMPLETED then DEPLOYMENT_STARTED for an instant deployment, ISO-8601 StartedAt/CompletedAt/OccurredAt, and Type='USER' (recorded).
Emulator: 200 but EventLog[1] is a PERCENTAGE_UPDATED entry where AWS has DEPLOYMENT_STARTED, Type is missing, and StartedAt/CompletedAt/OccurredAt are epoch floats.
Why: polling GetDeployment — the normal way to follow a deployment — hits the same unparseable timestamps and an event log that does not contain the start event.
Fix: as causes 1-2; the Get path must share the corrected serializer and the stored event log.
Traces: appconfig-end-to-end-deployment-workflow#task#8.

## Evidence
Wire replay (simparity-wire-aws-v1), all appconfig:

Cause 1 — appconfig-end-to-end-deployment-workflow#task#6 StartDeployment, 201/201: $.AppliedExtensions missing (expected []); $.CompletedAt missing; $.EventLog[0].EventType expected 'DEPLOYMENT_COMPLETED', got 'PERCENTAGE_UPDATED'; $.EventLog[0].Description expected 'Deployment completed instantly', got 'The configuration reached 100% of deployment targets.'; $.EventLog[0].OccurredAt / $.EventLog[1].OccurredAt / $.StartedAt got 1792766831.7267702; $.State expected 'COMPLETE', got 'DEPLOYING'; $.Type missing (expected 'USER'). Cluster also covers appconfig-experiment-run-full-lifecycle#setup#6, appconfig-experiment-run-lifecycle#task#6, appconfig-experiment-run-multiple-runs#setup#6, appconfig-experiment-run-treatment-overrides#setup#6.

Cause 2 — appconfig-deployment-stop-rollback#task#6 StartDeployment, 201/201: $.PercentageComplete expected 0.0, got 10.0; $.EventLog[0].EventType expected 'DEPLOYMENT_STARTED', got 'PERCENTAGE_UPDATED'; $.EventLog[0].TriggeredBy expected 'USER', got 'APPCONFIG'; $.EventLog[1] extra {'EventType': 'DEPLOYMENT_STARTED', 'TriggeredBy': 'USER', ...}; $.EventLog[0].OccurredAt and $.StartedAt expected '2026-08-08T07:17:26.948Z', got 1792766801.645569; $.AppliedExtensions missing; $.Type missing.

Cause 3 — appconfig-end-to-end-deployment-workflow#task#8 GetDeployment, 200/200: $.EventLog[1] expected {'EventType': 'DEPLOYMENT_STARTED', 'TriggeredBy': 'USER', ...}, got {'EventType': 'PERCENTAGE_UPDATED', 'TriggeredBy': 'APPCONFIG', ...}; $.CompletedAt got 1792766832.2267702; $.StartedAt got 1792766831.7267702; $.Type missing (expected 'USER').

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.
Cause 1: create_application -> create_environment -> create_configuration_profile(LocationUri='hosted') -> create_hosted_configuration_version -> create_deployment_strategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo='NONE') -> start_deployment(...). Inspect State (must be COMPLETE), CompletedAt/StartedAt (must be date-time strings), AppliedExtensions, Type and EventLog[0].EventType (must be DEPLOYMENT_COMPLETED).
Cause 2: same setup with a gradual strategy (e.g. DeploymentDurationInMinutes=5, GrowthFactor=10) -> start_deployment. PercentageComplete must be 0.0 and the only event DEPLOYMENT_STARTED/USER.
Cause 3: get_deployment(ApplicationId, EnvironmentId, DeploymentNumber=1) after cause 1 and inspect the same members.
Re-replay appconfig-end-to-end-deployment-workflow#task#6 plus both steps in trace_ids.

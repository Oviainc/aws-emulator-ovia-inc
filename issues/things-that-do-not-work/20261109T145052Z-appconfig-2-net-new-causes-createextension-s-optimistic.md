Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.concurrent-extension-version-optimistic-lock appconfig.final-bake-time-window-holds-the-deployment-non-terminal
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.concurrent-extension-version-optimistic-lock|run#22|AssertionError|assert loser_message == expected, f"loser got wrong message: {results}"
Issue-Key: appconfig.final-bake-time-window-holds-the-deployment-non-terminal|run#58|AssertionError|assert percentage_stamps, (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# appconfig: 2 net-new causes — CreateExtension's optimistic-lock ConflictException message is emulator-authored, and a completed deployment's EventLog never records PERCENTAGE_UPDATED

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExtension

## Description
Two net-new root causes in the appconfig service, grounded in two replayed behavioral checks. BOTH are separate defects and EVERY ONE must be closed before this issue is submitted; re-replay every check id in trace_ids to confirm. Neither check was marked ALREADY FILED and neither was marked OWNED by an open issue in this replay (the open issues named under each cause do not carry these checks' replay keys).

=== Cause 1 of 2: CreateExtension's optimistic-lock (LatestVersionNumber) conflict is refused with the emulator's own prose instead of AppConfig's wording ===
AWS: Smithy (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json) models CreateExtension as POST /extensions, success 201, with LatestVersionNumber bound to the Latest-Version-Number header and errors BadRequestException 400 / ConflictException 409 / ServiceQuotaExceededException 402 / InternalServerException 500. ConflictException carries only a Message member and no modeled text, so the wording is recorded behavior only. A CreateExtension that pins LatestVersionNumber to a value that is no longer the extension's current version is refused with ConflictException (HTTP 409) and one of exactly two messages, depending on how far the winning writer got:
  (a) "LatestVersionNumber supplied in request <supplied> does not match latest version <actual>." — fixed text "LatestVersionNumber supplied in request " / " does not match latest version " / "."; the variable parts are the LatestVersionNumber taken from the request and the extension's current version taken from state.
  (b) "Extension with id <extensionId> and versionNumber <version> already exists for account <accountId>." — fixed text "Extension with id " / " and versionNumber " / " already exists for account " / "."; the variable parts are the extension Id (Smithy Id, pattern ^[a-z0-9]{4,7}$) and version number from state and the caller's account id.
The error CLASS and the race outcome are already right in the emulator: of two concurrent racers exactly one wins, the loser gets ConflictException, the winner lands on version 2, and GetExtension then reports VersionNumber 2.
Emulator: refuses with ConflictException but the message "Extension <name> is at version <actual>, not <supplied>." — neither AWS form, and it reports the extension NAME where AWS reports only ids and version numbers.
Why: the message is the only place a conditional-write failure explains itself. A client or operator that matches, logs or displays AppConfig's conflict wording sees text AWS never emits, and a compare-and-swap retry loop keyed on either AWS sentence never matches — so the retry is skipped or the failure is misattributed. It also leaks a resource name into an error AWS keeps id-only.
Fix: the CreateExtension handler's optimistic-lock failure path in the appconfig service (the same formatter must produce the 'already exists for account' variant when the new version row has already been written, and the 'does not match latest version' variant when it has not).
Related open issues that do NOT carry this check's replay key: 20261102T092018Z-appconfig-3-net-new-causes-treatments-list-max.md Cause 2 and 20261029T195212Z-appconfig-control-plane-refusals-wrong-error-classes-and.md Cause 5 both record this same message defect, but this replay reports appconfig.concurrent-extension-version-optimistic-lock as unowned, so no filed issue carries its key. Fixing those should fix this; close all of them deliberately rather than assuming.
Traces: appconfig.concurrent-extension-version-optimistic-lock.

=== Cause 2 of 2: a deployment that rolls out and bakes to COMPLETE never records a PERCENTAGE_UPDATED event in its EventLog ===
AWS: Smithy models Deployment.EventLog -> DeploymentEvents of DeploymentEvent{EventType -> DeploymentEventType, TriggeredBy -> TriggeredBy, Description, ActionInvocations, OccurredAt}; DeploymentEventType enumerates PERCENTAGE_UPDATED, ROLLBACK_STARTED, ROLLBACK_COMPLETED, BAKE_TIME_STARTED, DEPLOYMENT_STARTED, DEPLOYMENT_COMPLETED, REVERT_COMPLETED, and DeploymentEvent.Description is documented as including "The percentage of hosts that received the deployment". The text itself is not modeled, so recorded behavior: AppConfig appends one event per lifecycle transition, and a rollout that advanced the configuration's availability records at least one APPCONFIG-triggered PERCENTAGE_UPDATED event whose Description is "Configuration available to <pct>% of clients" — fixed text "Configuration available to " / "% of clients", the single variable part being the rollout percentage from the deployment's own state formatted with two decimal places. Ordered by OccurredAt, the PERCENTAGE_UPDATED event(s) fall after DEPLOYMENT_STARTED and at or before BAKE_TIME_STARTED: the bake window opens only once the rollout has finished.
Emulator: a deployment started with GrowthFactor 100 / FinalBakeTimeInMinutes 1 reaches COMPLETE and its EventLog contains only ['DEPLOYMENT_COMPLETED', 'BAKE_TIME_STARTED', 'DEPLOYMENT_STARTED'] — no PERCENTAGE_UPDATED entry at any percentage, so the moment the configuration became available to clients is absent from the log entirely.
Why: the event log is the audit trail of a rollout. A client that reads it to establish when (and to what fraction of clients) a configuration became available can never do so, because the event AWS uses for exactly that does not exist; one that orders the bake relative to the rollout (BAKE_TIME_STARTED after the last PERCENTAGE_UPDATED) has nothing to compare against; and one that indexes the log positionally reads the wrong event for the wrong phase.
Fix: the deployment state machine's event-log recording in the appconfig deployment state — append an APPCONFIG-triggered PERCENTAGE_UPDATED event, with AWS's Description text, each time the rollout reaches a new percentage, before BAKE_TIME_STARTED is appended.
Related open issue that does NOT carry this check's replay key: 20261105T012540Z-appconfig-synthesized-response-content-diverges-deployment-eventlog-entries.md Causes 2 and 3 record the same missing PERCENTAGE_UPDATED event, but they are grounded in WIRE fixture steps (appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#13/#task#14) on the StopDeployment/GetDeployment read paths; this behavioral check exercises the COMPLETE (baked, non-rolled-back) path and is reported unowned. Verify a fix against this check too.
Traces: appconfig.final-bake-time-window-holds-the-deployment-non-terminal.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the booted emulator: 207/246 passed (84.1%), 30 mismatched, 9 errored, 0 skipped, 0 unloadable. bootstrap_signal was false and the emulator answered throughout, so this is not a bootstrap failure. Per-service: appconfig 194/220 (88%), 26 failed — 24 of those are marked OWNED by an open issue and are skipped here. The appconfig service is bootstrapped (crate service-com-amazonaws-appconfig-amazonappconfig).

Cause 1 — appconfig.concurrent-extension-version-optimistic-lock, verdict [mismatch]:
  AssertionError: loser got wrong message: [('ok', 2), ('err', 'ConflictException', 'Extension sim2real-behav-ext-9422086fa9eb is at version 2, not 1.')]
  The check creates an extension with Actions={'ON_DEPLOYMENT_COMPLETE': [{'Name':'notify','Uri':<sns topic arn>}]} (VersionNumber 1), then fires TWO concurrent CreateExtension(Name=<same>, Actions=<same>, Description='racer-<i>', LatestVersionNumber=1) calls and accepts EITHER AWS wording from the loser:
    "LatestVersionNumber supplied in request 1 does not match latest version 2."
    "Extension with id <extensionId> and versionNumber 2 already exists for account <accountId>."
  Expected: HTTP 409 ConflictException with one of those two Messages. Observed: HTTP 409 ConflictException with Message 'Extension <name> is at version 2, not 1.'
  Everything else matched: exactly one winner, exactly one ConflictException loser, winner VersionNumber == 2, and GetExtension reporting VersionNumber 2 (exactly one new version from the race). The extension name/id in the quoted text are per-run values that only fill the templates; the fixed text is the requirement.

Cause 2 — appconfig.final-bake-time-window-holds-the-deployment-non-terminal, verdict [mismatch]:
  AssertionError: a completed deployment must log at least one PERCENTAGE_UPDATED; got ['DEPLOYMENT_COMPLETED', 'BAKE_TIME_STARTED', 'DEPLOYMENT_STARTED']
  The check deploys a hosted configuration version with a strategy of DeploymentDurationInMinutes 0 / GrowthFactor 100 / FinalBakeTimeInMinutes 1 / ReplicateTo NONE on an UNMONITORED environment, confirms the deployment holds in BAKING at PercentageComplete 100.0 across three consecutive GetDeployment reads, waits the bake out to COMPLETE, and verifies the server's own timestamps bound the window. All of those assertions passed — the deployment reached COMPLETE, carried exactly one BAKE_TIME_STARTED entry, and CompletedAt - BAKE_TIME_STARTED.OccurredAt bracketed the declared 60s window. It then collects the OccurredAt of every PERCENTAGE_UPDATED event in EventLog so it can assert BAKE_TIME_STARTED >= max(PERCENTAGE_UPDATED):
  Expected: at least one EventLog entry with EventType 'PERCENTAGE_UPDATED' (APPCONFIG-triggered, Description 'Configuration available to <pct>% of clients'). Observed: none — the log holds only DEPLOYMENT_COMPLETED, BAKE_TIME_STARTED and DEPLOYMENT_STARTED, so the bake-after-rollout ordering assertion is unreachable.

Smithy consulted (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json): CreateExtension — POST /extensions, success 201, Name -> ExtensionOrParameterName pattern ^[^\/#:\n]{1,64}$, Actions -> ActionsMap @length 1..5 @required, LatestVersionNumber -> Integer bound to httpHeader 'Latest-Version-Number', output Extension{Id -> Id ^[a-z0-9]{4,7}$, Name, VersionNumber, Arn, Description, Actions, Parameters}, errors BadRequestException 400 / ConflictException 409 / ServiceQuotaExceededException 402 / InternalServerException 500; ConflictException = structure{Message: String}, httpError 409, no modeled message text. StartDeployment — POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, success 201, output Deployment{State -> DeploymentState, PercentageComplete -> Percentage, EventLog -> DeploymentEvents, StartedAt/CompletedAt -> Iso8601DateTime, FinalBakeTimeInMinutes -> MinutesBetween0And24Hours @range 0..1440}. DeploymentEvent = structure{EventType -> DeploymentEventType, TriggeredBy -> TriggeredBy, Description -> Description @length 0..1024, ActionInvocations, OccurredAt}; DeploymentEventType values [PERCENTAGE_UPDATED, ROLLBACK_STARTED, ROLLBACK_COMPLETED, BAKE_TIME_STARTED, DEPLOYMENT_STARTED, DEPLOYMENT_COMPLETED, REVERT_COMPLETED]; TriggeredBy values [USER, APPCONFIG, CLOUDWATCH_ALARM, INTERNAL_ERROR]; DeploymentEvent.Description is documented to include "The percentage of hosts that received the deployment". No message/description text is modeled, so both causes' exact wording rests on recorded behavior.

## Steps to Reproduce
Boot the emulator and point appconfig, appconfigdata and sns clients at its data listener.

Cause 1 — replay appconfig.concurrent-extension-version-optimistic-lock:
1. sns.create_topic(Name=<unique>) to get an action target Uri.
2. appconfig.create_extension(Name=N, Actions={'ON_DEPLOYMENT_COMPLETE': [{'Name': 'notify', 'Uri': <topic arn>}]}) -> VersionNumber 1.
3. Issue TWO concurrent appconfig.create_extension(Name=N, Actions=<same>, Description='racer-<i>', LatestVersionNumber=1) calls from a thread pool.
4. EXPECTED: one HTTP 201 with VersionNumber 2; the other HTTP 409 ConflictException whose Message is exactly "LatestVersionNumber supplied in request 1 does not match latest version 2." or "Extension with id <extensionId> and versionNumber 2 already exists for account <accountId>."
   OBSERVED: HTTP 409 ConflictException with Message "Extension <name> is at version 2, not 1."
5. A sequential equivalent (one CreateExtension with a stale LatestVersionNumber) reproduces the same wording.

Cause 2 — replay appconfig.final-bake-time-window-holds-the-deployment-non-terminal:
1. CreateApplication -> CreateEnvironment with NO Monitors -> CreateConfigurationProfile(LocationUri='hosted') -> CreateHostedConfigurationVersion twice (v1, then v2).
2. CreateDeploymentStrategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=1, ReplicateTo='NONE').
3. StartDeployment with that strategy and ConfigurationVersion='2'; poll GetDeployment until State is terminal (it reaches COMPLETE).
4. Read the terminal response's EventLog. EXPECTED: at least one entry with EventType 'PERCENTAGE_UPDATED', TriggeredBy 'APPCONFIG' and Description 'Configuration available to <pct>% of clients', with its OccurredAt at or before the BAKE_TIME_STARTED entry's OccurredAt. OBSERVED: EventLog EventTypes are only ['DEPLOYMENT_COMPLETED', 'BAKE_TIME_STARTED', 'DEPLOYMENT_STARTED'].

Re-replay both check ids in trace_ids to confirm both causes are closed.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.configuration-profile-update-immutable-type-rejected appconfig.stop-deployment-then-list-deployments-state appconfig.op-contract-stopDeployment appconfig.stop-deployment-during-bake-reverts-to-the-previous-version appconfig.start-deployment-latest-deployment-number-is-not-enforced appconfig.experiment-definition-run-observe-events appconfig.experiment-run-treatment-assignment-enforced-serverside
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.configuration-profile-update-immutable-type-rejected|run#12|AssertionError|assert status == 400, f"expected the server to refuse a smuggled Type change, got {status}: {body}"
Issue-Key: appconfig.stop-deployment-then-list-deployments-state|run#16|AssertionError|assert stopped["State"] == "ROLLED_BACK", (
Issue-Key: appconfig.op-contract-stopDeployment|run#20|AssertionError|assert final.get("CompletedAt") is None, "a rolled-back deployment must not report a completion time"
Issue-Key: appconfig.stop-deployment-during-bake-reverts-to-the-previous-version|run#42|AssertionError|assert event["TriggeredBy"] == "USER", (
Issue-Key: appconfig|StartDeployment|ConflictException
Issue-Key: appconfig.experiment-definition-run-observe-events|run#19|KeyError|assert run_state["ExposurePercentage"] == 0.0, (
Issue-Key: appconfig.experiment-run-treatment-assignment-enforced-serverside|run#6|AssertionError|assert definition["Control"]["Key"] == "c", (
Issue-Commit: a2b95246df63384d1b9b9d3ee28f05a727c934df

# AppConfig deployment and experiment state machine: smuggled Type accepted, rollback never finished, LatestDeploymentNumber over-enforced (7 causes)

> _Note: this issue was identified against the code base at commit `a2b95246df63384d1b9b9d3ee28f05a727c934df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeployment

## Description
Seven state-machine and response-fidelity defects in the AppConfig control plane: one over-permissive write, one over-strict refusal, three wrong deployment-stop representations and two wrong experiment representations. 7 causes are bundled; each is a distinct defect and all must be closed before submitting.

Cause 1 of 7: UpdateConfigurationProfile accepts an unmodeled Type field and applies the rest of the request.
AWS: Smithy UpdateConfigurationProfileRequest (PATCH /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}) has NO Type member at all — Type exists only on the response shape ConfigurationProfile — and the operation models BadRequestException (400). Recorded behavior: when a hand-signed PATCH smuggles a "Type" field past the SDK, the service inspects it anyway and refuses the WHOLE request with HTTP 400 and a Type-specific message, recorded as "Changing the Type of this configuration profile is not supported."; the co-submitted legitimate Description does not take effect either, and the profile's Type never moves off its creation value.
Emulator: answers HTTP 200 and returns the profile with the new Description applied (the smuggled Type is silently ignored rather than refused), so a partial write lands where AWS performs none.
Why: a client that (deliberately or through a hand-rolled request) asks to change an immutable field is told its request succeeded. It then believes either that the type changed or that the whole request was validated, when in fact AppConfig forbids the call outright. This is over-permissiveness on an immutable attribute, and the per-field drop makes the refusal boundary invisible.
Fix: the UpdateConfigurationProfile handler's request validation in the appconfig service — reject unknown/immutable members with BadRequestException before applying any field.
Traces: appconfig.configuration-profile-update-immutable-type-rejected.

Cause 2 of 7: StopDeployment answers the transitional ROLLING_BACK instead of the terminal ROLLED_BACK.
AWS: Smithy StopDeployment (DELETE .../deployments/{DeploymentNumber}, success 202, output Deployment); Deployment.State targets DeploymentState [BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED]. Recorded behavior: the 202 response already reports State "ROLLED_BACK" — AWS settles the record synchronously in the stop response.
Emulator: answers 202 with State "ROLLING_BACK".
Why: a client that stops a deployment and reads the response to confirm the rollback landed sees it still in flight; code that branches on ROLLED_BACK never takes that branch. Both values are modeled, so this is the wrong value, not a missing one.
Fix: the StopDeployment handler and the deployment state machine in the appconfig service.
Related open issue (does not cover this check): 20261029T190217Z-appconfig-stop-operations-diverge-stopdeployment-leaves-the-rollback.md carries this same root cause for its WIRE fixture keys only; this behavioral check is not in its replay set.
Traces: appconfig.stop-deployment-then-list-deployments-state.

Cause 3 of 7: a rolled-back deployment reports a CompletedAt timestamp.
AWS: Smithy Deployment.CompletedAt targets Iso8601DateTime (@timestampFormat date-time) and is the time the deployment COMPLETED. Recorded behavior: a deployment that was stopped and rolled back carries no CompletedAt — the member is absent from Get/List of a ROLLED_BACK deployment.
Emulator: emits CompletedAt for the rolled-back deployment.
Why: CompletedAt is how a client distinguishes a deployment that finished from one that was aborted; populating it on a rollback makes an aborted rollout indistinguishable from a successful one in dashboards, audits and idempotency logic.
Fix: the deployment state machine / Deployment serializer in the appconfig service — set CompletedAt only on the COMPLETE transition.
Traces: appconfig.op-contract-stopDeployment.

Cause 4 of 7: a user-requested rollback is attributed to APPCONFIG instead of USER.
AWS: Smithy DeploymentEvent.TriggeredBy targets the TriggeredBy enum [USER, APPCONFIG, CLOUDWATCH_ALARM, INTERNAL_ERROR], documented as "the entity that triggered the deployment event". Recorded behavior: when the caller invokes StopDeployment on an environment with no Monitors, the rollback events in the EventLog carry TriggeredBy "USER".
Emulator: carries TriggeredBy "APPCONFIG" for the caller's own stop.
Why: TriggeredBy is the only field that distinguishes an operator-initiated rollback from an alarm-driven or service-initiated one. Mis-attributing it makes a user stop and an automatic rollback indistinguishable in incident review and in automation that reacts only to alarm-driven rollbacks.
Fix: the StopDeployment handler's event-log emission in the appconfig service — attribute events it creates on behalf of the caller to USER.
Traces: appconfig.stop-deployment-during-bake-reverts-to-the-previous-version.

Cause 5 of 7: StartDeployment enforces LatestDeploymentNumber as an optimistic lock and refuses calls AWS accepts.
AWS: Smithy StartDeployment.LatestDeploymentNumber targets Integer and is OPTIONAL (smithy.api#default null, not @required); it is not echoed on StartDeployment's or GetDeployment's output. Recorded behavior: the value is not validated server-side — a current value, a one-deployment-stale value, 0, and an impossibly large value each start a genuine new deployment that reaches COMPLETE and the data plane; nothing is refused.
Emulator: raises ConflictException with Message "LatestDeploymentNumber <supplied> does not match the latest deployment number <actual> of this environment." — it implements the documented "start from the expected state" guidance as a server-side precondition.
Why: this is over-strictness, the dangerous direction: a client whose cached deployment number has drifted (any concurrent deployment, any retry) is refused a deployment real AWS performs, so deployment pipelines that pass the field as advisory metadata break against the emulator and only against the emulator.
Fix: the StartDeployment handler in the appconfig service — accept LatestDeploymentNumber unconditionally (it is advisory in this partition) instead of comparing it to the environment's latest deployment.
Traces: appconfig.start-deployment-latest-deployment-number-is-not-enforced.

Cause 6 of 7: GetExperimentRun omits ExposurePercentage when the run's exposure is zero.
AWS: Smithy ExperimentRun.ExposurePercentage targets NullablePercentage (float, @range 0.0-100.0). Recorded behavior: a run started without an ExposurePercentage comes back with ExposurePercentage present and equal to 0.0 on Start/Get/Update/Stop — AWS materialises the zero exposure rather than dropping the member.
Emulator: omits the member entirely, so a client reading the run raises KeyError/None where AWS guarantees a number in [0,100].
Why: an absent member reads as "exposure unknown" rather than "0% exposed"; code that branches on exposure, or that read-modify-writes a run, cannot act on it.
Fix: the ExperimentRun serializer in the appconfig service — emit ExposurePercentage, defaulting to 0.0.
Related open issue (does not cover this check): 20261029T190217Z-appconfig-experiment-run-responses-snapshot-treatment-keys-upper.md carries this cause for its WIRE fixture keys only.
Traces: appconfig.experiment-definition-run-observe-events.

Cause 7 of 7: service-generated treatment and control keys are upper-cased.
AWS: Smithy TreatmentInput (the write shape) has no Key member, so the keys are generated server-side; Treatment.Key targets TreatmentKey (@pattern ^[a-zA-Z0-9]{1,8}$). Recorded/documented behavior: AppConfig assigns "c" to the control and "t1", "t2", ... to the treatments in declaration order, lower-case, both in the CreateExperimentDefinition response and in the ExperimentDefinitionSnapshot of a started run.
Emulator: assigns "C" and "T1"/"T2" and carries that casing into the run snapshot.
Why: those keys are the identifiers a client uses to express TreatmentOverrides and to interpret which variant a user was served; a client that echoes back the keys AWS would have given it ('c'/'t1') does not match the emulator's state, and key comparison is case-sensitive.
Fix: the experiment-definition key generation in the appconfig service (and the snapshot that copies it).
Related open issues (do not cover this check): 20261029T190217Z-appconfig-experiment-definition-responses-service-generated-treatment-keys.md and 20261029T190217Z-appconfig-experiment-run-responses-snapshot-treatment-keys-upper.md carry this cause for their WIRE fixture keys only.
Traces: appconfig.experiment-run-treatment-assignment-enforced-serverside.

## Evidence
Cause 1 (appconfig.configuration-profile-update-immutable-type-rejected): hand-signed PATCH /applications/<appId>/configurationprofiles/<cpId> with body {"Description": "post-smuggle", "Type": "AWS.AppConfig.FeatureFlags"}. Expected HTTP 400 with a Type-specific Message ('Changing the Type of this configuration profile is not supported.') and NO field applied. Observed HTTP 200 with body {'ApplicationId': ..., 'Id': ..., 'Name': 'cp-immutable', 'Description': 'post-smuggle', 'LocationUri': 'hosted', 'Type': 'AWS.Freeform'} — the Description write landed.

Cause 2 (appconfig.stop-deployment-then-list-deployments-state): StopDeployment response $.State expected 'ROLLED_BACK', got 'ROLLING_BACK'.

Cause 3 (appconfig.op-contract-stopDeployment): after the rollback reached a terminal state, $.CompletedAt expected absent, got a timestamp ('a rolled-back deployment must not report a completion time').

Cause 4 (appconfig.stop-deployment-during-bake-reverts-to-the-previous-version): the environment has no Monitors and the caller issued the stop; EventLog entry $.TriggeredBy expected 'USER', got 'APPCONFIG'.

Cause 5 (appconfig.start-deployment-latest-deployment-number-is-not-enforced): StartDeployment with LatestDeploymentNumber naming a value one deployment stale raised ConflictException — 'An error occurred (ConflictException) when calling the StartDeployment operation: LatestDeploymentNumber 1 does not match the latest deployment number 2 of this environment.' AWS accepts this call and starts a new deployment (measured for a current, stale, 0 and 999999999 value). The recorded 1/2 are per-run deployment numbers; the requirement is that NO value is refused.

Cause 6 (appconfig.experiment-definition-run-observe-events): GetExperimentRun on a RUNNING run started with no ExposurePercentage — KeyError 'ExposurePercentage'; expected the member present with value 0.0.

Cause 7 (appconfig.experiment-run-treatment-assignment-enforced-serverside): CreateExperimentDefinition with two Treatments and a Control, no Keys supplied — $.Control.Key expected 'c', got 'C' (and Treatments[*].Key 't1'/'t2' expected against 'T1'/'T2').

Smithy support: UpdateConfigurationProfileRequest has no Type member, errors BadRequestException 400 / ResourceNotFoundException 404; StopDeployment success 202, output Deployment, Deployment.State -> DeploymentState [BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED], Deployment.CompletedAt -> Iso8601DateTime @timestampFormat date-time; DeploymentEvent.TriggeredBy -> TriggeredBy [USER, APPCONFIG, CLOUDWATCH_ALARM, INTERNAL_ERROR]; StartDeployment.LatestDeploymentNumber -> Integer, optional (default null), absent from the output shape; ExperimentRun.ExposurePercentage -> NullablePercentage float @range 0.0-100.0; Treatment.Key -> TreatmentKey @pattern ^[a-zA-Z0-9]{1,8}$ and TreatmentInput has no Key member.

## Steps to Reproduce
Cause 1: replay appconfig.configuration-profile-update-immutable-type-rejected. CreateConfigurationProfile(Type='AWS.Freeform'), UpdateConfigurationProfile(Description=...) through the SDK (succeeds), then sign a raw PATCH to the same profile with {"Description": "post-smuggle", "Type": "AWS.AppConfig.FeatureFlags"}. Expected 400 and GetConfigurationProfile unchanged; observed 200 with the Description applied.

Cause 2: replay appconfig.stop-deployment-then-list-deployments-state. Start a deployment with a non-zero duration, call StopDeployment and read $.State of the 202 response. Expected 'ROLLED_BACK', observed 'ROLLING_BACK'.

Cause 3: replay appconfig.op-contract-stopDeployment. Stop an in-flight deployment, poll GetDeployment to its terminal state and inspect CompletedAt. Expected absent on a rolled-back deployment.

Cause 4: replay appconfig.stop-deployment-during-bake-reverts-to-the-previous-version. Deploy with a bake time to an environment with NO Monitors, StopDeployment during the bake, then read the deployment's EventLog. Every rollback event must carry TriggeredBy 'USER'.

Cause 5: replay appconfig.start-deployment-latest-deployment-number-is-not-enforced. Start two deployments (so the latest number advances), then call StartDeployment with LatestDeploymentNumber set to the previous number (and again with 0 and with 999999999). Each must be accepted and create a new deployment; observed ConflictException.

Cause 6: replay appconfig.experiment-definition-run-observe-events. CreateExperimentDefinition, StartExperimentRun with no ExposurePercentage, then GetExperimentRun. ExposurePercentage must be present and 0.0.

Cause 7: replay appconfig.experiment-run-treatment-assignment-enforced-serverside. CreateExperimentDefinition with Treatments=[{Weight,FlagValue},{Weight,FlagValue}] and Control={Weight,FlagValue} (no Keys), then StartExperimentRun and GetExperimentRun. Control.Key must be 'c' and Treatments keys 't1','t2', in the definition response and in the run's ExperimentDefinitionSnapshot.

Re-replay all check ids listed under trace_ids to confirm the fix.

Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__appconfigdata.renamed-applications-old-name-keeps-resolving-to-its-original-owner-after-reassignment
Issue-Key: appconfig__appconfigdata.renamed-applications-old-name-keeps-resolving-to-its-original-owner-after-reassignment|run#28|AssertionError|assert still_old == ("ok", _V1_BYTES), (
Issue-Commit: e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29

# AppConfigData StartConfigurationSession re-resolves identifiers against live control-plane names, so an application's old name stops working the moment UpdateApplication renames it

> _Note: this issue was identified against the code base at commit `e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartConfigurationSession

## Description
One root cause in the bootstrapped appconfigdata data plane, grounded in one replayed behavioral check. Cause 1 of 1.

Cause 1 of 1: a session identifier given as an application NAME is resolved live against current control-plane state on every StartConfigurationSession, so renaming the application in the control plane immediately invalidates the old name at the data plane.
AWS: Smithy StartConfigurationSession — POST /configurationsessions, success 201; ApplicationIdentifier, EnvironmentIdentifier and ConfigurationProfileIdentifier all target Identifier (@length 1..128, @required) and each is documented as "the ID or the NAME" of its level; errors BadRequestException (400), ResourceNotFoundException (404), ThrottlingException (429), InternalServerException (500). The model says nothing about rename semantics, so this is recorded behavior only: after UpdateApplication (PATCH /applications/{ApplicationId}, 200) changes an application's Name — the id is unchanged and GetApplication reads back the new name — AppConfigData keeps serving the application under BOTH names. A session opened with the NEW name serves the deployed bytes; a session opened with the triple of IDs serves the deployed bytes; and a session opened with the freed OLD name still serves THAT application's deployed bytes. The binding is pinned to the application that held the name, not to whatever the control plane says owns the name now: when a second, unrelated application later claims the freed name and deploys its own configuration, the old name scoped to that new owner's own environment and profile names is refused ResourceNotFoundException with Message "ConfigurationProfile not found" rather than serving the new owner's bytes.
Emulator: immediately after the rename — with no second application in existence — StartConfigurationSession(ApplicationIdentifier=<old name>, EnvironmentIdentifier=<env name>, ConfigurationProfileIdentifier=<profile name>) is refused 404 ResourceNotFoundException with Message "Application not found". The handler calls `crate::state::resolve_deployed_triple(...)`, which resolves each identifier against the control plane's CURRENT records, so a name that no application currently carries is simply unresolvable. The positive controls in the same check (new name, and the id triple) pass, which isolates the defect to stale-name resolution.
Why: an application name is a documented session identifier, so long-lived consumers are routinely configured with names rather than ids. On AWS an operator renaming an application in the control plane does not interrupt them; against this emulator every such consumer's next StartConfigurationSession fails 404 and its poll loop stops, which makes the emulator unusable for testing rename/rollout procedures and teaches callers that a rename is a breaking change when it is not. Hypothesis (the check never reached this assertion, because it failed on the freed-name read above): the same live-resolution design also inverts the second half of AWS's behavior — once a reclaimed name does resolve to a new owner, the emulator would serve the NEW owner's deployed bytes under a name AWS refuses, silently cross-wiring one team's configuration into another's consumer.
Fix: the appconfigdata session-resolution layer (the identifier resolution `StartConfigurationSession` performs, and the shared `resolve_deployed_triple` helper it calls) must not re-resolve names against live control-plane names. It needs a name binding owned by appconfigdata state: the first time a name resolves it is recorded against the application/environment/profile id it resolved to, and thereafter that binding answers the name even after the control plane renames or reassigns it — including keeping the name refused for a different owner that later claims it.
Related, NOT covering: open issue 20261029T190019Z-appconfigdata-getlatestconfiguration-re-resolves-the-configuration-profile-on.md is the same FAMILY of defect (live re-resolution on the data plane) but covers GetLatestConfiguration re-resolving a configuration PROFILE on every poll so a profile deletion kills an already-active session; it carries no key for StartConfigurationSession name resolution after a rename and does not list this check id. Open issue 20261024T112615Z-appconfigdata-data-plane-deployed-configuration-never-served-sessions.md likewise does not carry it.
Traces: appconfig__appconfigdata.renamed-applications-old-name-keeps-resolving-to-its-original-owner-after-reassignment.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the booted emulator (45/51 passed; bootstrap_signal=false). Verdict `mismatch`; the check is not marked ALREADY FILED or OWNED.

check_appconfig__appconfigdata_renamed_applications_old_name_keeps_resolving_to_its_original_owner_after_reassignment.py line 169, `assert still_old == ("ok", _V1_BYTES)`:
  AssertionError: the freed old name must still resolve to the original application's bytes, got ('error', ('ResourceNotFoundException', 'Application not found'))

What passed before that point, which bounds the cause:
  - the deployment reached State COMPLETE and the ORIGINAL name served the deployed bytes (positive control);
  - UpdateApplication(ApplicationId=app, Name=new) returned the same Id with the new Name, and GetApplication read back the new Name;
  - a session opened with the NEW name served the deployed bytes;
  - a session opened with the id triple (app_id, env_id, cp_id) served the deployed bytes.
So only the freed OLD name diverges: expected a 201 session whose GetLatestConfiguration returns the deployed bytes, observed 404 ResourceNotFoundException with Message 'Application not found'.

Not reached (the check aborted at the assertion above), recorded by the same check as AWS's behavior for the reassignment half: after a second application claims the freed name and deploys its own configuration, StartConfigurationSession(old name, second env name, second profile name) must be refused ResourceNotFoundException with Message 'ConfigurationProfile not found'.

Emulator source: services/service-com-amazonaws-appconfigdata-appconfigdata/src/ops/start_configuration_session.rs lines 196-227 — `crate::state::resolve_deployed_triple(&*self.engine, &input.application_identifier, &input.environment_identifier, &input.configuration_profile_identifier)` is consulted on every mint and its `Err(unresolved)` is returned verbatim as ResourceNotFoundException{message, resource_type, referenced_by}; the resolution reads current control-plane records, so no historical name binding exists.

The generated names and ids in the recording are per-run values and are not part of any requirement; the requirements are the 201-with-token / deployed-bytes outcome for the old name, and the 404 ResourceNotFoundException 'ConfigurationProfile not found' outcome for the reclaimed name under its new owner.

Smithy support: StartConfigurationSession POST /configurationsessions code 201; ApplicationIdentifier/EnvironmentIdentifier/ConfigurationProfileIdentifier -> Identifier @length 1..128 @required; RequiredMinimumPollIntervalInSeconds @range 15..86400; errors BadRequestException 400 / ResourceNotFoundException 404 / ThrottlingException 429 / InternalServerException 500. UpdateApplication PATCH /applications/{ApplicationId} code 200, output Id + Name, errors BadRequestException 400 / ResourceNotFoundException 404 / InternalServerException 500.

## Steps to Reproduce
Boot the emulator and point appconfig and appconfigdata clients at its data listener (region/credentials irrelevant). Replay appconfig__appconfigdata.renamed-applications-old-name-keeps-resolving-to-its-original-owner-after-reassignment, or by hand:

1. create_application(Name=OLD) -> app_id; create_environment(ApplicationId=app_id, Name=ENV) -> env_id; create_configuration_profile(ApplicationId=app_id, Name=CP, LocationUri='hosted', Type='AWS.Freeform') -> cp_id.
2. create_deployment_strategy(DeploymentDurationInMinutes=0, GrowthFactor=100, GrowthType='LINEAR', FinalBakeTimeInMinutes=0, ReplicateTo='NONE') -> ds_id; create_hosted_configuration_version(Content=V1) -> version 1; start_deployment(...ConfigurationVersion='1') and poll get_deployment until State is terminal — it must be COMPLETE.
3. Positive control: appconfigdata.start_configuration_session(ApplicationIdentifier=OLD, EnvironmentIdentifier=ENV, ConfigurationProfileIdentifier=CP) then get_latest_configuration -> V1.
4. appconfig.update_application(ApplicationId=app_id, Name=NEW); get_application(ApplicationId=app_id)['Name'] == NEW.
5. Sessions by NEW name and by the (app_id, env_id, cp_id) triple must both still serve V1 (both pass today).
6. THE DIVERGENCE: start_configuration_session(ApplicationIdentifier=OLD, EnvironmentIdentifier=ENV, ConfigurationProfileIdentifier=CP). Expected: 201 with an InitialConfigurationToken whose GetLatestConfiguration returns V1. Observed: 404 ResourceNotFoundException, Message 'Application not found'.
7. The reassignment half, to be checked after the fix: create_application(Name=OLD) -> second_id (must be a NEW id), create_environment(Name=ENV2), create_configuration_profile(Name=CP2, LocationUri='hosted'), create_hosted_configuration_version(Content=V2), start_deployment with the same strategy and poll to COMPLETE. Then start_configuration_session(ApplicationIdentifier=OLD, EnvironmentIdentifier=ENV2, ConfigurationProfileIdentifier=CP2) must be refused 404 ResourceNotFoundException with Message 'ConfigurationProfile not found' — it must NOT serve V2 — while step 6's session on (OLD, ENV, CP) must still serve V1.

Re-replay the check id in trace_ids to confirm the fix.

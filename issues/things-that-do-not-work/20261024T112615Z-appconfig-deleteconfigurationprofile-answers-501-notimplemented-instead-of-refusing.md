Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__appconfigdata.configuration-profile-delete-before-its-versions-rejected appconfig__appconfigdata.duplicate-application-name-rejected
Issue-Key: appconfig__appconfigdata.configuration-profile-delete-before-its-versions-rejected|run#15|AssertionError|assert code == "BadRequestException", (
Issue-Key: appconfig__appconfigdata|ListApplications|NotImplemented
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# AppConfig DeleteConfigurationProfile answers 501 NotImplemented instead of refusing a profile that still has hosted versions, and ListApplications stub blocks the duplicate-name check (2 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConfigurationProfile

## Description
Two root causes in the bootstrapped appconfig control plane, both grounded in replayed behavioral checks whose ids carry the combined group token 'appconfig__appconfigdata'; the operations themselves are appconfig operations, so the issue is filed under appconfig. They are numbered Cause 1 of 2 and Cause 2 of 2; close BOTH before submitting and re-replay both check ids in trace_ids.

=== Cause 1 of 2: DeleteConfigurationProfile is an unimplemented scaffold stub, so the 'children first' refusal never happens ===
AWS: DeleteConfigurationProfile (DELETE /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}, modeled success 204) refuses to delete a configuration profile that still has hosted configuration versions beneath it - BadRequestException, HTTP 400 (Smithy, appconfig model: DeleteConfigurationProfile errors BadRequestException httpError 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404; recorded message example 'Cannot delete configuration profile <profile-id> because there are still hosted configuration versions existing under it.' - the status and error code are the requirement, the sentence is an example). The refusal must change nothing: the profile stays describable, its hosted versions stay listable, and the deployed configuration keeps being served. Once every hosted version is deleted the same call succeeds (204) and GetConfigurationProfile then answers ResourceNotFoundException/404.
Emulator: the call is refused with error code 'NotImplemented' (HTTP 501, 'operation DeleteConfigurationProfile is scaffolded but not implemented in this emulator'). The handler services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/delete_configuration_profile.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED:' marker and returns Scaffolded::not_implemented, so neither the refusal nor the successful delete exists. No open issue names DeleteConfigurationProfile as a stub (the open issue 20261023T145538Z-appconfig-deletes-of-application-configuration-profile-and-hosted.md describes resources staying readable AFTER a delete returns, which is a different, later observation and carries no behavioral replay key for this check).
Why: a client gets a server-fault 501, which SDKs do not treat as a client error, instead of the 400 that tells it to remove the hosted versions first; and because nothing is ever deleted, delete/recreate and deletion-protection flows cannot be exercised at all.
Fix: implement the operation in its handler file - validate that no hosted configuration versions remain for the profile and refuse with the modeled BadRequestException (400) when they do, otherwise remove the profile from appconfig state and answer the modeled 204 - then delete that file's SCAFFOLDED marker line. A 501 is correct for a stub, so implement the operation rather than changing the stub's error.
Traces: appconfig__appconfigdata.configuration-profile-delete-before-its-versions-rejected.

=== Cause 2 of 2: the ListApplications scaffold stub blocks the duplicate-application-name check ===
AWS: ListApplications (GET /applications) returns the account's applications, and CreateApplication with a name that already exists is accepted by AppConfig (names are not unique), which is exactly what the check enumerates to assert.
Emulator: ListApplications answers HTTP 501 'NotImplemented' ('operation ListApplications is scaffolded but not implemented in this emulator'), so the check errors before it can assert anything about duplicate names. The open issue 20261023T032202Z-appconfig-listapplications-stub.md already asks for this operation to be implemented, but its replay list names only appconfig.list-applications-* checks and does NOT carry this check id, so this cause binds the remaining blocked check to that same fix.
Why: anything that enumerates applications - a duplicate-name assertion, a create/delete reflection test, pagination - gets a server fault instead of a list.
Fix: the same handler as that open issue, services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/list_applications.rs; when it is implemented, re-replay this check id too (the duplicate-name behavior past that call is still unverified, because the emulator never got there).
Traces: appconfig__appconfigdata.duplicate-application-name-rejected.

## Evidence
Behavioral replay of suite simparity-behavioral-ga-prod-v1 against the booted emulator (13 checks, 0 passed, 4 mismatched, 9 errored). Neither failing check was marked ALREADY FILED or OWNED by an open issue.

Cause 1 - appconfig__appconfigdata.configuration-profile-delete-before-its-versions-rejected [mismatch]:
  File "check_appconfig__appconfigdata_configuration_profile_delete_before_its_versions_rejected.py", line 248, in run
    assert code == "BadRequestException"
  AssertionError: deleting a profile that still has hosted versions must be refused BadRequestException, got 'NotImplemented'
  Everything before that call succeeded in the same check: the application, environment and profile were created, two hosted configuration versions were created, the deployment reached State 'COMPLETE' and list_hosted_configuration_versions returned exactly those two versions. Confirmed in the checkout: services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/delete_configuration_profile.rs line 4 carries '//! SCAFFOLDED, NOT IMPLEMENTED:' and handle() ends in Err(crate::scaffolded::Scaffolded::not_implemented("DeleteConfigurationProfile")); the same file documents the modeled errors (BadRequestException 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404) and SUCCESS_STATUS 204.

Cause 2 - appconfig__appconfigdata.duplicate-application-name-rejected [error]:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListApplications operation: operation ListApplications is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.

Cause 1: create_application -> create_environment -> create_configuration_profile(LocationUri='hosted', Type='AWS.Freeform') -> create_hosted_configuration_version twice -> create_deployment_strategy(0 minutes, GrowthFactor 100, 0 bake, ReplicateTo='NONE') and start_deployment of version 1, waiting for State 'COMPLETE' -> delete_configuration_profile(ApplicationId=<app>, ConfigurationProfileId=<profile>). Expected BadRequestException / HTTP 400 while hosted versions remain, with the profile and both versions untouched afterwards; the emulator answers HTTP 501 with error code 'NotImplemented'. Then delete both hosted versions and repeat the call: expected 204, after which get_configuration_profile raises ResourceNotFoundException/404. Re-replay appconfig__appconfigdata.configuration-profile-delete-before-its-versions-rejected.

Cause 2: call list_applications() (or paginate it). Expected a 200 with the account's applications; the emulator answers 501 'NotImplemented'. Re-replay appconfig__appconfigdata.duplicate-application-name-rejected once ListApplications is implemented.

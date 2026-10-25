Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-application-crud-lifecycle
Issue-Step: teardown#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-application-crud-lifecycle appconfig-experiment-run-full-lifecycle appconfig-resource-tagging-workflow appconfig-application-list-reflects-create-and-delete appconfig-extension-and-association-lifecycle appconfig-configuration-profile-and-hosted-version-roundtrip appconfig-end-to-end-deployment-workflow
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|GetApplication|4xx->2xx|$
Issue-Key: appconfig|GetApplication|4xx->2xx|$.Message
Issue-Key: appconfig|GetApplication|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appconfig|GetApplication|4xx->2xx|status
Issue-Key: appconfig|GetHostedConfigurationVersion|4xx->2xx|$
Issue-Key: appconfig|GetHostedConfigurationVersion|4xx->2xx|status
Issue-Key: appconfig|GetConfigurationProfile|4xx->2xx|$
Issue-Key: appconfig|GetConfigurationProfile|4xx->2xx|status
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# AppConfig deletes of application, configuration profile and hosted version leave the resource readable

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApplication

## Description
After a successful Delete* call the emulator keeps serving the deleted resource with HTTP 200 instead of refusing with ResourceNotFoundException/404, so the delete is not observable in state. Five replayed clusters across three resource kinds. Numbered; close ALL FIVE before submitting.

=== Cause 1 of 5: GetApplication still 200 after DeleteApplication (polled) ===
AWS: a deleted application is gone — GetApplication answers ResourceNotFoundException with HTTP 404 and header x-amzn-errortype naming it (Smithy: GetApplication errors include ResourceNotFoundException httpError 404, members Message+ResourceName; recorded message example 'Application with Id <id> could not be found for account <account>.' — the status and error code are the requirement, the sentence is an example).
Emulator: keeps answering 200 with the full application document for 15 poll attempts, with no Message and no x-amzn-errortype header.
Why: a client that deletes and then confirms the deletion never converges; a CRUD lifecycle leaves stale state behind, and a retried create against the same name collides against a resource AWS no longer has.
Fix: DeleteApplication's handler/state must remove the application record (and cascade to its child environments/profiles), and GetApplication must miss on it.
Traces: appconfig-application-crud-lifecycle#teardown#6 (cluster of 3).

=== Cause 2 of 5: GetApplication still 200 after DeleteApplication (single shot) ===
AWS: as cause 1 — 404 ResourceNotFoundException immediately after the delete returns.
Emulator: 200 with the application document, no error body or errortype header.
Why: the same stale-state defect observed without polling, so even a non-racy client sees the deleted application.
Fix: as cause 1.
Traces: appconfig-application-list-reflects-create-and-delete#task#6 (cluster of 2).

=== Cause 3 of 5: GetHostedConfigurationVersion still 200 after the version was deleted ===
AWS: a deleted hosted configuration version is gone — GetHostedConfigurationVersion answers 404 ResourceNotFoundException (Smithy: ResourceNotFoundException httpError 404 on that operation).
Emulator: answers 200 with the stored content (observed body {'greeting': 'hello'}) for 15 poll attempts. Likely downstream of the open issue 20261023T032202Z-appconfig-deletehostedconfigurationversion-stub.md (DeleteHostedConfigurationVersion is an unimplemented scaffold), but that issue carries no wire replay key, so this step is filed here to bind it.
Why: configuration content AWS has destroyed is still servable, so a deployment or a GetConfiguration in the emulator can serve a version that no longer exists in production.
Fix: implement DeleteHostedConfigurationVersion against hosted-version state and make the Get path miss on the removed version.
Traces: appconfig-configuration-profile-and-hosted-version-roundtrip#teardown#10 (cluster of 2).

=== Cause 4 of 5: GetConfigurationProfile still 200 after DeleteConfigurationProfile ===
AWS: 404 ResourceNotFoundException for a deleted configuration profile.
Emulator: 200 with the profile document (observed Id/Name/LocationUri 'hosted'/Type 'AWS.Freeform') across 15 poll attempts.
Why: the profile remains listable and deployable in the emulator, and deletion-protection or recreate flows cannot be tested at all.
Fix: DeleteConfigurationProfile must remove the profile (and its hosted versions) from appconfig state.
Traces: appconfig-configuration-profile-and-hosted-version-roundtrip#teardown#12 (cluster of 2).

=== Cause 5 of 5: GetApplication still 200 after the whole fixture's teardown delete ===
AWS: 404 ResourceNotFoundException once the application is deleted.
Emulator: 200 with {'Id':..., 'Name':...} across 15 poll attempts — a distinct cluster from causes 1-2 (different asserted shape), same underlying gap.
Why: as cause 1; worth re-running separately because it is the last step of a nested teardown and confirms no cascade happens either.
Fix: as cause 1.
Traces: appconfig-configuration-profile-and-hosted-version-roundtrip#teardown#14 (cluster of 2).

## Evidence
Wire replay (simparity-wire-aws-v1), all appconfig:

Cause 1 — appconfig-application-crud-lifecycle#teardown#6 GetApplication: status expected 404, got 200; $.Message missing (expected 'Application with Id ho65pb1 could not be found for account 000000000000.'); $headers.x-amzn-errortype missing; poll: status 200 not in [404] after 15 attempts, body {'Id': 'ho65pb1', 'Name': 'sim2real-probe-appconfig-app-5271518f', 'Description': 'updated description'}. Also appconfig-experiment-run-full-lifecycle#teardown#23, appconfig-resource-tagging-workflow#teardown#9.

Cause 2 — appconfig-application-list-reflects-create-and-delete#task#6 GetApplication: status expected 404, got 200; $.Message missing; $headers.x-amzn-errortype missing. Also appconfig-extension-and-association-lifecycle#teardown#15.

Cause 3 — appconfig-configuration-profile-and-hosted-version-roundtrip#teardown#10 GetHostedConfigurationVersion: status expected 404, got 200; poll: status 200 not in [404] after 15 attempts, body {'greeting': 'hello'}. Also appconfig-end-to-end-deployment-workflow#teardown#13.

Cause 4 — appconfig-configuration-profile-and-hosted-version-roundtrip#teardown#12 GetConfigurationProfile: status expected 404, got 200; poll body {'ApplicationId': 'ho65pb1', 'Id': 'algcbw8', 'Name': 'sim2real-probe-appconfig-profile-81d7a5f5', 'Description': 'new description', 'LocationUri': 'hosted', 'Type': 'AWS.Freeform'}. Also appconfig-end-to-end-deployment-workflow#teardown#15.

Cause 5 — appconfig-configuration-profile-and-hosted-version-roundtrip#teardown#14 GetApplication: status expected 404, got 200; poll body {'Id': 'ho65pb1', 'Name': 'sim2real-probe-appconfig-app-81d7a5f5'}. Also appconfig-end-to-end-deployment-workflow#teardown#19.

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.
Causes 1/2/5: create_application -> delete_application(ApplicationId=<id>) -> get_application(ApplicationId=<id>). Must raise ResourceNotFoundException (404); the emulator returns 200 with the document.
Cause 3: create_application -> create_configuration_profile(LocationUri='hosted') -> create_hosted_configuration_version -> delete_hosted_configuration_version(VersionNumber=<n>) -> get_hosted_configuration_version(VersionNumber=<n>). Must be 404.
Cause 4: same setup -> delete_configuration_profile -> get_configuration_profile. Must be 404.
Re-replay appconfig-application-crud-lifecycle#teardown#6 plus every step in trace_ids.

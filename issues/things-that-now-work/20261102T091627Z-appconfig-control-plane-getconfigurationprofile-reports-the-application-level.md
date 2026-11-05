Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__appconfigdata.configuration-profile-identifier-not-found-rejected appconfig__appconfigdata.identical-hosted-content-reuses-its-version-number
Issue-Key: appconfig__appconfigdata.configuration-profile-identifier-not-found-rejected|run#15|AssertionError|assert message == expected_app_missing, (
Issue-Key: appconfig__appconfigdata.identical-hosted-content-reuses-its-version-number|run#9|AssertionError|assert again == first, (
Issue-Commit: e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29

# AppConfig control plane: GetConfigurationProfile reports the application-level not-found message, and CreateHostedConfigurationVersion does not reuse the version number of byte-identical content (2 causes)

> _Note: this issue was identified against the code base at commit `e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedConfigurationVersion

## Description
Two distinct root causes in the bootstrapped appconfig control plane, each grounded in one replayed behavioral check. Both are numbered below; close BOTH before submitting this issue. (The check ids carry the composite `appconfig__appconfigdata` prefix because the fixtures also drive the data plane, but the diverging responses in both causes are appconfig's own.)

Cause 1 of 2: GetConfigurationProfile answers the APPLICATION not-found message when the ApplicationId does not resolve, instead of AppConfig's single composite Configuration-Profile message.
AWS: Smithy GetConfigurationProfile — GET /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}, success 200, errors BadRequestException (400), ResourceNotFoundException (404), InternalServerException (500); ApplicationId targets Name (@length 1..64) and ConfigurationProfileId targets LongName (@length 1..128), both @httpLabel @required. Recorded behavior: whichever of the two path labels fails to resolve, the refusal is ResourceNotFoundException / HTTP 404 carrying ONE composite message — "Configuration Profile with Application Id <ApplicationId> and Configuration Profile Id <ConfigurationProfileId> could not be found for account <accountId>." FIXED text: "Configuration Profile with Application Id " / " and Configuration Profile Id " / " could not be found for account " / ".". VARIABLE parts: the two identifiers exactly as supplied on the request path, and the caller's account id from state. The check asserts that identical template twice — once for a never-created profile id under a real application (which the emulator now satisfies) and once for a real profile id read through an application id that does not exist — so the requirement is precisely that moving the broken member from the profile to the application does NOT change which lookup is reported missing.
Emulator: the status (404) and the error class (ResourceNotFoundException) are already right, but the message is the application-level form "Application with Id <ApplicationId> could not be found for account <accountId>." The handler resolves the application first and short-circuits with `crate::state::application_not_found(...)`, never reaching the profile lookup that builds the composite message.
Why: AppConfig deliberately reports one not-found subject for this operation; a client, contract test or operator tool that parses or asserts the refusal text sees a message AppConfig never sends for GetConfigurationProfile, and the emulator additionally discloses WHICH of the two identifiers was unresolvable where AWS does not.
Fix: the GetConfigurationProfile handler's lookup path in the appconfig service — the application-miss branch must emit the configuration-profile not-found message (both request identifiers plus the caller account), the same text the profile-miss branch already emits. See the file/line citation under Traces.
Related, NOT covering: the resolved issue 20261023T032202Z-appconfig-control-plane-fidelity-settings-route-missing-environmentstate.md (Cause 3) established this composite template for the PROFILE-miss variant and for application/hosted-version lookups; the unknown-APPLICATION variant of GetConfigurationProfile is not in its replay set and still diverges. Open issue 20261029T195212Z-appconfig-control-plane-refusals-wrong-error-classes-and.md also does not carry this check id.
Traces: appconfig__appconfigdata.configuration-profile-identifier-not-found-rejected.

Cause 2 of 2: CreateHostedConfigurationVersion allocates a new version number for content byte-identical to the profile's newest stored version, instead of reusing that version.
AWS: Smithy CreateHostedConfigurationVersion — POST /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}/hostedconfigurationversions, success 201, Content is the @httpPayload blob and VersionNumber is returned in the Version-Number header; errors BadRequestException (400), ConflictException (409), PayloadTooLargeException (413), ResourceNotFoundException (404), ServiceQuotaExceededException (402), InternalServerException (500). The model says nothing about de-duplication, so this is recorded behavior only: posting Content that is byte-for-byte equal to the newest version the profile already holds returns THAT version's number and stores no new version (ListHostedConfigurationVersions still reports exactly the one version, and GetHostedConfigurationVersion on it still returns the original bytes), while content differing by even one byte allocates the next number. The reused version is a real, deployable version: deploying it serves those bytes.
Emulator: every call allocates `profile.next_version_number` unconditionally, so the repeated identical write returns VersionNumber 2 and the profile holds two versions. There is no content comparison anywhere on the write path.
Why: any idempotent estate build (CloudFormation/Terraform/deploy script that re-posts the same configuration) doubles a profile's version history on every re-run, so version numbers drift from what AWS would have assigned and the per-profile hosted-version quota is consumed where AWS consumes none; and a client that re-posts known bytes to learn "which version is this content" is told a version number that does not exist on AWS.
Fix: the CreateHostedConfigurationVersion handler's version-allocation step in the appconfig service — before allocating, compare the bytes about to be STORED (i.e. after the feature-flags helper's stamping and after any PRE_CREATE_HOSTED_CONFIGURATION_VERSION replacement, since those are the bytes that get persisted) against the profile's latest stored version; when equal, answer with that existing version's number, bytes and metadata and leave `next_version_number`, the stored map and the byte reservation untouched.
Traces: appconfig__appconfigdata.identical-hosted-content-reuses-its-version-number.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the booted emulator (45/51 passed; bootstrap_signal=false). Both checks below are verdict `mismatch` and neither is marked ALREADY FILED or OWNED.

Cause 1 — appconfig__appconfigdata.configuration-profile-identifier-not-found-rejected, check_appconfig__appconfigdata_configuration_profile_identifier_not_found_rejected.py line 156, `assert message == expected_app_missing`:
  AssertionError: a real profile read through a nonexistent application reads 'Application with Id sim2real-scn-app-absent-9f565c9d9234 could not be found for account 000000000000.', expected 'Configuration Profile with Application Id sim2real-scn-app-absent-9f565c9d9234 and Configuration Profile Id algcbw8 could not be found for account 000000000000.'
  The error Code assertion (ResourceNotFoundException) PASSED, and the preceding assertion for a never-created profile id under a REAL application also passed, so the only divergence is which not-found template the application-miss branch uses. The application name, the profile id 'algcbw8' and the account '000000000000' are this run's values and fill the template; they are not part of it.
  Emulator source: services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/get_configuration_profile.rs lines 174-184 return `crate::state::application_not_found(&input.application_id)`; lines 185-196 hold the correct `crate::state::configuration_profile_not_found(&input.application_id, &input.configuration_profile_id)` that is never reached when the application is missing.

Cause 2 — appconfig__appconfigdata.identical-hosted-content-reuses-its-version-number, check_appconfig__appconfigdata_identical_hosted_content_reuses_its_version_number.py line 205, `assert again == first`:
  AssertionError: byte-identical content must reuse version 1 rather than allocating a new one — an emulator that appends doubles the history on every re-run of the estate build; got 2
  The check's first CreateHostedConfigurationVersion returned VersionNumber 1 (correct); the second, with identical Content and ContentType, returned 2 where AWS returns 1. The check's differential barrier (one byte different must allocate 2) and the follow-on assertions (ListHostedConfigurationVersions == [1] after the repeat, and the deployment of version 1 serving those bytes) were never reached.
  Emulator source: services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/create_hosted_configuration_version.rs lines 634-635 do `let version_number = profile.next_version_number; profile.next_version_number += 1;` with no comparison against `profile.hosted_versions`' newest entry.

Smithy support: GetConfigurationProfile GET /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId} code 200, errors BadRequestException 400 / ResourceNotFoundException 404 / InternalServerException 500. CreateHostedConfigurationVersion POST .../hostedconfigurationversions code 201, Content @httpPayload @required blob, VersionNumber @httpHeader Version-Number, errors BadRequestException 400 / ConflictException 409 / PayloadTooLargeException 413 / ResourceNotFoundException 404 / ServiceQuotaExceededException 402 / InternalServerException 500.

## Steps to Reproduce
Boot the emulator and point an appconfig client at its data listener (region/credentials irrelevant).

Cause 1 — replay appconfig__appconfigdata.configuration-profile-identifier-not-found-rejected:
1. create_application(Name=<unique>) -> app_id.
2. create_configuration_profile(ApplicationId=app_id, Name=<unique>, LocationUri='hosted', Type='AWS.Freeform') -> cp_id.
3. get_configuration_profile(ApplicationId=app_id, ConfigurationProfileId='<never-created id>'). Expected and observed: 404 ResourceNotFoundException with 'Configuration Profile with Application Id <app_id> and Configuration Profile Id <bogus> could not be found for account <account>.' (this one matches today).
4. get_configuration_profile(ApplicationId='<application id that was never created>', ConfigurationProfileId=cp_id). Expected: 404 ResourceNotFoundException with 'Configuration Profile with Application Id <bogus app> and Configuration Profile Id <cp_id> could not be found for account <account>.' Observed: 404 ResourceNotFoundException with 'Application with Id <bogus app> could not be found for account <account>.'
5. list_configuration_profiles(ApplicationId=app_id) must still report exactly [cp_id] (no refusal created anything).

Cause 2 — replay appconfig__appconfigdata.identical-hosted-content-reuses-its-version-number:
1. create_application -> create_environment -> create_configuration_profile(LocationUri='hosted', Type='AWS.Freeform').
2. create_hosted_configuration_version(Content=B, ContentType='application/json') -> VersionNumber must be 1 (matches today).
3. create_hosted_configuration_version with the SAME Content=B and ContentType -> Expected VersionNumber 1; observed 2.
4. list_hosted_configuration_versions -> Expected VersionNumbers [1]; observed [1, 2]. get_hosted_configuration_version(VersionNumber=1) must still return B.
5. create_hosted_configuration_version with Content=B' (one byte different) -> must be VersionNumber 2, and the list must then be [1, 2] (this differential barrier must keep passing after the fix).
6. Deploy ConfigurationVersion='1' with an all-at-once strategy and read it back through appconfigdata StartConfigurationSession + GetLatestConfiguration: the reused version must be the bytes served.

Re-replay both check ids listed in trace_ids to confirm the fix.

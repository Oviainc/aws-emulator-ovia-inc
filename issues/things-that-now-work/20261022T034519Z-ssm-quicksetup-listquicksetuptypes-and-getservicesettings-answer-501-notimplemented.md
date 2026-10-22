Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-quicksetup-catalog-list-quick-setup-types
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-quicksetup-catalog-list-quick-setup-types ssm-quicksetup-service-settings-configure
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup|ListQuickSetupTypes|2xx->5xx|$.QuickSetupTypeList
Issue-Key: ssm-quicksetup|ListQuickSetupTypes|2xx->5xx|status
Issue-Key: ssm-quicksetup|GetServiceSettings|2xx->5xx|status
Issue-Key: ssm-quicksetup|GetServiceSettings|2xx->5xx|$.ServiceSettings
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# ssm-quicksetup ListQuickSetupTypes and GetServiceSettings answer 501 NotImplemented

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListQuickSetupTypes

## Description
Two of the service's account-level read operations are not served at all: they answer HTTP 501 instead of the modeled 200 response. 2 causes; both must be closed.

Cause 1 of 2: ListQuickSetupTypes answers 501 instead of returning the type catalog.
AWS: GET /listQuickSetupTypes takes no input and returns 200 with ListQuickSetupTypesOutput.QuickSetupTypeList, a list of QuickSetupTypeOutput structures each carrying Type and LatestVersion (Smithy: ListQuickSetupTypes input Unit, output ListQuickSetupTypesOutput, http GET /listQuickSetupTypes code 200, @readonly; modeled errors are AccessDeniedException/ConflictException/InternalServerException/ThrottlingException only - NotImplemented is not a modeled outcome). The recording shows a non-empty catalog: an entry per supported Quick Setup type (AWSQuickSetupType-SSM, -PatchPolicy, -SSMHostMgmt, -CFGRecording and so on), each with a LatestVersion string such as '3.0'. The exact membership and version strings are catalog data, not a requirement; a populated QuickSetupTypeList of Type/LatestVersion pairs is.
Emulator: status 501, and $.QuickSetupTypeList is absent from the body entirely.
Why: this is the discovery call a client makes before it can build a ConfigurationDefinition, because ConfigurationDefinitionInput.Type and TypeVersion are chosen from this catalog. A client gets a 501 it cannot interpret (the SDK surfaces it as an unmodeled/unknown error rather than one of the operation's errors), so no configuration can be authored against the emulator at all.
Fix: the ListQuickSetupTypes handler in the ssm-quicksetup service crate - it needs to be implemented and to serialize QuickSetupTypeList from the service's type catalog.
Traces: ssm-quicksetup-catalog-list-quick-setup-types#task#1

Cause 2 of 2: GetServiceSettings answers 501 both before and after UpdateServiceSettings.
AWS: GET /serviceSettings takes no input and returns 200 with GetServiceSettingsOutput.ServiceSettings, a ServiceSettings structure whose ExplorerEnablingRoleArn is an IAMRoleArn (Smithy: GetServiceSettings input Unit, output GetServiceSettingsOutput, http GET /serviceSettings code 200, @readonly; modeled errors AccessDeniedException/ConflictException/InternalServerException/ThrottlingException). The recorded fixture reads settings once before configuring them and once after an UpdateServiceSettings call, and the second read reflects the role ARN that update set - so the operation must read persisted per-account/per-region settings state, not a constant.
Emulator: status 501 on both reads; on the post-update read $.ServiceSettings is missing where the recording carries ServiceSettings.ExplorerEnablingRoleArn set to the role the preceding update supplied.
Why: a client cannot read back the Explorer-enabling role, so it can neither confirm UpdateServiceSettings took effect nor branch on whether Quick Setup is configured in the region; the 501 is not one of the operation's modeled errors, so retry/fallback logic keyed on the modeled error set does not fire.
Fix: the GetServiceSettings handler in the ssm-quicksetup service crate, plus the service-settings state it must read (the same state UpdateServiceSettings writes).
Traces: ssm-quicksetup-service-settings-configure#setup#2 (read before update), ssm-quicksetup-service-settings-configure#task#4 (read after update)

## Evidence
Cause 1 - ssm-quicksetup-catalog-list-quick-setup-types#task#1, ListQuickSetupTypes: expected status 200 with $.QuickSetupTypeList = [{'Type': 'AWSQuickSetupType-CFGCPacks', 'LatestVersion': '3.0'}, ... 14 entries ...]; actual status 501, $.QuickSetupTypeList missing (None).

Cause 2 - ssm-quicksetup-service-settings-configure#setup#2, GetServiceSettings: expected status 200; actual status 501.
Cause 2 - ssm-quicksetup-service-settings-configure#task#4, GetServiceSettings: expected status 200 with $.ServiceSettings = {'ExplorerEnablingRoleArn': '<role arn set by the preceding UpdateServiceSettings>'}; actual status 501, $.ServiceSettings missing (None).

Smithy citations: ListQuickSetupTypes -> ListQuickSetupTypesOutput.QuickSetupTypeList (list of QuickSetupTypeOutput{Type, LatestVersion}); GetServiceSettings -> GetServiceSettingsOutput.ServiceSettings (ServiceSettings{ExplorerEnablingRoleArn: IAMRoleArn}). Neither operation models a NotImplemented/501 outcome.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: GET /listQuickSetupTypes against the ssm-quicksetup endpoint with sigv4 (fixture ssm-quicksetup-catalog-list-quick-setup-types, step task#1). Observe 501 instead of 200 with a populated QuickSetupTypeList.
3. Cause 2: GET /serviceSettings (fixture ssm-quicksetup-service-settings-configure, step setup#2). Observe 501 instead of 200.
4. Cause 2: PUT /serviceSettings with an ExplorerEnablingRoleArn, then GET /serviceSettings again (same fixture, step task#4). Observe 501 and no ServiceSettings member, instead of 200 echoing the role ARN just configured.

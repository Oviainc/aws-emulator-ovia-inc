Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehub.published-app-version-freezes-resource-mapping
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub.published-app-version-freezes-resource-mapping|run#22|KeyError|v["appVersion"]: v["identifier"] for v in rh.list_app_versions(appArn=app_arn)["appVersions"]
Issue-Commit: c4d9e9a843977049f6a1e41e862e9a0efee7c1e6

# resiliencehub ListAppVersions omits AppVersionSummary.identifier, so no listed version (including the draft) carries its version identifier

> _Note: this issue was identified against the code base at commit `c4d9e9a843977049f6a1e41e862e9a0efee7c1e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAppVersions

## Description
One net-new root cause, surfaced by the behavioral suite (resiliencehub 1/11 passed; the other 9 failures are already filed — 4 are OWNED by 20261029T191752Z-resiliencehub-publishappversion-mints-numbered-versions-instead-of-the.md and 5 are the ALREADY FILED unimplemented-StartAppAssessment cluster, 20261030T040357Z-resiliencehub-startappassessment-stub.md).

Related but NOT covering this: the open wire-corpus issue 20261029T195027Z-resiliencehub-success-responses-omit-modeled-output-members-tags.md describes the same omission as its 'Cause 6 of 6' from a single wire step ($.appVersions[0].identifier missing on resiliencehub-assessment-preconditions#task#8). That issue is still open and its replay key is a wire fixture step, not this behavioral check, so this behavioral occurrence is filed here with its own replay key; a fixer closing either must satisfy both.

=== Cause 1 of 1: every AppVersionSummary returned by ListAppVersions is missing `identifier` ===

AWS: ListAppVersions returns the required `appVersions` list of AppVersionSummary. Smithy (com.amazonaws.resiliencehub#AppVersionSummary): members are `appVersion` (EntityVersion, required, pattern ^\S{1,50}$), `identifier` (LongOptional, documented "Identifier of the application version."), `creationTime` (TimeStamp) and `versionName` (EntityVersion); ListAppVersions http POST /list-app-versions, success 200. Recorded behavior: `identifier` is PRESENT on every listed entry — the `draft` entry carries identifier 0, and each published entry carries its monotonically increasing publish sequence number (the same value PublishAppVersion returns in its own `identifier` member, 1 for the first publish, 2 for the second). The member is serialized for all versions, not only published ones, so a client can read it unconditionally and use it as the ordering/correlation key.

Emulator: the member is dropped from the body. `summarise_versions` hard-codes `identifier: None` for the draft summary on the documented rationale that "Only a PUBLISHED version has an identifier", which is not AWS's behavior (AWS reports 0 for the draft). With the member absent, a client indexing the entry by `identifier` gets nothing at all: replay fails with `KeyError: 'identifier'` while building `{v["appVersion"]: v["identifier"] for v in rh.list_app_versions(appArn=app_arn)["appVersions"]}` immediately after a successful publish (PublishAppVersion itself answered appVersion='release', identifier=1, so the publish path is fine — only the list projection diverges). The call is otherwise a clean 200; status and `appVersion` labels match.

Why: `identifier` is the only ordering key a client has over app versions — the version label is an opaque string, and the two aliases (`draft`, `release`) carry no sequence information. Without it a client cannot tell which listed version is newest, cannot correlate the `identifier` a PublishAppVersion response returned with the entry the list reports for it, and cannot distinguish the still-aliased release from a superseded numbered version. Any consumer that reads the field without a null guard (the documented, always-present member) crashes rather than degrading.

Fix: the ListAppVersions handler's AppVersionSummary projection and the app-version state record behind it — the draft summary must carry identifier 0 and each published snapshot its stored publish sequence number, so the member is always emitted.

Traces: resiliencehub.published-app-version-freezes-resource-mapping (behavioral check, errors at the ListAppVersions read that follows the first publish). Source: services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/list_app_versions.rs:243-269 (`summarise_versions`, `identifier: None` at line 249 for the draft entry; published entries take `contents.identifier`, which is only populated if PublishAppVersion stored it).

## Evidence
Suite report: resiliencehub 1/11 (9%), 10 failed. Of those, 4 mismatches are marked OWNED by 20261029T191752Z-resiliencehub-publishappversion-mints-numbered-versions-instead-of-the.md and 5 errors are the ALREADY FILED StartAppAssessment scaffold cluster; this is the one remaining failure.

[error] resiliencehub.published-app-version-freezes-resource-mapping
  File "check_resiliencehub_published_app_version_freezes_resource_mapping.py", line 86, in run
    v["appVersion"]: v["identifier"] for v in rh.list_app_versions(appArn=app_arn)["appVersions"]
  KeyError: 'identifier'

Expected vs actual (HTTP 200 in both cases; only the body shape diverges):
  expected ListAppVersions after one publish: appVersions = [ {appVersion: "draft", identifier: 0, creationTime: <app creation instant>}, {appVersion: "release", identifier: 1, creationTime: <publish instant>} ]
  actual:   each entry carries appVersion and creationTime but NO `identifier` member at all, so the check's lookup of v["identifier"] raises KeyError before it can reach its assertions (after_first["release"] == 1, after_second["release"] == 2, after_second["1"] == 1).

Preceding call in the same check succeeded and matched AWS: PublishAppVersion -> appVersion == "release", identifier == 1 (the asserts at check lines 81-83 passed), which is why the failure is isolated to the list projection rather than to the publish path.

Smithy citation: com.amazonaws.resiliencehub#AppVersionSummary$identifier -> LongOptional, doc "Identifier of the application version."; ListAppVersions output appVersions -> AppVersionList (required). The specific numbers above (0 for draft, 1 and 2 for the first and second publish of one run) are the sequence values this run's requests produced — the requirement is that the member is always present and equal to the version's publish sequence number, with 0 for the draft.

## Steps to Reproduce
1. CreateResiliencyPolicy(policyName=<name>, tier="NonCritical", policy={Software/Hardware/AZ each with rtoInSecs/rpoInSecs}) -> policyArn.
2. CreateApp(name=<name>, policyArn=<policyArn>) -> appArn.
3. ListAppVersions(appArn=<appArn>) on the fresh application. Observe the single `draft` entry has no `identifier` member; AWS reports identifier 0 for it.
4. CreateAppVersionAppComponent(appArn, name=<component>, type="AWS::ResilienceHub::StorageAppComponent"), then CreateAppVersionResource(appArn, resourceName=<res>, logicalResourceId={identifier: <bucket>}, physicalResourceId=<bucket>, resourceType="AWS::S3::Bucket", appComponents=[<component>]).
5. PublishAppVersion(appArn) -> appVersion "release", identifier 1.
6. ListAppVersions(appArn) again. Observe neither the `draft` nor the `release` entry carries `identifier`, so a client cannot map a listed version to the sequence number the publish returned; reading v["identifier"] raises KeyError.
Full replay: run_behavioral_suite() and re-run resiliencehub.published-app-version-freezes-resource-mapping.

Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-all-set-types-coexist-on-detector
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-all-set-types-coexist-on-detector guardduty-threat-and-trusted-entity-set-lifecycle guardduty-threat-entity-set-lifecycle guardduty-threatentityset-lifecycle-with-metadata guardduty-trusted-entity-set-lifecycle guardduty-trustedentityset-lifecycle-with-metadata guardduty-coverage-statistics-and-list guardduty-usage-and-free-trial-statistics guardduty-usage-and-freetrial-statistics guardduty-malware-scan-settings-and-scan-listing guardduty-malware-scan-settings-inspection guardduty-administrator-master-account-relationship guardduty-member-account-association-queries-and-disassociate guardduty-investigation-lifecycle guardduty-tagging-created-with-tags guardduty-tagging-round-trip guardduty-detector-duplicate-create-error guardduty-detector-features-and-tags guardduty-detector-lifecycle-basic
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetThreatEntitySet|2xx->2xx|$.expectedBucketOwner
Issue-Key: guardduty|GetTrustedEntitySet|2xx->2xx|$.expectedBucketOwner
Issue-Key: guardduty|GetCoverageStatistics|2xx->2xx|$.coverageStatistics.countByCoverageStatus.HEALTHY
Issue-Key: guardduty|GetCoverageStatistics|2xx->2xx|$.coverageStatistics.countByCoverageStatus.UNHEALTHY
Issue-Key: guardduty|GetCoverageStatistics|2xx->2xx|$.coverageStatistics.countByResourceType.EC2
Issue-Key: guardduty|GetCoverageStatistics|2xx->2xx|$.coverageStatistics.countByResourceType.ECS
Issue-Key: guardduty|GetCoverageStatistics|2xx->2xx|$.coverageStatistics.countByResourceType.EKS
Issue-Key: guardduty|ListCoverage|2xx->2xx|$
Issue-Key: guardduty|GetUsageStatistics|2xx->2xx|$.nextToken
Issue-Key: guardduty|GetMalwareScanSettings|2xx->2xx|$.scanResourceCriteria.exclude
Issue-Key: guardduty|GetMalwareScanSettings|2xx->2xx|$.scanResourceCriteria.include
Issue-Key: guardduty|GetAdministratorAccount|2xx->2xx|$.administrator
Issue-Key: guardduty|GetMasterAccount|2xx->2xx|$.master
Issue-Key: guardduty|CreateInvestigation|2xx->2xx|status
Issue-Key: guardduty|GetDetector|2xx->2xx|$.dataSources.database.rds.databaseEvents.status
Issue-Key: guardduty|GetDetector|2xx->2xx|$.features[*]
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Read responses omit required scalars and invent empty sub-structures

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetThreatEntitySet

## Description
Successful (200) read responses are serialized with the wrong member set: members GuardDuty always emits are dropped, and members GuardDuty omits are emitted as empty or synthesized objects. All ten causes are shape defects on a passing status. 10 causes.

Cause 1 of 10: GetThreatEntitySet omits expectedBucketOwner.
AWS: echoes back the expectedBucketOwner the set was created with. Smithy: GetThreatEntitySetResponse.ExpectedBucketOwner (jsonName 'expectedBucketOwner', target ExpectedBucketOwner, @length 12..12, @pattern ^[0-9]+$) — a 12-digit account id taken from the create request.
Emulator: the member is absent from the body.
Why: a client reading a set to confirm which account must own the S3 location sees nothing and cannot verify or round-trip the value through UpdateThreatEntitySet.
Fix: the GetThreatEntitySet serializer and the entity-set state record that must retain expectedBucketOwner.
Traces: guardduty-all-set-types-coexist-on-detector#task#12.

Cause 2 of 10: GetTrustedEntitySet omits expectedBucketOwner.
AWS: same member, same 12-digit account-id value from the create request.
Emulator: absent.
Why: as cause 1, for the trusted-entity-set family.
Fix: the GetTrustedEntitySet serializer / state record.
Traces: guardduty-all-set-types-coexist-on-detector#task#13.

Cause 3 of 10: GetCoverageStatistics omits the zero-valued buckets.
AWS: for the requested statisticsType it returns every bucket of the dimension, including buckets whose count is zero — countByResourceType carries an entry per resource type (EC2, ECS, EKS) and countByCoverageStatus an entry per status (HEALTHY, UNHEALTHY). Smithy: GetCoverageStatistics.$.coverageStatistics targets CoverageStatistics; the keys are the enum members of the requested dimension, the counts come from state.
Emulator: emits the maps without the zero entries, so a detector with no coverage yields empty maps.
Why: a dashboard that indexes countByResourceType['EC2'] gets a missing key instead of 0 and either errors or renders blank.
Fix: the GetCoverageStatistics handler — enumerate the dimension and default each bucket to 0.
Traces: guardduty-coverage-statistics-and-list#task#2.

Cause 4 of 10: ListCoverage emits a 'resources' member on an empty page.
AWS: when no coverage rows match, the body carries no 'resources' member at all (recorded body is {}).
Emulator: emits {'resources': []}.
Why: a client distinguishing 'member absent' from 'empty list' (for example to decide whether coverage has ever been computed) reads the wrong answer.
Fix: the ListCoverage serializer — omit the list when it has no members rather than writing an empty array.
Traces: guardduty-coverage-statistics-and-list#task#3.

Cause 5 of 10: GetUsageStatistics omits nextToken on the last page.
AWS: emits nextToken as the empty string on the terminal page. Smithy: GetUsageStatisticsResponse.NextToken (jsonName 'nextToken'); the operation is @paginated with inputToken/outputToken NextToken.
Emulator: the member is absent.
Why: a paginator that loops 'while nextToken is present' is fine, but one that reads the member unconditionally (as the recorded AWS contract allows) gets a missing key; and the absent-vs-empty distinction changes which SDK paginators terminate.
Fix: the GetUsageStatistics serializer / pagination code that must emit the terminal empty token.
Traces: guardduty-usage-and-free-trial-statistics#task#2.

Cause 6 of 10: GetMalwareScanSettings omits the include and exclude sub-structures.
AWS: scanResourceCriteria carries both 'include' and 'exclude' members, present as empty objects when no criteria are configured.
Emulator: both members are absent.
Why: a client that reads scanResourceCriteria.include to add a criterion must first construct the map itself; round-tripping a read into UpdateMalwareScanSettings drops the structure.
Fix: the GetMalwareScanSettings serializer.
Traces: guardduty-malware-scan-settings-and-scan-listing#task#2.

Cause 7 of 10: GetAdministratorAccount invents an empty 'administrator' object.
AWS: when the detector has no administrator, the recorded response body carries no 'administrator' member. Smithy marks GetAdministratorAccountResponse.Administrator required, so this is recorded behavior only — the recording shows AWS omitting it rather than emitting an empty structure.
Emulator: emits 'administrator': {}.
Why: a client checks for the presence of 'administrator' to decide whether the account is a member of an organization; an empty object reads as 'there is an administrator' and the client then dereferences absent accountId/invitationId.
Fix: the GetAdministratorAccount serializer — omit the member when no administrator relationship exists.
Traces: guardduty-administrator-master-account-relationship#task#2.

Cause 8 of 10: GetMasterAccount invents an empty 'master' object.
AWS: the recorded body omits 'master' when there is no master account.
Emulator: emits 'master': {}.
Why: as cause 7, on the legacy master-account surface.
Fix: the GetMasterAccount serializer.
Traces: guardduty-administrator-master-account-relationship#task#3.

Cause 9 of 10: GetInvestigation invents an 'investigation.cloud' block.
AWS: the recorded investigation body carries no 'cloud' member. Smithy: Investigation.Cloud (jsonName 'cloud', target CloudDetails) is optional, documented as 'Details about the cloud environment in which the investigation was performed' — AWS populates it only when the investigation has produced that detail.
Emulator: always synthesizes it from the request/endpoint context (provider/region/account).
Why: a client treating the presence of 'cloud' as a signal that the investigation has resolved its environment acts on a value the service never asserted.
Fix: the GetInvestigation serializer / investigation state — only emit cloud when the investigation record actually holds it.
Traces: guardduty-investigation-lifecycle#task#4 (the GetInvestigation step of that fixture).

Cause 10 of 10: GetDetector reports the wrong RDS data-source status and a features entry missing additionalConfiguration.
AWS: the detector's dataSources.database.rds.databaseEvents.status reflects the enablement the detector was created with — the recording shows 'ENABLED'. Each entry of 'features' carries the member set {name, status, updatedAt, additionalConfiguration}, with additionalConfiguration present (null when the feature has none).
Emulator: reports databaseEvents.status 'DISABLED' and emits features entries without additionalConfiguration.
Why: a client that reads GetDetector to confirm the data sources it just enabled sees RDS login events disabled although it enabled them, and feature-configuration code that reads additionalConfiguration gets a missing key.
Fix: the detector state's data-source defaulting (the RDS databaseEvents branch) and the GetDetector feature serializer.
Traces: guardduty-detector-duplicate-create-error#task#3.

## Evidence
Cause 1: $.expectedBucketOwner expected present (a 12-digit account id), got None.
Cause 2: $.expectedBucketOwner expected present, got None.
Cause 3: $.coverageStatistics.countByResourceType.EC2/.ECS/.EKS and .countByCoverageStatus.HEALTHY/.UNHEALTHY expected 0, got None.
Cause 4: $ expected {}, got {'resources': []}.
Cause 5: $.nextToken expected '', got None.
Cause 6: $.scanResourceCriteria.include and .exclude expected {}, got None.
Cause 7: $.administrator expected absent (None), got {}.
Cause 8: $.master expected absent (None), got {}.
Cause 9: $.investigation.cloud expected absent (None), got a populated {provider, region, account} object.
Cause 10: $.dataSources.database.rds.databaseEvents.status expected 'ENABLED', got 'DISABLED'; $.features[0] expected the member set {name, status, updatedAt, additionalConfiguration}, got {name, status, updatedAt}.

## Steps to Reproduce
1. Replay guardduty-all-set-types-coexist-on-detector steps task#12 and task#13 — the entity-set reads omit expectedBucketOwner.
2. Replay guardduty-coverage-statistics-and-list steps task#2 and task#3; guardduty-usage-and-free-trial-statistics step task#2; guardduty-malware-scan-settings-and-scan-listing step task#2.
3. Replay guardduty-administrator-master-account-relationship steps task#2 and task#3 on a detector with no administrator.
4. Replay guardduty-investigation-lifecycle (GetInvestigation step, task#5) and guardduty-detector-duplicate-create-error step task#3.

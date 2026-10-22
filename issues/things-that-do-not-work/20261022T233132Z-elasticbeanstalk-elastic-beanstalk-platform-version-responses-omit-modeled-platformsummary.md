Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-catalog-list-platform-versions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-catalog-list-platform-versions elasticbeanstalk-platform-catalog-and-account-quotas
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|ListPlatformVersions|2xx->2xx|$.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.member[*]
Issue-Key: elasticbeanstalk|ListPlatformVersions|2xx->2xx|$.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.member.OperatingSystemName
Issue-Key: elasticbeanstalk|ListPlatformVersions|2xx->2xx|$.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.member.OperatingSystemVersion
Issue-Key: elasticbeanstalk|ListPlatformVersions|2xx->2xx|$.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.member.PlatformBranchLifecycleState
Issue-Key: elasticbeanstalk|ListPlatformVersions|2xx->2xx|$.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.member.PlatformCategory
Issue-Key: elasticbeanstalk|ListPlatformVersions|2xx->2xx|$.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.member.PlatformLifecycleState
Issue-Key: elasticbeanstalk|ListPlatformVersions|2xx->2xx|$.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.member.SupportedAddonList
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.CustomAmiList
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.DateCreated
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.DateUpdated
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.Description
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.Frameworks
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.Maintainer
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.OperatingSystemName
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.OperatingSystemVersion
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.PlatformBranchLifecycleState
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.PlatformCategory
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.PlatformLifecycleState
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.ProgrammingLanguages
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.SupportedAddonList
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.SupportedTierList
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Elastic Beanstalk platform-version responses omit modeled PlatformSummary and PlatformDescription members

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPlatformVersions

## Description
THREE causes on the platform-version surface, sharing one defect: the emulator's platform records carry only a handful of members, so every descriptive member modeled on PlatformSummary/PlatformDescription is omitted, and the lifecycle-state members are emitted lowercase.

Cause 1 of 3: ListPlatformVersions members are stale and incomplete.
AWS: Smithy ListPlatformVersions -> ListPlatformVersionsResult{PlatformSummaryList}; PlatformSummary models PlatformArn, PlatformOwner, PlatformStatus, PlatformCategory, OperatingSystemName, OperatingSystemVersion, SupportedTierList, SupportedAddonList, PlatformLifecycleState, PlatformVersion, PlatformBranchName, PlatformBranchLifecycleState. Recorded behavior: 22 members, each carrying PlatformCategory ('.NET Core'), OperatingSystemName ('Amazon Linux'), OperatingSystemVersion ('2023'), SupportedAddonList (Log/CloudWatch, Log/S3, Monitoring/Healthd, WorkerDaemon/SQSD), PlatformLifecycleState 'Recommended' and PlatformBranchLifecycleState 'Supported' (capitalized), with the current platform version first.
Emulator: 200, but each member carries only PlatformArn/PlatformOwner/PlatformStatus/SupportedTierList/PlatformLifecycleState/PlatformVersion/PlatformBranchName/PlatformBranchLifecycleState, the lifecycle members are lowercase ('recommended', 'supported'), and the inventory is offset to older versions (member[0] is '.NET 8 ... /3.3.0' where AWS returns '.NET 10 ... /3.11.9').
Why: a client filtering or displaying platforms by category, OS or supported add-ons sees those members as absent, and a client selecting the recommended version by PlatformLifecycleState=='Recommended' matches nothing.
Fix: the ListPlatformVersions handler and the platform catalog it serves in services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/ (list_platform_versions.rs).
Traces: elasticbeanstalk-catalog-list-platform-versions#task#1.

Cause 2 of 3: the same omission on a single filtered platform version.
AWS: recorded behavior — the single returned PlatformSummary carries PlatformCategory 'Python', OperatingSystemName 'Amazon Linux', OperatingSystemVersion '2023', SupportedAddonList, PlatformLifecycleState 'Recommended', PlatformBranchLifecycleState 'Supported'.
Emulator: PlatformCategory, OperatingSystemName, OperatingSystemVersion and SupportedAddonList are missing (None) and the two lifecycle members are lowercase.
Why: this is the member-level proof that the omission is in the record, not in the filter; it fails even when the right platform is selected.
Fix: same handler plus the catalog record shape.
Traces: elasticbeanstalk-platform-catalog-and-account-quotas#task#3.

Cause 3 of 3: DescribePlatformVersion returns a near-empty PlatformDescription.
AWS: Smithy DescribePlatformVersion -> DescribePlatformVersionResult{PlatformDescription}, which models PlatformCategory, Description, Maintainer, OperatingSystemName/Version, ProgrammingLanguages, Frameworks, CustomAmiList, SupportedTierList, SupportedAddonList, DateCreated, DateUpdated, PlatformLifecycleState ('Recommended'), PlatformBranchLifecycleState ('Supported'). Recorded behavior: all of those are populated for an AWS-managed platform (e.g. Description '64bit Amazon Linux 2023 running Python 3.13', Maintainer 'aws-elasticbeanstalk-platforms@amazon.com', ProgrammingLanguages [{Name 'Python', Version '3.13'}], CustomAmiList with a pv and an hvm entry).
Emulator: 200, but PlatformCategory, Frameworks, CustomAmiList, Description, Maintainer, DateCreated, DateUpdated, OperatingSystemVersion, OperatingSystemName, ProgrammingLanguages, SupportedTierList, SupportedAddonList and the lifecycle members are all absent.
Why: DescribePlatformVersion is the detail call — a client inspecting a platform's languages, add-ons, AMIs or maintainer gets nothing, and timestamps a caller may order on are missing.
Fix: the DescribePlatformVersion handler and the catalog record it projects (src/ops/describe_platform_version.rs).
Traces: elasticbeanstalk-platform-catalog-and-account-quotas#task#4.

Note: the message-only not-found divergence on DescribePlatformVersion in the same fixture (step task#5, 400 both sides, same error code, only the wording of the 'does not exist' message differs) is deliberately NOT filed here.

## Evidence
Cause 1 — $.ListPlatformVersionsResponse...PlatformSummaryList.member[0]: expected {'PlatformCategory': '.NET Core', 'OperatingSystemVersion': '2023', 'OperatingSystemName': 'Amazon Linux', 'SupportedAddonList': {...}, 'PlatformLifecycleState': 'Recommended', 'PlatformBranchLifecycleState': 'Supported', 'PlatformArn': '...platform/.NET 10 running on 64bit Amazon Linux 2023/3.11.9'}; got {'PlatformArn': '...platform/.NET 8 running on 64bit Amazon Linux 2023/3.3.0', 'PlatformLifecycleState': 'recommended', 'PlatformBranchLifecycleState': 'supported'} with no PlatformCategory/OperatingSystem*/SupportedAddonList (status 200 both).
Cause 2 — member.PlatformCategory expected 'Python', got None; member.OperatingSystemVersion expected '2023', got None; member.OperatingSystemName expected 'Amazon Linux', got None; member.SupportedAddonList expected {'member': ['Log/CloudWatch','Log/S3','Monitoring/Healthd','WorkerDaemon/SQSD']}, got None; member.PlatformBranchLifecycleState expected 'Supported', got 'supported'; member.PlatformLifecycleState expected 'Recommended', got 'recommended'.
Cause 3 — PlatformDescription.Description expected '64bit Amazon Linux 2023 running Python 3.13', got None; .Maintainer expected 'aws-elasticbeanstalk-platforms@amazon.com', got None; .ProgrammingLanguages expected {'member': {'Name': 'Python', 'Version': '3.13'}}, got None; .CustomAmiList, .Frameworks, .SupportedTierList, .SupportedAddonList, .PlatformCategory, .OperatingSystemName, .OperatingSystemVersion, .DateCreated, .DateUpdated all expected present, got None (status 200 both).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Replay fixture elasticbeanstalk-catalog-list-platform-versions step task#1 (ListPlatformVersions) and compare PlatformSummaryList members field by field with the recorded response.
3. Replay fixture elasticbeanstalk-platform-catalog-and-account-quotas step task#3 (filtered ListPlatformVersions) — PlatformCategory, OperatingSystemName/Version and SupportedAddonList are absent; lifecycle states are lowercase.
4. Replay the same fixture's step task#4 (DescribePlatformVersion) — PlatformDescription is returned with almost every modeled member absent.

Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-catalog-list-platform-branches
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-catalog-list-platform-branches elasticbeanstalk-catalog-list-platform-versions elasticbeanstalk-platform-catalog-and-account-quotas
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|ListPlatformBranches|2xx->2xx|$.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList
Issue-Key: elasticbeanstalk|ListPlatformVersions|2xx->2xx|$.ListPlatformVersionsResponse.ListPlatformVersionsResult.PlatformSummaryList.member[*]
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.CustomAmiList.member[*].ImageId
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.Frameworks.member.Name
Issue-Key: elasticbeanstalk|DescribePlatformVersion|2xx->2xx|$.DescribePlatformVersionResponse.DescribePlatformVersionResult.PlatformDescription.Frameworks.member.Version
Issue-Commit: beb9379647f6ddd981e0914c2206ccd791a0c1ce

# Elastic Beanstalk platform catalog omits platform versions and fabricates platform detail fields (3 causes)

> _Note: this issue was identified against the code base at commit `beb9379647f6ddd981e0914c2206ccd791a0c1ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPlatformVersions

## Description
THREE distinct root causes in the elasticbeanstalk emulator, all in the same service, all in the platform catalog the read-only platform APIs serve. The open issue 20261024T015947Z-elasticbeanstalk-elastic-beanstalk-custom-platform-versions-are-not-registered.md covers ADJACENT ground (CUSTOM platform versions created through CreatePlatformVersion not being registered) but does not carry these fixture steps — the replay reported no owner for any of them. THREE causes must be closed before submitting.

Cause 1 of 3: ListPlatformBranches returns an EMPTY branch list for a platform AWS publishes one branch for.
AWS: Smithy ListPlatformBranches(Filters) -> ListPlatformBranchesResult{PlatformBranchSummaryList}; recorded behavior for the 'Packer' platform filter: exactly one PlatformBranchSummary — PlatformName 'Packer', BranchName 'Elastic Beanstalk Packer Builder', LifecycleState 'Supported', BranchOrder 10000, with an empty SupportedTierList.
Emulator: HTTP 200 with PlatformBranchSummaryList serialized as an empty value (no member element at all) — the replay reports a type mismatch at $.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList, expected a member structure, got ''. The platform is simply absent from the emulator's catalog, so the filter matches nothing.
Why: a client that enumerates branches to find the Packer builder concludes the platform does not exist in this account/region and cannot build a custom platform at all; an empty list is also indistinguishable from an over-restrictive filter, so the caller has nothing to retry.
Fix: the platform-branch catalog data served by the ListPlatformBranches handler must include the Packer platform — services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/list_platform_branches.rs and the platform catalog held in crate::state.
Traces: elasticbeanstalk-catalog-list-platform-branches#task#9 (same cluster also fails at task#13).

Cause 2 of 3: the managed platform-version catalog ListPlatformVersions serves is a different inventory from AWS's.
AWS: Smithy ListPlatformVersions -> PlatformSummaryList of PlatformSummary; recorded behavior: the list holds the managed AWSElasticBeanstalk platform versions, each carrying PlatformArn, PlatformVersion, PlatformBranchName, PlatformCategory, PlatformBranchLifecycleState, PlatformLifecycleState ('Recommended' for the one recommended version of each branch, absent otherwise), OperatingSystemName/Version, SupportedTierList and SupportedAddonList — e.g. the '.NET 8 running on 64bit Amazon Linux 2023' branch at version 3.11.9 with PlatformLifecycleState 'Recommended'.
Emulator: the same positions hold different platform versions entirely — '.NET 10 running on 64bit Amazon Linux 2023' at PlatformVersion 3.4.0, Corretto entries at other versions — and the PlatformLifecycleState member AWS sets on the recommended version is not emitted. Every odd-indexed member of the list diverges (34 member keys in the replay diff), so the branch/version pairing and the recommended marker are both wrong.
Why: PlatformArn is the handle every other operation takes (CreateEnvironment, CreateConfigurationTemplate, DescribeConfigurationOptions, DescribePlatformVersion). A client that resolves 'the recommended version of branch X' from this list gets an ARN AWS never minted and cannot find the recommended version at all, so platform-pinned deployments resolve to nothing.
Fix: the managed platform-version catalog served by the ListPlatformVersions handler, including the per-branch 'Recommended' derivation — src/ops/list_platform_versions.rs and the platform catalog in crate::state.
Traces: elasticbeanstalk-catalog-list-platform-versions#task#1.

Cause 3 of 3: DescribePlatformVersion fabricates framework and custom-AMI detail AWS leaves empty.
AWS: Smithy DescribePlatformVersion(PlatformArn) -> DescribePlatformVersionResult{PlatformDescription}; PlatformDescription.Frameworks (PlatformFrameworks) and .CustomAmiList (CustomAmiList) are optional detail members. Recorded behavior for a managed platform version: the Frameworks member element carries an EMPTY Name and an EMPTY Version, and the CustomAmiList member element carries an EMPTY ImageId — a managed platform has no custom AMI and no framework detail to report.
Emulator: it invents values in those slots — Frameworks.member.Name 'Gunicorn' with no Version at all, and CustomAmiList.member[0].ImageId set to a generated 'ami-...' identifier.
Why: CustomAmiList is what a caller reads to learn which AMI a platform pins; reporting a fabricated AMI id for a managed platform sends tooling looking for an EC2 image that does not exist, and a fabricated framework name makes platform-capability checks pass against data AWS never returns. The defect is the fabrication, not the particular ami id or framework string.
Fix: the PlatformDescription projection in the DescribePlatformVersion handler must not synthesize Frameworks/CustomAmiList detail for managed platform versions — src/ops/describe_platform_version.rs and the platform catalog in crate::state.
Traces: elasticbeanstalk-platform-catalog-and-account-quotas#task#4.

## Evidence
Cause 1 (fixture elasticbeanstalk-catalog-list-platform-branches step task#9, expected 200 / actual 200): $.ListPlatformBranchesResponse.ListPlatformBranchesResult.PlatformBranchSummaryList type_mismatch — expected {'member': {'PlatformName': 'Packer', 'BranchName': 'Elastic Beanstalk Packer Builder', 'LifecycleState': 'Supported', 'BranchOrder': '10000', 'SupportedTierList': ''}}, got ''. Same cluster at task#13.
Cause 2 (fixture elasticbeanstalk-catalog-list-platform-versions step task#1, expected 200 / actual 200): $...PlatformSummaryList.member[1] expected PlatformArn '.../platform/.NET 8 running on 64bit Amazon Linux 2023/3.11.9', PlatformVersion '3.11.9', PlatformBranchName '.NET 8 running on 64bit Amazon Linux 2023', PlatformLifecycleState 'Recommended'; got PlatformArn '.../platform/.NET 10 running on 64bit Amazon Linux 2023/3.4.0', PlatformVersion '3.4.0', PlatformBranchName '.NET 10 running on 64bit Amazon Linux 2023' and NO PlatformLifecycleState. 34 member keys of the list diverge (member[1], [3], [5] ... [67]). The specific version strings are the recorded inventory, not per-run values.
Cause 3 (fixture elasticbeanstalk-platform-catalog-and-account-quotas step task#4, expected 200 / actual 200): $...PlatformDescription.Frameworks.member.Name expected '' got 'Gunicorn'; Frameworks.member.Version expected '' got None; CustomAmiList.member[0].ImageId expected '' got 'ami-f69bc39ea51d44198' (the ami id is this run's fabricated value — the requirement is that the member be empty, not that it equal any particular id).

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. Cause 1: eb.list_platform_branches(Filters=[{'Attribute':'PlatformName','Operator':'=','Values':['Packer']}]) -> expect one PlatformBranchSummary for 'Elastic Beanstalk Packer Builder', observe an empty PlatformBranchSummaryList.
3. Cause 2: eb.list_platform_versions() -> compare PlatformSummaryList member-for-member against the recording: PlatformArn/PlatformVersion/PlatformBranchName pairing and the per-branch PlatformLifecycleState='Recommended' marker.
4. Cause 3: eb.describe_platform_version(PlatformArn=<a managed platform arn from step 3>) -> inspect PlatformDescription.Frameworks and PlatformDescription.CustomAmiList; AWS returns empty members, the emulator returns a framework name and a generated ami id.
5. Re-replay elasticbeanstalk-catalog-list-platform-branches#task#9, elasticbeanstalk-catalog-list-platform-versions#task#1 and elasticbeanstalk-platform-catalog-and-account-quotas#task#4.

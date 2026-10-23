Issue-Source: trace-sweep-divergence
Issue-Trace: eks-catalog-describe-cluster-versions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-catalog-describe-cluster-versions eks-eks-catalog-read-only-discovery eks-catalog-list-access-policies
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|DescribeClusterVersions|2xx->2xx|$.clusterVersions[*]
Issue-Key: eks|ListAccessPolicies|2xx->2xx|$.accessPolicies[arn="arn
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# EKS read-only catalog operations serve incomplete and incorrect static data

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterVersions

## Description
The two EKS discovery APIs that serve service-managed catalogs answer HTTP 200 with payloads that do not match the recorded catalogs: cluster-version entries are missing/wrong and the managed cluster-access-policy list is largely empty. These are pure read paths with no account state, so the catalog is a fixed data set the emulator must carry. THREE causes; the issue is not complete until all three are closed.

Cause 1 of 3 - DescribeClusterVersions omits catalog entries entirely.
AWS: Smithy (com-amazonaws-eks-awswesleyfrontend): DescribeClusterVersions (GET /cluster-versions) returns clusterVersions: ClusterVersionList of ClusterVersionInformation, whose members are clusterVersion, clusterType, defaultPlatformVersion, defaultVersion, releaseDate, endOfStandardSupportDate, endOfExtendedSupportDate, status (deprecated, replaced by versionStatus), versionStatus, kubernetesPatchVersion, controlPlaneScalingTiers (list, length 0..10) and controlPlaneComponentConfig; it is paginated on nextToken/maxResults. Recorded behavior: the unfiltered call returns a full list of supported Kubernetes versions, each with clusterType 'eks', a defaultPlatformVersion, support dates, versionStatus, kubernetesPatchVersion and a populated controlPlaneScalingTiers set (tier names, apiRequestConcurrency, podSchedulingRatePerSecond, clusterDatabaseSizeGb and component-config overrides with defaultValue/constraints).
Emulator: 200, but the recorded entries are MISSING from the response (every indexed element $.clusterVersions[0] through $.clusterVersions[7] reports missing), so the catalog the emulator serves is shorter than and/or not aligned with the real one.
Why: client code that picks a cluster version (or reads support dates / scaling tiers to choose a tier) finds no usable version and cannot proceed; version-pinning logic tested against the emulator is never exercised.
Fix: the DescribeClusterVersions handler's static catalog data in the eks crate (and the serialization of nested controlPlaneScalingTiers/controlPlaneComponentConfig).
Traces: eks-catalog-describe-cluster-versions#task#1.

Cause 2 of 3 - DescribeClusterVersions entries differ field-by-field from the recorded catalog on the discovery path.
AWS: as cause 1; the recorded first entry carries a complete ClusterVersionInformation including defaultVersion, the three date fields, versionStatus plus the deprecated status mirror, kubernetesPatchVersion and controlPlaneScalingTiers with per-tier component-config constraints.
Emulator: 200 with $.clusterVersions[0] present but value_mismatch against the recorded entry - the element exists with different/absent members.
Why: a caller cannot rely on any member of the catalog (notably versionStatus and the end-of-support dates used for upgrade planning, and the scaling-tier fields used to size a control plane).
Fix: same component as cause 1 - the catalog table and its member-level serialization; this leg must be re-replayed separately because it exercises the discovery fixture's request shape rather than the plain catalog call.
Traces: eks-eks-catalog-read-only-discovery#task#1.

Cause 3 of 3 - ListAccessPolicies does not return the AWS-managed cluster access policies.
AWS: Smithy: ListAccessPolicies (GET /access-policies) returns accessPolicies: list of AccessPolicy {name, arn}; its only modeled error is ServerException, i.e. it always succeeds and is paginated on nextToken/maxResults. Recorded behavior: the response enumerates the AWS-managed policy catalog, each entry with a name and the matching arn of the form arn:aws:eks::aws:cluster-access-policy/<name> (examples: AmazonEKSClusterAdminPolicy, AmazonEKSAdminPolicy, AmazonEKSEditPolicy, AmazonEKSViewPolicy, AmazonEKSAutoNodePolicy, AmazonEKSArgoCDPolicy, AWSBatchClusterPolicy, ...).
Emulator: 200, but the recorded policies are absent from accessPolicies (every arn-keyed entry reports missing).
Why: AssociateAccessPolicy requires one of these ARNs, so a client that discovers the policy set from the API (rather than hardcoding) can never associate an access policy; access-entry authorization flows are untestable end to end.
Fix: the ListAccessPolicies handler's managed-policy catalog data (name + derived arn) in the eks crate, keeping it consistent with the policy ARNs AssociateAccessPolicy accepts.
Traces: eks-catalog-list-access-policies#task#1.

## Evidence
Corpus simparity-wire-aws-v1, 35/216 steps passed. All three causes returned status 200 as expected; the BODIES diverge.

Cause 1: eks-catalog-describe-cluster-versions#task#1 DescribeClusterVersions - $.clusterVersions[0] .. $.clusterVersions[7]: missing. Expected element 0 example: {'clusterVersion': '1.37', 'clusterType': 'eks', 'defaultPlatformVersion': 'eks.4', 'defaultVersion': False, 'releaseDate': 1790812800.0, 'endOfStandardSupportDate': 1827619200.0, 'endOfExtendedSupportDate': 1859241600.0, 'status': 'STANDARD_SUPPORT', 'versionStatus': 'STANDARD_SUPPORT', 'kubernetesPatchVersion': '1.37.0', 'controlPlaneScalingTiers': [{'tierName': 'tier-xl', 'apiRequestConcurrency': 2000, 'podSchedulingRatePerSecond': 167, 'clusterDatabaseSizeGb': 16, ...}, ...]} (the concrete versions/dates are examples; the requirement is a populated, member-complete catalog).
Cause 2: eks-eks-catalog-read-only-discovery#task#1 DescribeClusterVersions - $.clusterVersions[0]: value_mismatch against the same recorded ClusterVersionInformation element.
Cause 3: eks-catalog-list-access-policies#task#1 ListAccessPolicies - $.accessPolicies[arn="arn:aws:eks::aws:cluster-access-policy/<name>"]: missing, got None, for AIDevOpsAgentAccessPolicy, AWSBatchClusterPolicy, AWSBatchNamespacePolicy, AmazonAIOpsAssistantPolicy, AmazonARCRegionSwitchScalingPolicy, AmazonEKSACKPolicy, AmazonEKSArgoCDClusterPolicy, AmazonEKSArgoCDPolicy, AmazonEKSAutoNodePolicy and further entries.

## Steps to Reproduce
1. Boot the emulator and point an EKS client at it. No account state is needed - these are read-only catalogs.
2. Cause 1: describe_cluster_versions() -> expect a non-empty clusterVersions list whose entries each carry clusterVersion, clusterType='eks', defaultPlatformVersion, defaultVersion, releaseDate, endOfStandardSupportDate, endOfExtendedSupportDate, status, versionStatus, kubernetesPatchVersion and a populated controlPlaneScalingTiers; observe the recorded entries missing.
3. Cause 2: issue the same GET /cluster-versions as the discovery fixture and compare element 0 member-by-member with the recorded ClusterVersionInformation.
4. Cause 3: list_access_policies() -> expect the AWS-managed policy catalog, each {name, arn='arn:aws:eks::aws:cluster-access-policy/<name>'}; observe the recorded policies absent. Then feed one returned arn to associate_access_policy to confirm the two agree.

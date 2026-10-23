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
Issue-Commit: 294bb54d0ff7456773c5757da3560684137bcaef

# EKS read-only discovery catalogs are incomplete: cluster versions and managed access policies are not served

> _Note: this issue was identified against the code base at commit `294bb54d0ff7456773c5757da3560684137bcaef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterVersions

## Description
Replaying the wire corpus (simparity-wire-aws-v1) against the EKS emulator (services/service-com-amazonaws-eks-awswesleyfrontend) shows THREE distinct root causes in the service's static, read-only discovery catalogs: both DescribeClusterVersions and ListAccessPolicies answer HTTP 200 (matching AWS) but with catalog contents AWS does not serve. All three causes must be closed; the issue is NOT complete until all three are.

Cause 1 of 3 — DescribeClusterVersions omits the cluster-version catalog entries entirely.
AWS: Smithy: DescribeClusterVersions GET /cluster-versions returns DescribeClusterVersionsResponse{clusterVersions: ClusterVersionList, nextToken}, paginated (items=clusterVersions, pageSize=maxResults 1..100); the catalog's contents are recorded behavior only. The recording shows a list of ClusterVersion entries, each carrying clusterVersion, clusterType, defaultPlatformVersion, defaultVersion, releaseDate, endOfStandardSupportDate, endOfExtendedSupportDate, status/versionStatus, kubernetesPatchVersion and controlPlaneScalingTiers (tierName, apiRequestConcurrency, podSchedulingRatePerSecond, clusterDatabaseSizeGb, controlPlaneComponentConfigOverrides).
Emulator: the response has none of the recorded entries — every index the recording asserts (clusterVersions[0] through clusterVersions[7]) is reported missing, i.e. the emulator serves a shorter/empty list.
Why: version discovery is how a client picks a Kubernetes version and support window before CreateCluster; an empty/short catalog makes every such client either fail outright or pick a version the emulator will then reject, and support-window logic (end of standard/extended support) cannot be exercised at all.
Fix: the DescribeClusterVersions handler's static catalog data (and its pagination over it) in the eks crate.
Traces: eks-catalog-describe-cluster-versions#task#1.

Cause 2 of 3 — the one cluster-version entry DescribeClusterVersions does serve has diverging fields.
AWS: as above, recorded behavior only; the recording's first entry is a fully populated ClusterVersion including controlPlaneScalingTiers with per-tier concurrency/scheduling-rate/database-size and the nested controlPlaneComponentConfigOverrides defaults and constraints.
Emulator: returns an entry at clusterVersions[0] whose value does not match the recorded entry (value_mismatch over the whole structure, including the scaling-tier substructure). So even where the catalog is populated, the per-version metadata is not the AWS catalog.
Why: a client reading defaultPlatformVersion, versionStatus or the scaling tiers off this response configures a cluster against values AWS never returns, so tier/version-dependent behavior diverges silently downstream.
Fix: the same DescribeClusterVersions catalog data and its ClusterVersion serializer (the scaling-tier/override substructure in particular).
Traces: eks-eks-catalog-read-only-discovery#task#1.

Cause 3 of 3 — ListAccessPolicies does not serve the managed cluster-access policy catalog.
AWS: Smithy: ListAccessPolicies GET /access-policies returns accessPolicies (AccessPoliciesList of {name, arn}), paginated, with only ServerException modeled — so the catalog itself is recorded behavior only. The recording lists the AWS-managed cluster-access policies by ARN of the form arn:aws:eks::aws:cluster-access-policy/<Name> with the matching name, including AmazonEKSBlockStoragePolicy, AmazonEKSComputePolicy, AmazonEKSEventPolicy, AmazonEKSKROPolicy, AmazonEKSLoadBalancingPolicy, AmazonEKSNetworkingPolicy, AmazonEKSPodIdentityPolicy, AmazonEKSSecretAdminPolicy and AmazonEKSSecretReaderPolicy.
Emulator: each of those policy ARNs is absent from the returned list (matched by arn, got None), so the served catalog is a strict subset of AWS's.
Why: these ARNs are the input to AssociateAccessPolicy; a client that discovers policies rather than hard-coding ARNs cannot associate any of the missing policies, and an access-management flow built against the emulator silently covers fewer policies than AWS offers.
Fix: the ListAccessPolicies handler's managed-policy catalog data (and its pagination) in the eks crate.
Traces: eks-catalog-list-access-policies#task#1.

No open issue carries these fixtures' replay keys; the nearest open EKS issues concern not-found error shapes, not the discovery catalogs.

## Evidence
Replay: simparity-wire-aws-v1, 35/216 steps passed, 23 divergence clusters (all eks). All three causes are 200 -> 200 body divergences, so the status line and error handling are not at issue — only the served catalog contents.

Cause 1 — fixture eks-catalog-describe-cluster-versions step task#1, DescribeClusterVersions: $.clusterVersions[0] .. $.clusterVersions[7] all 'missing'; example expected entry {'clusterVersion': '1.37', 'clusterType': 'eks', 'defaultPlatformVersion': 'eks.4', 'defaultVersion': False, 'status': 'STANDARD_SUPPORT', 'versionStatus': 'STANDARD_SUPPORT', 'kubernetesPatchVersion': '1.37.0', 'controlPlaneScalingTiers': [{'tierName': 'tier-xl', 'apiRequestConcurrency': 2000, 'podSchedulingRatePerSecond': 167, 'clusterDatabaseSizeGb': 16, ...}, ...]} (values quoted as examples of the recorded catalog shape).

Cause 2 — fixture eks-eks-catalog-read-only-discovery step task#1, DescribeClusterVersions: $.clusterVersions[0] value_mismatch against the same recorded first entry, including the controlPlaneScalingTiers / controlPlaneComponentConfigOverrides substructure.

Cause 3 — fixture eks-catalog-list-access-policies step task#1, ListAccessPolicies: $.accessPolicies[arn="arn:aws:eks::aws:cluster-access-policy/AmazonEKSBlockStoragePolicy"] missing (expected {'name': 'AmazonEKSBlockStoragePolicy', 'arn': 'arn:aws:eks::aws:cluster-access-policy/AmazonEKSBlockStoragePolicy'}, got None), and likewise for AmazonEKSComputePolicy, AmazonEKSEventPolicy, AmazonEKSKROPolicy, AmazonEKSLoadBalancingPolicy, AmazonEKSNetworkingPolicy, AmazonEKSPodIdentityPolicy, AmazonEKSSecretAdminPolicy, AmazonEKSSecretReaderPolicy.

No state_reset_failures were reported for this replay, and all three steps are read-only, so none of these depend on prior fixture state.

## Steps to Reproduce
1. Boot the emulator and point an EKS client at its data listener.
2. Cause 1: describe_cluster_versions() with no filters. Expect the AWS cluster-version catalog: multiple ClusterVersion entries, each with clusterVersion/clusterType/defaultPlatformVersion/defaultVersion/releaseDate/endOfStandardSupportDate/endOfExtendedSupportDate/status/versionStatus/kubernetesPatchVersion/controlPlaneScalingTiers. Observe the recorded entries absent. Re-replay fixture eks-catalog-describe-cluster-versions step task#1.
3. Cause 2: describe_cluster_versions() as part of the read-only discovery flow and compare the first entry field by field against the recorded ClusterVersion, including controlPlaneScalingTiers and controlPlaneComponentConfigOverrides. Re-replay fixture eks-eks-catalog-read-only-discovery step task#1.
4. Cause 3: list_access_policies() and collect the returned arns. Expect every AWS-managed cluster-access policy ARN (arn:aws:eks::aws:cluster-access-policy/<Name>, e.g. AmazonEKSBlockStoragePolicy, AmazonEKSComputePolicy, AmazonEKSEventPolicy, AmazonEKSKROPolicy, AmazonEKSLoadBalancingPolicy, AmazonEKSNetworkingPolicy, AmazonEKSPodIdentityPolicy, AmazonEKSSecretAdminPolicy, AmazonEKSSecretReaderPolicy) with its matching name; observe them missing. Re-replay fixture eks-catalog-list-access-policies step task#1.
5. Confirm by re-replaying the wire corpus: all three steps must pass on body contents.

Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution elasticache-vvd-traj-cache-subnet-group-build-mistakes
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeCacheSubnetGroups|2xx->2xx|$.DescribeCacheSubnetGroupsResponse.DescribeCacheSubnetGroupsResult.CacheSubnetGroups.CacheSubnetGroup.Subnets.Subnet[*]
Issue-Key: elasticache|DescribeCacheSubnetGroups|2xx->2xx|$.DescribeCacheSubnetGroupsResponse.DescribeCacheSubnetGroupsResult.CacheSubnetGroups.CacheSubnetGroup.SupportedNetworkTypes
Issue-Key: elasticache|CreateCacheSubnetGroup|2xx->2xx|$.CreateCacheSubnetGroupResponse.CreateCacheSubnetGroupResult.CacheSubnetGroup.Subnets.Subnet[*]
Issue-Key: elasticache|CreateCacheSubnetGroup|2xx->2xx|$.CreateCacheSubnetGroupResponse.CreateCacheSubnetGroupResult.CacheSubnetGroup.SupportedNetworkTypes
Issue-Commit: 85304fbaaec1e137da409d02b027804fcd09632b

# ElastiCache cache subnet group responses omit the modeled SupportedNetworkTypes member

> _Note: this issue was identified against the code base at commit `85304fbaaec1e137da409d02b027804fcd09632b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCacheSubnetGroups

## Description
2 net-new wire causes in the elasticache CacheSubnetGroup serializer: every response that carries a CacheSubnetGroup drops the modeled SupportedNetworkTypes member, both on the group and on each Subnet entry. The replay marked neither step as owned by an open issue. The behavioral issue 20261108T093128Z-ec2-elasticache-elasticache-in-vpc-control-plane-diverges-from-aws.md describes the same absence as cause 4 from a behavioral check, and 20261102T083323Z-elasticache-cache-subnet-group-creation-and-engine-version.md describes neighbouring subnet-group defects, but neither carries these fixtures' wire replay keys, so both wire steps are filed here.

Cause 1 of 2: DescribeCacheSubnetGroups omits SupportedNetworkTypes. AWS: Smithy - CacheSubnetGroup.SupportedNetworkTypes targets NetworkTypeList (member enum NetworkType: ipv4 | ipv6 | dual_stack) and Subnet.SupportedNetworkTypes targets the same list; both are members of the shape DescribeCacheSubnetGroups returns (output shape CacheSubnetGroupMessage -> CacheSubnetGroups -> CacheSubnetGroup -> Subnets -> Subnet). The recording shows a group built over subnets of an IPv4-only VPC serializing SupportedNetworkTypes with the single member value 'ipv4' at the group level and on every Subnet entry; the value is derived from the address families of the subnets' VPC, not from the request. Emulator: HTTP 200 with the member absent - '$.DescribeCacheSubnetGroupsResponse.DescribeCacheSubnetGroupsResult.CacheSubnetGroups.CacheSubnetGroup.SupportedNetworkTypes: missing (expected {\'member\': \'ipv4\'}, got None)', and each '...Subnets.Subnet[i]' entry likewise lacks SupportedNetworkTypes. Hypothesis: the same recorded diff also shows the two Subnet entries in the opposite relative order from the recording (the recording's first entry is the us-west-2b subnet, the emulator's is the us-west-2a one); list order is not stated by the model, so only the missing member is treated as the requirement here. Why: a client that reads the group's network types to decide ipv6/dual-stack eligibility gets a KeyError or silently defaults, and cannot tell an IPv4-only group from a dual-stack one. Fix: the CacheSubnetGroup/Subnet serializer and the subnet-group read path in the elasticache handlers - derive the list from the address families of the referenced subnets' VPC. Traces: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#task#5 (also task#6) and elasticache-vvd-traj-cache-subnet-group-build-mistakes#task#11.

Cause 2 of 2: CreateCacheSubnetGroup's own response omits SupportedNetworkTypes. AWS: Smithy - CreateCacheSubnetGroup returns CreateCacheSubnetGroupResult.CacheSubnetGroup, the same CacheSubnetGroup shape, so SupportedNetworkTypes (group level and per Subnet) is part of the create response too; the recording shows 'ipv4' there immediately at creation time. Emulator: HTTP 200 with '$.CreateCacheSubnetGroupResponse.CreateCacheSubnetGroupResult.CacheSubnetGroup.SupportedNetworkTypes: missing (expected {\'member\': \'ipv4\'}, got None)' and the same omission on each Subnet entry. Why: a client that creates a group and uses the returned object without a follow-up describe never sees the network types at all, so the defect is not fixed by patching the describe path alone. Fix: the shared CacheSubnetGroup serializer used by CreateCacheSubnetGroup (and ModifyCacheSubnetGroup) in the elasticache handler, not a per-operation patch. Traces: elasticache-vvd-traj-cache-subnet-group-build-mistakes#task#9.

## Evidence
Both quotes are from the wire replay of this corpus (verdict: mismatch, expected status 200, actual status 200).

Cause 1 - fixture elasticache-tgwc-traj-cache-subnet-group-vpc-resolution, step task#5 (cluster also covers elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#task#6 and elasticache-vvd-traj-cache-subnet-group-build-mistakes#task#11), operation DescribeCacheSubnetGroups:
  $.DescribeCacheSubnetGroupsResponse.DescribeCacheSubnetGroupsResult.CacheSubnetGroups.CacheSubnetGroup.SupportedNetworkTypes: missing (expected {'member': 'ipv4'}, got None)
  $...CacheSubnetGroup.Subnets.Subnet[0]: missing (expected {'SupportedNetworkTypes': {'member': 'ipv4'}, 'SubnetIdentifier': <subnet-b>, 'SubnetAvailabilityZone': {'Name': 'us-west-2b'}}, got {'SubnetIdentifier': <subnet-a>, 'SubnetAvailabilityZone': {'Name': 'us-west-2a'}})
  $...CacheSubnetGroup.Subnets.Subnet[1]: missing (expected {'SupportedNetworkTypes': {'member': 'ipv4'}, 'SubnetIdentifier': <subnet-a>, 'SubnetAvailabilityZone': {'Name': 'us-west-2a'}}, got {'SubnetIdentifier': <subnet-b>, 'SubnetAvailabilityZone': {'Name': 'us-west-2b'}})
  (the subnet ids are environment-generated values; the requirement is the presence of SupportedNetworkTypes on the group and on every Subnet entry)

Cause 2 - fixture elasticache-vvd-traj-cache-subnet-group-build-mistakes, step task#9, operation CreateCacheSubnetGroup:
  $.CreateCacheSubnetGroupResponse.CreateCacheSubnetGroupResult.CacheSubnetGroup.SupportedNetworkTypes: missing (expected {'member': 'ipv4'}, got None)
  $...CacheSubnetGroup.Subnets.Subnet[0] / Subnet[1]: the same per-subnet SupportedNetworkTypes omission.

Smithy cited: CacheSubnetGroup.SupportedNetworkTypes -> NetworkTypeList (NetworkType enum ipv4|ipv6|dual_stack); Subnet.SupportedNetworkTypes -> NetworkTypeList; CreateCacheSubnetGroup output CacheSubnetGroup; DescribeCacheSubnetGroups output CacheSubnetGroupMessage.CacheSubnetGroups.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 2: in an IPv4-only VPC with two subnets in different AZs, call elasticache CreateCacheSubnetGroup with those SubnetIds and inspect CreateCacheSubnetGroupResult.CacheSubnetGroup - observe no SupportedNetworkTypes on the group and none on either Subnet entry, where AWS returns ['ipv4'] in both places. Replay fixture elasticache-vvd-traj-cache-subnet-group-build-mistakes step task#9.
3. Cause 1: call DescribeCacheSubnetGroups for that group - the same two omissions appear in the describe payload. Replay fixture elasticache-tgwc-traj-cache-subnet-group-vpc-resolution step task#5 (and task#6, and elasticache-vvd-traj-cache-subnet-group-build-mistakes step task#11).

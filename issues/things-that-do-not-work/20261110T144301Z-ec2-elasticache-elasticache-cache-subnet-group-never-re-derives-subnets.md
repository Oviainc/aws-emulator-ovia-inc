Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elasticache.vvd-cache-subnet-group-outlives-its-deleted-subnets-and-vpc
Issue-Key: ec2__elasticache.vvd-cache-subnet-group-outlives-its-deleted-subnets-and-vpc|_poll#10|Timeout|time.sleep(interval_s)
Issue-Commit: 8ffd4f499c6d6f6b1d86ee4455070f9416ebee70

# ElastiCache cache subnet group never re-derives Subnets/SupportedNetworkTypes after its subnets and VPC are deleted, so the orphaned-group flow never converges

> _Note: this issue was identified against the code base at commit `8ffd4f499c6d6f6b1d86ee4455070f9416ebee70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCacheSubnetGroups

## Description
One root cause (Cause 1 of 1). A cache subnet group's membership is stored as the subnet-id list the create was given and never resolved against EC2 again, so a group whose subnets and VPC have been deleted keeps reporting them and the documented "orphaned group" state is unreachable.

Cause 1 of 1: DescribeCacheSubnetGroups reports a stale Subnets list (and no re-derived SupportedNetworkTypes) for a group whose subnets and VPC no longer exist.
AWS: Smithy: CacheSubnetGroup carries CacheSubnetGroupName, CacheSubnetGroupDescription, VpcId, Subnets (SubnetList) and SupportedNetworkTypes (NetworkTypeList, ipv4 | ipv6 | dual_stack); DescribeCacheSubnetGroups models exactly one error, CacheSubnetGroupNotFoundFault (awsQueryError code CacheSubnetGroupNotFoundFault, HTTP 400). Recorded behavior: after EC2 DeleteSubnet on every subnet a cache subnet group names and DeleteVpc on its VPC (EC2 thereafter refuses DescribeVpcs with code InvalidVpcID.NotFound and message "The vpc ID '<vpc-id>' does not exist" — fixed text, the id taken from the request), the cache subnet group SURVIVES: it keeps its CacheSubnetGroupName and its now-dangling VpcId, while Subnets and SupportedNetworkTypes re-derive as EMPTY lists. DeleteCacheSubnetGroup on that orphan then succeeds, after which DescribeCacheSubnetGroups answers 400 CacheSubnetGroupNotFoundFault with message "Cache subnet group <name> not found." (fixed text; <name> is the requested group name).
Emulator: the group's Subnets never become empty. The check polls DescribeCacheSubnetGroups every 3 s (its own ceiling is 120 s) for Subnets == [] and is killed by the suite's 60 s per-check budget still inside that poll — error "Check timed out after 60.0s" — so the emulator is still serving the create-time subnet ids for subnets EC2 has already deleted, and the group's SupportedNetworkTypes is likewise not re-derived. Related open issue 20261108T093128Z-ec2-elasticache-elasticache-in-vpc-control-plane-diverges-from-aws.md describes the same "subnet groups never re-derive" shape and owns sibling checks in this service, but it does not carry this check id, so a fixer re-replaying that issue's list never runs this flow.
Why: a client that uses DescribeCacheSubnetGroups to decide whether a group is still usable gets subnet ids that no longer resolve in EC2 and an ipv4/ipv6/dual_stack capability list computed from deleted subnets; it cannot detect the emptied orphan state AWS exposes, so cleanup and re-placement logic (create a cluster in the group, or garbage-collect it) acts on network that does not exist and fails later at the EC2 boundary instead of reading the truth from ElastiCache.
Fix: the CacheSubnetGroup projection in the elasticache crate's state — resolve Subnets and SupportedNetworkTypes against the live EC2 subnets at read time (dropping subnets that no longer exist) instead of echoing the list stored by CreateCacheSubnetGroup, leaving VpcId as stored.
Traces: ec2__elasticache.vvd-cache-subnet-group-outlives-its-deleted-subnets-and-vpc.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout (79/186 passed). Failing check ec2__elasticache.vvd-cache-subnet-group-outlives-its-deleted-subnets-and-vpc, verdict mismatch, detail: "Error: Check timed out after 60.0s".
The check: creates a /16 VPC and two /24 subnets, CreateCacheSubnetGroup over both (asserts the group starts with 2 Subnets — that assert passes), DeleteSubnet on both, DeleteVpc, asserts EC2 then answers (InvalidVpcID.NotFound, "The vpc ID '<vpc>' does not exist") — then enters _poll(lambda: describe_cache_subnet_groups(CacheSubnetGroupName=<name>)["CacheSubnetGroups"][0], lambda x: x["Subnets"] == [], timeout_s=120.0, interval_s=3.0, description="the group's subnet list re-derived as empty"). The run ends inside that poll, i.e. every probe still returned a non-empty Subnets list.
Expected (recorded AWS): the orphan reads (CacheSubnetGroupName=<name>, VpcId=<deleted vpc id>, Subnets=[]) with SupportedNetworkTypes == [], then DeleteCacheSubnetGroup succeeds and DescribeCacheSubnetGroups answers 400 CacheSubnetGroupNotFoundFault "Cache subnet group <name> not found."
Actual (emulator): Subnets never empties, so the post-delete assertions on the orphan and its own delete are never reached.
Model evidence: CacheSubnetGroup declares Subnets (SubnetList) and SupportedNetworkTypes (NetworkTypeList: ipv4|ipv6|dual_stack); DescribeCacheSubnetGroups binds only CacheSubnetGroupNotFoundFault -> {CacheSubnetGroupNotFoundFault, 400}.

## Steps to Reproduce
1. Boot the emulator from this checkout and run suite simparity-behavioral-aws-v1.
2. CreateVpc 10.0.0.0/16 and two subnets in two available AZs; CreateCacheSubnetGroup(CacheSubnetGroupName=<name>, SubnetIds=[both]) -> 2 Subnets reported.
3. DeleteSubnet on both subnets, DeleteVpc on the VPC; confirm DescribeVpcs answers InvalidVpcID.NotFound "The vpc ID '<vpc>' does not exist".
4. DescribeCacheSubnetGroups(CacheSubnetGroupName=<name>) -> expect the group to survive with VpcId unchanged, Subnets == [] and SupportedNetworkTypes == []; observe the create-time subnet ids still reported (the check's 3 s poll never satisfies and the 60 s budget kills it).
5. DeleteCacheSubnetGroup(CacheSubnetGroupName=<name>), then DescribeCacheSubnetGroups(CacheSubnetGroupName=<name>) -> expect 400 CacheSubnetGroupNotFoundFault "Cache subnet group <name> not found."
6. Re-replay ec2__elasticache.vvd-cache-subnet-group-outlives-its-deleted-subnets-and-vpc.

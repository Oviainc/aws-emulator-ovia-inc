Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elasticache.tgwc-memcached-cluster-resolves-to-isolated-route-table-and-group ec2__elasticache.tgwc-memcached-transit-encryption-on-old-engine-rejected ec2__elasticache.vvd-auth-token-without-transit-encryption-rejected ec2__elasticache.vvd-cache-subnet-group-derives-vpc-and-zones-from-subnets ec2__elasticache.vvd-cache-subnet-group-duplicate-name-rejected ec2__elasticache.vvd-cache-subnet-group-outlives-its-deleted-subnets-and-vpc ec2__elasticache.vvd-replication-group-id-over-forty-chars-rejected ec2__elasticache.vvd-replication-group-unknown-subnet-group-rejected ec2__elasticache.vvd-single-node-automatic-failover-rejected ec2__elasticache.vvd-valkey-engine-version-not-offered-rejected
Issue-Key: ec2__elasticache.tgwc-memcached-cluster-resolves-to-isolated-route-table-and-group|run#34|AssertionError|assert len(enis) == 1, f"one cache node should own one ENI in {sub}, found {enis!r}"
Issue-Key: ec2__elasticache.tgwc-memcached-transit-encryption-on-old-engine-rejected|_error#12|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it. It returned {result!r}")
Issue-Key: ec2__elasticache.vvd-auth-token-without-transit-encryption-rejected|run#11|AssertionError|assert code == "InvalidParameterValue", f"AUTH without TLS raised {code!r}"
Issue-Key: ec2__elasticache.vvd-cache-subnet-group-derives-vpc-and-zones-from-subnets|run#19|KeyError|assert g["SupportedNetworkTypes"] == ["ipv4"], f"an IPv4-only VPC yields {g['SupportedNetworkTypes']!r}"
Issue-Key: ec2__elasticache.vvd-cache-subnet-group-duplicate-name-rejected|run#10|AssertionError|assert code == "CacheSubnetGroupAlreadyExists", (
Issue-Key: ec2__elasticache.vvd-cache-subnet-group-outlives-its-deleted-subnets-and-vpc|_poll#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: ec2__elasticache.vvd-replication-group-id-over-forty-chars-rejected|run#13|AssertionError|assert got == identifier_error, f"the identifier {bad!r} raised {got!r}, expected {identifier_error!r}"
Issue-Key: ec2__elasticache.vvd-replication-group-unknown-subnet-group-rejected|run#16|AssertionError|assert got == want, (
Issue-Key: ec2__elasticache.vvd-single-node-automatic-failover-rejected|run#16|AssertionError|assert got == want, f"failover on one node raised {got!r}, expected {want!r}"
Issue-Key: ec2__elasticache.vvd-valkey-engine-version-not-offered-rejected|run#10|AssertionError|assert got == want, f"valkey {bad} raised {got!r}, expected {want!r}"
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# ElastiCache in-VPC control plane diverges from AWS in nine ways: cache nodes mint no ENI, SupportedNetworkTypes is absent, subnet groups never re-derive, and six refusals carry the wrong code or message

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationGroup

## Description
Nine net-new root causes in the ElastiCache control plane as reached from EC2 networking. Related open issues describe neighbouring elasticache defects but carry none of these checks' replay keys: 20261102T083323Z-elasticache-cache-subnet-group-creation-and-engine-version.md, 20261102T083323Z-elasticache-refusals-carry-the-wrong-error-code-and.md, 20261105T143748Z-elasticache-teardown-refusals-diverge-from-aws-a-valid.md, 20261030T130325Z-elasticache-reference-catalogs-diverge-from-aws-engine-versions.md.

Cause 1 of 9: a cache cluster's node never materialises a network interface in its subnet. AWS: recorded behavior only — the Smithy model says nothing about ENIs, and the recording shows that an available single-node memcached cluster in a cache subnet group owns exactly one requester-managed ENI in that subnet, carrying the cluster's security group and a private address drawn from the subnet CIDR (RequesterManaged=true). Emulator: after the cluster reports CacheClusterStatus=available, ec2:DescribeNetworkInterfaces filtered by subnet-id=<the group's subnet> and group-id=<the cluster's security group> answers an empty list — AssertionError: "one cache node should own one ENI in subnet-..., found []". Why: a client cannot see or reason about the cache's network presence — no ENI to attribute an address to, no security-group binding to verify, and any diagnosis that locates a cache by its interface finds nothing. Fix: the elasticache CreateCacheCluster/replication-group state and its EC2 network-interface materialisation (the requester-managed ENI per node), not the EC2 describe path. Traces: ec2__elasticache.tgwc-memcached-cluster-resolves-to-isolated-route-table-and-group.

Cause 2 of 9: CreateCacheCluster accepts transit encryption on a memcached engine version that does not support it. AWS: Smithy — CreateCacheCluster.TransitEncryptionEnabled (BooleanOptional) and EngineVersion (String); modeled error InvalidParameterCombinationException, awsQueryError code InvalidParameterCombination, HTTP 400. Recorded: memcached 1.6.6 is an offered version, but TransitEncryptionEnabled=true with EngineVersion=1.6.6 is refused 400 InvalidParameterCombination with the message "Encryption features are not supported for engine version <engine-version>. Please use engine version 1.6.12" (fixed text; <engine-version> is the EngineVersion from the request), and no cluster is created — DescribeCacheClusters then answers CacheClusterNotFound "CacheCluster not found: <cluster-id>". Emulator: the call returns HTTP 200 with CacheClusterStatus=creating and TransitEncryptionEnabled=true, so an unsupported combination silently becomes a real cluster. Why: a client that relies on AWS refusing the pair ships a cluster it believes is encrypted in transit on an engine that cannot encrypt it. Fix: CreateCacheCluster validation in the elasticache handler (engine-version/encryption compatibility), before any state is written. Traces: ec2__elasticache.tgwc-memcached-transit-encryption-on-old-engine-rejected.

Cause 3 of 9: an AUTH token without transit encryption is refused with the wrong error code. AWS: Smithy — CreateReplicationGroup carries both AuthToken and TransitEncryptionEnabled and models both InvalidParameterValueException (code InvalidParameterValue, 400) and InvalidParameterCombinationException (code InvalidParameterCombination, 400). Recorded: AuthToken set with TransitEncryptionEnabled=false is refused 400 InvalidParameterValue with the message "The AUTH token is only supported when encryption-in-transit is enabled" (fixed text, no variable parts) — AWS pins InvalidParameterValue to the single field that is wrong. Emulator: answers code InvalidParameterCombination instead. Why: clients branch on the error code; code-keyed retry/validation logic treats a single-field value error as a combination error and reports the wrong field to the user. Fix: the error classification in CreateReplicationGroup's validation. Traces: ec2__elasticache.vvd-auth-token-without-transit-encryption-rejected.

Cause 4 of 9: cache subnet groups omit the modeled SupportedNetworkTypes member. AWS: Smithy — CacheSubnetGroup.SupportedNetworkTypes (NetworkTypeList, values ipv4|ipv6|dual_stack) is a member of the shape returned by CreateCacheSubnetGroup, ModifyCacheSubnetGroup and DescribeCacheSubnetGroups. Recorded: a group over subnets of an IPv4-only VPC reads SupportedNetworkTypes == ["ipv4"]. Emulator: the key is absent from the DescribeCacheSubnetGroups payload — KeyError: 'SupportedNetworkTypes'. Why: any client that reads the group's network types (to decide ipv6/dual-stack eligibility) crashes or silently defaults. Fix: the CacheSubnetGroup serializer/state in the elasticache handlers (derive the list from the subnets' VPC address families). Traces: ec2__elasticache.vvd-cache-subnet-group-derives-vpc-and-zones-from-subnets.

Cause 5 of 9: a duplicate cache-subnet-group name loses precedence to subnet validation. AWS: Smithy — CreateCacheSubnetGroup models CacheSubnetGroupAlreadyExistsFault (awsQueryError code CacheSubnetGroupAlreadyExists, 400) and InvalidSubnet (400). Recorded: a CreateCacheSubnetGroup whose name already exists AND whose SubnetIds name a deleted subnet is refused 400 CacheSubnetGroupAlreadyExists with the message "Cache subnet group <name> already exists." (fixed text; <name> is CacheSubnetGroupName from the request) — the name conflict is evaluated first; once the name is freed the identical call fails on the subnet instead. Emulator: answers InvalidParameterValue, i.e. the subnet is validated first and the name conflict is never reported. Why: a client retrying on a name conflict (to pick a new name) instead sees a value error about a subnet it did not change, and cannot tell a taken name from a bad subnet. Fix: validation ordering in CreateCacheSubnetGroup (name-uniqueness check before subnet resolution) in the elasticache handler. Traces: ec2__elasticache.vvd-cache-subnet-group-duplicate-name-rejected.

Cause 6 of 9: a cache subnet group never re-derives after its subnets and VPC are deleted. AWS: recorded behavior only — the model carries Subnets and SupportedNetworkTypes as derived output. Recorded: when EC2 deletes every subnet the group names and then the VPC, the group survives, keeps its name and its now-dangling VpcId, and its Subnets list and SupportedNetworkTypes re-derive as EMPTY. Emulator: DescribeCacheSubnetGroups keeps returning the two deleted SubnetIdentifiers with their zones indefinitely (polled 120 s), so the group reports membership of subnets EC2 answers InvalidSubnetID.NotFound for. Why: a client reconciling cache placement against EC2 sees a group pointing at subnets that do not exist and cannot detect the orphaned state. Fix: the cache-subnet-group read path/state in the elasticache handlers must derive Subnets (and SupportedNetworkTypes) from live EC2 subnets rather than storing the request's list. Traces: ec2__elasticache.vvd-cache-subnet-group-outlives-its-deleted-subnets-and-vpc.

Cause 7 of 9: the invalid-replication-group-identifier refusal message is emulator-authored. AWS: Smithy — InvalidParameterValueException (code InvalidParameterValue, 400). Recorded: an id starting with a digit, ending with a hyphen, or containing two consecutive hyphens is refused 400 InvalidParameterValue with the message "Replication Group ID specified is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens." (entirely fixed text). Emulator: same code but the message reads "The parameter ReplicationGroupIdentifier is not a valid identifier. Identifiers must begin with a letter; ..." — the leading sentence names a parameter AWS does not name. Why: clients and tests that match the documented message text (or surface it to users) diverge. Fix: the refusal message template in CreateReplicationGroup's identifier validation. Traces: ec2__elasticache.vvd-replication-group-id-over-forty-chars-rejected.

Cause 8 of 9: an engine version absent from the catalogue is refused with the wrong code and message. AWS: Smithy — CreateReplicationGroup models InvalidParameterCombinationException (code InvalidParameterCombination, 400). Recorded: with a resolvable CacheSubnetGroupName, an EngineVersion the engine's catalogue does not offer (e.g. a Redis-only version, or a version past the catalogue) is refused 400 InvalidParameterCombination with the message "Cannot find version <engine-version> for <engine>" (fixed text; <engine-version> and <engine> are taken from the request). Emulator: answers 400 InvalidParameterValue with "Invalid value <engine-version> for EngineVersion: no cache parameter group family supports it." — wrong code and emulator-authored prose that blames the parameter-group family. Why: a client distinguishing an unsupported-version combination from a bad single value branches wrongly, and the version/engine pair is not named as AWS names it. Fix: engine-version catalogue validation and its error classification in CreateReplicationGroup. Traces: ec2__elasticache.vvd-valkey-engine-version-not-offered-rejected, ec2__elasticache.vvd-replication-group-unknown-subnet-group-rejected.

Cause 9 of 9: the automatic-failover-on-one-node refusal message is emulator-authored. AWS: Smithy — InvalidParameterCombinationException (code InvalidParameterCombination, 400). Recorded: AutomaticFailoverEnabled=true (or MultiAZEnabled) with NumCacheClusters=1 is refused 400 InvalidParameterCombination with the message "When using automatic failover, there must be at least 2 cache clusters in the replication group." (entirely fixed text), and nothing is created. Emulator: correct code but the message reads "AutomaticFailoverEnabled requires at least one replica in every node group (shard) to promote." Why: the message is part of the contract clients log, match and show; it must read as AWS words it. Fix: the refusal message template in CreateReplicationGroup's failover validation. Traces: ec2__elasticache.vvd-single-node-automatic-failover-rejected.

## Evidence
All quotes are from the behavioral replay of this suite.

Cause 1 — ec2__elasticache.tgwc-memcached-cluster-resolves-to-isolated-route-table-and-group [mismatch]:
AssertionError: one cache node should own one ENI in subnet-<id>, found []
(expected exactly one requester-managed ENI in the group's subnet carrying the cluster's security group, with an address inside the subnet CIDR).

Cause 2 — ec2__elasticache.tgwc-memcached-transit-encryption-on-old-engine-rejected [mismatch]:
Expected: ('InvalidParameterCombination', 'Encryption features are not supported for engine version 1.6.6. Please use engine version 1.6.12').
Actual: HTTP 200 with {'CacheCluster': {... 'Engine': 'memcached', 'EngineVersion': '1.6.6', 'CacheClusterStatus': 'creating', 'TransitEncryptionEnabled': True ...}} — AssertionError: the call succeeded; real AWS rejects it.

Cause 3 — ec2__elasticache.vvd-auth-token-without-transit-encryption-rejected [mismatch]:
Expected code 'InvalidParameterValue' with message 'The AUTH token is only supported when encryption-in-transit is enabled'.
Actual: AssertionError: AUTH without TLS raised 'InvalidParameterCombination'.

Cause 4 — ec2__elasticache.vvd-cache-subnet-group-derives-vpc-and-zones-from-subnets [error]:
KeyError: 'SupportedNetworkTypes' (expected ["ipv4"] for an IPv4-only VPC).

Cause 5 — ec2__elasticache.vvd-cache-subnet-group-duplicate-name-rejected [mismatch]:
Expected ('CacheSubnetGroupAlreadyExists', 'Cache subnet group <name> already exists.').
Actual: AssertionError: a duplicate name with an unknown subnet raised 'InvalidParameterValue', not the name conflict.

Cause 6 — ec2__elasticache.vvd-cache-subnet-group-outlives-its-deleted-subnets-and-vpc [mismatch]:
Expected Subnets == [] and SupportedNetworkTypes == [] after the subnets and VPC are deleted.
Actual: AssertionError: the group's subnet list re-derived as empty not met within 120.0s. Last result: {... 'VpcId': 'vpc-<id>', 'Subnets': [{'SubnetIdentifier': 'subnet-<a>', ...}, {'SubnetIdentifier': 'subnet-<b>', ...}], ...} (both subnets already answer InvalidSubnetID.NotFound in EC2).

Cause 7 — ec2__elasticache.vvd-replication-group-id-over-forty-chars-rejected [mismatch]:
Expected ('InvalidParameterValue', 'Replication Group ID specified is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.').
Actual ('InvalidParameterValue', 'The parameter ReplicationGroupIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.').

Cause 8 — ec2__elasticache.vvd-valkey-engine-version-not-offered-rejected and ec2__elasticache.vvd-replication-group-unknown-subnet-group-rejected [mismatch]:
Expected ('InvalidParameterCombination', 'Cannot find version 6.2 for valkey').
Actual ('InvalidParameterValue', 'Invalid value 6.2 for EngineVersion: no cache parameter group family supports it.').

Cause 9 — ec2__elasticache.vvd-single-node-automatic-failover-rejected [mismatch]:
Expected ('InvalidParameterCombination', 'When using automatic failover, there must be at least 2 cache clusters in the replication group.').
Actual ('InvalidParameterCombination', 'AutomaticFailoverEnabled requires at least one replica in every node group (shard) to promote.').

## Steps to Reproduce
Boot the emulator and, for each cause, drive the ElastiCache and EC2 control planes directly:
1 (ENI): create a VPC, one subnet, a security group; CreateCacheSubnetGroup over that subnet; CreateCacheCluster (memcached, 1 node, that subnet group and security group); wait for available; ec2:DescribeNetworkInterfaces with Filters subnet-id=<subnet> and group-id=<sg> — observe an empty list where AWS returns one requester-managed ENI.
2 (TLS on old engine): CreateCacheCluster Engine=memcached EngineVersion=1.6.6 TransitEncryptionEnabled=true — observe 200/creating instead of 400 InvalidParameterCombination.
3 (AUTH): CreateReplicationGroup with AuthToken set and TransitEncryptionEnabled=false — observe code InvalidParameterCombination instead of InvalidParameterValue.
4 (network types): CreateCacheSubnetGroup then DescribeCacheSubnetGroups — observe SupportedNetworkTypes missing.
5 (duplicate name): CreateCacheSubnetGroup with an existing name and a deleted subnet id — observe InvalidParameterValue instead of CacheSubnetGroupAlreadyExists.
6 (orphan group): delete the group's subnets and VPC in EC2, then DescribeCacheSubnetGroups — observe the deleted subnets still listed.
7 (identifier): CreateReplicationGroup with an id starting with a digit / ending in '-' / containing '--' — compare the message text.
8 (engine version): CreateReplicationGroup Engine=valkey EngineVersion=6.2 with a resolvable subnet group — observe InvalidParameterValue instead of InvalidParameterCombination 'Cannot find version 6.2 for valkey'.
9 (failover): CreateReplicationGroup NumCacheClusters=1 AutomaticFailoverEnabled=true — compare the message text.
Then re-replay the nine checks listed in trace_ids to confirm the fix.

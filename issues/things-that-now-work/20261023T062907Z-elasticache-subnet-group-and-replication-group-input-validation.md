Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-network-security-groups
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-network-security-groups elasticache-replication-group-lifecycle-redis elasticache-reserved-node-purchase
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateCacheSubnetGroup|4xx->2xx|$.ErrorResponse
Issue-Key: elasticache|CreateCacheSubnetGroup|4xx->2xx|status
Issue-Key: elasticache|DescribeCacheSubnetGroups|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|CreateReplicationGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateReplicationGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|DescribeReplicationGroups|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|DescribeReplicationGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|DescribeReplicationGroups|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|DescribeReplicationGroups|4xx->4xx|status
Issue-Key: elasticache|DescribeReservedCacheNodesOfferings|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|DescribeReservedCacheNodesOfferings|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|PurchaseReservedCacheNodesOffering|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|DescribeReservedCacheNodes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# ElastiCache subnet-group and replication-group input validation is skipped and reserved-node errors lack RequestId

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheSubnetGroup

## Description
Seven divergences on the cache-subnet-group, replication-group and reserved-cache-node operations: one input the service refuses is accepted with HTTP 200, one refusal uses the wrong status and modeled error because validation runs after the existence check, and the rest are awsQuery error envelopes with no RequestId. Each cause is separate and must be closed separately. (replay: state_reset_failures=0.)

Cause 1 of 7: CreateCacheSubnetGroup accepts subnet ids that do not exist.
AWS: Smithy: CreateCacheSubnetGroup binds InvalidParameterValueException (awsQueryError code `InvalidParameterValue`, HTTP 400) alongside InvalidSubnet. Recorded behavior for a syntactically subnet-shaped but nonexistent SubnetIds entry is HTTP 400 with Error.Type `Sender`, Error.Code `InvalidParameterValue` naming the offending ids (recorded example message 'Some input subnets in :[subnet-0123456789abcdef0] are invalid.'), and NO subnet group is created.
Emulator: answers HTTP 200 and creates the group (no ErrorResponse element at all).
Why: the client believes it owns a usable subnet group; every cluster placement into it rests on a subnet that does not exist, and the failure surfaces far from the call that caused it.
Fix: subnet resolution/validation in the CreateCacheSubnetGroup handler, before any state mutation.
Traces: elasticache-network-security-groups#task#2.

Cause 2 of 7: DescribeCacheSubnetGroups error envelope omits RequestId.
AWS: Smithy: DescribeCacheSubnetGroups faults are serialized as <ErrorResponse><Error>{Type,Code,Message}</Error><RequestId>...</RequestId></ErrorResponse>; the recorded 400 response carries a RequestId.
Emulator: status and Error contents match, but $.ErrorResponse.RequestId is absent.
Why: the envelope is not the protocol's error shape and clients lose the request id they log/surface.
Fix: the shared awsQuery error serializer for elasticache (the success path already emits ResponseMetadata.RequestId).
Traces: elasticache-network-security-groups#task#3.

Cause 3 of 7: CreateReplicationGroup identifier-length refusal omits RequestId.
AWS: Smithy: CreateReplicationGroup binds InvalidParameterValueException (code `InvalidParameterValue`, HTTP 400). Recorded behavior for an over-long ReplicationGroupId is HTTP 400 `InvalidParameterValue` with a RequestId element; recorded example message 'Replication Group ID cannot be longer than 40 characters.'
Emulator: status 400 and code match and its message also names the member and its 1-40 bound, but $.ErrorResponse.RequestId is absent.
Why: as Cause 2.
Fix: same shared awsQuery error serializer.
Traces: elasticache-replication-group-lifecycle-redis#task#2.

Cause 4 of 7: DescribeReplicationGroups runs the existence check before input validation, so it answers 404 ReplicationGroupNotFoundFault where the service answers 400 InvalidParameterValue.
AWS: Smithy: DescribeReplicationGroups models BOTH InvalidParameterValueException (code `InvalidParameterValue`, HTTP 400) and ReplicationGroupNotFoundFault (code `ReplicationGroupNotFoundFault`, HTTP 404). Recorded behavior for an over-long ReplicationGroupId is the validation error first: HTTP 400, Error.Code `InvalidParameterValue` (recorded example message 'Replication Group ID cannot be longer than 50 characters.').
Emulator: HTTP 404 with Error.Code `ReplicationGroupNotFoundFault`, and no RequestId in the envelope.
Why: the client is told the group does not exist rather than that its identifier is invalid, so it retries/creates against an id that can never be accepted instead of fixing its input; status-based retry logic also treats 404 and 400 differently.
Fix: precondition ordering in the DescribeReplicationGroups handler — validate ReplicationGroupId before resolving it.
Traces: elasticache-replication-group-lifecycle-redis#task#3.

Cause 5 of 7: DescribeReservedCacheNodesOfferings not-found error omits RequestId.
AWS: Smithy: DescribeReservedCacheNodesOfferings answers a not-found fault with HTTP 404 for an unknown ReservedCacheNodesOfferingId; the recorded envelope carries a RequestId (recorded example message 'ReservedCacheNodesOfferingId <id> not found.').
Emulator: status and code match; $.ErrorResponse.RequestId absent.
Why: as Cause 2.
Fix: same shared awsQuery error serializer.
Traces: elasticache-reserved-node-purchase#task#1.

Cause 6 of 7: PurchaseReservedCacheNodesOffering not-found error omits RequestId.
AWS: recorded behavior — HTTP 404 for an unknown offering id with the fault's code and a RequestId element in the envelope.
Emulator: status and Error contents match; $.ErrorResponse.RequestId absent.
Why: as Cause 2.
Fix: same shared awsQuery error serializer.
Traces: elasticache-reserved-node-purchase#task#2.

Cause 7 of 7: DescribeReservedCacheNodes not-found error omits RequestId.
AWS: Smithy: DescribeReservedCacheNodes models ReservedCacheNodeNotFoundFault (code `ReservedCacheNodeNotFound`, HTTP 404); the recorded envelope carries a RequestId.
Emulator: status and code match; $.ErrorResponse.RequestId absent.
Why: as Cause 2.
Fix: same shared awsQuery error serializer.
Traces: elasticache-reserved-node-purchase#task#3.

Related but NOT covering: open issue 20261022T193451Z-elasticache-cache-cluster-subnet-group-validation-is-over.md reports the invalid-subnet acceptance from the behavioral suite. It carries no wire replay key for elasticache-network-security-groups#task#2 or any other step above, so these causes are unexercised by it.

## Evidence
Replay of trace set simparity-wire-aws-v1, all clusters verdict=mismatch, stub_path empty, owned_by empty:
1. elasticache-network-security-groups#task#2 CreateCacheSubnetGroup(SubnetIds=['subnet-0123456789abcdef0']) — expected HTTP 400 {Error:{Type:Sender, Code:InvalidParameterValue, Message:'Some input subnets in :[subnet-0123456789abcdef0] are invalid.'}, RequestId:...}; got HTTP 200 with $.ErrorResponse missing.
2. elasticache-network-security-groups#task#3 DescribeCacheSubnetGroups — 400 both sides; $.ErrorResponse.RequestId missing (expected present, got None).
3. elasticache-replication-group-lifecycle-redis#task#2 CreateReplicationGroup(over-long ReplicationGroupId) — 400/InvalidParameterValue both sides; $.ErrorResponse.RequestId missing.
4. elasticache-replication-group-lifecycle-redis#task#3 DescribeReplicationGroups(over-long id) — expected HTTP 400 Code=InvalidParameterValue; got HTTP 404 Code=ReplicationGroupNotFoundFault, $.ErrorResponse.RequestId missing.
5. elasticache-reserved-node-purchase#task#1 DescribeReservedCacheNodesOfferings(unknown offering id) — 404 both sides; $.ErrorResponse.RequestId missing.
6. elasticache-reserved-node-purchase#task#2 PurchaseReservedCacheNodesOffering(unknown offering id) — 404 both sides; $.ErrorResponse.RequestId missing.
7. elasticache-reserved-node-purchase#task#3 DescribeReservedCacheNodes — 404 both sides; $.ErrorResponse.RequestId missing.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: replay elasticache-network-security-groups#task#2 — CreateCacheSubnetGroup with a SubnetIds entry that does not exist, expecting HTTP 400 InvalidParameterValue naming the invalid subnet and no group created.
3. Cause 2: step task#3 of the same fixture — DescribeCacheSubnetGroups error path; inspect the raw XML for a <RequestId> child of <ErrorResponse>.
4. Causes 3,4: replay elasticache-replication-group-lifecycle-redis steps task#2 and task#3 with an over-long ReplicationGroupId — CreateReplicationGroup must return 400 InvalidParameterValue with a RequestId, and DescribeReplicationGroups must return 400 InvalidParameterValue (validation before lookup), not 404 ReplicationGroupNotFoundFault.
5. Causes 5-7: replay elasticache-reserved-node-purchase steps task#1, task#2, task#3 against an unknown ReservedCacheNodesOfferingId and inspect each ErrorResponse for RequestId.
6. Confirm the fix by re-replaying every step named in Traces.

Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-cache-cluster-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-cache-cluster-lifecycle
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateCacheCluster|4xx->2xx|$.ErrorResponse
Issue-Key: elasticache|CreateCacheCluster|4xx->2xx|status
Issue-Key: elasticache|DescribeCacheClusters|4xx->2xx|$.ErrorResponse
Issue-Key: elasticache|DescribeCacheClusters|4xx->2xx|status
Issue-Key: elasticache|ModifyCacheCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|ModifyCacheCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|ModifyCacheCluster|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|ModifyCacheCluster|4xx->4xx|status
Issue-Key: elasticache|RebootCacheCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|RebootCacheCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|RebootCacheCluster|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|RebootCacheCluster|4xx->4xx|status
Issue-Key: elasticache|ListAllowedNodeTypeModifications|4xx->2xx|$.ErrorResponse
Issue-Key: elasticache|ListAllowedNodeTypeModifications|4xx->2xx|status
Issue-Key: elasticache|DeleteCacheCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|DeleteCacheCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|DeleteCacheCluster|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|DeleteCacheCluster|4xx->4xx|status
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# ElastiCache cache-cluster operations skip node-type validation and never answer CacheClusterNotFound

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheCluster

## Description
Six independent wire divergences on the cache-cluster surface of the implemented elasticache emulator. CreateCacheCluster accepts a node type the service refuses, and every cache-cluster lookup for an id that does not exist answers something other than the modeled CacheClusterNotFound. Each cause is separate and must be closed separately. (replay: state_reset_failures=0, so none of these are cross-fixture state bleed.)

Cause 1 of 6: CreateCacheCluster accepts an unknown CacheNodeType instead of refusing it.
AWS: Smithy: CreateCacheCluster binds InvalidParameterValueException (awsQueryError code `InvalidParameterValue`, HTTP 400). Recorded behavior for a CacheNodeType that is not a real node type is HTTP 400 with Error.Type `Sender`, Error.Code `InvalidParameterValue` and no cluster created; recorded example message: "Invalid Cache Node Type: cache.not.a.real.type."
Emulator: answers HTTP 200 and creates the cluster from the bogus node type, so no ErrorResponse element is produced at all.
Why: a client that relies on AWS validating the node type gets a "successful" create and a cluster that could never exist on the real service; everything it does with that cluster afterwards is built on an invalid shape, and the failure never surfaces at the call that caused it.
Fix: CacheNodeType validation in the CreateCacheCluster handler, applied before any state mutation.
Traces: elasticache-cache-cluster-lifecycle#task#3.

Cause 2 of 6: DescribeCacheClusters for an absent CacheClusterId answers HTTP 200 instead of 404 CacheClusterNotFound.
AWS: Smithy: DescribeCacheClusters models CacheClusterNotFoundFault (code `CacheClusterNotFound`, HTTP 404). Recorded behavior for an id that does not exist is HTTP 404 with Error.Type `Sender`, Error.Code `CacheClusterNotFound`; recorded example message "CacheCluster not found: <CacheClusterId>".
Emulator: returns HTTP 200 with a cluster body and no ErrorResponse. Hypothesis: the cluster accepted in Cause 1 is still resident, so the absent-id probe resolves.
Why: existence checks that clients build on ("describe, 404 means gone") silently succeed, so deletion/absence logic never takes its error path.
Fix: the DescribeCacheClusters handler's id lookup and not-found path in the cache-cluster state store.
Traces: elasticache-cache-cluster-lifecycle#task#4.

Cause 3 of 6: ModifyCacheCluster on an absent cluster answers 400 InvalidCacheClusterState instead of 404 CacheClusterNotFound.
AWS: Smithy: ModifyCacheCluster models BOTH CacheClusterNotFoundFault (code `CacheClusterNotFound`, HTTP 404) and InvalidCacheClusterStateFault (code `InvalidCacheClusterState`, HTTP 400); recorded behavior for a nonexistent id is the former, HTTP 404 `CacheClusterNotFound`.
Emulator: answers HTTP 400 with Error.Code `InvalidCacheClusterState` (recorded example message "Cache cluster <id> is in creating state, not available."), i.e. a state gate on a resource that should not exist.
Why: the client reads "wait and retry, it is still creating" for a cluster that will never appear, so a converge loop spins instead of failing fast.
Fix: precondition ordering in the ModifyCacheCluster handler — resolve existence (404) before evaluating cluster state.
Traces: elasticache-cache-cluster-lifecycle#task#5.

Cause 4 of 6: RebootCacheCluster on an absent cluster answers 400 InvalidCacheClusterState instead of 404 CacheClusterNotFound.
AWS: Smithy: RebootCacheCluster models only CacheClusterNotFoundFault (code `CacheClusterNotFound`, HTTP 404) and InvalidCacheClusterStateFault (code `InvalidCacheClusterState`, HTTP 400); recorded behavior for a nonexistent id is HTTP 404 `CacheClusterNotFound`.
Emulator: HTTP 400, Error.Code `InvalidCacheClusterState` (recorded example message "Cache cluster <id> is in creating state, not available.").
Why: same misdirection as Cause 3 on the reboot path — a retryable-looking error for a permanent condition.
Fix: existence-before-state ordering in the RebootCacheCluster handler.
Traces: elasticache-cache-cluster-lifecycle#task#6.

Cause 5 of 6: ListAllowedNodeTypeModifications for an absent cluster answers HTTP 200 instead of 404 CacheClusterNotFound.
AWS: Smithy: ListAllowedNodeTypeModifications models CacheClusterNotFoundFault (code `CacheClusterNotFound`, HTTP 404). Recorded behavior for an id that does not exist is HTTP 404 `CacheClusterNotFound`, Error.Type `Sender`.
Emulator: HTTP 200 with a modifications body and no ErrorResponse.
Why: scaling tooling believes it has a valid scale-up/scale-down set for a cluster that does not exist, and chooses a node type from a list the service never produced.
Fix: the ListAllowedNodeTypeModifications handler's cluster/replication-group resolution.
Traces: elasticache-cache-cluster-lifecycle#task#7.

Cause 6 of 6: DeleteCacheCluster on an absent cluster answers 400 InvalidCacheClusterState instead of 404 CacheClusterNotFound.
AWS: Smithy: DeleteCacheCluster models CacheClusterNotFoundFault (code `CacheClusterNotFound`, HTTP 404) alongside InvalidCacheClusterStateFault (400); recorded behavior for a nonexistent id is HTTP 404 `CacheClusterNotFound`.
Emulator: HTTP 400, Error.Code `InvalidCacheClusterState`.
Why: idempotent teardown ("delete, treat 404 as already deleted") never terminates, because the emulator reports a transient-looking state error forever.
Fix: existence-before-state ordering in the DeleteCacheCluster handler.
Traces: elasticache-cache-cluster-lifecycle#task#8.

Related but NOT covering: open issue 20261022T193451Z-elasticache-cache-cluster-subnet-group-validation-is-over.md describes behavioral-suite findings on the same surface (over-permissive validation, state-gate ordering, not-found text). It carries no wire replay key for these fixture steps, so none of the six causes above is exercised by it.

## Evidence
Replay of fixture elasticache-cache-cluster-lifecycle (trace set simparity-wire-aws-v1), all six clusters verdict=mismatch, stub_path empty, owned_by empty:
1. task#3 CreateCacheCluster(CacheNodeType='cache.not.a.real.type') — expected HTTP 400 {Error:{Type:Sender, Code:InvalidParameterValue, Message:'Invalid Cache Node Type: cache.not.a.real.type.'}, RequestId:...}; got HTTP 200, $.ErrorResponse missing.
2. task#4 DescribeCacheClusters(absent id) — expected HTTP 404 Code=CacheClusterNotFound ('CacheCluster not found: <id>'); got HTTP 200, $.ErrorResponse missing.
3. task#5 ModifyCacheCluster(absent id) — expected 404/CacheClusterNotFound; got 400/InvalidCacheClusterState, message 'Cache cluster <id> is in creating state, not available.', and $.ErrorResponse.RequestId missing.
4. task#6 RebootCacheCluster(absent id) — expected 404/CacheClusterNotFound; got 400/InvalidCacheClusterState, $.ErrorResponse.RequestId missing.
5. task#7 ListAllowedNodeTypeModifications(absent id) — expected 404/CacheClusterNotFound; got 200, $.ErrorResponse missing.
6. task#8 DeleteCacheCluster(absent id) — expected 404/CacheClusterNotFound; got 400/InvalidCacheClusterState, $.ErrorResponse.RequestId missing.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture elasticache-cache-cluster-lifecycle from trace set simparity-wire-aws-v1.
2. Cause 1: step task#3 — CreateCacheCluster with CacheNodeType='cache.not.a.real.type' and expect HTTP 400 InvalidParameterValue with no cluster created.
3. Causes 2-6: steps task#4, task#5, task#6, task#7, task#8 — DescribeCacheClusters / ModifyCacheCluster / RebootCacheCluster / ListAllowedNodeTypeModifications / DeleteCacheCluster against a CacheClusterId that was never created, each expecting HTTP 404 with Error.Code CacheClusterNotFound.
4. Confirm the fix by re-replaying every step named in Traces, not only task#3.

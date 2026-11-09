Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.delete-replication-group-snapshotting-state-blocked elasticache.elasticache-concurrent-incr-on-one-key-loses-no-increments elasticache.elasticache-redis-maxmemory-policy-evicts-under-pressure
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.delete-replication-group-snapshotting-state-blocked|_assert_error#14|AssertionError|assert code == expected_code, f"expected {expected_code}, got {code}"
Issue-Key: elasticache.elasticache-concurrent-incr-on-one-key-loses-no-increments|_ready_prober#10|TimeoutError|raise TimeoutError(f"a RESP connection to {subject} at {host}:{port}: the run budget ended first; not graded")
Issue-Key: elasticache.elasticache-redis-maxmemory-policy-evicts-under-pressure|_ready_prober#18|TimeoutError|raise TimeoutError(f"a RESP connection to {subject} at {host}:{port}: the run budget ended first; not graded")
Issue-Commit: e3ea9c3d18c5ac215ea88ebad8a0c22355767e25

# ElastiCache: DeleteReplicationGroup never applies the snapshot-in-progress state gate (answers NotImplemented), and a cache cluster reported 'available' accepts no RESP connection on the endpoint it advertises (2 causes)

> _Note: this issue was identified against the code base at commit `e3ea9c3d18c5ac215ea88ebad8a0c22355767e25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteReplicationGroup

## Description
Two net-new root causes in the elasticache emulator, both grounded in failing behavioral checks that the suite did NOT mark ALREADY FILED or OWNED.

Cause 1 of 2: DeleteReplicationGroup does not refuse a delete while a snapshot of a member cache cluster is in progress — it answers the generic NotImplemented error instead of the modeled InvalidReplicationGroupState.
  AWS: Smithy (services/service-com-amazonaws-elasticache-amazonelasticachev9) models DeleteReplicationGroup with error InvalidReplicationGroupStateFault, awsQueryError code `InvalidReplicationGroupState`, HTTP 400. AWS refuses DeleteReplicationGroup while a member cluster is being snapshotted and returns 400 InvalidReplicationGroupState with the message `Cache cluster <cache-cluster-id> is not in a valid state to be deleted.` — fixed text is `Cache cluster ` + ` is not in a valid state to be deleted.`; the single variable part is the member cluster id taken from the request's replication group state (the id in MemberClusters), never a literal from one run. The group must still exist afterwards, and once the snapshot reaches `available` the same DeleteReplicationGroup must succeed and the group must disappear.
  Emulator: the call is answered with error code `NotImplemented` (recorded verdict: `AssertionError: expected InvalidReplicationGroupState, got NotImplemented`), so neither the state gate nor the modeled success path exists; no refusal code, no AWS message, and the positive control (delete after the snapshot completes) is unreachable.
  Why: a client that guards teardown on InvalidReplicationGroupState (retry-until-deletable loops, CloudFormation-style waiters, the check's own `_delete_replication_group_and_wait_gone`) sees an unmodeled error code it cannot classify; it either aborts teardown or treats the group as permanently undeletable, and a client that expects the delete to be rejected while snapshotting instead learns nothing about the group's real state.
  Fix: the DeleteReplicationGroup handler and its state validation (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_replication_group.rs) plus the replication-group/snapshot state it must consult. Related open issue 20261108T093128Z-elasticache-deletereplicationgroup-stub.md records that this operation is an unimplemented scaffold stub; it does not state this state-gate refusal, its code, or its message, which is what this cause requires.
  Traces: elasticache.delete-replication-group-snapshotting-state-blocked.

Cause 2 of 2: a cache cluster the control plane reports `available` advertises a node endpoint on the requested port, but nothing at that endpoint ever accepts a RESP connection, so no data-plane behavior can be exercised.
  AWS: recorded behavior only (the Smithy model describes the control plane, not RESP). In AWS, once DescribeCacheClusters reports CacheClusterStatus `available` and returns CacheNodes[].Endpoint {Address, Port}, a client inside the cluster's VPC can open a TCP connection to that address/port and a PING is answered with +PONG; subsequent commands (INCR, SET/GET, eviction under a maxmemory-policy) behave as the Redis/Valkey engine does.
  Emulator: the control plane provisions the cluster, reports it available and advertises an address and the requested port 6379, but repeated RESP connection attempts from the runner over the whole polling budget never get a reply — the probe loop exhausts its budget (`TimeoutError: a RESP connection to cache cluster <cluster-id> at <address>:6379: the run budget ended first`). An advertised endpoint that never accepts a connection is not a data plane.
  Why: every client that actually uses an ElastiCache cluster connects to the advertised endpoint. Against this emulator the control plane claims success and hands out an endpoint that cannot be connected to, so application code hangs or fails at connect time and no cache semantics (atomic INCR under concurrency, maxmemory eviction) can be observed.
  Fix: the cache-cluster data plane — whatever component is supposed to listen on the address/port the cache-cluster handler and its state advertise (the cache-cluster endpoint serializer must only advertise an address a RESP listener is actually bound to). Related open issues 20261108T152921Z-elasticache-in-vpc-data-plane-unverifiable-the-emulator.md and 20261105T090541Z-elasticache-every-in-vpc-data-plane-check-is.md describe the in-VPC Lambda probe runtime (_ssl / register_at_fork) blocking other data-plane checks; they do not cover these two checks, whose probe runs locally on the runner and still finds nothing listening on the advertised endpoint.
  Traces: elasticache.elasticache-concurrent-incr-on-one-key-loses-no-increments, elasticache.elasticache-redis-maxmemory-policy-evicts-under-pressure.

## Evidence
Cause 1 — check elasticache.delete-replication-group-snapshotting-state-blocked:
  Expected: ClientError with Error.Code == `InvalidReplicationGroupState` (HTTP 400) and Error.Message == `Cache cluster <member-cluster-id> is not in a valid state to be deleted.`, the replication group still present on a following DescribeReplicationGroups, and a later DeleteReplicationGroup (snapshot `available`) succeeding with Status `deleting` then the group gone.
  Actual: `AssertionError: expected InvalidReplicationGroupState, got NotImplemented` at check_elasticache_delete_replication_group_snapshotting_state_blocked.py:110 (_assert_error), i.e. DeleteReplicationGroup answered the NotImplemented error code.

Cause 2 — checks elasticache.elasticache-concurrent-incr-on-one-key-loses-no-increments and elasticache.elasticache-redis-maxmemory-policy-evicts-under-pressure:
  Expected: after CreateCacheCluster -> `available` and a node Endpoint whose Port is the requested 6379, a RESP `PING` on that address/port is answered, then INCR replies are exactly 1..N (no lost updates) / keys are evicted per the maxmemory-policy.
  Actual: the prober's PING is never answered; the loop runs the whole budget and raises `TimeoutError: a RESP connection to cache cluster <cluster-id> at <address>:6379: the run budget ended first; not graded` (check...concurrent_incr...py:548 / check...maxmemory_policy...py:561). Nothing accepted a RESP connection on the endpoint the control plane advertised.

## Steps to Reproduce
Cause 1:
1. CreateCacheSubnetGroup over a subnet of a fresh VPC.
2. CreateReplicationGroup (Engine=redis, CacheNodeType=cache.t3.micro, NumCacheClusters=1) and wait for Status `available`.
3. Read the group's sole MemberClusters id and call CreateSnapshot(CacheClusterId=<member>, SnapshotName=<name>); wait until DescribeSnapshots reports the snapshot (SnapshotStatus `creating`).
4. Call DeleteReplicationGroup(ReplicationGroupId=<rg>). Expect 400 InvalidReplicationGroupState with `Cache cluster <member> is not in a valid state to be deleted.`; the emulator answers NotImplemented.
5. Wait for the snapshot to reach `available` and call DeleteReplicationGroup again: it must succeed and the group must go away.

Cause 2:
1. CreateCacheCluster (Engine=redis, CacheNodeType=cache.t3.micro, Port=6379) in a cache subnet group over private subnets; wait for CacheClusterStatus `available`.
2. Read CacheNodes[0].Endpoint {Address, Port} from DescribeCacheClusters(ShowCacheNodeInfo=true).
3. From inside the cluster's VPC open a TCP socket to that Address:Port and send RESP `PING`. Expect +PONG; the emulator never answers, so the probe loop exhausts its budget.
4. With a connection established, issue concurrent INCR on one key (replies must be exactly 1..N) and fill the cache past maxmemory to observe the configured eviction policy.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.concurrent-update-cluster-shard-capacity-conflict
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.concurrent-update-cluster-shard-capacity-conflict|run#9|AssertionError|assert len(successes) == 1, f"expected exactly one accepted UpdateCluster, got {results}"
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# DocDB Elastic UpdateCluster accepts concurrent shard-capacity modifications instead of rejecting the second with ConflictException

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCluster

## Description
Cause 1 of 1: UpdateCluster does not treat a cluster that is already being modified as a conflicting state, so two overlapping modifications both apply silently.
AWS: UpdateCluster on an ACTIVE elastic cluster moves it out of ACTIVE into a transient modifying state for the duration of the change; a second UpdateCluster issued against the same cluster while that change is in flight is REFUSED with 409 ConflictException. Of two concurrent UpdateCluster calls that each set a different shardCapacity, exactly one is accepted and exactly one is rejected with error code ConflictException. Smithy: UpdateCluster (PUT /cluster/{clusterArn}) models ConflictException at httpError 409, documented "There was an access conflict"; the single-writer semantics for an in-flight cluster modification are recorded behavior (the check asserts the error CODE, not a message, so no message text is required here).
Emulator: both concurrent calls returned 200 — the replay observed results [('ok', 4), ('ok', 8)], i.e. two accepted UpdateCluster calls with conflicting shardCapacity values and no ConflictException at all. Nothing guards the cluster's state transition, so the second write simply overwrites the first.
Why: this is an over-permissive accept of a call real AWS refuses. A client that relies on ConflictException to serialize its own control-plane changes (retry-on-conflict, leader election over a cluster, idempotent reconcilers) never sees the conflict, so two reconcilers can both believe they won and the cluster's final shardCapacity is a race — the opposite of the last-writer-detected contract AWS provides. Code paths handling 409 are also dead against the emulator.
Fix: UpdateCluster's handler and the cluster's state machine in the service state (services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/update_cluster.rs plus the shared cluster state) — the handler must transition the cluster out of ACTIVE for the duration of the modification and refuse any further UpdateCluster on a cluster not in a modifiable status with ConflictException (409).
Traces: docdb-elastic.concurrent-update-cluster-shard-capacity-conflict.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, [mismatch] docdb-elastic.concurrent-update-cluster-shard-capacity-conflict:
AssertionError: expected exactly one accepted UpdateCluster, got [('ok', 4), ('ok', 8)]
The check creates a cluster, waits for ACTIVE, then fires two concurrent UpdateCluster calls with different shardCapacity values and requires len(successes) == 1, len(errors) == 1 and errors[0] == "ConflictException". The emulator returned success for both, so no 409 ConflictException was produced. The specific capacities 4 and 8 are the check's per-run inputs, not part of the requirement; the requirement is that exactly one concurrent modification is accepted and the other is refused with code ConflictException.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. POST /cluster (CreateCluster) with valid required members and wait until GetCluster reports status ACTIVE.
3. Issue two UpdateCluster calls (PUT /cluster/{clusterArn}) concurrently against that clusterArn, one with shardCapacity=4 and one with shardCapacity=8.
4. Observe that BOTH return 200. Expected: exactly one returns 200 and the other returns 409 with error code ConflictException because the cluster is already being modified.
5. Or re-replay behavioral check docdb-elastic.concurrent-update-cluster-shard-capacity-conflict.

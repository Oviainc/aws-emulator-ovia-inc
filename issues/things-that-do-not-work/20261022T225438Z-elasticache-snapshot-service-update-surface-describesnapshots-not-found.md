Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.snapshot-not-found-semantics elasticache.snapshot-retention-limit-zero-disables-automatic-snapshots elasticache.batch-update-action-unknown-service-update elasticache.snapshot-retention-limit-auto-expiry
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeSnapshots|SnapshotNotFoundFault
Issue-Key: elasticache|DescribeSnapshots|InvalidParameterValue
Issue-Key: elasticache.batch-update-action-unknown-service-update|_assert_error#8|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code}"
Issue-Key: elasticache|CreateSnapshot|InvalidCacheClusterState
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# ElastiCache snapshot/service-update surface: DescribeSnapshots not-found and source-filter semantics, Batch*UpdateAction NotImplemented, and an immediate ModifyCacheCluster locking the cluster out of CreateSnapshot (4 causes)

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSnapshots

## Description
Four distinct, independent root causes in the emulator's ElastiCache snapshot and service-update surface. Each must be closed separately; they are bundled only because they all belong to the `elasticache` service. None of them is covered by the already-open issues 20261022T193451Z-elasticache-nine-operation-families-answer-notimplemented-instead-of.md (which enumerates nine OTHER operation families and does not mention Batch{Apply,Stop}UpdateAction, DescribeSnapshots or ModifyCacheCluster) or 20261022T193451Z-elasticache-cache-cluster-subnet-group-validation-is-over.md (cache-cluster/subnet-group create-time validation), nor by any of the auto-filed per-operation stub issues.

Cause 1 of 4: DescribeSnapshots with a SnapshotName that does not exist answers 404 instead of an empty list.
AWS: Smithy (com-amazonaws-elasticache-amazonelasticachev9) models DescribeSnapshots -> DescribeSnapshotsListMessage{Marker, Snapshots} with errors CacheClusterNotFoundFault (404), InvalidParameterCombination (400), InvalidParameterValue (400), SnapshotNotFoundFault (awsQueryError `SnapshotNotFoundFault`, 404). Recorded behavior: DescribeSnapshots is a FILTER, so an unknown `SnapshotName` returns HTTP 200 with `Snapshots: []`; the modeled SnapshotNotFoundFault is reserved for the mutating operations on a snapshot name (DeleteSnapshot, CopySnapshot). The read/mutate asymmetry is the whole contract of the check.
Emulator: returns HTTP 404 `SnapshotNotFoundFault` with message "Snapshot <name> not found." for any SnapshotName the store does not hold (the handler short-circuits on `!store.snapshots.contains_key(&key)` before building the page, and its comment states the choice explicitly).
Why: every client that probes for a snapshot by name before creating or restoring one sees a hard error where AWS returns an empty page. Code written against AWS treats `Snapshots == []` as "not there yet" and only special-cases SnapshotNotFound on the mutation path; against this emulator the probe raises, so poll/wait loops (including every `_wait_for_snapshot_status`-style loop) abort instead of waiting, and `exists?` helpers report a transport failure.
Fix: the DescribeSnapshots handler's name filter — the unknown-name branch must fall through to an empty page rather than returning the modeled fault; the fault belongs to the snapshot mutations.
Traces: elasticache.snapshot-not-found-semantics (the check fails on its FIRST call: `describe_snapshots(SnapshotName=<unknown>)` raised SnapshotNotFoundFault, so nothing after it was exercised).

Cause 2 of 4: DescribeSnapshots rejects the `automated`/`manual` spellings of SnapshotSource.
AWS: Smithy: `DescribeSnapshotsMessage$SnapshotSource` is a plain String with NO enum/pattern constraint; its documentation reads "If set to system, the output shows snapshots that were automatically created by ElastiCache. If set to user the output shows snapshots that were manually created. If omitted, the output shows both". Recorded behavior shows AWS ALSO accepts the spellings the response field itself uses — `Snapshot$SnapshotSource` is reported as `automated`/`manual` — so `DescribeSnapshots(CacheClusterId=..., SnapshotSource="automated")` answers HTTP 200 with the automatically-created snapshots (an empty list on a cluster with backups disabled) rather than refusing the request.
Emulator: answers HTTP 400 `InvalidParameterValue`, recorded example message "Invalid value automated for SnapshotSource: must be system or user." — it maps only `system`/`user` and refuses everything else, even though the same handler's own comment notes the stored source is spelled `automated`/`manual`.
Why: a client that filters by the value the API gave it back (`automated` / `manual`) is refused, so the round trip "read a snapshot's SnapshotSource, then list only snapshots of that source" is impossible; the accepted vocabulary must be a superset of the one the service emits.
Fix: the DescribeSnapshots handler's SnapshotSource mapping (accept the response spellings alongside system/user, and keep 400 InvalidParameterValue only for values in neither vocabulary).
Traces: elasticache.snapshot-retention-limit-zero-disables-automatic-snapshots.

Cause 3 of 4: BatchApplyUpdateAction / BatchStopUpdateAction answer NotImplemented instead of their modeled result and fault.
AWS: Smithy: both operations take {ReplicationGroupIds (list, length 0..20), CacheClusterIds (list, length 0..20), ServiceUpdateName (required)} and return UpdateActionResultsMessage{ProcessedUpdateActions, UnprocessedUpdateActions}; their ONLY modeled errors are InvalidParameterValueException (awsQueryError `InvalidParameterValue`, 400) and ServiceUpdateNotFoundFault (awsQueryError `ServiceUpdateNotFoundFault`, 404). Recorded behavior: a `ServiceUpdateName` that is not in the account/region catalogue is refused with 404 `ServiceUpdateNotFoundFault` (recorded example message "ServiceUpdate <name> not found."), while the SAME call with a name DescribeServiceUpdates returned does not raise that fault — it returns the results message, reporting the unknown replication-group id under UnprocessedUpdateActions.
Emulator: both operations are scaffold stubs (`services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/batch_apply_update_action.rs` carries the SCAFFOLDED marker, as does `batch_stop_update_action.rs`) and answer error code `NotImplemented`, message "operation BatchApplyUpdateAction is scaffolded but not implemented in this emulator". Note the suite did NOT mark this check as covered by an auto-filed stub issue, and no open issue names these two operations.
Why: the fault that keys on an unknown service-update name is unreachable, so a client cannot tell a stale update name from a service outage, and the positive path (apply/stop an update that exists, with per-resource processed/unprocessed accounting) cannot be exercised at all. `NotImplemented` is not a modeled error for these operations, so SDK error handling never dispatches on it.
Fix: the BatchApplyUpdateAction and BatchStopUpdateAction handlers plus a shared ServiceUpdateName lookup against the service-update catalogue already held in `state.rs` (the catalogue DescribeServiceUpdates serves).
Traces: elasticache.batch-update-action-unknown-service-update.

Cause 4 of 4: an ApplyImmediately=True ModifyCacheCluster publishes the new value while simultaneously taking the cluster out of a snapshot-able state, so the next call is refused.
AWS: Smithy: ModifyCacheCluster returns the CacheCluster and models InvalidCacheClusterStateFault (awsQueryError `InvalidCacheClusterState`, 400); CreateSnapshot models the same fault. Recorded behavior: changing only a NON-disruptive setting such as `SnapshotRetentionLimit` with ApplyImmediately=True does not make the cluster unusable — once DescribeCacheClusters reports the new `SnapshotRetentionLimit`, the immediately following `CreateSnapshot` on that cluster is accepted. The invariant the recording relies on is that the applied value and a snapshot-able cluster state become visible together; AWS never publishes the applied value while still refusing state-gated calls on it.
Emulator: ModifyCacheCluster applies non-pending members to the stored record AND unconditionally calls `begin_transition(STATUS_MODIFYING)` for every immediate modify, with a fixed transition span (state.rs `CLUSTER_TRANSITION`). DescribeCacheClusters therefore reports `SnapshotRetentionLimit = 3` at once while `CacheClusterStatus` is still `modifying`, and the next CreateSnapshot is refused with HTTP 400 `InvalidCacheClusterState`, recorded example message "Cache cluster <id> is in modifying state, not available."
Why: the documented way to confirm a modify landed is to read the field back; a client that does exactly that is then refused by the very next state-gated operation, which no amount of correct client code anticipates. It also makes a non-disruptive settings change behave like a disruptive one.
Fix: the ModifyCacheCluster handler's state machine — a modify that applies only members outside PendingModifiedValues must leave the cluster `available` (or must not publish the applied value before the cluster is settled again); the shared cluster-settling helper in `state.rs` is the other half.
Traces: elasticache.snapshot-retention-limit-auto-expiry.

## Evidence
Replayed against the emulator (behavioral suite simparity-behavioral-aws-v1; elasticache 2/139 passed):

Cause 1 — elasticache.snapshot-not-found-semantics [error]:
  call: DescribeSnapshots(SnapshotName="sim2real-behav-ecs-missing-a368abf052d0")
  expected: HTTP 200, {"Snapshots": []}
  actual:   botocore.errorfactory.SnapshotNotFoundFault: An error occurred (SnapshotNotFoundFault) when calling the DescribeSnapshots operation: Snapshot sim2real-behav-ecs-missing-a368abf052d0 not found. (HTTP 404)
  The check aborts here, before its DeleteSnapshot/CopySnapshot/CreateSnapshot legs run.

Cause 2 — elasticache.snapshot-retention-limit-zero-disables-automatic-snapshots [error]:
  call: DescribeSnapshots(CacheClusterId=<cluster>, SnapshotSource="automated")
  expected: HTTP 200, {"Snapshots": []} (no automatic snapshots with SnapshotRetentionLimit=0)
  actual:   botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValue) when calling the DescribeSnapshots operation: Invalid value automated for SnapshotSource: must be system or user. (HTTP 400)

Cause 3 — elasticache.batch-update-action-unknown-service-update [mismatch]:
  call: BatchApplyUpdateAction(ServiceUpdateName="sim2real-behav-esu-<id>", ReplicationGroupIds=["sim2real-behav-erg-upd-<id>"])
  expected: HTTP 404, Error.Code ServiceUpdateNotFoundFault (recorded example message "ServiceUpdate <name> not found.")
  actual:   AssertionError: expected ServiceUpdateNotFoundFault, got NotImplemented
  Same for BatchStopUpdateAction. The check first establishes the positive control with a name from DescribeServiceUpdates (which the emulator does serve), so the divergence is specific to the two batch operations.

Cause 4 — elasticache.snapshot-retention-limit-auto-expiry [error]:
  sequence: CreateCacheCluster(Engine=redis, cache.t3.micro, NumCacheNodes=1, SnapshotRetentionLimit=1) -> wait available -> ModifyCacheCluster(SnapshotRetentionLimit=3, ApplyImmediately=True) -> DescribeCacheClusters reports SnapshotRetentionLimit=3 -> CreateSnapshot(CacheClusterId=<cluster>, SnapshotName=<name>)
  expected: CreateSnapshot accepted (HTTP 200) once the modified value is observable
  actual:   botocore.errorfactory.InvalidCacheClusterStateFault: An error occurred (InvalidCacheClusterState) when calling the CreateSnapshot operation: Cache cluster sim2real-snaplimit-c186e000a974 is in modifying state, not available. (HTTP 400)

## Steps to Reproduce
Boot the emulator and replay the four checks named in trace_ids, or drive the elasticache client directly:
1. DescribeSnapshots(SnapshotName="<a name never created>") -> expect HTTP 200 with Snapshots: []; observe HTTP 404 SnapshotNotFoundFault.
2. CreateCacheCluster(Engine="redis", CacheNodeType="cache.t3.micro", NumCacheNodes=1, SnapshotRetentionLimit=0); wait for CacheClusterStatus=available; DescribeSnapshots(CacheClusterId=<id>, SnapshotSource="automated") -> expect HTTP 200 with Snapshots: []; observe HTTP 400 InvalidParameterValue "Invalid value automated for SnapshotSource: must be system or user." (same for SnapshotSource="manual").
3. ServiceUpdateName = DescribeServiceUpdates().ServiceUpdates[0].ServiceUpdateName; BatchApplyUpdateAction(ServiceUpdateName=<that name>, ReplicationGroupIds=["<unknown id>"]) -> expect the UpdateActionResultsMessage (no ServiceUpdateNotFoundFault); then BatchApplyUpdateAction(ServiceUpdateName="<a name not in the catalogue>", ReplicationGroupIds=["<unknown id>"]) -> expect HTTP 404 ServiceUpdateNotFoundFault; observe NotImplemented in both cases. Repeat with BatchStopUpdateAction.
4. CreateCacheCluster(Engine="redis", CacheNodeType="cache.t3.micro", NumCacheNodes=1, SnapshotRetentionLimit=1); wait available; ModifyCacheCluster(CacheClusterId=<id>, SnapshotRetentionLimit=3, ApplyImmediately=True); poll DescribeCacheClusters until SnapshotRetentionLimit==3; immediately CreateSnapshot(CacheClusterId=<id>, SnapshotName="<new>") -> expect HTTP 200; observe HTTP 400 InvalidCacheClusterState "...is in modifying state, not available."

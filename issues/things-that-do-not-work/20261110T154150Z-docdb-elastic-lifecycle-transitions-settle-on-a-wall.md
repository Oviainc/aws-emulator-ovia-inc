Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.copy-cluster-snapshot-success docdb-elastic.restore-cluster-from-snapshot-success
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.copy-cluster-snapshot-success|_wait_for_status#15|Timeout|time.sleep(interval_s)
Issue-Key: docdb-elastic.restore-cluster-from-snapshot-success|_wait_for_cluster_status#14|Timeout|time.sleep(interval_s)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# DocDB Elastic lifecycle transitions settle on a wall-clock window instead of being settled by the first read, so the CopyClusterSnapshot and RestoreClusterFromSnapshot happy paths never complete

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyClusterSnapshot

## Description
Cause 1 of 1 — every just-minted cluster and snapshot is pinned in its transient status for a fixed wall-clock window measured from the mutating call, so the FIRST read a polling client makes after that call always still reports the transient status, and a happy-path flow that chains three transitions can never be observed to reach its settled state inside a check's wall-clock budget. Failing checks: docdb-elastic.copy-cluster-snapshot-success, docdb-elastic.restore-cluster-from-snapshot-success.

AWS: Smithy: ClusterSnapshot$status and Cluster$status target `Status`, whose values include CREATING, COPYING, ACTIVE, STOPPING, STOPPED, DELETING; CopyClusterSnapshot is documented as "Copies a snapshot of an elastic cluster" and RestoreClusterFromSnapshot as "Restores an elastic cluster from a snapshot". The model states no timing. Recorded behavior only for the pacing: the recorded happy path is that the mutating call's OWN response carries the transient status (CREATING for a created/restored cluster or a created snapshot, COPYING for a copy) and a subsequent Get* converges on ACTIVE, at which point the copy carries its own snapshotArn/snapshotName, the source clusterArn and snapshotType MANUAL, and the restored cluster carries its own clusterArn/clusterEndpoint with the source's shard topology and adminUserName. Nothing in the model or the recordings makes the transient status survive a client's later reads.

Emulator: state.rs pins the transient status to a 500 ms wall-clock due instant (`const SETTLE_AFTER: Duration = Duration::from_millis(500)`, `settle_instant()`, `ClusterRecord::begin_transition`, `ClusterRecord::settle_due`, `SnapshotRecord::settle_due`), and every mint path stores `settles_at: Some(state::settle_instant())` (create_cluster_snapshot.rs, copy_cluster_snapshot.rs) or calls `record.begin_transition(state::STATUS_CREATING)` (restore_cluster_from_snapshot.rs). A reader issued milliseconds after the mutating call is therefore always inside the window and is answered the transient status, so each transition costs a polling client one whole poll interval. Both failing checks chain exactly three transitions (cluster -> ACTIVE, snapshot -> ACTIVE, then the copy -> ACTIVE / the restored cluster -> ACTIVE) and poll at 20 s, and both are cut off at exactly "Check timed out after 60.0s" — 3 x 20 s — one poll short of the final ACTIVE. Hypothesis: the 60 s cut-off is reached because each of the three waits burns its first poll on the pinned transient status; no status is permanently stuck (settle_due does complete COPYING/CREATING -> ACTIVE), and the suite's two-transition docdb-elastic flows do finish.

Why: a client that performs the documented happy path — copy a snapshot, or restore a cluster from a snapshot, and wait for ACTIVE — can never observe the completed resource within the budget, so the end state of CopyClusterSnapshot and RestoreClusterFromSnapshot (the copy's distinct ARN, its MANUAL type and source clusterArn; the restored cluster's own endpoint, inherited shard topology and adminUserName) is unverifiable and any workflow gated on ACTIVE stalls proportionally to its poll interval for every transition it chains.

Fix: the settle pacing in this service's state, and the mint paths that arm it: a resource should already be settled by the time of the first subsequent read (the transient status belongs to the mutating call's own response, which the handlers already return), rather than being withheld for a wall-clock window that outlives that read. The state-gated refusals that need a transient window (the concurrent-UpdateCluster ConflictException and the DeleteCluster/StartCluster state refusals) must keep working off the record's own transient status, so the window must not simply be removed where those refusals depend on it.

Traces: docdb-elastic.copy-cluster-snapshot-success and docdb-elastic.restore-cluster-from-snapshot-success, both [mismatch] "Check timed out after 60.0s". Code: services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/state.rs lines 19-26 (SETTLE_AFTER), 222-250 (begin_transition/settle_due), 334-342 (SnapshotRecord::settle_due), 438-441 (settle_instant); src/ops/create_cluster_snapshot.rs line 229; src/ops/copy_cluster_snapshot.rs lines 263-274; src/ops/restore_cluster_from_snapshot.rs lines 284-285. Related but distinct open issues on this service concern message wording and state gating (20261102T113456Z-docdb-elastic-conflict-validation-refusals-on-createcluster-updatecluster.md, 20261107T123650Z-docdb-elastic-deletecluster-is-not-state-gated-a.md); neither covers this pacing.

## Evidence
Expected (AWS / recorded happy path):
- CopyClusterSnapshot: GetClusterSnapshot on the copy converges to status ACTIVE; the copy's snapshotArn differs from the source's, snapshotName is the requested targetSnapshotName (lowercased), clusterArn is the source snapshot's clusterArn, snapshotType is MANUAL.
- RestoreClusterFromSnapshot: GetCluster on the restored cluster converges to status ACTIVE with its own clusterArn and clusterEndpoint, the source's shardCapacity/shardCount/shard count and the snapshot's adminUserName.

Actual (emulator):
- docdb-elastic.copy-cluster-snapshot-success — [mismatch] "Error: Check timed out after 60.0s". The flow is CreateCluster -> wait ACTIVE, CreateClusterSnapshot -> wait ACTIVE, CopyClusterSnapshot -> wait ACTIVE, polling every 20 s; the first poll of each wait is answered with the transient status (CREATING, CREATING, COPYING) because the record's settles_at is 500 ms in the future when that poll arrives, so the third wait is still pending at the 60 s cut-off.
- docdb-elastic.restore-cluster-from-snapshot-success — [mismatch] "Error: Check timed out after 60.0s". Same three-transition flow (CreateCluster, CreateClusterSnapshot, RestoreClusterFromSnapshot), same arithmetic.
- Both checks' own HTTP calls succeed (200); no error code and no refusal is involved — the resources are simply never reported settled before the budget is gone. For contrast, the suite's docdb-elastic flows that chain only two transitions complete.

Source evidence: state.rs `const SETTLE_AFTER: Duration = Duration::from_millis(500)` with the comment "The span is deliberately non-zero, so a caller acting straight after CreateCluster still observes CREATING"; `settle_due` only completes the move once `Instant::now() >= due`.

## Steps to Reproduce
1. Boot the emulator and point a docdb-elastic client at it.
2. CopyClusterSnapshot path: CreateCluster(authType=PLAIN_TEXT, shardCapacity=2, shardCount=1); poll GetCluster every 20 s until ACTIVE; CreateClusterSnapshot on that clusterArn; poll GetClusterSnapshot every 20 s until ACTIVE; CopyClusterSnapshot(snapshotArn=<source>, targetSnapshotName=<new>); poll GetClusterSnapshot on the copy every 20 s until ACTIVE. Observe that each of the three waits needs at least two polls because the poll issued immediately after each mutating call is answered the transient status, so the flow needs >60 s of wall clock and the check is cut off at 60 s.
3. RestoreClusterFromSnapshot path: CreateCluster(shardCapacity=2, shardCount=2); wait ACTIVE; CreateClusterSnapshot; wait ACTIVE; RestoreClusterFromSnapshot(clusterName=<new>, snapshotArn=<snapshot>); poll GetCluster on the restored ARN until ACTIVE. Same three-wait stall.
4. Minimal reproduction of the cause: call CreateClusterSnapshot and then GetClusterSnapshot immediately; the status is CREATING, and it stays transient for any read issued within 500 ms of the create.

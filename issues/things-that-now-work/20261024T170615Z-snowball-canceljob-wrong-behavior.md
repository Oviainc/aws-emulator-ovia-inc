Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# Cancelling a cluster node job does not release its slot in the cluster's five-node quorum

_Filed: 2026-10-24T17:06:15Z_
_Operation: CancelJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A cluster's quorum is computed purely from the number of node job IDs ever attached to the cluster, with no regard for whether those node jobs are still live. `CancelJob` moves a cluster node job to `Cancelled` but leaves the job's ID in `StoredCluster.node_job_ids`, and nothing ever recomputes the cluster's state downward.

Two observable consequences:

1. A cluster that has lost a node still behaves as if it has its full quorum. Create a cluster with `InitialClusterSize=5` (cluster becomes `Pending`), then `CancelJob` one or more of its node jobs. `DescribeCluster` still reports `ClusterState=Pending`, and a `CreateJob` naming that `ClusterId` — the request that should supply the replacement node — is refused with `ClusterLimitExceededException` even though the cluster now holds fewer than five live nodes. The cluster can also no longer be cancelled (`CancelCluster` requires `AwaitingQuorum`), so it is wedged: it cannot be completed, re-provisioned, or cancelled.

2. The quorum can be reached with fewer than five live nodes. Create a cluster with `InitialClusterSize=4` (`AwaitingQuorum`), `CancelJob` one node, then `CreateJob` with that `ClusterId`. The new node makes `node_job_ids.len() == 5`, so `add_cluster_node_jobs` flips the cluster to `Pending` while only four of its node jobs are live.

Expected: a cancelled node job is not a node the cluster will ship with, so it must not count toward the five-node quorum — `ClusterLimitExceededException` documents the rule as "If you have fewer than five nodes for your cluster and you have more nodes to create for this cluster, try again and create jobs until your cluster has exactly five nodes", and `CreateCluster` documents "The cluster does not ship until these five node jobs have been created". So after cancelling a node, a replacement `CreateJob` against the cluster must be accepted, and a cluster short of five live nodes must not report/behave as though it has its quorum.

Actual: cancelled node jobs are counted as nodes forever; the cluster's state and its quorum accounting are computed from a list that only ever grows.

## Evidence

Model/spec grounding: `ClusterLimitExceededException` — "Job creation failed. Currently, clusters support five nodes. If you have fewer than five nodes for your cluster and you have more nodes to create for this cluster, try again and create jobs until your cluster has exactly five nodes." `CreateCluster` — "Each cluster supports five nodes. You use the CreateJob action separately to create the jobs for each of these nodes. The cluster does not ship until these five node jobs have been created." `CancelJob` — "You can only cancel a job before its JobState value changes to PreparingAppliance" (so cancelling a freshly created `New` node job is permitted).

Handler/state evidence:
- `src/ops/cancel_job.rs`, `handle` (~lines 136-195): looks the job up in `store.jobs`, checks `job_state.is_cancellable()`, then sets `job_state = JobState::Cancelled` for the job and its job parts. It never touches `store.clusters`, so `node_job_ids` and `cluster_state` are left exactly as they were — even when the cancelled job is a cluster node (`StoredJob.cluster_id` is `Some`).
- `src/state.rs`, `StoredCluster::has_quorum()`: `self.node_job_ids.len() >= CLUSTER_NODE_QUORUM` — a pure count with no `JobState` filter.
- `src/state.rs`, `add_cluster_node_jobs` (~line 1290): `if cluster.cluster_state == ClusterState::AwaitingQuorum && cluster.has_quorum() { cluster.cluster_state = ClusterState::Pending; }` — the only quorum-driven state move, and it is one-way.
- `src/ops/create_job.rs`, cluster branch (~lines 314-343): refuses with `ClusterLimitExceededException` on `cluster.has_quorum()`, i.e. on the stale count.

Test coverage: none. `tests/all/cluster_cancellation.rs` only covers `CancelCluster` cascading down to its nodes (cluster-first direction) and cancel-twice / unknown-cluster refusals; `tests/all/job_lifecycle.rs` `cancel_job_moves_the_job_to_cancelled_once` cancels standalone jobs and export orders only. No test cancels a cluster node job and then re-reads the cluster or creates a replacement node, so the behavior is unasserted.

## Steps to Reproduce

1. CreateAddress with a deliverable address for the request's region; keep the returned AddressId.
2. CreateCluster with {"JobType": "LOCAL_USE", "SnowballType": "EDGE", "ShippingOption": "SECOND_DAY", "AddressId": "<id>", "ForceCreateJobs": false, "InitialClusterSize": 5}; keep the ClusterId and the five JobIds from JobListEntries.
3. DescribeCluster on that ClusterId — ClusterState is Pending.
4. CancelJob on one of the five node JobIds — succeeds (the node was in JobState New).
5. DescribeJob on that node — JobState is Cancelled; DescribeCluster still reports ClusterState=Pending.
6. CreateJob with {"ClusterId": "<that cluster>"} to supply the replacement node — observe a 400 ClusterLimitExceededException although the cluster now has only four live node jobs.
7. CancelCluster on that ClusterId — observe a 400 InvalidJobStateException (not AwaitingQuorum), so the cluster cannot be re-provisioned or cancelled.
8. Second variant: repeat steps 1-2 with "InitialClusterSize": 4 (cluster is AwaitingQuorum), CancelJob one node, then CreateJob with that ClusterId — the create succeeds and DescribeCluster flips to Pending with only four live node jobs.

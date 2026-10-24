Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# CreateJob adds a node job to a cancelled cluster instead of refusing the create

_Filed: 2026-10-24T15:57:00Z_
_Operation: CreateJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateJob` with a `ClusterId` only checks that the cluster exists and that it has not yet reached its five-node quorum. It never looks at the cluster's `ClusterState`. So after `CancelCluster` has moved a cluster to `Cancelled` (and cancelled the node jobs it held), a further `CreateJob` naming that same `ClusterId` answers 200 with a fresh `JobId`, registers that job in the cancelled cluster's node list, and stores it in `JobState` `New`.

Expected: a cluster that has been cancelled is a terminated order — "You can only cancel a cluster job while it's in the AwaitingQuorum status" and "The cluster does not ship until these five node jobs have been created" — so AWS refuses a node creation against it with one of the client errors CreateJob declares (`InvalidResourceException` / `InvalidInputCombinationException`), and the cluster's node set does not grow.

Actual: 200 + a new `New` job whose `JobMetadata.ClusterId` points at a `Cancelled` cluster. The resulting state is self-contradictory and observable: `DescribeCluster` reports `ClusterState=Cancelled` while `DescribeJob` on the new node reports `JobState=New`, and the extra node counts toward the quorum so a later legitimate node create can be refused with `ClusterLimitExceededException` on a cluster nobody can use.

## Evidence

Spec/model: `CreateJob` input `ClusterId` — "If you're creating a job for a node in a cluster, you need to provide only this clusterId value"; `CancelCluster` — "You can only cancel a cluster job while it's in the `AwaitingQuorum` status"; `ClusterLimitExceededException` — "clusters support five nodes … create jobs until your cluster has exactly five nodes". Handler: services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/create_job.rs, the `if let Some(cluster_id) = input.cluster_id` branch (~lines 290-376) checks `store.clusters.get(&cluster_id)` and `cluster.has_quorum()` only — no `cluster.cluster_state` check, even though `crate::state::ClusterState` already models `Cancelled` and exposes `is_cancellable`/`is_updatable` guards used by CancelCluster and UpdateCluster. State helper `add_cluster_node_jobs` (src/state.rs ~line 1197) only guards the forward `AwaitingQuorum -> Pending` move, so it happily extends `node_job_ids` of a `Cancelled` cluster. Tests: tests/all/cluster_cancellation.rs covers cancel-twice and cancel-of-unknown-cluster, and tests/all/job_lifecycle.rs covers CreateJob/CancelJob, but no test creates a job against a cancelled cluster, so the behavior is unasserted.

## Steps to Reproduce

1. CreateAddress with a deliverable address and keep the returned `AddressId`.
2. CreateCluster with that `AddressId`, `JobType=LOCAL_USE`, `SnowballType=EDGE`, `ShippingOption=STANDARD`, `ForceCreateJobs=false` and no `InitialClusterSize`; keep the returned `ClusterId`.
3. CancelCluster with that `ClusterId` (succeeds — the cluster is `AwaitingQuorum`).
4. DescribeCluster: `ClusterMetadata.ClusterState` is `Cancelled`.
5. CreateJob with only `{"ClusterId": "<that cluster>"}`.
6. Observe 200 and a new `JobId`; DescribeJob on it reports `JobState=New` and `ClusterId=<the cancelled cluster>`, i.e. a live node job on a cancelled cluster.

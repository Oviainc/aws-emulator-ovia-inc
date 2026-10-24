Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# UpdateCluster changes never reach the cluster's existing node jobs, leaving the cluster and its nodes inconsistent

_Filed: 2026-10-24T18:37:19Z_
_Operation: UpdateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCluster` writes the supplied members onto the stored cluster only. The node jobs that already belong to that cluster — the ones `CreateCluster` minted for `InitialClusterSize`, or the ones `CreateJob` attached with `ClusterId` before the update — keep the values they inherited at creation time.

Expected: a cluster can only be updated while its `ClusterState` is `AwaitingQuorum`, i.e. before any device has been prepared or shipped, and a cluster's node jobs are by definition the cluster's attributes (`CreateJobRequest$ClusterId`: "you need to provide only this clusterId value. The other job attributes are inherited from the cluster"). All devices of a Snow Family cluster ship together to the cluster's one address at the cluster's one shipping speed, so after `UpdateCluster` changes `AddressId` / `ShippingOption` / `Notification` / `Resources` / `RoleARN` / `Description`, a `DescribeJob` on each existing node job reports the updated values, exactly as `DescribeCluster` does.

Actual: `DescribeCluster` reports the new values while `DescribeJob` on every pre-existing node of that same cluster still reports the old ones — e.g. a cluster whose `AddressId` is address B with node jobs that still name address A, which describes a cluster that would have to ship to two destinations. The divergence is permanent: the nodes are in `JobState` `New`, so a caller can only repair it by calling `UpdateJob` once per node, and nothing in the API tells a caller which nodes were created before the update.

## Evidence

Spec/model: `UpdateCluster` doc — "While a cluster's ClusterState value is in the AwaitingQuorum state, you can update some of the information associated with a cluster" (so the update always happens before any node device is prepared or shipped); `CreateJobRequest$ClusterId` — "The other job attributes are inherited from the cluster"; `ClusterMetadata$AddressId`/`$ShippingOption` model one address and one shipping speed for the whole cluster, and well-established Snow Family behavior is that every device in a cluster is delivered to that single cluster address.

Handler: services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/update_cluster.rs — `handle` ends with `apply_update(cluster, &input)` (line ~242) and the surrounding comment states the choice explicitly: "The cluster is the only record this operation writes … a node created after this update carries the updated values while one created before it keeps what it inherited, which is what `UpdateJob` is for." `apply_update` (lines 257-282) touches only `StoredCluster`; `store.jobs` is never iterated, and `StoredCluster.node_job_ids` (src/state.rs ~line 657) is not consulted.

Tests: tests/all/cluster_updates.rs `update_cluster_writes_the_members_it_names_and_leaves_the_rest_alone` (line ~310) asserts only `DescribeCluster` output; no test inspects a node job after an `UpdateCluster`, so neither behavior is pinned today.

## Steps to Reproduce

1. CreateAddress with a deliverable address -> AddressId A; CreateAddress again -> AddressId B.
2. CreateCluster with {JobType: LOCAL_USE, SnowballType: EDGE, ShippingOption: STANDARD, AddressId: A, InitialClusterSize: 2, ForceCreateJobs: false} and keep the ClusterId plus the two JobIds returned in JobListEntries.
3. DescribeCluster -> ClusterState is AwaitingQuorum, AddressId is A.
4. UpdateCluster with {ClusterId, AddressId: B, ShippingOption: EXPRESS, Description: "second attempt"} -> 200.
5. DescribeCluster -> ClusterMetadata now reports AddressId B, ShippingOption EXPRESS, the new Description.
6. DescribeJob on either node JobId -> JobMetadata still reports AddressId A, ShippingDetails.ShippingOption STANDARD and the cluster's original Description, i.e. live cluster nodes that disagree with their own cluster about where the devices ship.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# UpdateJob on a cluster node job leaves the cluster stale, breaking the crate's own "a node holds the cluster's attributes" invariant

_Filed: 2026-10-25T00:38:59Z_
_Operation: UpdateJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A cluster node job (a `StoredJob` whose `cluster_id` is `Some`, minted either by `CreateCluster`'s `InitialClusterSize` or by a `CreateJob` naming the cluster) is created in `JobState::New`, so `UpdateJob` accepts it: `job.job_state.is_updatable()` is true. The handler then writes the supplied members straight onto that node job and never looks at `store.clusters` at all — the whole `handle` body of `update_job.rs` contains no reference to the cluster store (`grep -i cluster src/ops/update_job.rs` matches only the generated doc header and the error enum).

That contradicts the invariant this crate states and enforces in the other direction. `UpdateCluster` propagates every member it takes to the cluster's existing node jobs precisely because, in this crate's words, a node job does not hold copies of its own attributes but the cluster's one address and one shipping speed. After an `UpdateJob` on a node, the two records disagree: `DescribeCluster` reports the cluster's `AddressId`/`ShippingOption`/`Description`/`RoleARN`/`Resources`/`Notification`/`OnDeviceServiceConfiguration`, `DescribeJob` on the node reports the updated ones, and a node created later by `CreateJob` (which calls `StoredCluster::node_job`, state.rs:722) inherits the cluster's values again — so two nodes of the same cluster end up ordered to different addresses at different shipping speeds, which is exactly the cluster/node inconsistency the already-fixed `UpdateCluster` issue (20261024T183719Z-snowball-updatecluster-wrong-behavior) established as a defect, only reached from the job side.

Nothing in the handler refuses the update for a node job either, so there is no honest refusal: the call answers 200 and persists a divergence a later `DescribeCluster`/`DescribeJob` pair exposes.

## Evidence

The invariant, stated and enforced in the sibling handler — services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/update_cluster.rs:238-248:

    // A cluster's node jobs carry the cluster's attributes rather than copies of their
    // own: "you need to provide only this clusterId value. The other job attributes are
    // inherited from the cluster" (`CreateJobRequest$ClusterId`), and `ClusterMetadata`
    // models ONE address and ONE shipping speed for the whole cluster, so every device
    // the cluster orders is delivered to that one address at that one speed. Updating
    // the cluster therefore updates the whole order: the attributes reach the node jobs
    // the cluster already holds too ...
    apply_update(cluster, &input);
    let node_job_ids = cluster.node_job_ids.clone();
    for job_id in node_job_ids { ... apply_update_to_node(job, &input); ... }

The other side — src/ops/update_job.rs:230-232 and 258-264 touch only the job and its job parts (a cluster node has none: `StoredCluster::node_job` sets `job_part_ids: Vec::new()`, src/state.rs:748-750):

    let order: Vec<String> = ::std::iter::once(input.job_id.clone())
        .chain(job.job_part_ids.iter().cloned())
        .collect();
    ...
    for job_id in order {
        if let Some(job) = store.jobs.get_mut(&job_id) {
            if job.job_state.is_updatable() { apply_update(job, &input); }

and src/ops/update_job.rs:279-294 writes the cluster-level members onto that node:

    fn apply_update(job: &mut crate::state::StoredJob, input: &UpdateJobInput) {
        if let Some(address_id) = &input.address_id { job.address_id = Some(address_id.clone()); }
        ...
        if let Some(shipping_option) = &input.shipping_option { job.shipping_option = Some(shipping_option.clone()); }

A node inherits the cluster's values at creation time — src/state.rs:731-734 (`shipping_option: Some(self.shipping_option.clone())`, `address_id: Some(self.address_id.clone())`) — so a later node re-diverges from an UpdateJob-ed sibling.

Test coverage: the invariant is pinned for the UpdateCluster direction only — tests/all/cluster_node_updates.rs:6-12: "a cluster's node job holds the cluster's attributes ... so every member an `UpdateCluster` names is reported by a later `DescribeJob` on each of that cluster's node jobs exactly as `DescribeCluster` reports it". The reverse direction is unasserted: `grep -i cluster tests/all/job_updates.rs` returns no matches, so no test drives `UpdateJob` against a cluster node job.

## Steps to Reproduce

1. CreateAddress with a deliverable address for the request's region; keep AddressId A1. CreateAddress a second, different deliverable address; keep AddressId A2.
2. CreateCluster with {"JobType":"LOCAL_USE","SnowballType":"EDGE","ShippingOption":"SECOND_DAY","AddressId":"A1","ForceCreateJobs":false,"InitialClusterSize":2}; keep the ClusterId and the two node JobIds from JobListEntries.
3. UpdateJob with {"JobId":"<first node>","AddressId":"A2","ShippingOption":"EXPRESS","Description":"node only"} — it answers 200.
4. DescribeJob on that node: AddressId is A2, ShippingDetails.ShippingOption is EXPRESS, Description is "node only".
5. DescribeCluster on the ClusterId: AddressId is still A1 and ShippingOption is still SECOND_DAY — the cluster and its node now disagree about the one address and one shipping speed the cluster orders its devices at.
6. DescribeJob on the second node: still A1/SECOND_DAY, so two nodes of one cluster are ordered to different addresses at different speeds.
7. CreateJob with {"ClusterId":"<that cluster>"} to attach a third node, then DescribeJob on it: it inherits A1/SECOND_DAY from the cluster, diverging from the updated node again.

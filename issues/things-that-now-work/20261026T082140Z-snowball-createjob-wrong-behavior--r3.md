Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# CreateJob lets a cluster node keep its own address and shipping speed, the divergence UpdateJob's own rule says cannot exist

_Filed: 2026-10-26T08:21:40Z_
_Operation: CreateJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate holds two opposite rules about whether a cluster's node job may carry an address / shipping option of its own.

`CreateJob`'s cluster-node branch writes every supplied member onto the NODE ONLY and never onto the cluster, so a request that names `ClusterId` together with `AddressId` / `ShippingOption` / `Description` / `RoleARN` / `Notification` / `Resources` / `OnDeviceServiceConfiguration` stores a node whose values differ from the cluster's, with the cluster record left untouched.

`UpdateJob` (and `UpdateCluster`) implement the opposite rule: they treat exactly those eight members as the CLUSTER's, write them to the cluster AND to every updatable node job, and the handler states that this is what keeps 'the cluster and its nodes never end up ordering devices to different addresses at different speeds'. `UpdateJob` even refuses the request outright (InvalidJobStateException) when the cluster has left `AwaitingQuorum`, precisely to avoid 'writing one record out of step with the rest'.

So the state `UpdateJob` goes out of its way to prevent is the state `CreateJob` creates routinely, and the next `UpdateJob`/`UpdateCluster` on any node of that cluster silently overwrites the node's own address with the cluster's — the per-node value `CreateJob` accepted is lost without any error.

## Evidence

CreateJob writes the supplied members onto the node only; the cluster is never updated:

services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/create_job.rs:464-466
```
            node.shipping_option = supplied(input.shipping_option).or(node.shipping_option);
            node.description = supplied(input.description).or(node.description);
            node.address_id = supplied(input.address_id).or(node.address_id);
```
and its stated reason, create_job.rs:445-448: "A member the request does carry is this job's own, since every one of them is a modelled input of this operation ... So each member is the value supplied, and the cluster's only where none was."

The sibling handler in the SAME crate states the contrary rule for the same eight members:

services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/update_job.rs:249-258
```
        // A node job holds no copy of the attributes its cluster models once for the whole
        // order — "you need to provide only this clusterId value. The other job attributes
        // are inherited from the cluster" (`CreateJobRequest$ClusterId`), and
        // `ClusterMetadata` models ONE address and ONE shipping speed, so every device the
        // cluster orders is delivered to that one address at that one speed. Those
        // attributes are therefore the cluster's to hold
```
services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/update_job.rs:301-307
```
        // The records this update reaches: the named job always, and with it the rest of
        // the order whose attributes it carries. For a cluster node that is the cluster
        // itself and the cluster's other node jobs — the same records an `UpdateCluster`
        // reaches — so the cluster and its nodes never end up ordering devices to
        // different addresses at different speeds, and a node created later inherits the
        // updated values from the cluster.
```
and the write that makes it true, update_job.rs:312 `apply_cluster_update(cluster, &input);` / update_job.rs:322 `apply_cluster_members(job, &input);` applied over `cluster.node_job_ids`.

Test coverage: tests/all/cluster_node_job_members.rs and tests/all/cluster_node_job_updates.rs exercise the create-a-node and update-a-node paths; neither pins the cross-record divergence described here.

## Steps to Reproduce

1. CreateAddress twice, obtaining ADID-A and ADID-B.
2. CreateCluster with JobType=LOCAL_USE, SnowballType=EDGE, ShippingOption=SECOND_DAY, AddressId=ADID-A.
3. CreateJob with ClusterId=<the new cluster>, AddressId=ADID-B, ShippingOption=NEXT_DAY.
4. DescribeCluster: AddressId is still ADID-A and ShippingOption still SECOND_DAY; DescribeJob on the new node: AddressId is ADID-B and ShippingDetails.ShippingOption is NEXT_DAY — the one-address/one-speed invariant update_job.rs:249-258 states is already broken.
5. UpdateJob on that same node with Description="x" (or any cluster member): the cluster's ADID-A/SECOND_DAY are written back over the node, discarding the ADID-B/NEXT_DAY values CreateJob had accepted, with a 200 and no error.

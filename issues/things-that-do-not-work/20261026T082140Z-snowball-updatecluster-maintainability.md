Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# The cluster-member application rule is written four times across update_cluster.rs and update_job.rs

_Filed: 2026-10-26T08:21:40Z_
_Operation: UpdateCluster | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "apply the eight members a cluster holds for its whole order, leaving an omitted member as it was" is implemented four times in this crate: twice against `StoredCluster` and twice against `StoredJob`, once per handler. The four copies walk the same member list (`AddressId`, `ForwardingAddressId`, `Description`, `RoleARN`, `ShippingOption`, `Notification`, `Resources`, `OnDeviceServiceConfiguration`) in the same order with the same `if let Some(..) = ..` shape, differing only in the input struct they read from.

Because each handler owns its own copy, adding or removing a member from the cluster-held set — or changing how an omitted member is read — has to be done in four places, and a copy that is missed makes `UpdateCluster` and `UpdateJob` disagree about what a cluster's order carries, which is the very invariant update_job.rs:301-307 relies on. A single application helper parameterised over the two input types keeps every response and every existing test byte-for-byte identical.

## Evidence

Copy 1 — StoredJob, in update_cluster.rs:307-332:
```
fn apply_update_to_node(job: &mut crate::state::StoredJob, input: &UpdateClusterInput) {
    if let Some(address_id) = &input.address_id {
        job.address_id = Some(address_id.clone());
    }
    ...
    if let Some(configuration) = &input.on_device_service_configuration {
        job.on_device_service_configuration = Some(configuration.clone());
    }
}
```
Copy 2 — StoredJob again, in update_job.rs:372-397:
```
fn apply_cluster_members(job: &mut crate::state::StoredJob, input: &UpdateJobInput) {
    if let Some(address_id) = &input.address_id {
        job.address_id = Some(address_id.clone());
    }
    ...
    if let Some(configuration) = &input.on_device_service_configuration {
        job.on_device_service_configuration = Some(configuration.clone());
    }
}
```
Copy 3 — StoredCluster, in update_cluster.rs:271-296 (`fn apply_update(cluster: &mut crate::state::StoredCluster, input: &UpdateClusterInput)`), and copy 4 — StoredCluster again, in update_job.rs:435-460 (`fn apply_cluster_update(cluster: &mut crate::state::StoredCluster, input: &UpdateJobInput)`), both of which assign `cluster.address_id = address_id.clone(); ... cluster.on_device_service_configuration = Some(configuration.clone());` over the identical member list.

The crate already states that these two handlers must agree, update_job.rs:301-307: "For a cluster node that is the cluster itself and the cluster's other node jobs — the same records an `UpdateCluster` reaches".

Tests covering all four copies, which must stay unchanged: tests/all/cluster_updates.rs, tests/all/cluster_node_updates.rs, tests/all/cluster_node_job_updates.rs and tests/all/job_updates.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/update_cluster.rs at lines 271-296 and 307-332.
2. Open services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/update_job.rs at lines 372-397 and 435-460.
3. Compare the four bodies: the same eight members in the same order, assigned the same way, once per (target record, input struct) pair.
4. Note that changing the cluster-held member set requires editing all four, and that cluster_updates.rs / cluster_node_updates.rs / cluster_node_job_updates.rs / job_updates.rs assert the same resulting values from both handlers.

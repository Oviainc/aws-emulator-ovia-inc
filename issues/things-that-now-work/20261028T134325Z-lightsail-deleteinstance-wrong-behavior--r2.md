Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# Deleting an instance leaves the bucket access grant it held, so a bucket keeps reporting a deleted instance in resourcesReceivingAccess and can no longer be deleted without forceDelete

_Filed: 2026-10-28T13:43:25Z_
_Operation: DeleteInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteInstance` releases every attachment another record holds to the instance EXCEPT the bucket access grant `SetResourceAccessForBucket` writes. `release_attachments_to_instance` walks the disks, the static IPs and the load balancers, and nothing walks `Bucket::resources_receiving_access` — even though that grant is a reference held by ANOTHER record (the bucket's) and keyed only by the instance's name.

Two observable consequences, both of which the crate's own rules say must not happen:

1. `GetBuckets` (with `includeConnectedResources`) keeps reporting a `resourcesReceivingAccess` entry naming an instance `GetInstance` answers `DoesNotExist` for — a read observing an access held by a record that is gone.
2. `DeleteBucket` counts that stale entry as one of the documented force-delete conditions ("the bucket has instances that were granted access to it"), so a bucket whose only grantee was the deleted instance can never be deleted without `forceDelete`, and there is no call left that can clear the grant: `SetResourceAccessForBucket` with `access=deny` first resolves `resourceName` against the instances of the Region and refuses with `DoesNotExist` because the instance no longer exists (src/ops/set_resource_access_for_bucket.rs:219-224). The stale grant is therefore unremovable.

This is a different gap from the already-fixed disk/static-IP/load-balancer release: that one is implemented in `release_attachments_to_instance`, and the bucket grant is simply not among the records it walks.

## Evidence

The rule the delete states for itself — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_instance.rs:176-182:
```
            // Removing the instance also releases every attachment another
            // stored resource holds to it — a block storage disk, a static IP,
            // a load balancer's registration — because an attachment names "the
            // resources to which" a resource "is attached" (the modelled
            // `Disk$attachedTo`) and this instance is about to be none of them.
            // The release is part of the same step, so no read can observe a
            // resource attached to an instance this account no longer holds.
```
What the release actually walks — src/state.rs:13395-13456 (`fn release_attachments_to_instance`): `for disk in self.disks.values_mut()` (13397), `for static_ip in self.static_ips.values_mut()` (13419), `for balancer in self.load_balancers.values_mut()` (13438) — and no walk over `self.buckets`.

The grant that is left behind lives on the BUCKET's record — src/state.rs:2183-2194:
```
    pub fn set_resource_access(&mut self, name: &str, resource_type: &str, allow: bool) {
        let held = self
            .resources_receiving_access
            .iter()
            .position(|resource| resource.name.as_deref() == Some(name));
        match (allow, held) {
            (true, None) => self
                .resources_receiving_access
                .push(ResourceReceivingAccess {
                    name: Some(name.to_string()),
```
and it blocks the bucket's delete — src/ops/delete_bucket.rs:229-235:
```
            if held.resources_receiving_access > 0 {
                blocked_by.push(format!(
                    "{} resource{} were granted access to it",
                    held.resources_receiving_access,
```
The model says the member describes instances that HAVE access — smithy_query(query="doc", shape="Bucket", member="resourcesReceivingAccess"): "An array of objects that describe Lightsail instances that have access to the bucket."

Test coverage that states the rule but never exercises the bucket grant — tests/all/instance_delete_attachments.rs:14-21:
```
//! The rule under test: removing a resource releases every attachment another
//! stored resource holds to it, so no read reports a resource attached to one
//! the account no longer holds.
```
(every case in that suite makes its attachment with `AttachDisk`; tests/all/bucket_resource_access.rs never calls `DeleteInstance`), so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateInstances "host" in us-east-1a and let it settle to running; CreateBucket "grant-bucket" in us-east-1.
2. SetResourceAccessForBucket {bucketName: "grant-bucket", resourceName: "host", access: "allow"} -> HTTP 200.
3. GetBuckets {bucketName: "grant-bucket", includeConnectedResources: true} -> resourcesReceivingAccess names "host".
4. DeleteInstance {instanceName: "host"} -> HTTP 200; GetInstance "host" -> DoesNotExist.
5. GetBuckets {bucketName: "grant-bucket", includeConnectedResources: true} -> resourcesReceivingAccess STILL names "host", an instance the account no longer holds.
6. DeleteBucket {bucketName: "grant-bucket"} (no forceDelete) -> 400 "Bucket is in use: ... 1 resource were granted access to it"; SetResourceAccessForBucket with access="deny" for "host" -> 400 DoesNotExist (src/ops/set_resource_access_for_bucket.rs:219-224), so the stale grant cannot be cleared at all.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# A device still named by a stored connection is deleted, contradicting the rule DeleteConnection states for the same reference

_Filed: 2026-10-28T02:37:10Z_
_Operation: DeleteDevice | Class: validation_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConnection` stores `device_id` and `connected_device_id` as references into `store.devices`, resolved within the global network. `DeleteConnection` explains that no dependency refusal applies to a connection precisely BECAUSE removing the connection is what makes those devices deletable in turn — i.e. the crate's stated invariant is that a device with a connection on it is NOT deletable.

`DeleteDevice` does not implement that half: it counts only link associations and customer gateway associations and then removes the device. The connection record survives, naming a device that `GetDevices` no longer reports, and `GetConnections` keeps listing it. Because `DeleteGlobalNetwork` blocks on devices/sites/core networks but not on connections, the devices can be deleted first and the global network after them, leaving the connection record orphaned entirely (its ARN is scoped to a global network the store no longer holds, so `ListTagsForResource`, `UpdateConnection`, `DeleteConnection` and `GetConnections` can no longer reach it).

## Evidence

The invariant as DeleteConnection states it — src/ops/delete_connection.rs:187-190: "Nothing hangs off a connection: the two devices and the optional links it names are its REFERENCES, which outlive it, so no dependency refusal applies here and the record goes away on its own. Removing it is what makes those devices, and the global network above them, deletable in turn."

The half that is missing — src/ops/delete_device.rs:175-203 counts exactly two dependent kinds and nothing else:
```
let link_associations = store.link_associations.values().filter(...).count();
let gateway_associations = store.customer_gateway_associations.values().filter(...).count();
// Listed in the order the operation's own documentation names them.
if let Some(refusal) = crate::state::dependents_block_delete(
    crate::state::DEVICE_KIND,
    &[(LINK_ASSOCIATION_KIND, link_associations), (CUSTOMER_GATEWAY_ASSOCIATION_KIND, gateway_associations)],
) { return Err(...); }
let removed = store.devices.remove(&input.device_id)...
```
while the same file states the harm it is guarding against (delete_device.rs:164-168): "so a device that is still associated with either is refused rather than deleted in a way that leaves the association pointing at a device nobody can read."

The reference that is left dangling — src/ops/create_connection.rs:299-300: `device_id: input.device_id, connected_device_id: input.connected_device_id,` (both resolved against `store.devices` at create time, lines 222-239).

No test pins the allowance: tests/all/delete_device.rs never creates a connection on the device it deletes (it only imports `connection_lifecycle::without_state`).

## Steps to Reproduce

1. CreateGlobalNetwork; CreateSite; CreateDevice twice in that global network
2. CreateConnection(DeviceId=device1, ConnectedDeviceId=device2) → 200
3. DeleteDevice(device1) → 200 (expected: the dependency ValidationException 400 naming the connection that still references it)
4. GetConnections on the global network still lists the connection naming device1, while GetDevices no longer reports device1
5. DeleteDevice(device2), DeleteSite, DeleteGlobalNetwork → all 200: the connection record now names a global network the store does not hold, and no operation can read or delete it any more

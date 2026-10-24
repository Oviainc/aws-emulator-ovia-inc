Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Release block storage attachments when the instance holding them is deleted

_Filed: 2026-10-24T01:05:05Z_
_Operation: DeleteInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Deleting an instance that still has a block storage disk attached leaves the DISK record pointing at an instance that no longer exists: Lightsail never reports a disk attached to a non-existent resource.

Expected: after DeleteInstance on an instance with an attached disk, the disk is no longer attached — GetDisk reports `isAttached` false, no `attachedTo`, no `path`, and state `available` (Lightsail detaches the volume as part of deleting the instance) — or, if the service refuses the delete while a disk is attached, DeleteInstance answers OperationFailureException and the instance survives. Either way the store never holds an attachment to a deleted instance.

Actual: `store.remove_instance(&name)` removes only the instance entry from `instances`; every disk whose `attached_to` names it keeps `attached_to = <deleted name>`, `path = /dev/xvdf`, and state `in-use`. GetDisk/GetDisks then report a disk attached to an instance that GetInstance answers DoesNotExist for, and the disk cannot be reused until the caller issues an extra DetachDisk that AWS would not need.

## Evidence

Spec/model: `Disk$attachedTo` is documented as "The resources to which the disk is attached" and `DiskState` `in-use` means the disk is in use by an instance — neither can be true of a deleted instance; AttachDisk itself refuses an instance the account does not hold, so the emulator admits a state its own write path forbids. Handler: `src/ops/delete_instance.rs` (handle body calls `store.remove_instance(&name)` only); `src/state.rs::remove_instance` is a bare `self.instances.remove(name)` and `advance_clock` only releases an attachment for a disk that was given a `due` of `available` by DetachDisk. Tests: `tests/all/instance_lifecycle.rs::deleting_an_instance_removes_it_from_every_read` and `tests/all/disk_attachment.rs` never delete an instance while a disk is attached, so nothing asserts this.

## Steps to Reproduce

1. CreateInstances "host" in us-east-1a and wait for `running`; CreateDisk "data" in us-east-1a and wait for `available`
2. AttachDisk {diskName: "data", instanceName: "host", diskPath: "/dev/xvdf"}; GetDisk "data" shows isAttached true, attachedTo "host", state in-use
3. DeleteInstance {instanceName: "host"} -> HTTP 200
4. GetInstance "host" -> DoesNotExist, but GetDisk "data" still reports state in-use and attachedTo "host" — a disk attached to an instance that no longer exists

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: fa5ed22240e89dcfd43a22a078f364ee68f69599

# AttachDisk leaves the disk's pending `due` in place, breaking the invariant the settle loop states for an attaching operation

_Filed: 2026-10-25T09:20:00Z_
_Operation: AttachDisk | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fa5ed22240e89dcfd43a22a078f364ee68f69599`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The clock's settle loop in `state.rs` states the obligation an attaching operation has in so many words: a disk that settles into `available` has its `attached_to` and `path` cleared, "An operation that ATTACHES a disk therefore has to put it in `in-use` and clear the `due` it carries, or the settle it was owed would release the attachment that operation just made." `AttachDisk` does the first half (`state = DISK_STATE_IN_USE`) and omits the second: it never assigns `disk.due = None`. Every other handler that moves a disk's attachment honours the rule — `release_attachments_to_instance` explicitly clears a `due` whose state is `available` after releasing the attachment. The attachment `AttachDisk` just made is therefore protected only by the accident that no current code path leaves a disk in `available` with an outstanding `due`; any future `due` on an `available` disk (or a settle ordering change) silently un-attaches a disk that a 200 response said was attached.

## Evidence

The invariant, services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:10962-10975:
```
                // A disk that has settled into `available` is attached to
                // nothing — that is what the modelled `DiskState` value means
                // next to `in-use` — so settling there is also what completes
                // the detachment `DetachDisk` started: the attachment and the
                // device path it was mounted at are released together. A disk
                // that reached `available` from `pending` was attached to
                // nothing already, so the same step is a no-op for it. An
                // operation that ATTACHES a disk therefore has to put it in
                // `in-use` and clear the `due` it carries, or the settle it was
                // owed would release the attachment that operation just made.
                if disk.state == DISK_STATE_AVAILABLE {
                    disk.attached_to = None;
                    disk.path = None;
                }
```
The attaching operation that does not clear it, src/ops/attach_disk.rs:300-306:
```
            let Some(disk) = store.disk_mut(&disk_name) else {
                // Unreachable: the disk was read above under this same lock.
                return Err(not_found(crate::state::RESOURCE_TYPE_DISK, &disk_name));
            };
            disk.attached_to = Some(instance_name.clone());
            disk.path = Some(disk_path);
            disk.state = crate::state::DISK_STATE_IN_USE;
```
(no `disk.due = None;` anywhere in the handler).

The sibling path that does honour it, src/state.rs:10022-10029:
```
            // A detachment the clock still owed this disk has nothing left to
            // do: the attachment it was going to release is already released,
            // and leaving the `due` in place would only re-apply the same
            // settle. A `due` of any OTHER state is left alone — it is not this
            // attachment's.
            if disk.due.is_some_and(|due| due.state == DISK_STATE_AVAILABLE) {
                disk.due = None;
            }
```
Test coverage: tests/all/disk_attachment.rs and tests/all/disk_detach.rs cover attach/detach round-trips; neither asserts anything about the stored `due` after an attach, so the omission is unpinned either way.

## Steps to Reproduce

1. Read src/state.rs:10962-10975 — the settle loop states that an operation that attaches a disk must put it in `in-use` AND clear the `due` it carries.
2. Read src/ops/attach_disk.rs:300-306 — the attachment sets `attached_to`, `path` and `state` but never assigns `disk.due = None`.
3. Compare with src/state.rs:10022-10029, where the instance-deletion release path does clear a `DISK_STATE_AVAILABLE` `due` for exactly this reason.
4. Conclude that AttachDisk is the one attachment site in the crate that does not satisfy the invariant its own state module states.

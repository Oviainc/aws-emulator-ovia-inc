Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# forceDeleteAddOns is dropped on the strength of a comment the crate now contradicts, and the add-on's automatic snapshots are destroyed whatever the flag says

_Filed: 2026-10-27T14:59:33Z_
_Operation: DeleteInstance | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteInstance` discards `forceDeleteAddOns` with `let _ = input.force_delete_add_ons;`, justified by a comment that says the only thing an add-on leaves behind is its automatic snapshots "and snapshots are not part of this slice, so there is nothing further for the flag to delete". That premise is no longer true of this crate: automatic snapshots ARE part of the slice — the store keeps them in `self.auto_snapshots`, `EnableAddOn`/`DisableAddOn` schedule them, `GetAutoSnapshots` reports them as a resource of the account, `DeleteAutoSnapshot` deletes one, and `CreateDiskFromSnapshot`/`CopySnapshot` restore and copy from them by date. `remove_instance` deletes the whole schedule, every copy it has taken included, unconditionally.

So the one thing the documented flag exists to govern — "whether to delete all add-ons for the instance" — is exactly what the delete destroys without being asked: a caller who omits the flag (or sends `false`) loses every automatic snapshot the instance's add-on had taken, and the handler's own justification for ignoring the member is contradicted by the state module it calls. `DeleteDisk` carries the identical comment and the identical unconditional removal (`remove_disk`), so the same gap is in both deletes.

## Evidence

The dropped member and the premise it rests on — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_instance.rs:167-172:
```
        // `forceDeleteAddOns` asks for the instance's add-ons to go with it. An
        // add-on lives on the instance record, so removing the record removes
        // them whatever the flag says; the only thing an add-on leaves behind is
        // its automatic snapshots, and snapshots are not part of this slice, so
        // there is nothing further for the flag to delete.
        let _ = input.force_delete_add_ons;
```
The crate DOES hold those snapshots, and the delete destroys them — src/state.rs:12778-12790:
```
    pub fn remove_instance(&mut self, name: &str) -> Option<RemovedInstance> {
        let instance = self.instances.remove(name)?;
        // The instance's automatic snapshots go with it, for the reason spelled
        // out on [`Self::remove_disk`]: they are addressed through the instance
        // and have no identity without it.
        self.auto_snapshots
            .remove(&(RESOURCE_TYPE_INSTANCE, name.to_string()));
```
and they are a reportable resource of the account — src/ops/get_auto_snapshots.rs:181-188:
```
            let Some((resource_type, auto_snapshots)) = store.auto_snapshots_of(&name) else {
                return Err(not_found(&name));
            };
            Ok(GetAutoSnapshotsOutput {
                resource_name: Some(name.clone()),
                resource_type: Some(resource_type.to_string()),
                auto_snapshots: Some(auto_snapshots),
```
The model documents the member as governing exactly this — smithy_query(query="doc", shape="DeleteInstanceRequest", member="forceDeleteAddOns"): "A Boolean value to indicate whether to delete all add-ons for the instance."

The identical pair in the disk delete: src/ops/delete_disk.rs:167-172 ("those are not part of this slice, so there is nothing further for the flag to delete") against src/state.rs:12437-12444 (`remove_disk` → `self.auto_snapshots.remove(...)`).

Test coverage: tests/all/auto_snapshots.rs and tests/all/auto_snapshot_delete.rs exercise the add-on's copies, tests/all/instance_delete_attachments.rs and tests/all/disk_delete.rs exercise the deletes — no case sends `forceDeleteAddOns` at all, so neither behaviour is pinned.

## Steps to Reproduce

1. Call CreateInstances for "i1" with addOns=[{"addOnType":"AutoSnapshot","autoSnapshotAddOnRequest":{"snapshotTimeOfDay":"06:00"}}].
2. Let the schedule take at least one copy (or inspect the schedule) and call GetAutoSnapshots with resourceName="i1" — the copies are reported.
3. Call DeleteInstance with instanceName="i1" and NO forceDeleteAddOns member (its documented default is not to delete the add-ons).
4. Observe HTTP 200 — and that the instance's whole automatic-snapshot schedule, every copy included, was removed by remove_instance (src/state.rs:12783-12784), so nothing the flag could have protected survives and the member changed nothing about the call.
5. Read src/ops/delete_instance.rs:167-172 next to src/state.rs:12778-12790: the comment that justifies dropping the member says those snapshots are not part of this slice, while the code it calls removes them from this slice's own map.

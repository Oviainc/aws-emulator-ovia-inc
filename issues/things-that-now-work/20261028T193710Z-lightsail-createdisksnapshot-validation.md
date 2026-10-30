Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 160eac1be171dea85ae8441ad5f20b2dc0c6ebbb

# CreateDiskSnapshot accepts a snapshot name an instance snapshot already holds, the cross-kind uniqueness check CopySnapshot applies and ExportSnapshot depends on

_Filed: 2026-10-28T19:37:10Z_
_Operation: CreateDiskSnapshot | Class: validation_

> _Note: this issue was identified against the code base at commit `160eac1be171dea85ae8441ad5f20b2dc0c6ebbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two snapshot creates check their OWN map only, so one name can end up held by both a disk snapshot and an instance snapshot. `CopySnapshot` — the third operation in this crate that mints a snapshot name — states and enforces the opposite rule ("A snapshot name addresses ONE snapshot of this account"), and the crate's name-resolution paths assume it: `ExportSnapshot` resolves a `sourceSnapshotName` by trying the disk-snapshot map first and the instance-snapshot map only if that misses, and the tagging plane (`with_tagged_record`) does the same. Once the collision exists:

* `ExportSnapshot` can never export the instance snapshot of that name — it always exports the disk snapshot;
* `TagResource`/`UntagResource` for that name always reach the disk snapshot, so the instance snapshot's tags are unreachable;
* `CopySnapshot` refuses `targetSnapshotName` for a name that is free in the kind it is copying.

The crate contradicts itself here: either the cross-kind check CopySnapshot applies is wrong, or the two creates skip it. Note the related name-reuse rule that IS pinned by a test (`tests/all/auto_snapshots.rs:667`, "A Lightsail name is unique per resource TYPE") covers an instance and a disk sharing a name — no test covers two snapshot kinds sharing one, and no test pins the ambiguous `ExportSnapshot`/`TagResource` resolution this produces.

## Evidence

The sibling that enforces the rule — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/copy_snapshot.rs:218-229:
```
// A snapshot name addresses ONE snapshot of this account — a name is
// resolved across both kinds wherever this service takes a snapshot
// name (`ExportSnapshot`) — so copying onto a name already taken is
// refused rather than silently replacing a record a restore may
// already be using.
if store.disk_snapshot(&target_name).is_some()
    || store.instance_snapshot(&target_name).is_some()
{
    return Err(invalid(format!(
        "a snapshot named '{target_name}' already exists"
```

The handler that skips it — src/ops/create_disk_snapshot.rs:236-243:
```
// A snapshot name addresses one snapshot, so creating one this
// account already holds is refused rather than silently replacing a
// record a restore may already be using.
if store.disk_snapshot(&name).is_some() {
    return Err(invalid(format!(
        "a disk snapshot named '{name}' already exists"
    )));
}
```
and the same single-map check in src/ops/create_instance_snapshot.rs:204-211 (`if store.instance_snapshot(&name).is_some()`).

The resolution that becomes ambiguous — src/ops/export_snapshot.rs:283 and :313:
```
if let Some(snapshot) = store.disk_snapshot(name) {
    ...
    return Some(Exported { ... RESOURCE_TYPE_DISK_SNAPSHOT ... });
}
let snapshot = store.instance_snapshot(name)?;
```
and the tagging plane, src/state.rs:15233-15236 + 15268-15273:
```
/// ONE name addresses one resource: `GetActiveNames` answers "the names of
/// all active (not deleted) resources" of the account as a single flat list,
/// with no resource type qualifying them, so at most one of the maps below
/// holds a given name.
...
if let Some(record) = self.disk_snapshots.get_mut(name) {
    return Some(body(record, RESOURCE_TYPE_DISK_SNAPSHOT));
}
if let Some(record) = self.instance_snapshots.get_mut(name) {
    return Some(body(record, RESOURCE_TYPE_INSTANCE_SNAPSHOT));
}
```

Test coverage: tests/all/snapshot_copy.rs covers CopySnapshot's duplicate-target refusal and tests/all/export_snapshot_source_kinds.rs covers ExportSnapshot over each kind separately; no test creates one name in both snapshot maps.

## Steps to Reproduce

1. CreateInstances with instanceNames ["web"] and wait for it to settle.
2. CreateInstanceSnapshot with instanceName "web", instanceSnapshotName "snap-x" — 200.
3. CreateDisk "vol" (same Region), then CreateDiskSnapshot with diskName "vol", diskSnapshotName "snap-x" — 200, although the account already holds a snapshot called "snap-x".
4. ExportSnapshot with sourceSnapshotName "snap-x": the record's sourceInfo.resourceType is always DiskSnapshot — the instance snapshot of that name can never be exported.
5. TagResource with resourceName "snap-x" and a tag, then GetInstanceSnapshot("snap-x"): the instance snapshot carries no tags (the disk snapshot got them).
6. CopySnapshot with sourceSnapshotName "other-instance-snap" and targetSnapshotName "snap-x": refused as "already exists" — the same name the create accepted.

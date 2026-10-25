Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d7f2082fceb83802a8fa1cf49a5205bc74f19057

# CreateDiskFromSnapshot restores from a disk snapshot that is still `pending`, skipping the completeness check both sibling restore handlers apply

_Filed: 2026-10-25T00:21:18Z_
_Operation: CreateDiskFromSnapshot | Class: validation_

> _Note: this issue was identified against the code base at commit `d7f2082fceb83802a8fa1cf49a5205bc74f19057`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other "create from snapshot" handler in this crate refuses a snapshot whose copy the clock has not finished yet: `CreateInstancesFromSnapshot` refuses unless the instance snapshot is `available`, and `CreateRelationalDatabaseFromSnapshot` refuses unless the database snapshot is `available`. `CreateDiskFromSnapshot` performs the other two checks its siblings perform — the snapshot's Region, and the requested size against the snapshotted volume — but never looks at `snapshot.state`.

A disk snapshot is created `pending` and only the clock settles it to `completed` (`create_disk_snapshot.rs:292-305`). So `CreateDiskSnapshot` immediately followed by `CreateDiskFromSnapshot` succeeds and produces a disk restored from a snapshot the crate's own records say is still being taken — the exact condition the sibling handlers' comments call out as "there is nothing complete to restore from yet". The restored disk then settles into `available` and reads back as a normal, fully restored volume.

## Evidence

Sibling 1 — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_instances_from_snapshot.rs:340-355:
```
// A snapshot is restorable only once every block it
// captures is captured: while Lightsail is still taking
// it the snapshot reports `pending`
// (`InstanceSnapshot$state`), and there is nothing
// complete to restore from yet. ...
if snapshot.state != crate::state::INSTANCE_SNAPSHOT_STATE_AVAILABLE {
    return Err(operation_failure(format!(
        "The instance snapshot {} is {}: an instance can be restored \
         only from a snapshot that is {}.", ...
```

Sibling 2 — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_relational_database_from_snapshot.rs:345-355:
```
// A snapshot still being taken holds no complete copy of the
// database, so there is nothing to restore from until the copy the
// clock owes it is done.
if snapshot.state != crate::state::RELATIONAL_DATABASE_SNAPSHOT_STATE_AVAILABLE {
    return Err(invalid(format!(
        "relationalDatabaseSnapshotName '{snapshot_name}' is {}, and only a snapshot \
         that is {} can be restored", ...
```

This handler — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_disk_from_snapshot.rs:274-321 reads only the name, region and size off the snapshot and checks only those two:
```
let (snapshot_name, snapshot_region, snapshot_size_in_gb) = match &source {
    RequestedSource::Snapshot(requested) => {
        let snapshot = store.disk_snapshot(requested).ok_or_else(|| { ... })?;
        (snapshot.name.clone(), snapshot.location.region_name.clone(), snapshot.size_in_gb)
    }
...
if snapshot_region != region { ... }
if size_in_gb < snapshot_size_in_gb { ... }
```
`snapshot.state` is never consulted (confirmed by `grep -n 'SNAPSHOT_STATE_\|snapshot.state' src/ops/create_disk_from_snapshot.rs` → no match), while the snapshot is stored `pending` with the completion owed to the clock — create_disk_snapshot.rs:292-305:
```
let due = crate::state::DueNamedState::after(now, crate::state::DISK_SNAPSHOT_STATE_COMPLETED);
... state: crate::state::DISK_SNAPSHOT_STATE_PENDING,
```

Test coverage: `tests/all/disk_restore_from_snapshot.rs` always waits for the snapshot first — `wait_for_snapshot_state(emulator, row.snapshot_name, "completed")` at lines 275, 665, 999, 1014 — so no existing test pins a restore from a `pending` snapshot.

## Steps to Reproduce

1. POST /ls/api/2016-11-28/CreateDisk with {"diskName":"src-disk","availabilityZone":"us-east-1a","sizeInGb":8} and wait for GetDisk to report state `available`.
2. POST /ls/api/2016-11-28/CreateDiskSnapshot with {"diskSnapshotName":"snap-1","diskName":"src-disk"}.
3. Without waiting, POST /ls/api/2016-11-28/GetDiskSnapshot with {"diskSnapshotName":"snap-1"} — it reports state `pending`.
4. Immediately POST /ls/api/2016-11-28/CreateDiskFromSnapshot with {"diskName":"restored","diskSnapshotName":"snap-1","availabilityZone":"us-east-1a","sizeInGb":8}.
5. Observe HTTP 200 and a `restored` disk that settles to `available`. Compare with the same sequence for CreateInstancesFromSnapshot or CreateRelationalDatabaseFromSnapshot, which refuse a snapshot that has not reached `available`.

Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0b62ce506db227f8753aee084e9b25daeb773a55

# Restoring a disk from an automatic snapshot always fails with "this account has taken none", on the strength of a comment the crate now contradicts

_Filed: 2026-10-28T16:06:02Z_
_Operation: CreateDiskFromSnapshot | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0b62ce506db227f8753aee084e9b25daeb773a55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDiskFromSnapshot` supports two sources: a manual `diskSnapshotName`, or an AUTOMATIC snapshot addressed by `sourceDiskName` plus `restoreDate`/`useLatestRestorableAutoSnapshot`. The handler validates the second form fully (mutual exclusivity, `YYYY-MM-DD` form), resolves the source disk — and then, for EVERY such request, returns the `DoesNotExist` envelope saying "this account has taken none", justified by a comment claiming "This store holds none — the add-on records the hour it would run at and nothing takes a copy".

That premise is false of this crate. The store keeps `auto_snapshots` with a `taken` list per resource; the clock takes a copy on the add-on's daily schedule and settles it into `Success`; `GetAutoSnapshots` reports the copies; `DeleteAutoSnapshot` removes one by date; and the sibling `CopySnapshot` resolves exactly this source form out of `store.auto_snapshots` — including the DISK case, where it reads the disk's `size_in_gb` and builds a `DiskSnapshotSource::Disk` from the stored record. So the crate already owns every piece a disk restore needs, and this handler refuses instead of using it.

Result: a caller who enables `AutoSnapshot` on a disk, waits for the copy to report `Success` through `GetAutoSnapshots`, and then restores it is told the copy does not exist, even though `CopySnapshot` will copy that same copy in the same emulator run.

## Evidence

The unconditional refusal and the comment that justifies it — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_disk_from_snapshot.rs:289-304:
```
                // An AUTOMATIC snapshot is one the `AutoSnapshot` add-on takes on
                // the daily schedule a disk carries. This store holds none — the
                // add-on records the hour it would run at and nothing takes a
                // copy — so, once the source disk is known to exist, there is no
                // automatic snapshot of it to restore from and the request is
                // refused with the service's `DoesNotExist` envelope rather than
                // answered with a disk restored from nothing.
                RequestedSource::AutoSnapshot { disk_name, date } => {
                    store
                        .disk(disk_name)
                        .ok_or_else(|| not_found(crate::state::RESOURCE_TYPE_DISK, disk_name))?;
                    return Err(crate::scaffolded::Scaffolded::does_not_exist(format!(
                        "no automatic snapshot of the Disk {disk_name} is available to \
                         restore ({date}): this account has taken none"
                    )));
                }
```

The crate DOES take those copies — src/state.rs:5876-5887 (`StoredAutoSnapshots::take`, pushing a `StoredAutoSnapshot` with a `date` and a due `Success`) driven by the clock pass at src/state.rs:14998-15031:
```
        // The `AutoSnapshot` add-on takes a copy of the resource it is enabled
        // on once a day: that copy was owed at the instant the schedule named
        // whether or not anybody called, so the clock is what takes it
...
            schedule.take(due_at_ms, captured);
```

The sibling that resolves the same source form for a DISK — src/ops/copy_snapshot.rs:576-591 and 622-634:
```
    let taken = store
        .auto_snapshots
        .get(&(resource_type, resource_name.to_string()))
        .map(|schedule| schedule.taken.as_slice())
        .unwrap_or_default();
...
    let disk = store
        .disk(resource_name)
        .ok_or_else(|| resource_not_found(resource_name))?;
    Ok(Copied {
        content: Content::DiskSnapshot {
            size_in_gb: disk.size_in_gb,
            source: crate::state::DiskSnapshotSource::Disk {
```

The model documents the path as a real one — smithy_query(query="doc", shape="CreateDiskFromSnapshotRequest", member="useLatestRestorableAutoSnapshot"): "A Boolean value to indicate whether to use the latest available automatic snapshot." … "Define this parameter only when creating a new disk from an automatic snapshot."

Test coverage does NOT pin the defect: tests/all/disk_restore_from_snapshot.rs:834-876 states its rule as "an automatic snapshot is one the `AutoSnapshot` add-on took, and this account has taken none" and every case it sends is one where the account genuinely holds no matching copy — a disk with no add-on, a date no copy was taken on, and an absent disk. Meanwhile tests/all/snapshot_copy.rs:421-438 (`Source::AutomaticSnapshotOfDiskByDate`) waits for a DISK's automatic copy to report `Success` and copies it by `restoreDate` successfully.

## Steps to Reproduce

1. Call CreateDisk for "d1" in some availability zone with addOns=[{"addOnType":"AutoSnapshot","autoSnapshotAddOnRequest":{"snapshotTimeOfDay":"06:00"}}].
2. Poll GetAutoSnapshots with resourceName="d1" until one entry reports status "Success" and note its `date`.
3. Confirm the copy is real: call CopySnapshot with sourceRegion=<region>, sourceResourceName="d1", restoreDate=<that date>, targetSnapshotName="from-auto" — it succeeds.
4. Now call CreateDiskFromSnapshot with diskName="d2", availabilityZone=<same zone>, sizeInGb=<at least d1's size>, sourceDiskName="d1", restoreDate=<that date> (or useLatestRestorableAutoSnapshot=true).
5. Observe HTTP 400 NotFoundException / code DoesNotExist, message "no automatic snapshot of the Disk d1 is available to restore (...): this account has taken none" — for a copy GetAutoSnapshots just reported and CopySnapshot just copied.
6. Read src/ops/create_disk_from_snapshot.rs:289-296 next to src/state.rs:5876-5887 and 14998-15031: the comment that justifies the refusal says nothing takes a copy, while the state module it calls takes one on the clock.

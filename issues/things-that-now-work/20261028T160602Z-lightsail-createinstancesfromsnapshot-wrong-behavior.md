Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0b62ce506db227f8753aee084e9b25daeb773a55

# Restoring an instance from an automatic snapshot always fails with "this account has taken none", on the strength of a comment the crate now contradicts

_Filed: 2026-10-28T16:06:02Z_
_Operation: CreateInstancesFromSnapshot | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0b62ce506db227f8753aee084e9b25daeb773a55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateInstancesFromSnapshot` supports two sources: a manual `instanceSnapshotName`, or an AUTOMATIC snapshot addressed by `sourceInstanceName` plus `restoreDate`/`useLatestRestorableAutoSnapshot`. The handler parses the second form correctly, resolves the source instance — and then, for EVERY such request, returns the `DoesNotExist` envelope saying "this account has taken none". The justification given in the comment above the refusal is that the store holds no automatic snapshots: "This store holds none — the add-on records the hour it would run at and nothing takes a copy".

That premise is false of this crate. `crate::state` holds `auto_snapshots: BTreeMap<(&str, String), StoredAutoSnapshots>` with a `taken: Vec<StoredAutoSnapshot>` per resource; the clock TAKES those copies on the add-on's daily schedule (`StoredAutoSnapshots::take`) and settles them into `Success`; `GetAutoSnapshots` reports them; `DeleteAutoSnapshot` deletes one by date; and the sibling `CopySnapshot` resolves exactly this source form out of `store.auto_snapshots` (by `restoreDate`, or the newest `Success` copy for `useLatestRestorableAutoSnapshot`) and copies an instance's captured blueprint/bundle/attached disks into a manual snapshot.

So a caller who enables the `AutoSnapshot` add-on on an instance, waits for the copy to reach `Success` (which `GetAutoSnapshots` and `CopySnapshot` both confirm exists), and then restores it with `CreateInstancesFromSnapshot` is told the copy does not exist. The copy is there, and the crate already has the resolution code — `CopySnapshot` reads the same schedule the same way, including `copy.from_attached_disks`, which is precisely what an instance restore needs.

A second symptom of the same defect: the `(None, true)` arm stuffs the sentinel string `"the latest restorable automatic snapshot"` into the `date` field of `RequestedSource::AutoSnapshot`, a field the dated arm fills with a `YYYY-MM-DD` value. It is only harmless because the value is never used for a lookup — it is interpolated into the refusal message.

## Evidence

The unconditional refusal and the comment that justifies it — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_instances_from_snapshot.rs:363-382:
```
                    // An AUTOMATIC snapshot is one the `AutoSnapshot` add-on
                    // takes on the daily schedule an instance carries. This
                    // store holds none — the add-on records the hour it would run
                    // at and nothing takes a copy — so, once the source instance
                    // is known to exist, there is no automatic snapshot of it to
                    // restore from and the request is refused with the service's
                    // `DoesNotExist` envelope rather than answered with an
                    // instance restored from nothing.
                    RequestedSource::AutoSnapshot {
                        instance_name,
                        date,
                    } => {
                        store.instance(instance_name).ok_or_else(|| {
                            not_found(crate::state::RESOURCE_TYPE_INSTANCE, instance_name)
                        })?;
                        return Err(crate::scaffolded::Scaffolded::does_not_exist(format!(
                            "no automatic snapshot of the Instance {instance_name} is \
                             available to restore ({date}): this account has taken none"
                        )));
                    }
```

The crate DOES take copies — src/state.rs:14998-15031 (inside the clock pass):
```
        // The `AutoSnapshot` add-on takes a copy of the resource it is enabled
        // on once a day: that copy was owed at the instant the schedule named
        // whether or not anybody called, so the clock is what takes it — and
        // what settles one the service has finished taking into `Success`.
...
            schedule.take(due_at_ms, captured);
            schedule.due_at_ms = Some(next_auto_snapshot_due_ms(due_at_ms, &hour));
```
and src/state.rs:5876-5887:
```
    fn take(&mut self, at_ms: i64, from_attached_disks: Option<Vec<StoredDisk>>) {
        let date = utc_date(at_ms);
        if self.taken.iter().any(|taken| taken.date == date) {
            return;
        }
        self.taken.push(StoredAutoSnapshot {
            date,
            created_at_ms: at_ms,
            status: AUTO_SNAPSHOT_STATUS_IN_PROGRESS,
            due: Some(DueNamedState::after(at_ms, AUTO_SNAPSHOT_STATUS_SUCCESS)),
            from_attached_disks,
        });
```

The sibling that resolves the very same source form against that map — src/ops/copy_snapshot.rs:576-591:
```
    let taken = store
        .auto_snapshots
        .get(&(resource_type, resource_name.to_string()))
        .map(|schedule| schedule.taken.as_slice())
        .unwrap_or_default();
    // One copy a day, so a date addresses exactly one of a resource's copies;
    // the latest RESTORABLE one is the newest the service has finished taking,
    // the copies being held oldest first.
    let copy = match date {
        Some(date) => taken.iter().find(|copy| copy.date == date),
        None => taken
            .iter()
            .rev()
            .find(|copy| copy.status == crate::state::AUTO_SNAPSHOT_STATUS_SUCCESS),
    }
```

The model documents the path as a real one — smithy_query(query="doc", shape="CreateInstancesFromSnapshotRequest", member="sourceInstanceName"): "The name of the source instance from which the source automatic snapshot was created." … "Define this parameter only when creating a new instance from an automatic snapshot."

Test coverage does NOT pin the defect: tests/all/instance_restore_from_snapshot.rs:1000-1012 only exercises sources for which the account genuinely holds no matching copy (an absent instance, and instances whose schedule has taken nothing yet), while tests/all/snapshot_copy.rs:421-489 drives the clock until a copy reports `Success` (`wait_for_success`) and copies it successfully — proving the copies exist and are addressable by `restoreDate` and by `useLatestRestorableAutoSnapshot`.

## Steps to Reproduce

1. Call CreateInstances for "i1" (any blueprint/bundle) and wait for it to report `running`.
2. Call EnableAddOn with resourceName="i1" and addOnRequest={"addOnType":"AutoSnapshot","autoSnapshotAddOnRequest":{"snapshotTimeOfDay":"06:00"}}.
3. Poll GetAutoSnapshots with resourceName="i1" until one entry reports status "Success" and note its `date` (this is the same wait tests/all/snapshot_copy.rs performs).
4. Confirm the copy is real: call CopySnapshot with sourceRegion=<region>, sourceResourceName="i1", restoreDate=<that date>, targetSnapshotName="from-auto" — it succeeds.
5. Now call CreateInstancesFromSnapshot with instanceNames=["i2"], availabilityZone=<zone>, bundleId=<bundle>, sourceInstanceName="i1", useLatestRestorableAutoSnapshot=true (or restoreDate=<that date>).
6. Observe HTTP 400 NotFoundException / code DoesNotExist, message "no automatic snapshot of the Instance i1 is available to restore (...): this account has taken none" — for a copy GetAutoSnapshots just reported and CopySnapshot just copied.
7. Read src/ops/create_instances_from_snapshot.rs:363-370 next to src/state.rs:14998-15031: the comment that justifies the refusal says nothing takes a copy, while the state module it calls takes one on the clock.

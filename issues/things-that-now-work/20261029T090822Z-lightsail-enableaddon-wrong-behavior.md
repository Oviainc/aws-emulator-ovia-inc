Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8031858802602324d4d3567159831ce5c3f80459

# An enabled AutoSnapshot add-on never reports nextSnapshotTimeOfDay, although the store holds the instant the next copy is due

_Filed: 2026-10-29T09:08:22Z_
_Operation: EnableAddOn | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8031858802602324d4d3567159831ce5c3f80459`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every path that enables or modifies the `AutoSnapshot` add-on goes through the crate's one owner of that conversion, `crate::state::add_ons_from_requests` (reached from `EnableAddOn` via `enabled_add_ons`, and from the instance/disk creates). That function builds the stored `AddOn` with `name`, `status` and `snapshotTimeOfDay` and then `..Default::default()`, so `nextSnapshotTimeOfDay` is left `None` — and nothing else in the crate ever assigns it: `next_snapshot_time_of_day` appears exactly once outside the model JSON, in the type definition itself (`src/ops/types/shared_types.rs:592`).

The stored add-ons are reported back verbatim (`add_ons: Some(self.add_ons.clone())` for an instance, and the same for a disk), so `GetInstance`, `GetInstances`, `GetDisk` and `GetDisks` report an enabled automatic-snapshot add-on that says WHEN the daily copy is taken but never says when the NEXT one is due — the member the model dedicates to exactly that.

This is not information the emulator lacks: the store keeps the next due instant for every resource whose add-on is enabled (`StoredAutoSnapshots::due_at_ms`, documented as "The instant the next automatic snapshot of the resource is due, while the add-on is enabled on it"), `reconcile_auto_snapshots` sets it when the add-on is enabled and the clock re-arms it after each copy with `next_auto_snapshot_due_ms`. So the crate computes the value the member reports and then drops it on the floor, while the sibling member on the same structure (`snapshotTimeOfDay`) is materialised so carefully that the handler even mints a service-chosen hour when the request names none.

## Evidence

The modelled member that is never filled — smithy_query(query="shape", shape="AddOn"), member `nextSnapshotTimeOfDay`:
```
"The next daily time an automatic snapshot will be created."
"The time shown is in HH:00 format, and in Coordinated Universal Time (UTC)."
"The snapshot is automatically created between the time shown and up to 45 minutes after."
```

The one owner of the conversion drops it — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:15934-15941 (inside `add_ons_from_requests`, the "AutoSnapshot" arm):
```
                Ok(AddOn {
                    name: Some(ADD_ON_TYPE_AUTO_SNAPSHOT.to_string()),
                    status: Some(ADD_ON_STATUS_ENABLED.to_string()),
                    snapshot_time_of_day: Some(
                        named.unwrap_or_else(|| default_snapshot_time_of_day(resource_name)),
                    ),
                    ..Default::default()
                })
```
reached from this operation at src/ops/enable_add_on.rs:196 + 243-260:
```
                let enabled = enabled_add_ons(resource_type, &name, &request).map_err(invalid)?;
...
    crate::state::add_ons_from_requests(
        crate::state::ADD_ON_REQUEST_MEMBER,
        resource_name,
        requests,
    )
```

The value IS held by this crate — src/state.rs:5976-5987:
```
/// What the `AutoSnapshot` add-on of one resource has taken, and when it next
/// takes a copy.
#[derive(Debug, Default)]
pub struct StoredAutoSnapshots {
    /// The instant the next automatic snapshot of the resource is due, while
    /// the add-on is enabled on it. `None` once the add-on is disabled: the
    /// copies already taken stay, and no further one is owed.
    pub due_at_ms: Option<i64>,
```
and it is maintained on both sides of the clock — src/state.rs:13467-13473 (`reconcile_auto_snapshots`) and src/state.rs:15414 (`advance_clock`):
```
            schedule.due_at_ms = Some(next_auto_snapshot_due_ms(due_at_ms, &hour));
```

What a caller gets back is the stored structure unchanged — src/state.rs:5179 (instance view) and src/state.rs:5674 (disk view):
```
            add_ons: Some(self.add_ons.clone()),
```

No writer exists anywhere in the crate: grep for `next_snapshot_time_of_day|nextSnapshotTimeOfDay` over the crate matches only src/ops/types/shared_types.rs:592 (`pub next_snapshot_time_of_day: Option<String>,`) and the model JSON — no handler, no state function, and no comment explaining the omission.

Test coverage: tests/all/resource_add_ons.rs:365-405 asserts the reported add-on field by field (`name`, `status`, `snapshotTimeOfDay`, `threshold`, `duration`) and tests/all/add_on_snapshot_hour.rs, add_on_disable.rs, auto_snapshots.rs and instance_lifecycle.rs all read `snapshotTimeOfDay`; no test anywhere mentions `nextSnapshotTimeOfDay`, so nothing pins the member's absence and filling it changes no existing assertion.

## Steps to Reproduce

1. CreateInstances with instanceNames ["web"], a blueprint and bundle this service publishes, and wait for the instance to settle.
2. EnableAddOn with resourceName="web" and addOnRequest={"addOnType":"AutoSnapshot","autoSnapshotAddOnRequest":{"snapshotTimeOfDay":"06:00"}} — 200.
3. GetInstance with instanceName="web": addOns[0] reports name=AutoSnapshot, status=Enabled and snapshotTimeOfDay=06:00, but carries NO nextSnapshotTimeOfDay (the member is absent/null).
4. Confirm the service does know the answer: the same schedule's `due_at_ms` is what the clock uses to take the next copy (visible through GetAutoSnapshots once the hour comes round), and src/state.rs:13467-13473/15414 keep it current for exactly as long as the add-on is enabled.
5. Read src/state.rs:15934-15941 beside src/ops/types/shared_types.rs:592: the structure the store keeps and returns has the member, the one place that builds it fills everything but that member with `..Default::default()`, and no other line in the crate ever assigns it.

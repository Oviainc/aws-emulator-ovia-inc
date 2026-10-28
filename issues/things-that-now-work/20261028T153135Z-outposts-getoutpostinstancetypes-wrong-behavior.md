Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 48bb9604e330d1bf8edda7143fefbdaecc9daf20

# One asset-scoped capacity task erases the order-derived configuration of every other asset on the Outpost

_Filed: 2026-10-28T15:31:35Z_
_Operation: GetOutpostInstanceTypes | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `48bb9604e330d1bf8edda7143fefbdaecc9daf20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetOutpostInstanceTypes` reports `crate::state::configured_instance_types`, which is documented as combining TWO records of an Outpost's configuration: a completed capacity task, scoped to "the hardware that task named — one asset of the Outpost, or the Outpost itself when the task named none", and otherwise "the ORDER its hardware arrived on, until a capacity task reconfigures it".

The implementation does not honour that scoping. `configured_instance_type_names` collects the standing completed tasks into a `BTreeMap` keyed by scope and then, if that map is non-empty for ANY scope, returns only those tasks' pools and RETURNS EARLY — skipping the order-derived branch entirely. A task scoped to one single asset therefore wipes the configuration of every OTHER piece of compute hardware on the Outpost from the report.

This is reachable with ordinary calls: a RACK Outpost is installed with `RACK_HARDWARE`, which contains SIX `ASSET_TYPE_COMPUTE` units (state.rs:2322-2345), each with a ten-digit `AssetId` the modelled `AssetIdInput` (`^\d{10}$`) accepts (state.rs:2404-2413). A caller reads those ids out of `ListAssets`, runs `StartCapacityTask` with `AssetId` set to one of them, waits for it to complete, and then `GetOutpostInstanceTypes` collapses from "the largest size of each instance family the ordered configurations publish EC2 capacity of" to just that one asset's pools — the other five compute assets' capacity is reported as if it did not exist. A task scoped to one asset did not reconfigure them, so their order-derived configuration should still stand.

## Evidence

The rule the code states for itself, `services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:3020-3030`:

```
    /// * a COMPLETED capacity task. "The instance type capacities configured for this
    ///   asset. This can be changed through a capacity task", as
    ///   `ComputeAttributes$InstanceTypeCapacities` documents it, so the pools of such
    ///   a task are the capacity configured on the hardware that task named — one
    ///   asset of the Outpost, or the Outpost itself when the task named none — and on
    ///   each of those the task that completed LAST is the configuration that stands.
...
    /// * the ORDER its hardware arrived on, until a capacity task reconfigures it.
```

The code under that doc, `src/state.rs:3062-3095`:

```
        let mut standing: BTreeMap<Option<&str>, &CapacityTaskRecord> = BTreeMap::new();
        for task in self.capacity_tasks.values().filter(|task| {
            task.outpost_id == outpost_id && task.status == CAPACITY_TASK_COMPLETED
        }) {
            let scope = task.asset_id.as_deref();
...
        if !standing.is_empty() {
            for task in standing.values() {
                for pool in &task.requested_instance_pools {
                    if pool.count > 0 {
                        push_once(&mut names, &pool.instance_type);
                    }
                }
            }
            return names;
        }
        // Nothing has reconfigured this Outpost, so what is configured on it is what
        // its hardware arrived carrying ...
```

`!standing.is_empty()` is true for a task scoped to ONE asset, so the order-derived branch below it is never reached for the Outpost's other hardware.

The handler that reports it, `src/ops/get_outpost_instance_types.rs:236-245`:

```
        let (instance_types, next_token) = crate::state::page(
            crate::state::CONFIGURED_INSTANCE_TYPE_PAGE,
            store
                .configured_instance_types(&region, &outpost_id)
```

The hardware that makes the case reachable, `src/state.rs:2322-2345` — six `ASSET_TYPE_COMPUTE` units in `RACK_HARDWARE`:

```
    HardwareUnit {
        asset_type: ASSET_TYPE_COMPUTE,
        rack_elevation: Some(5.0),
    },
```

Test coverage: `tests/all/configured_instance_types.rs:9-21` states the same two rules ("A COMPLETED capacity task configures the hardware it named — one asset of the Outpost, or the Outpost itself when it named none" / "2. Until a capacity task reconfigures it, what is configured is what the hardware the Outpost's uncancelled orders subscribe to arrived carrying"). Its task table (lines 236-318) only ever uses Outposts whose assets were injected by `crate::outpost_assets::record` with no installed order hardware, and its order table (`an_outpost_no_capacity_task_has_touched_is_configured_by_its_orders`, lines 443-...) never runs a capacity task — so NO test covers an asset-scoped task on an Outpost that also holds order-derived configuration. The divergence is unpinned.

## Steps to Reproduce

1. CreateSite, then CreateOutpost with SupportedHardwareType=RACK.
2. CreateOrder against that Outpost with a line item naming a published CatalogItemId from ListCatalogItems; wait for the order to reach COMPLETED and the Outpost to settle ACTIVE.
3. GET /outposts/{OutpostId}/instanceTypes — note the reported InstanceTypes: the largest size of each family the ordered configuration publishes EC2 capacity of (the order-derived configuration of all six compute assets).
4. GET /outposts/{OutpostIdentifier}/assets and pick the AssetId of ONE compute asset.
5. POST /outposts/{OutpostIdentifier}/capacity with {"AssetId": "<that one asset>", "InstancePools": [{"InstanceType": "m5.large", "Count": 1}]} and wait for GetCapacityTask to report COMPLETED.
6. GET /outposts/{OutpostId}/instanceTypes again: the answer is now only ["m5.large"] — the configuration of the five compute assets the task never named has vanished, although nothing reconfigured them.

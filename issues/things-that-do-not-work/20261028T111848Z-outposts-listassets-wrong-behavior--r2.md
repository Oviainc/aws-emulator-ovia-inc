Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fa255a5b72d32034e1d300bb9323da879d85edac

# A completed capacity task never reaches the asset it configured: ComputeAttributes.InstanceTypeCapacities is hardcoded absent, so ListAssets denies the configuration GetOutpostInstanceTypes reports

_Filed: 2026-10-28T11:18:48Z_
_Operation: ListAssets | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fa255a5b72d32034e1d300bb9323da879d85edac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAssets` projects each installed hardware unit through `crate::state::installed_hardware`, which builds `ComputeAttributes` with `instance_type_capacities: None` as a literal. Nothing in the crate ever writes that member afterwards — it is written in exactly one place and read in exactly one place (the generated type), so for every asset of every Outpost, in every state, `ComputeAttributes.InstanceTypeCapacities` is permanently absent.

That contradicts the rule this same crate states and implements for the other read of the same fact. `crate::state::configured_instance_type_names` treats a COMPLETED capacity task's `RequestedInstancePools` as the per-asset configured capacity, scoped by the task's own `AssetId` (`let scope = task.asset_id.as_deref();`), and `GetOutpostInstanceTypes` serves exactly that. So after a real (non-dry-run) capacity task scoped to a compute asset settles COMPLETED:

* `GET /outposts/{OutpostId}/instanceTypes` reports the pools' instance types as the Outpost's configured capacity, and
* `GET /outposts/{OutpostIdentifier}/assets` reports that very asset with no `InstanceTypeCapacities` at all — the member the model (and the crate's own doc comment, which quotes it verbatim) says carries "the instance type capacities configured for this asset" and that "can be changed through a capacity task".

Two reads of one stored fact therefore disagree: one says the asset is configured with N instances of a type, the other says nothing has configured it. A caller that uses `ListAssets` to discover per-asset capacity (the only place the model exposes it) sees a capacity task as having had no effect on the hardware it named.

This is not the already-filed StartCapacityTask gap (20261027T201852Z, "a real capacity task always completes"): that one is about the task's own verdict never being computed. Here the task does complete and the crate DOES record its pools as the standing configuration — the defect is that the asset projection is never reconciled with it.

## Evidence

Smithy, `ComputeAttributes$InstanceTypeCapacities` (smithy_query query="shape" shape="ComputeAttributes"):

    "InstanceTypeCapacities": {"target": "AssetInstanceCapacityList", "type": "list",
      "doc": "<p>The instance type capacities configured for this asset. This can be changed through a\n      capacity task.</p>"}

The crate's own rule, quoting that same sentence and binding it to a completed task's pools per asset — `services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:2973-2982`:

    /// * a COMPLETED capacity task. "The instance type capacities configured for this
    ///   asset. This can be changed through a capacity task", as
    ///   `ComputeAttributes$InstanceTypeCapacities` documents it, so the pools of such
    ///   a task are the capacity configured on the hardware that task named — one
    ///   asset of the Outpost, or the Outpost itself when the task named none — and on
    ///   each of those the task that completed LAST is the configuration that stands.

The path that HONOURS it, scoped per asset — `src/state.rs:3016-3035`:

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

served by `src/ops/get_outpost_instance_types.rs:236-245`:

        let (instance_types, next_token) = crate::state::page(
            crate::state::CONFIGURED_INSTANCE_TYPE_PAGE,
            store
                .configured_instance_types(&region, &outpost_id)

The sibling path that does NOT honour it — `src/state.rs:2498-2510` (the only write of the member in the crate):

                compute_attributes: (unit.asset_type == ASSET_TYPE_COMPUTE).then(|| {
                    ComputeAttributes {
                        host_id: Some(host_id(sequence, position)),
                        state: Some(COMPUTE_ASSET_STATE_ACTIVE.to_string()),
                        instance_families: Some(instance_families.clone()),
                        // "The instance type capacities configured for this asset.
                        // This can be changed through a capacity task": nothing has
                        // configured this host's capacity per instance task, so it
                        // reports none rather than a split nobody asked for.
                        instance_type_capacities: None,

read back unchanged by `src/ops/list_assets.rs:224-234`:

        let (assets, next_token) = crate::state::page(
            crate::state::ASSET_PAGE,
            record
                .assets
                .iter()

That `None` is the whole story: `grep -n instance_type_capacities src/` matches only `src/state.rs:2507` and the generated field declaration `src/ops/types/list_assets_types.rs:48` — no code path ever assigns it a value, and `ListAssets` settles only Outposts (`src/ops/list_assets.rs:216`, `store.settle_outposts();`) where `GetOutpostInstanceTypes` settles both (`src/ops/get_outpost_instance_types.rs:223-224`, `store.settle_outposts(); store.settle_capacity_tasks();`).

Test coverage: `tests/all/outpost_hardware_installation.rs:325-331` asserts the member is absent only for a FRESHLY installed host — "a host nothing has configured reports no per-type capacity" — which is the uncontested case and states no reason for it to stay absent afterwards. `tests/all/outpost_assets.rs:319-351` compares the member against whatever the row seeded into the store directly. `tests/all/configured_instance_types.rs:151-164` seeds `instance_type_capacities: None` on its own asset fixture and then asserts only `GetOutpostInstanceTypes`. No test exercises `ListAssets` after a capacity task completes, so nothing pins this behaviour.

## Steps to Reproduce

1. Boot the outposts emulator. POST /sites to create a site, then POST /outposts with SupportedHardwareType=RACK on that site.
2. POST /orders for that Outpost with a LineItems entry naming a catalog SKU, and poll GET /orders/{OrderId} until it reports COMPLETED so the hardware installs.
3. GET /outposts/{OutpostIdentifier}/assets and note the ten-digit AssetId of a unit whose AssetType is COMPUTE; its ComputeAttributes carries HostId, State, InstanceFamilies and MaxVcpus but no InstanceTypeCapacities (correct at this point — nothing has configured it).
4. POST /outposts/{OutpostIdentifier}/capacity with {"AssetId":"<that asset id>","InstancePools":[{"InstanceType":"m5.4xlarge","Count":2}],"DryRun":false}.
5. Poll GET /outposts/{OutpostIdentifier}/capacity/{CapacityTaskId} until CapacityTaskStatus is COMPLETED.
6. GET /outposts/{OutpostId}/instanceTypes — m5.4xlarge is reported as the Outpost's configured capacity, so the crate did record the task as the configuration that stands.
7. GET /outposts/{OutpostIdentifier}/assets again — the same compute asset still reports no InstanceTypeCapacities, so the per-asset view denies the configuration the per-Outpost view just reported.

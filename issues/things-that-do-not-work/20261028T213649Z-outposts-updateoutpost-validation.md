Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 772fb5ab5b7496e2df886721dbd75d9882bc460e

# UpdateOutpost rips out an Outpost's installed hardware without consulting the capacity tasks scoped to it, the destruction DeleteOutpost refuses with the same modelled ConflictException

_Filed: 2026-10-28T21:36:49Z_
_Operation: UpdateOutpost | Class: validation_

> _Note: this issue was identified against the code base at commit `772fb5ab5b7496e2df886721dbd75d9882bc460e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A `SupportedHardwareType` change on an installed Outpost now re-installs hardware (`OutpostRecord::set_supported_hardware_type` clears `self.assets` and re-arms `active_at`). The handler consults nothing else, so the capacity tasks whose `AssetId` named the units just destroyed are left exactly where they were. Three states a later caller observes follow:

1. An ACTIVE capacity task survives the change. `GetCapacityTask` goes on reporting it, `settle_capacity_tasks` goes on to drive it to `COMPLETED`, and `StartCapacityTask` goes on refusing a task on the NEW hardware with a 409 naming a task that was configuring hardware the Outpost no longer holds. The crate's own `DeleteOutpost` refuses exactly this destruction against exactly this fact (`unfinished_work_on_outpost` -> `active_capacity_task_on_outpost`), and `UpdateOutpost` declares the same `ConflictException (HTTP 409)` -- "Updating or deleting this resource can cause an inconsistent state" -- and never answers it.

2. A COMPLETED asset-scoped task becomes an orphan that still speaks for hardware that is gone. `configured_instance_type_names` reports the pools of every standing asset-scoped task without checking the asset still exists, AND `holds_unreconfigured_compute` returns true for the newly installed compute units (they are in no `standing` entry), so `GetOutpostInstanceTypes` answers the order-derived instance types PLUS the orphan task's pools.

3. Worse, a completed task's asset scope silently re-attaches to DIFFERENT hardware, because asset ids are derived from `(outpost sequence, position)` and the two configurations share positions. `SERVER_HARDWARE` is one COMPUTE unit at position 1; `RACK_HARDWARE`'s position 1 is a POWERSHELF. So after a SERVER -> RACK update a task that configured the server's compute unit stands, by id, on the rack's power shelf, and `GetOutpostInstanceTypes` attributes its pools to hardware that never ran them.

This is the residual of the already-fixed form-factor/asset pairing gap: that fix made the hardware follow the form factor, but nothing was taught to reconcile -- or refuse -- the capacity tasks that reference the hardware it takes out. The same clearing also leaves `ConnectionRecord::asset_id` pointing at a unit that no longer exists.

## Evidence

The write that destroys the hardware -- `services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/update_outpost.rs:219-221`:

        if let Some(form_factor) = crate::state::non_empty(input.supported_hardware_type) {
            record.set_supported_hardware_type(form_factor);
        }

what it does -- `src/state.rs:555-566`:

    pub fn set_supported_hardware_type(&mut self, form_factor: String) {
        if self.outpost.supported_hardware_type.as_deref() == Some(form_factor.as_str()) {
            return;
        }
        self.outpost.supported_hardware_type = Some(form_factor);
        let installed = self.outpost.life_cycle_status.as_deref() == Some(LIFE_CYCLE_STATUS_ACTIVE);
        if !installed && self.active_at.is_none() {
            return;
        }
        self.assets.clear();
        self.active_at = Some(std::time::Instant::now() + OUTPOST_PROVISIONING);
    }

Nothing in that function, and nothing in the handler, touches `capacity_tasks`.

The SIBLING in this same crate that refuses the same destruction on the same fact -- `src/ops/delete_outpost.rs:190-206`:

        // Work still outstanding on the Outpost is what the modelled
        // `ConflictException` ("Updating or deleting this resource can cause an
        // inconsistent state") is for: removing the Outpost would destroy a
        // capacity task nothing cancelled or completed, ...
        if let Some(outstanding) = store.unfinished_work_on_outpost(&region, &outpost_id) {
            return Err(DeleteOutpostErrorModeled::ConflictException(ConflictException {

and the check behind it -- `src/state.rs:4088-4097`:

        if let Some(active) = self.active_capacity_task_on_outpost(outpost_id) {
            ...
                work: format!("capacity task {}", active.capacity_task_id),

`UpdateOutpost` models that very error -- `OPERATIONS.md:324`: "**Errors:** `AccessDeniedException (HTTP 403)`, `ConflictException (HTTP 409)`, `InternalServerException (HTTP 500)`, `NotFoundException (HTTP 404)`, `ValidationException (HTTP 400)`", and `smithy_query(query="shape",shape="ConflictException")`'s doc for it is "Updating or deleting this resource can cause an inconsistent state."

The orphan task still speaking for gone hardware -- `src/state.rs:3109-3112`:

        // Then each ASSET a task has reconfigured, which that task configured alone.
        for (_, task) in standing.iter().filter(|(scope, _)| scope.is_some()) {
            push_configured_pools(&mut names, task);
        }

with no test that the asset is still held, beside `src/state.rs:3124-3138` (`holds_unreconfigured_compute`), which returns true for any compute asset no standing task names -- so the new units re-admit the order-derived types at the same time.

The slot reuse that re-attaches a task scope to other hardware -- `src/state.rs:2409-2417`:

    fn asset_slot(sequence: u64, position: usize) -> u64 {
        let outpost = sequence % (ASSET_ID_SPACE / HARDWARE_UNITS_PER_OUTPOST);
        let position = u64::try_from(position).unwrap_or_default() % HARDWARE_UNITS_PER_OUTPOST;
        outpost * HARDWARE_UNITS_PER_OUTPOST + position
    }
    fn asset_id(sequence: u64, position: usize) -> String { format!("{:010}", asset_slot(sequence, position)) }

against the two configurations: `src/state.rs:2310-2314` -- `RACK_HARDWARE` begins `asset_type: ASSET_TYPE_POWERSHELF, rack_elevation: Some(1.0)` (position 1), while `src/state.rs:2368-2371` is the whole of `SERVER_HARDWARE`: `const SERVER_HARDWARE: &[HardwareUnit] = &[HardwareUnit { asset_type: ASSET_TYPE_COMPUTE, rack_elevation: None }];` (also position 1).

Test coverage: `tests/all/outpost_hardware_type_change.rs` is the suite for this write and never has a capacity task in flight across the update -- the only task it starts is started AFTER the update and the settle (`tests/all/outpost_hardware_type_change.rs:453-462`, inside `an_asset_installed_by_an_update_is_usable_where_the_api_takes_one`), and the rule-1/rule-2 test (lines 156-317) never calls the capacity route at all. `tests/all/capacity_task_lifecycle.rs` never patches an Outpost. So nothing pins the current behaviour.

## Steps to Reproduce

1. CreateSite, then CreateOutpost on it with SupportedHardwareType=RACK; keep the OutpostId.
2. Wait past OUTPOST_PROVISIONING and GET /outposts/{OutpostId} (LifeCycleStatus ACTIVE), then GET /outposts/{OutpostId}/assets and keep the AssetId of one COMPUTE unit (a position-5..10 slot).
3. POST /outposts/{OutpostId}/capacity with {"InstancePools":[{"InstanceType":"<a type the Outpost supports>","Count":1}],"AssetId":"<that compute AssetId>"} -- 200, task REQUESTED.
4. Immediately PATCH /outposts/{OutpostId} with {"SupportedHardwareType":"SERVER"} -- observe HTTP 200, where DeleteOutpost on the same Outpost at the same instant answers 409 ConflictException naming that capacity task.
5. GET /outposts/{OutpostId}/assets -- empty (the rack was taken out). GET /outposts/{OutpostId}/capacity/{CapacityTaskId} -- the task is still there, still reporting an AssetId that ListAssets no longer lists, and GET /outposts/{OutpostId}/supportedInstanceTypes?AssetId=<that AssetId> now answers 404 NotFoundException.
6. Wait for the task to settle to COMPLETED and for the new SERVER hardware to install, then GET /outposts/{OutpostId}/instanceTypes -- it reports the orphan task's pools (hardware that is gone) alongside the order-derived instance types of the newly installed server.
7. For the slot-reuse case: do the same starting from SupportedHardwareType=SERVER with a task scoped to the server's single compute asset, let it complete, then PATCH to RACK and settle -- the completed task's asset scope now matches the rack's POWERSHELF at the same ten-digit slot, and GetOutpostInstanceTypes reports its pools as configured on that unit.

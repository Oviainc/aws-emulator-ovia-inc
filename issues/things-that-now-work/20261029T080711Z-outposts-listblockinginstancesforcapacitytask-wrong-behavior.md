Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4c62cef672c36ef491d018c691144f86baa80e9d

# The blocking-instance list ignores the capacity task's AssetId scope, so it names instances the crate's own capacity weighing says hold the task up nowhere

_Filed: 2026-10-29T08:07:11Z_
_Operation: ListBlockingInstancesForCapacityTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4c62cef672c36ef491d018c691144f86baa80e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A capacity task may be scoped to ONE asset of an Outpost (`StartCapacityTask$AssetId`), and the crate's capacity weighing honours that scope: `weigh_capacity` counts only instances whose `AssetId` is the one the task named (`in_scope`), so an instance running on a different asset of the same Outpost holds up nothing and `needs_evacuation` never fires for it.

`ListBlockingInstancesForCapacityTask` has the task record in hand (it reads `task.instances_to_exclude` off it, and `CapacityTaskRecord::asset_id` is right beside it), but it filters the Outpost's instances by `instance_can_be_stopped` ALONE and never by the task's asset scope. For an asset-scoped task it therefore reports every stoppable instance of the whole Outpost as blocking that task — instances the same crate's weighing never counted and that stopping would free no capacity in the task's scope.

The two sides are supposed to be the same rule: `needs_evacuation`'s own doc asserts that the instances it weighs "are exactly the ones `ListBlockingInstancesForCapacityTask` recommends stopping", `instance_can_be_stopped`'s doc says it is "Stated once here because two rules read it", and the handler comment claims it applies "the same one the dry-run capacity verdict reads". Only the verdict applies the asset scope. The sibling report that is also scoped to one asset, `GetOutpostSupportedInstanceTypes`, does narrow to the named asset (`asset_of_outpost`), so the crate already owns this scoping.

Severity LOW because `OutpostRecord::instances` is only ever constructed as `Vec::new()` today (filed separately as the open ListAssetInstances issue), so the divergence is not observable yet — but the two copies of the rule have already drifted and will answer differently the moment an instance is recorded.

## Evidence

The crate's claim that the two agree — `services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:1059-1072`:

    /// Whether the configuration fits only once the instances the task would need
    /// stopped have stopped: the hardware can hold it
    /// ([`Self::verdict`]), but not beside everything running on it now.
    ///
    /// Those instances are exactly the ones
    /// `ListBlockingInstancesForCapacityTask` recommends stopping, so a task is held
    /// up only by instances that list names, and never by an Outpost's other
    /// tenants' ones — capacity THEY hold is capacity the configuration was already
    /// weighed without.
    fn needs_evacuation(&self) -> bool {
        self.unsupported.is_none()
            && self.requested <= self.free_after_evacuation
            && self.requested > self.free
    }

and the same claim on the shared predicate — `src/state.rs:1135-1142`:

    /// Stated once here because two rules read it: which instances
    /// `ListBlockingInstancesForCapacityTask` recommends stopping, and how much of the
    /// hardware's capacity a task could have
    /// ([`ComAmazonawsOutpostsOutpostsolafserviceState::capacity_verdict`]).
    pub fn instance_can_be_stopped(instance: &AssetInstance, exclude: &ExcludedInstances<'_>) -> bool {
        instance.account_id.as_deref() == Some(::engine::emulator_account_id())
            && !exclude.names(instance)
    }

The weighing side, which DOES apply the task's asset scope — `src/state.rs:3556` and `src/state.rs:3625-3628`:

        let in_scope = |asset: Option<&str>| asset_id.is_none_or(|named| asset == Some(named));

        for instance in record
            .iter()
            .flat_map(|record| record.instances.iter())
            .filter(|instance| in_scope(instance.asset_id.as_deref()))

The list side, which does not — `src/ops/list_blocking_instances_for_capacity_task.rs:252-266` (the `task` record resolved at :230-243 is never asked for its `asset_id`):

        let (blocking_instances, next_token) = crate::state::page(
            crate::state::BLOCKING_INSTANCE_PAGE,
            record
                .instances
                .iter()
                .enumerate()
                // Which instances a task would need stopped is the rule
                // `crate::state::instance_can_be_stopped` states, and the same one
                // the dry-run capacity verdict reads: an instance this task could
                // stop is one it recommends stopping.
                .filter(|(_, instance)| {
                    crate::state::instance_can_be_stopped(instance, &exclude)
                })
                .map(|(position, instance)| (position as u64, projection(instance))),

The scope really is on the record the handler holds — `src/state.rs:926-928` (`capacity_task_projection!`, which reports it as the task's own `AssetId`):

            outpost_id: Some(record.outpost_id.clone()),
            order_id: record.order_id.clone(),
            asset_id: record.asset_id.clone(),

and the sibling asset-scoped report that does narrow — `src/ops/get_outpost_supported_instance_types.rs:286-301`:

        let hardware = match input.asset_id.as_deref().filter(|id| !id.is_empty()) {
            Some(asset_id) => {
                let asset = store
                    .asset_of_outpost(&outpost_id, asset_id)
                    ...
                Hardware::Asset(asset.compute_attributes.as_ref())
            }
            None => Hardware::Outpost,
        };

Test coverage today: `tests/all/blocking_instances.rs` exercises the list against an Outpost that records no instance at all (its only `##` rows at :758 and :860 are cross-list token refusals), and `tests/all/asset_scoped_capacity_configuration.rs` covers the asset scope only for the configuration a completed task leaves — no test starts an asset-scoped task and compares the blocking list against the weighing, so the drift is unpinned in either direction.

## Steps to Reproduce

1. Read src/state.rs:1063-1067: the crate asserts the instances its weighing counts "are exactly the ones ListBlockingInstancesForCapacityTask recommends stopping".
2. Read src/state.rs:3556 and 3625-3628: weigh_capacity filters the Outpost's instances with in_scope(instance.asset_id), i.e. only the asset the task named.
3. Read src/ops/list_blocking_instances_for_capacity_task.rs:252-266: the list filters record.instances with instance_can_be_stopped only, never comparing instance.asset_id against task.asset_id, although `task` is in scope and carries that member (src/state.rs:928).
4. Consequence, once an instance can be recorded on an Outpost: POST /outposts/{op}/capacity with AssetId=<asset A> and InstancePools, then GET /outposts/{op}/capacity/{cap}/blockingInstances — instances running on asset B are reported as blocking a task scoped to asset A, while StartCapacityTask(DryRun=true) for the same configuration reports COMPLETED because those same instances were never weighed.

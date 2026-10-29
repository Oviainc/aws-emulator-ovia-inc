Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: c6e8792d89a3879b99e75c495a834b1d8b961dff

# A real (non-dry-run) capacity task always completes: the configuration is never weighed and FAIL_TASK is never honoured

_Filed: 2026-10-27T20:18:52Z_
_Operation: StartCapacityTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c6e8792d89a3879b99e75c495a834b1d8b961dff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartCapacityTask` has two paths through the same handler, and only the dry-run one applies the crate's capacity rule.

The `DryRun` branch calls `store.capacity_verdict(...)` — the crate's own rule for "whether what it asks for is at or above the instance capacity available to it" — and reports `CapacityTaskStatus = FAILED` with `Failed.Type = UNSUPPORTED_CAPACITY_CONFIGURATION` for a configuration the hardware cannot hold (too many vCPUs, or an instance type none of the hardware in scope runs).

The non-dry-run branch, which runs a few lines below on the SAME inputs, never calls `capacity_verdict` and never looks at the blocking instances the crate already computes (`instance_can_be_stopped`, the list `ListBlockingInstancesForCapacityTask` serves). It stores `status = REQUESTED`, `failed = None` and the fixed progression `capacity_task_progression(now)` = `[IN_PROGRESS, COMPLETED]`, so EVERY started task runs to `COMPLETED` on the clock, whatever it asked for. The handler's own comment admits it: "Nothing in this slice fails a task."

Two consequences:

1. The identical request answers `FAILED`/`UNSUPPORTED_CAPACITY_CONFIGURATION` with `DryRun=true` and `COMPLETED` with `DryRun=false`, so the dry run a caller uses to pre-flight a plan contradicts what actually happens — the opposite of what the modelled `DryRun` is for ("determine if the instance type and instance size changes is above or below available instance capacity").
2. `TaskActionOnBlockingInstances` is stored and echoed but never acted on, so `FAIL_TASK` — modelled as "The capacity task fails" — completes the task instead, and `WAIT_FOR_EVACUATION` is indistinguishable from it.

The damage is not confined to the task: `configured_instance_type_names` takes the `requested_instance_pools` of the COMPLETED task that stands on each piece of hardware as the Outpost's configuration, so a task asking for an instance type the Outpost's hardware does not run completes and then `GetOutpostInstanceTypes` reports that type as the Outpost's configured capacity — a configuration the crate's own `capacity_verdict` and `GetOutpostSupportedInstanceTypes` both deny the hardware can hold.

## Evidence

Smithy, `StartCapacityTaskInput$TaskActionOnBlockingInstances` (smithy_query doc): "Specify one of the following options in case an instance is blocking the capacity task from running. ... `FAIL_TASK` - The capacity task fails."

Smithy, `StartCapacityTaskInput$DryRun`: "You can request a dry run to determine if the instance type and instance size changes is above or below available instance capacity. Requesting a dry run does not make any changes to your plan."

The branch that DOES apply the rule — `src/ops/start_capacity_task.rs:327-358`:

        if input.dry_run {
            let verdict = store.capacity_verdict(
                &region,
                &outpost_id,
                asset_id.as_deref(),
                &input.instance_pools,
                input.instances_to_exclude.as_ref(),
            );
            ...
                capacity_task_status: Some(verdict.status.to_string()),
                failed: verdict.failed,

The sibling branch that skips it, with the comment admitting the gap — `src/ops/start_capacity_task.rs:441-453`:

            task_action_on_blocking_instances: crate::state::non_empty(
                input.task_action_on_blocking_instances,
            ),
            // A just-started task is awaiting its next step, so it reports the
            // non-terminal status until the progression the clock owes falls due;
            // every read materialises it through `settle_capacity_tasks`.
            status: crate::state::CAPACITY_TASK_REQUESTED.to_string(),
            // Nothing in this slice fails a task. See `CAPACITY_TASK_FAILED`.
            failed: None,
            ...
            pending: crate::state::capacity_task_progression(now),

and the progression that fixes the outcome regardless of the request — `src/state.rs:721-723`:

    pub fn capacity_task_progression(now: std::time::SystemTime) -> VecDeque<CapacityTaskTransition> {
        capacity_task_steps(now, &[CAPACITY_TASK_IN_PROGRESS, CAPACITY_TASK_COMPLETED])
    }

The rule the real start skips — `src/state.rs:3024-3027` / `3111-3122`:

    /// The verdict on the capacity configuration `pools` asks of the Outpost
    /// `outpost_id` names in `region`, ... whether what it asks for is at or above the instance
    /// capacity available to it.
    ...
            let runs_it = *runs.entry(instance_type).or_insert_with(|| { ... });
            if !runs_it {
                return CapacityVerdict::unsupported(instance_type_not_run(instance_type));

The knock-on: a completed task's pools become the Outpost's standing configuration — `src/state.rs:2881-2900`:

        for task in self.capacity_tasks.values().filter(|task| {
            task.outpost_id == outpost_id && task.status == CAPACITY_TASK_COMPLETED
        }) { ... }
        if !standing.is_empty() {
            for task in standing.values() {
                for pool in &task.requested_instance_pools {
                    if pool.count > 0 {
                        push_once(&mut names, &pool.instance_type);

read by `src/ops/get_outpost_instance_types.rs:239` (`store.configured_instance_types(&region, &outpost_id)`).

Test coverage today: `tests/all/capacity_task_dry_run.rs:21-27` states and exercises the verdict rule for dry runs only ("At or below it the configuration is `COMPLETED`; above it, and for an instance type that hardware does not run at all, it is `FAILED` with the modelled `UNSUPPORTED_CAPACITY_CONFIGURATION`"). `tests/all/capacity_task_lifecycle.rs:16-18` pins the real path as "advances through the modelled statuses on the clock alone, ending `COMPLETED`" and exercises it with a FITTING plan (`pool("m5.4xlarge", 1)`, line 493); the over-capacity row `json!({ "InstancePools": [pool("c5.24xlarge", 9999)] })` at line 296 is only asserted to be echoed back, and `TaskActionOnBlockingInstances` is only asserted to be echoed (lines 345-353, 399-400). No test states a reason for a real task never failing.

## Steps to Reproduce

1. Boot the outposts emulator; CreateSite then CreateOutpost with SupportedHardwareType=RACK and wait for the Outpost to settle so its hardware is installed.
2. POST /outposts/{OutpostId}/capacity with {"InstancePools":[{"InstanceType":"c5.24xlarge","Count":9999}],"DryRun":true} — observe 200 with CapacityTaskStatus=FAILED and Failed.Type=UNSUPPORTED_CAPACITY_CONFIGURATION.
3. Send the same body with "DryRun":false — observe 200 with CapacityTaskStatus=REQUESTED and no Failed member.
4. Poll GET /outposts/{OutpostId}/capacity/{CapacityTaskId} until it settles — it reports COMPLETED, although the dry run of the identical plan said the hardware cannot hold it.
5. GET /outposts/{OutpostId}/instanceTypes — the impossible pool's instance type is now reported as the Outpost's configured capacity.
6. Repeat with an Outpost that has a recorded running instance of another account (so ListBlockingInstancesForCapacityTask is non-empty) and send "TaskActionOnBlockingInstances":"FAIL_TASK" — the task still completes instead of failing.

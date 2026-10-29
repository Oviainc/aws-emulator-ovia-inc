Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 67d3ef025eb7c6b921e8b895d06c7f32bf688664

# UpdateTable deregisters the table's Application Auto Scaling targets for an update that re-sets the capacity mode the table is already in

_Filed: 2026-10-28T22:44:38Z_
_Operation: UpdateTable | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `67d3ef025eb7c6b921e8b895d06c7f32bf688664`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateTable` decides whether to tear down the table's auto scaling resources from the REQUESTED mode alone, not from a change of mode:

    leaves_provisioned_mode = capacity.throughput_mode != crate::state::PROVISIONED_THROUGHPUT_MODE;

So the flag is true not only for a table that is MOVED out of provisioned mode, but also for a table that was never in it: an `UpdateTable {capacitySpecification: {throughputMode: PAY_PER_REQUEST}}` against a table that is ALREADY `PAY_PER_REQUEST` changes no throughput capacity, yet it still runs the whole teardown — `table.auto_scaling_specification = None`, `table.replica_specifications.clear()`, and, once the store lock is released, `crate::state::retire_auto_scaling(..)`.

`retire_auto_scaling` is not a local reset: it calls Application Auto Scaling's `apply_managed_target(.., None)` for both capacity dimensions, whose `None` branch REMOVES the scalable target, every scaling policy of that target, its scheduled actions and the CloudWatch alarms those own. This crate deliberately treats Application Auto Scaling as the ONE copy of a table's auto scaling settings and documents that a target a caller registered through Application Auto Scaling's own API is reported as the table's settings — so a request that changes nothing about the table destroys settings in another service's store that `GetTableAutoScalingSettings` was reporting a moment earlier, with no call of their own having touched them.

The handler already knows how to tell a real move from a re-set: the statement immediately above computes `was_pay_per_request` for exactly that reason, and its comment says a re-set of the mode the table is already in "changed no throughput capacity, so it deliberately leaves the existing stamp where it is". The comment over the broken line states the narrower rule the code does not implement — "a table MOVED OUT of that mode" — and `retire_auto_scaling`'s own doc states its premise as "the table is going away", which is not true here either. The fix is to gate the teardown on an actual transition (the table was provisioned and the update leaves it unprovisioned), the same way `last_update_to_pay_per_request` is gated one statement earlier; a genuine move keeps behaving exactly as it does today.

This is a different defect from the fixed issue `20261025T083648Z-keyspaces-updatetable-wrong-behavior--r2` ("UpdateTable LEAVES per-Region overrides on a table it just moved out of PROVISIONED"): that one asked for the teardown to happen on a real move, which must stay; this one is the teardown firing when there is no move at all.

## Evidence

The break — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/update_table.rs:685-703`:
```
            // Auto scaling is "the optional auto scaling settings for a table in
            // provisioned capacity mode", so a table moved OUT of that mode has no
            // provisioned throughput for a policy to move and therefore no auto scaling
            // settings at all: the settings go, here and in Application Auto Scaling.
            leaves_provisioned_mode =
                capacity.throughput_mode != crate::state::PROVISIONED_THROUGHPUT_MODE;
            if leaves_provisioned_mode {
                table.auto_scaling_specification = None;
                ...
                table.replica_specifications.clear();
            }
```
The sibling rule in the SAME block, which does distinguish a move from a re-set — `src/ops/update_table.rs:669-684`:
```
            // "The timestamp of the last operation that changed the provisioned
            // throughput capacity of a table": the summary member is named for a move TO
            // `PAY_PER_REQUEST`, so it is stamped only when this update is that move. An
            // update that re-sets the mode a table is ALREADY in changed no throughput
            // capacity, so it deliberately leaves the existing stamp where it is.
            let was_pay_per_request = table
                .capacity_specification
                .as_ref()
                .is_some_and(|existing| {
                    existing.throughput_mode == crate::state::PAY_PER_REQUEST_THROUGHPUT_MODE
                });
            if !was_pay_per_request
                && capacity.throughput_mode == crate::state::PAY_PER_REQUEST_THROUGHPUT_MODE
            {
                table.last_update_to_pay_per_request = Some(now);
            }
```
The cross-service teardown it triggers — `src/ops/update_table.rs:772-779`:
```
        if leaves_provisioned_mode {
            crate::state::retire_auto_scaling(
                &*self.engine,
                &region,
                &input.keyspace_name,
                &input.table_name,
            );
        }
```
whose stated premise is a table that is going away — `src/state.rs:2866-2874`:
```
/// Retire both of a table's scalable targets, with their policies and the alarms those
/// minted: the table is going away, so the capacities it was scaled on no longer exist.
///
/// Call it with NO store lock held.
pub fn retire_auto_scaling<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    keyspace_name: &str,
    table_name: &str,
) {
```
What is destroyed in the other store — `services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/state.rs:2417-2436`:
```
        match wanted {
            // "Deregistering a scalable target deletes the scaling policies and the
            // scheduled actions that are associated with it" — the same cascade, because
            // this is the same resource going away.
            None => {
                if let Some(removed) = store.targets.remove(key) {
                    store.arn_index.remove(&removed.arn);
                    ...
                    store.policies.retain(|policy_key, _| &policy_key.target != key);
```
and why those targets are the table's reported settings even when a caller registered them directly — `src/state.rs:2886-2894`:
```
/// The auto scaling settings of a table as they stand NOW: what Application Auto Scaling
/// holds for each of the table's two capacities, which is where the settings live.
///
/// Per capacity, the settings are the registered scalable target's capacity window and
/// the target-tracking policy that steers it — including one a caller registered through
/// Application Auto Scaling itself, since that is the same resource this service's own
/// writes produce.
```

Test coverage: nothing pins the no-move path, so nothing constrains the fix. Every row of `tests/all/update_table_capacity_mode_replica_settings.rs` starts from a PROVISIONED table — `tests/all/update_table_capacity_mode_replica_settings.rs:243-245`:
```
/// RULE 1: a capacity-mode change that takes a table out of provisioned mode leaves it
/// with no per-Region read capacity or read capacity auto scaling setting in any Region,
/// and one that leaves the table in provisioned mode keeps every one of them.
```
and `tests/all/auto_scaling_propagation.rs` only ever moves a table that really was provisioned, so a gate on an actual transition leaves both suites' assertions unchanged.

## Steps to Reproduce

1. Start one engine with both Amazon Keyspaces and Application Auto Scaling mounted, as tests/all/auto_scaling_propagation.rs does
2. CreateKeyspace {"keyspaceName":"app"}, then CreateTable {"keyspaceName":"app","tableName":"ledger","schemaDefinition":{...}} with NO capacitySpecification, so the table is in the modelled default PAY_PER_REQUEST mode
3. Through Application Auto Scaling's own API, RegisterScalableTarget {ServiceNamespace:"cassandra", ResourceId:"keyspace/app/table/ledger", ScalableDimension:"cassandra:table:ReadCapacityUnits", MinCapacity:5, MaxCapacity:50} and PutScalingPolicy for a TargetTrackingScaling policy on it
4. GetTableAutoScalingSettings {"keyspaceName":"app","tableName":"ledger"} — it reports that target as the table's readCapacityAutoScaling (minimumUnits 5 / maximumUnits 50)
5. UpdateTable {"keyspaceName":"app","tableName":"ledger","capacitySpecification":{"throughputMode":"PAY_PER_REQUEST"}} — the mode the table is already in, so nothing about its throughput changes, and the response is a plain 200
6. GetTableAutoScalingSettings again — autoScalingSpecification is now absent, and DescribeScalableTargets / DescribeScalingPolicies show the target, its policy and its CloudWatch alarms deleted: expected an update that re-sets the current mode to leave them exactly as they were, as the sibling lastUpdateToPayPerRequestTimestamp rule in the same block does

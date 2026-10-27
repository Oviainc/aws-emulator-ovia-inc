Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# RestoreTable reproduces the source table's auto scaling from this service's stale stored copy instead of what Application Auto Scaling holds

_Filed: 2026-10-27T14:57:09Z_
_Operation: RestoreTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RestoreTable` carries the source table's auto scaling settings over to the restored table by cloning `crate::state::Table::auto_scaling_specification` — the specification THIS service last wrote — rather than by reading the settings as they stand now.

This crate states, and its sibling handler implements, the opposite rule: a Keyspaces table's auto scaling settings LIVE in Application Auto Scaling, and `GetTableAutoScalingSettings` reports what that store holds "whichever service wrote them" (`crate::state::reported_auto_scaling`). `RestoreTable` is the only other operation that needs a table's current auto scaling settings and it is the one that does not call that helper, so the two operations answer differently for the same table:

1. A source table whose scalable target and target-tracking policy were registered or re-tuned directly through Application Auto Scaling: `GetTableAutoScalingSettings` reports those settings (pinned by the crate's own suite), while `RestoreTable` copies `auto_scaling_specification` — `None`, or the older window — so the restored table lands with no auto scaling, or with a window the source no longer has.
2. A source table whose Keyspaces-configured target was deregistered through Application Auto Scaling: `GetTableAutoScalingSettings` then reports no settings at all (also pinned), but `RestoreTable` still clones the stale stored specification and `publish_auto_scaling` re-registers a scalable target and a `<keyspace>-<table>-scaling-policy` for the restored table — conjuring settings the source does not have.

The model says these settings are restored "based on the table's settings as of the current time", which is exactly what the stored field is not.

## Evidence

Smithy `RestoreTable` documentation (smithy_query query="doc" shape="com.amazonaws.keyspaces#RestoreTable"), which states the behaviour itself:

> "In addition to the table's schema, data, and TTL settings, `RestoreTable` restores the capacity mode, auto scaling settings, encryption settings, and point-in-time recovery settings from the source table. Unlike the table's schema data and TTL settings, which are restored based on the selected timestamp, **these settings are always restored based on the table's settings as of the current time** or when the table was deleted."

What the handler actually reads — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/restore_table.rs:420-428` (`source` is the store clone taken at `restore_table.rs:348`):
```
        let auto_scaling_specification = match input.auto_scaling_specification {
            Some(supplied) => {
                crate::state::resolve_auto_scaling(Some(supplied), &effective_capacity)
                    .map_err(invalid)?
            }
            None => source.auto_scaling_specification.clone().filter(|_| {
                effective_capacity.throughput_mode == crate::state::PROVISIONED_THROUGHPUT_MODE
            }),
        };
```
and it then publishes that stale value as the restored table's own, `src/ops/restore_table.rs:605-618`:
```
        // The restored table's auto scaling settings are Application Auto Scaling
        // resources of the RESTORED table, so they are registered for it the way a create
        // registers them — whether they came with this request or from the source table.
        if let Some(specification) = auto_scaling_specification.as_ref() {
            crate::state::publish_auto_scaling(
```

The rule this crate states for where those settings live — `src/state.rs:2844-2858`:
```
/// The auto scaling settings of a table as they stand NOW: what Application Auto Scaling
/// holds for each of the table's two capacities, which is where the settings live.
///
/// Per capacity, the settings are the registered scalable target's capacity window and
/// the target-tracking policy that steers it — including one a caller registered through
/// Application Auto Scaling itself, since that is the same resource this service's own
/// writes produce. A capacity with NO scalable target is a capacity auto scaling is off
/// for ... one that was never configured, or whose target has since been deregistered,
/// has no settings at all.
```

The sibling that calls the helper — `src/ops/get_table_auto_scaling_settings.rs:237-242`:
```
        let auto_scaling_specification = crate::state::reported_auto_scaling(
            &*self.engine,
            &table.keyspace_name,
            &table.table_name,
            table.auto_scaling_specification.as_ref(),
        );
```
and `grep` over `src/ops/` shows `reported_auto_scaling` is called from that file only, while `restore_table.rs` calls `publish_auto_scaling` alone.

Test coverage: the GetTableAutoScalingSettings side is pinned — `tests/all/auto_scaling_propagation.rs:15` ("and the settings Amazon Keyspaces reports are the ones Application Auto Scaling holds, whichever service wrote them") and `tests/all/auto_scaling_propagation.rs:762-849` (`settings_registered_through_application_auto_scaling_are_reported_by_keyspaces`: `RegisterScalableTarget` + `PutScalingPolicy` straight to Application Auto Scaling, then `GetTableAutoScalingSettings` reports `minimumUnits`/`maximumUnits`/`targetValue`, and deregistering removes them). The RestoreTable side is NOT pinned for that case: `tests/all/restore_table.rs:192-228` (`provisioned_with_auto_scaling`) and `:258-291` only exercise settings written through Keyspaces itself, where the stored copy and Application Auto Scaling still agree.

## Steps to Reproduce

1. Start an emulator with both Amazon Keyspaces and Application Auto Scaling on one engine (as tests/all/auto_scaling_propagation.rs does).
2. CreateKeyspace {"keyspaceName":"as_ks"}; wait for ACTIVE.
3. CreateTable {"keyspaceName":"as_ks","tableName":"src","schemaDefinition":{...},"pointInTimeRecovery":{"status":"ENABLED"},"capacitySpecification":{"throughputMode":"PROVISIONED","readCapacityUnits":5,"writeCapacityUnits":5}} with NO autoScalingSpecification; wait for ACTIVE.
4. Call Application Auto Scaling directly: RegisterScalableTarget {"ServiceNamespace":"cassandra","ResourceId":"keyspace/as_ks/table/src","ScalableDimension":"cassandra:table:ReadCapacityUnits","MinCapacity":3,"MaxCapacity":30} and PutScalingPolicy with a TargetTrackingScaling configuration.
5. GetTableAutoScalingSettings {"keyspaceName":"as_ks","tableName":"src"} — reports readCapacityAutoScaling minimumUnits 3 / maximumUnits 30 (pinned by tests/all/auto_scaling_propagation.rs:843-844).
6. RestoreTable {"sourceKeyspaceName":"as_ks","sourceTableName":"src","targetKeyspaceName":"as_ks","targetTableName":"restored"}; wait for ACTIVE.
7. GetTableAutoScalingSettings {"keyspaceName":"as_ks","tableName":"restored"} — autoScalingSpecification is absent, although the model says the restore reproduces "auto scaling settings ... from the source table ... as of the current time". Mirror case: configure auto scaling through CreateTable, then DeregisterScalableTarget both dimensions through Application Auto Scaling (GetTableAutoScalingSettings on the source then reports nothing), restore, and the restored table comes back WITH a scalable target and policy re-registered from the stale stored copy.

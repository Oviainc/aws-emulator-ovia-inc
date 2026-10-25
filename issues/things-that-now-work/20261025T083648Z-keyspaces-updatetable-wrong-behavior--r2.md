Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a727425709f65cfb01a0ac76d7787005b46d11cb

# UpdateTable leaves per-Region readCapacityUnits/readCapacityAutoScaling overrides on a table it just moved out of PROVISIONED

_Filed: 2026-10-25T08:36:48Z_
_Operation: UpdateTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a727425709f65cfb01a0ac76d7787005b46d11cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `UpdateTable` changes `capacitySpecification.throughputMode` away from `PROVISIONED`, it clears the table-level `auto_scaling_specification` and retires the Application Auto Scaling targets — but it leaves `table.replica_specifications` untouched. Those entries carry exactly the two per-Region settings this crate says belong only to PROVISIONED: `readCapacityUnits` and `readCapacityAutoScaling`.

Two observable consequences, both from state this crate refuses to accept as input:

1. `GetTable` then reports, per replica, `capacitySpecification.throughputMode = PAY_PER_REQUEST` together with the stale `readCapacityUnits` override (`get_table.rs:252` + `get_table.rs:323-325` via `Table::read_capacity_units_in`), while the table-level summary reports no read capacity at all.
2. `GetTableAutoScalingSettings` answers 200 with per-Region auto scaling settings for a PAY_PER_REQUEST table, because `has_replica_auto_scaling()` is still true and short-circuits the not-found branch (`get_table_auto_scaling_settings.rs:240-241`) — even though the update just decided the table has "no auto scaling settings at all".

The sibling path for the same situation does drop them: `RestoreTable` keeps the source table's `replica_specifications` only while the restored table lands in PROVISIONED. And the shared helper `resolve_replica_specifications` refuses, as 400 ValidationException, the very combination `UpdateTable` here creates.

## Evidence

UpdateTable's own comment states the rule it then only half applies — services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/update_table.rs:677-686:
```
            // Auto scaling is "the optional auto scaling settings for a table in
            // provisioned capacity mode", so a table moved OUT of that mode has no
            // provisioned throughput for a policy to move and therefore no auto scaling
            // settings at all: the settings go, here and in Application Auto Scaling.
            leaves_provisioned_mode =
                capacity.throughput_mode != crate::state::PROVISIONED_THROUGHPUT_MODE;
            if leaves_provisioned_mode {
                table.auto_scaling_specification = None;
            }
            table.capacity_specification = Some(capacity);
```
Nothing in the rest of the handler touches `table.replica_specifications`; the only assignment to it is the one that happens when the request itself carries the member (update_table.rs:725-728):
```
        if let Some(replicas) = replica_specifications {
            // Already merged onto the Regions the table had settings for, ...
            table.replica_specifications = replicas;
        }
```

The sibling operation DOES drop them for a non-provisioned table — services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/restore_table.rs:507-519:
```
            None => match effective_capacity.throughput_mode
                == crate::state::PROVISIONED_THROUGHPUT_MODE
            {
                false => ::std::collections::BTreeMap::new(),
                true => source_replica_specifications
                    .into_iter()
                    .filter(|(replica_region, _)| { ... })
```
with the stated reason at restore_table.rs:495-498: "kept only for a Region the target keyspace replicates to, and only while the restored table is provisioned, which is the only mode a per-Region read capacity or auto scaling setting belongs to".

The shared helper refuses that state as input — services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:2196-2207:
```
        let overrides_read_capacity = specification.read_capacity_units.is_some()
            || specification.read_capacity_auto_scaling.is_some();
        let throughput_mode = capacity.map(|capacity| capacity.throughput_mode.as_str());
        if overrides_read_capacity && throughput_mode != Some(PROVISIONED_THROUGHPUT_MODE) {
            return Err(format!(
                "replicaSpecifications entry for Region '{}' overrides the read capacity of a \
                 table in capacity mode '{PROVISIONED_THROUGHPUT_MODE}'; ...
```
and an existing test pins that refusal — tests/all/update_table_scaling_settings.rs:937-944:
```
        (
            "a per-Region read capacity override on a PAY_PER_REQUEST table",
            multi,
            on_demand_table,
            json!({"replicaSpecifications": [{"region": REGION, "readCapacityUnits": 5}]}),
            400,
            "ValidationException",
        ),
```
So the state is unreachable by request but reachable by a mode switch. The GetTable/GetTableAutoScalingSettings readers that surface it: get_table.rs:317-325 and get_table_auto_scaling_settings.rs:240-279 (`table.replica_auto_scaling(...)`, state.rs:1092-1121).

## Steps to Reproduce

1. CreateKeyspace with replicationSpecification.replicationStrategy = MULTI_REGION and a regionList naming the current Region plus one more.
2. CreateTable in that keyspace with capacitySpecification {throughputMode: PROVISIONED, readCapacityUnits: 10, writeCapacityUnits: 10} and replicaSpecifications [{region: <other region>, readCapacityUnits: 5, readCapacityAutoScaling: <enabled settings>}]; wait out the CREATING span.
3. UpdateTable with only capacitySpecification {throughputMode: PAY_PER_REQUEST}; wait out the UPDATING span.
4. GetTable: the replica entry reports throughputMode PAY_PER_REQUEST together with readCapacityUnits 5 — a combination UpdateTable refuses as input with 400 ValidationException (tests/all/update_table_scaling_settings.rs:937).
5. GetTableAutoScalingSettings: answers 200 with replicaSpecifications auto scaling settings, instead of the not-found answer the handler gives a table with no auto scaling settings, because has_replica_auto_scaling() is still true.

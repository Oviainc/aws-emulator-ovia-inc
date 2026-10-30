Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# Fold the duplicated "which Regions a MULTI_REGION keyspace reports" rule into one helper

_Filed: 2026-10-28T21:57:23Z_
_Operation: GetKeyspace | Class: maintainability_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Keyspace::reported_replication_regions` and `Keyspace::table_replica_regions` are the SAME rule and the SAME computation written twice: both test `self.replication_strategy == MULTI_REGION_STRATEGY` and, when it holds, hand back `self.replication_regions` (one as an owned `Vec<String>`, the other as a `&[String]` of the very same field). Nothing else distinguishes them — only the return type.

Because there are two copies, the Regions one report of a keyspace carries can drift from the Regions another report of the SAME keyspace carries. `GetKeyspace` demonstrates the exposure directly: its handler calls BOTH helpers on one borrowed keyspace in consecutive lines — `reported_replication_regions()` fills the response's `replicationRegions`, `table_replica_regions()` decides the `replicationGroupStatuses` entries — so an edit to the gate or to the list in either helper alone makes one `GetKeyspace` response contradict itself, and makes `GetTable`/`GetTableAutoScalingSettings` (which use the second copy) contradict `ListKeyspaces` (which uses the first).

The crate's own test suite states that this is ONE rule, not two, which is what makes the duplication a defect rather than a style choice.

The fix is a pure de-duplication: express one helper in terms of the other (e.g. `reported_replication_regions()` returning `self.table_replica_regions().map(<[String]>::to_vec)` — exactly the conversion `update_table.rs` and `restore_table.rs` already perform at their call sites). Every response stays byte-for-byte identical and every existing test keeps passing.

## Evidence

The two copies of the rule, 16 lines apart in one `impl Keyspace` block — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:878-881`:
```
    pub fn reported_replication_regions(&self) -> Option<Vec<String>> {
        (self.replication_strategy == MULTI_REGION_STRATEGY)
            .then(|| self.replication_regions.clone())
    }
```
and `src/state.rs:897-900`:
```
    pub fn table_replica_regions(&self) -> Option<&[String]> {
        (self.replication_strategy == MULTI_REGION_STRATEGY)
            .then_some(self.replication_regions.as_slice())
    }
```
Same predicate, same field, two bodies.

One handler calling both copies on the same keyspace — `src/ops/get_keyspace.rs:196-197`:
```
        let replication_regions = keyspace.reported_replication_regions();
        let replication_group_regions = keyspace.table_replica_regions();
```
consumed by one response, `src/ops/get_keyspace.rs:203-217`:
```
            replication_regions,
            // "This shows the summary status of the keyspace after a new Amazon Web
            // Services Region was added": one entry per Region the keyspace is replicated
            // in, carrying the keyspace's own status in that Region.
            replication_group_statuses: replication_group_regions.map(|regions| {
```

The crate already insists it is ONE rule — `src/ops/get_keyspace.rs:192-195`:
```
        // Which Regions a report of this keyspace carries, decided once on the stored
        // keyspace itself ([`crate::state::Keyspace::reported_replication_regions`] /
        // [`crate::state::Keyspace::table_replica_regions`]) rather than restated here, so
        // this report and every other one answer the same rule.
```

Test coverage that pins the behaviour the fix must not change (both suites keep passing, since no response value moves):
* `tests/all/reported_replication_regions.rs:3-15` — "Every reader of a keyspace reports the SAME `replicationRegions` ... It is one rule over the stored keyspace, so `GetKeyspace` and the `ListKeyspaces` summary of the same keyspace cannot answer it differently".
* `tests/all/multi_region_table_replicas.rs:9-17` — "A report of a table carries one Region-specific entry per Region the OWNING keyspace replicates the table to ... `GetTable$replicaSpecifications` ... `GetTableAutoScalingSettings$replicaSpecifications`".
* `tests/all/get_keyspace.rs` exercises `replicationRegions` and `replicationGroupStatuses` of the same `GetKeyspace` response together.

Call sites that already convert between the two shapes, showing the delegation is mechanical — `src/ops/update_table.rs:361-362`:
```
        let replica_regions: Option<Vec<String>> =
            keyspace.table_replica_regions().map(<[String]>::to_vec);
```

## Steps to Reproduce

1. Read services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:878-881 and :897-900 — the same `replication_strategy == MULTI_REGION_STRATEGY` gate over the same `replication_regions` field, implemented twice
2. Read src/ops/get_keyspace.rs:192-217 — one handler calls both copies on one keyspace and reports each in a different member of the same response
3. Change the gate or the list in ONE of the two helpers (for example, make it also report the request's own Region for a SINGLE_REGION keyspace): GetKeyspace's `replicationRegions` and `replicationGroupStatuses` now disagree with each other, and GetTable's `replicaSpecifications` disagrees with the ListKeyspaces summary — a drift no compiler or test catches until it is observed on the wire
4. Confirm the fix is response-preserving: `reported_replication_regions()` can be `self.table_replica_regions().map(<[String]>::to_vec)`, the exact conversion src/ops/update_table.rs:361-362 and src/ops/restore_table.rs:471-473 already do at their call sites
5. Run tests/all/reported_replication_regions.rs, tests/all/multi_region_table_replicas.rs and tests/all/get_keyspace.rs — all still pass, with no response value changed

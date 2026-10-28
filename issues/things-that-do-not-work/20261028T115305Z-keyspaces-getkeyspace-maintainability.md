Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# GetKeyspace re-implements the MULTI_REGION gate on replicationRegions instead of calling Keyspace::reported_replication_regions

_Filed: 2026-10-28T11:53:05Z_
_Operation: GetKeyspace | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "report `replicationRegions` only when the keyspace's `replicationStrategy` is `MULTI_REGION`, and report the stored region list unchanged when it is" is implemented twice in this crate.

Once as a method on the stored keyspace, `crate::state::Keyspace::reported_replication_regions`, which `ListKeyspaces` calls to fill `KeyspaceSummary$replicationRegions`; and once inline in the `GetKeyspace` handler, which computes the same `replication_strategy == MULTI_REGION_STRATEGY` test itself and applies it with the same `.then(|| keyspace.replication_regions.clone())`. The two expressions are character-for-character the same computation over the same two fields of the same `Keyspace` record, so the copies can drift: a change to how a keyspace reports its replication group (what the list holds, which strategies report one, whether the list is de-duplicated or re-ordered on the way out) has to be made in both places, and only one of them carries the doc sentence that justifies the gate.

The fix is confined to `get_keyspace.rs`: fill `replication_regions` from `keyspace.reported_replication_regions()` and keep the local `multi_region` flag (or `keyspace.table_replica_regions().is_some()`) for the `replicationGroupStatuses` branch that still needs it. The helper returns exactly `Option<Vec<String>>` with exactly the same value, so every response byte and every existing test stays unchanged.

## Evidence

Copy 1 — the helper, `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:871-881`:
```
    /// The `replicationRegions` a report of this keyspace carries: the stored region
    /// list for a `MULTI_REGION` keyspace, and nothing for any other strategy.
    ///
    /// "If the `replicationStrategy` of the keyspace is `MULTI_REGION`, a list of
    /// replication Regions is returned" (`KeyspaceSummary$replicationRegions`), and the
    /// member's modelled minimum length is 2 ...
    pub fn reported_replication_regions(&self) -> Option<Vec<String>> {
        (self.replication_strategy == MULTI_REGION_STRATEGY)
            .then(|| self.replication_regions.clone())
    }
```
and its only caller, `src/ops/list_keyspaces.rs:244`:
```
                    replication_regions: keyspace.reported_replication_regions(),
```

Copy 2 — the same rule written out again in `src/ops/get_keyspace.rs:197` and `:203`:
```
        let multi_region = keyspace.replication_strategy == crate::state::MULTI_REGION_STRATEGY;
...
            replication_regions: multi_region.then(|| keyspace.replication_regions.clone()),
```
with the same justification restated above it, `src/ops/get_keyspace.rs:192-196`: "If the replicationStrategy of the keyspace is MULTI_REGION, a list of replication Regions is returned" — and only then ...".

Precedent in this crate for hoisting exactly this kind of shared rule rather than keeping a copy per handler, `src/state.rs:398-399` / `:413`: "One page of a listing, and whether a further item follows it — the page window every `@paginated` operation of this service reports through" / `pub fn page_window<Item>(`.

Tests that pin both sides and must stay unchanged: `tests/all/get_keyspace.rs:11` ("`replicationRegions` and `replicationGroupStatuses` accompany a `MULTI_REGION`") with the assertions at `tests/all/get_keyspace.rs:178-191`, and `tests/all/list_keyspaces.rs:11-12` ("`replicationRegions` only where the strategy is `MULTI_REGION`") with `tests/all/list_keyspaces.rs:116` and `:230-250`; also `tests/all/single_region_replication.rs:20`, `tests/all/replication_region_set.rs:297`, `tests/all/service_managed_catalog.rs:149-151`.

## Steps to Reproduce

1. Read services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:871-881 — `Keyspace::reported_replication_regions` gates the stored region list on `replication_strategy == MULTI_REGION_STRATEGY`
2. Read src/ops/list_keyspaces.rs:244 — ListKeyspaces fills `replicationRegions` through that helper
3. Read src/ops/get_keyspace.rs:197 and :203 — GetKeyspace recomputes the identical strategy test and the identical `.then(|| keyspace.replication_regions.clone())`, bypassing the helper
4. Note that a change to either copy (which strategies report a list, or what the list contains) leaves the other answering the old rule, while the fix — calling the helper from get_keyspace.rs — leaves every response and every test byte-identical

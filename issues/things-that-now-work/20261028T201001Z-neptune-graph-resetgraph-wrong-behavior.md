Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: db5b8cf4b40010fa5fa0137914026d79724270f1

# Final-snapshot naming skips the crate's own reserved-prefix check, so a graph named gs-* gets a snapshotName the modelled SnapshotName pattern forbids

_Filed: 2026-10-28T20:10:01Z_
_Operation: ResetGraph | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `db5b8cf4b40010fa5fa0137914026d79724270f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`skipSnapshot=false` on `ResetGraph` (and on `DeleteGraph`, which reaches the same helper) names the final snapshot `{graphName}-final-snapshot` via `crate::state::free_final_snapshot_name`. That helper's own doc-comment claims the name it returns satisfies the modelled `SnapshotName` constraints "(`@length(1..63)`, pattern `^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`)", but the code only checks the LENGTH and that the name is free in the Region — it never applies the reserved-prefix half of that pattern.

A `graphName` may legitimately begin with `gs`: the modelled `GraphName` pattern forbids only the `g-` prefix, and the crate's own `CreateGraph` test table pins `("g0-x", true)`. So `graphName = "gs-data"` is accepted, and resetting (or deleting) that graph with `skipSnapshot=false` stores a snapshot whose `name` is `gs-data-final-snapshot` — a value starting with the `gs-` prefix the model reserves for snapshot IDs.

Two consequences inside this one crate:

1. `GetGraphSnapshot`, `DeleteGraphSnapshot` and `ListGraphSnapshots` then report that stored name in their REQUIRED `name` member, which targets `SnapshotName` with pattern `^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$` — the emulator emits an output value its own model says cannot exist.
2. The service refuses the very name it minted: `CreateGraphSnapshot` runs `crate::state::resource_name_is_well_formed(&input.snapshot_name, "gs-")` and answers 400 `ValidationException` for `gs-data-final-snapshot`, so one path accepts a name the sibling path on the same namespace rejects.

The prefix check already exists in the same module (`resource_name_is_well_formed`, whose doc says it is "where that constraint is enforced for every operation that accepts a name") and is reached by `CreateGraph`, `CreateGraphSnapshot`, `CreateGraphUsingImportTask` and `RestoreGraphFromSnapshot` — the final-snapshot path is the one name-minting site that skips it.

## Evidence

The helper claims the constraint and does not check it — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs:1456-1477`:
```
    /// A free `snapshotName` for the FINAL snapshot of the graph named `graph_name`
    /// IN `region`, satisfying the modelled `SnapshotName` constraints
    /// (`@length(1..63)`, pattern `^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`).
...
        let after_the_graph = format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}");
        if after_the_graph.len() <= MAX_RESOURCE_NAME_CHARS
            && self.snapshot_id_named(&after_the_graph, region).is_none()
        {
            return after_the_graph;
        }
```
(only length + uniqueness; `FINAL_SNAPSHOT_SUFFIX = "-final-snapshot"`, `src/state.rs:2138`).

The name is stored unchecked by the shared final-snapshot rule — `src/state.rs:1535-1540`:
```
        let id = self.next_snapshot_id();
        let name = self.free_final_snapshot_name(&graph.name, &id, region);
        self.insert_snapshot(Snapshot {
...
            name,
```
reached by `ResetGraph` (`src/ops/reset_graph.rs:219-228`: `if !input.skip_snapshot { store.mint_final_snapshot(&graph, &region, now) ... }`) and by `DeleteGraph` (`src/ops/delete_graph.rs:226-235`).

The crate HAS the check and the sibling path uses it — `src/ops/create_graph_snapshot.rs:176` and `:343`:
```
        if !crate::state::resource_name_is_well_formed(&input.snapshot_name, SNAPSHOT_ID_PREFIX) {
...
const SNAPSHOT_ID_PREFIX: &str = "gs-";
```
and `src/state.rs:2524-2528`:
```
/// treated as matching — so this is where that constraint is enforced for every
/// operation that accepts a name.
pub fn resource_name_is_well_formed(name: &str, forbidden_prefix: &str) -> bool {
    if name.starts_with(forbidden_prefix) {
        return false;
```

A `gs`-prefixed graphName is accepted: `GraphName`'s pattern forbids only `g-` (`smithy_query(query="operation",operation="ResetGraph")` -> output `name`: pattern `^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`), and the crate pins that a name may start with `g` followed by a non-hyphen — `src/ops/create_graph.rs:368`: `("g0-x", true)`.

The model forbids the minted value on the read side: `smithy_query(query="shape",shape="GraphSnapshotSummary")` -> `"name": {"target": "SnapshotName", "required": true, "length": {"min": 1, "max": 63}, "pattern": "^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$"}`.

Test coverage: `tests/all/final_snapshot_rule.rs:562-566` asserts the snapshot is `json!(format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}"))`, but every row's `stem` is a plain name (`"empty"`, `"one-node"`, `"three-nodes"`, `"encrypted"` — `tests/all/final_snapshot_rule.rs:405-449`), so no test exercises a `gs`-prefixed graph name and nothing in the crate justifies the divergence.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName="gs-data", provisionedMemory=16, deletionProtection=false) — accepted, because only the `g-` prefix is forbidden for a graphName.
3. Poll GetGraph until status is AVAILABLE.
4. ResetGraph(graphIdentifier=<id>, skipSnapshot=false) (or DeleteGraph(graphIdentifier=<id>, skipSnapshot=false)).
5. ListGraphSnapshots() — observe a snapshot whose required `name` is "gs-data-final-snapshot", which violates the modelled SnapshotName pattern `^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`; GetGraphSnapshot on its id reports the same name.
6. CreateGraphSnapshot(graphIdentifier=<any available graph>, snapshotName="gs-data-final-snapshot") — observe HTTP 400 ValidationException "Member must satisfy regular expression pattern: (?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*": the service refuses the exact name it minted itself.

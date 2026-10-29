Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 08a55bcc278a7e8a745e318c09a635676933476e

# A STOPPED graph cannot be snapshotted by CreateGraphSnapshot but IS snapshotted by DeleteGraph's final snapshot, and the handler comment asserting the two gates agree is false

_Filed: 2026-10-29T03:56:39Z_
_Operation: CreateGraphSnapshot | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `08a55bcc278a7e8a745e318c09a635676933476e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateGraphSnapshot` refuses any source graph that is not `AVAILABLE`, and its handler comment justifies that gate by claiming it is "the same precondition `UpdateGraph` and `DeleteGraph` state". `UpdateGraph` does state it (`graph.status != GraphStatus::Available`), but `DeleteGraph` does not: it gates on `GraphStatus::is_at_rest()`, which `crate::state` defines as `AVAILABLE | STOPPED`.

The disagreement is observable, not just a stale comment. `DeleteGraph` with the required `skipSnapshot: false` on a `STOPPED` graph passes its gate and then calls `crate::state::mint_final_snapshot`, which captures that stopped graph's data and configuration into a real, restorable snapshot record. So this crate both does and does not permit a snapshot of a `STOPPED` graph, depending on which operation asks:

* `CreateGraphSnapshot` on a `STOPPED` graph -> 409 ConflictException, no snapshot.
* `DeleteGraph(skipSnapshot=false)` on the same `STOPPED` graph -> 200, and a snapshot of that stopped graph is persisted and later restorable via `RestoreGraphFromSnapshot`.

`mint_final_snapshot`'s own doc insists it is "ONE rule, stated once" that "Every operation that empties or removes a graph's data takes the same snapshot", so the snapshot DeleteGraph takes of a stopped graph is by construction the same artefact `CreateGraphSnapshot` declines to produce. One of the two gates is wrong; the crate contains both and asserts they are the same.

The practical effect is that the only way a caller can obtain a snapshot of a stopped graph through this emulator is to delete the graph, which is the opposite of what a caller stopping a graph to snapshot it cheaply would do.

## Evidence

`CreateGraphSnapshot`'s gate and the claim about it — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/create_graph_snapshot.rs:280-294`:
```
        // Only a serving graph can be snapshotted: while an earlier transition is
        // still outstanding the graph's data is in flight, which is the same
        // precondition `UpdateGraph` and `DeleteGraph` state. The wording is the
        // service's one precondition refusal
        // (`crate::state::graph_status_precondition_conflict`), which fronts no
        // identifier: the graph was addressed by one the caller sent.
        if source_graph_status != crate::state::GraphStatus::Available {
            return Err(CreateGraphSnapshotErrorModeled::ConflictException(
                crate::state::graph_status_precondition_conflict(
                    "create the graph snapshot",
                    source_graph_status,
                ),
            )
            .into());
        }
```

What `DeleteGraph` actually states — `src/ops/delete_graph.rs:209-235`:
```
        // A graph whose status change is still in flight cannot be deleted yet — which
        // includes a graph already on its way out, whose deletion is in flight. One at
        // REST can, whether it rests serving (`AVAILABLE`) or stopped (`STOPPED`): a
        // stopped graph is a graph that still exists, and deletion is how it goes away.
        if !graph_status.is_at_rest() {
...
        if !input.skip_snapshot {
            store
                .mint_final_snapshot(&graph, &region, now)
```
and `is_at_rest` admits STOPPED — `src/state.rs:179-181`:
```
    pub const fn is_at_rest(self) -> bool {
        matches!(self, Self::Available | Self::Stopped)
    }
```

The snapshot DeleteGraph then takes is the same artefact, by the helper's own doc — `src/state.rs:1496-1501`:
```
    /// ONE rule, stated once: a final snapshot captures the graph AS THE CALL FOUND IT
    /// — the data it holds and the configuration it is provisioned with, under the
    /// graph's own encryption key — and is not restorable until the clock makes its
    /// move to `AVAILABLE` due, exactly as an explicitly named snapshot is not. Every
    /// operation that empties or removes a graph's data takes the same snapshot, so
    /// neither what it captures, nor the span it settles over, nor the wording of the
```

Test coverage: `tests/all/graph_snapshot_lifecycle.rs` and `tests/all/graph_state_gate_messages.rs` cover CreateGraphSnapshot's refusals, and `tests/all/final_snapshot_rule.rs` covers the DeleteGraph/ResetGraph final snapshot — no test takes a final snapshot of a STOPPED graph, so nothing pins the asymmetry either way.

## Steps to Reproduce

1. Create a graph (POST /graphs, graphName `stopped-snap`, provisionedMemory 16) and wait out PROVISIONING_SPAN so GetGraph reports AVAILABLE.
2. POST /graphs/{id}/stop (StopGraph), then wait out PROVISIONING_SPAN so GetGraph reports STOPPED.
3. POST /snapshots (CreateGraphSnapshot) with that graphIdentifier and snapshotName `snap-of-stopped` — observe HTTP 409 ConflictException "...because the graph is in the STOPPED state."
4. DELETE /graphs/{id} (DeleteGraph) with `skipSnapshot: false` on the same STOPPED graph — observe HTTP 200, and GET /snapshots (ListGraphSnapshots) now reports a snapshot whose `sourceGraphId` is that graph: the crate just produced the snapshot of a STOPPED graph that step 3 refused.
5. Note that `create_graph_snapshot.rs:280-285` asserts its gate is "the same precondition ... `DeleteGraph` state[s]", which step 3 versus step 4 disproves.

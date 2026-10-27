Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: d195aa8999564fdeb7cf069de4924bbc75898d97

# Factor the one "skipSnapshot=false takes a final snapshot of the graph as found" rule the two neptune-graph handlers implement separately into a shared store helper

_Filed: 2026-10-26T09:00:00Z_
_Operation: ResetGraph | Class: maintainability_

> _Note: this issue was identified against the code base at commit `d195aa8999564fdeb7cf069de4924bbc75898d97`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two operations of `services/service-com-amazonaws-neptunegraph-amazonneptunegraph` implement the
SAME rule — a request whose required `skipSnapshot` is `false` takes a FINAL snapshot of the graph
as the call found it, before the data goes — and each implements it with its own copy of the same
span:

* `src/ops/delete_graph.rs` (the final snapshot a delete takes before the graph is torn down)
* `src/ops/reset_graph.rs` (the final snapshot a reset takes before the data is emptied)

Each copy is the same snapshot-store ceiling check against `crate::state::MAX_SNAPSHOTS` with the
same `ConflictException` sentence written out as a literal in both files, the same
`next_snapshot_id()` / `free_final_snapshot_name(graph_name, &id, &region)` naming pair, and the
same whole `crate::state::Snapshot { .. }` literal — the `graph-snapshot/<id>` ARN, the Region, the
source graph id, `SnapshotStatus::Creating` with `settles_at: now + PROVISIONING_SPAN`, the source
graph's `kmsKeyIdentifier`, an empty tag map, the captured `GraphConfiguration` and the captured
`GraphData` — followed by `insert_snapshot`. The duplicated span is policy, not wiring: a change to
what a final snapshot captures, to the span it settles over, or to the wording of the no-room
refusal has to be made in both files, and a copy that is missed diverges silently because each
handler has its own test.

`crate::state` is already the shared home for exactly this kind of decision — `free_final_snapshot_name`
(the naming half of this very rule), `graph_name_conflict`/`snapshot_name_conflict`, `graph_not_found`
and the `MAX_*` ceilings all live there, each so that no two operations word or bound one thing
differently. The rule both handlers implement is one rule, so one store helper — e.g.
`ComAmazonawsNeptunegraphAmazonneptunegraphState::mint_final_snapshot(&graph, &region, now) ->
Result<String, String>`, returning the minted snapshot id or the reason there was no room — can
serve both, leaving each handler only its own refusal wording.

The refactor changes NO response: both handlers already mint the same record for the same graph,
and the no-room condition is already reported as the same modelled `ConflictException` by each. It
must not touch any existing test, and should add one test that drives the shared path through an
operation from each former copy (a `DeleteGraph` with `skipSnapshot=false` and a `ResetGraph` with
`skipSnapshot=false`, each read back through `GetGraphSnapshot`).

## Evidence

* `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/delete_graph.rs` — the
  `MAX_SNAPSHOTS` conflict, `next_snapshot_id`, `free_final_snapshot_name`, the `Snapshot { .. }`
  literal and `insert_snapshot`.
* `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/reset_graph.rs` — the
  second copy of the same span, comments included.
* `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs` —
  `free_final_snapshot_name`, `FINAL_SNAPSHOT_SUFFIX`, `MAX_SNAPSHOTS`, `PROVISIONING_SPAN`,
  `GraphConfiguration`: the shared home this span belongs in, and the precedent for putting one
  decision there once.

## Steps to Reproduce

1. Open `src/ops/delete_graph.rs` and `src/ops/reset_graph.rs` side by side at their
   `if input.skip_snapshot { None } else { .. }` blocks: the ceiling check, its message, the id and
   name minting, the record literal and the insert are the same code in each.
2. Change what a final snapshot captures (add a member to `crate::state::Snapshot`, or move the
   span it settles over) and note that both handlers must be edited for the two final-snapshot
   paths to stay in step.
3. Note that `crate::state` already owns the naming half of this rule
   (`free_final_snapshot_name`) and the ceiling it is bounded by (`MAX_SNAPSHOTS`), so the shared
   home for the rest of it exists.

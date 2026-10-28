Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# StartImportTask accepts an import into a non-empty graph, and justifies it with a claim the crate contradicts

_Filed: 2026-10-28T02:35:37Z_
_Operation: StartImportTask | Class: validation_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy documentation for `StartImportTask` states two preconditions on the target graph: it must be AVAILABLE *and* it must be empty. The handler enforces only the status half and deliberately skips the emptiness half, justifying the skip with a factual claim about this crate that the crate itself contradicts: the comment says "this emulator has no graph data plane, so a stored graph holds no data for an import to collide with", but a `Graph` record in this very crate carries a `data: GraphData` field holding nodes and edges, `ResetGraph` exists solely to empty it, `GetGraphSummary` computes its whole report from it, and `RestoreGraphFromSnapshot` hands a freshly minted graph the nodes and edges a snapshot captured.

Consequence: a graph restored from a snapshot that captured data comes up AVAILABLE holding nodes and edges, and `StartImportTask` against it is accepted (201, a task record minted, the graph driven to IMPORTING) even though the model says the graph "needs to be empty". No branch, no helper and no refusal exists for the documented emptiness precondition, while the sibling precondition from the same sentence is checked three lines above it.

## Evidence

Smithy model — the sentence that states BOTH preconditions, verbatim from `smithy_query(query="doc",shape="StartImportTask")`:

    "Import data into existing Neptune Analytics graph from Amazon Simple Storage Service (S3). The graph needs to be empty and in the AVAILABLE state."

The handler checks the status half and explicitly declines the emptiness half — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/start_import_task.rs:299-315:

    // "The graph needs to be empty and in the AVAILABLE state": while an earlier
    // transition is still outstanding the graph's data is in flight, which is the
    // same precondition `UpdateGraph` and `CreateGraphSnapshot` state. Emptiness is
    // not checked here — this emulator has no graph data plane, so a stored graph
    // holds no data for an import to collide with.
    if graph_status != crate::state::GraphStatus::Available {
        return Err(StartImportTaskErrorModeled::ConflictException(
            ConflictException {
                message: format!(
                    "graph {graph_id} is in status {} and cannot accept an import task",

The crate DOES hold a graph data plane, so the stated reason is false:

- src/state.rs:421-424 — `Graph` carries the data:

      /// The nodes and edges this graph holds: the data every reading of the graph's
      /// contents (a summary of it, a query over it, an export of it) is computed
      /// from. A freshly created graph holds none.
      pub data: GraphData,

- src/state.rs:286 and src/state.rs:313 — the writers: `pub fn insert_node(...)`, `pub fn insert_edge(...)`.
- src/ops/reset_graph.rs:237-240 — an operation whose whole purpose is emptying that data:

      // The data this call empties is gone once it returns: the graph keeps its
      // identity, configuration, tags and encryption key, and only the nodes and edges
      // it held go, with the statistics over the emptied data recomputed as of now.
      resetting.data.clear(::std::time::SystemTime::now());

- src/ops/restore_graph_from_snapshot.rs:377-379 — a NON-EMPTY graph reaches AVAILABLE through the public API:

      // The restored graph holds the data the snapshot captured; the statistics
      // over it are the ones this graph came up with.
      data: captured_data.into_as_of(create_time),

- src/ops/get_graph_summary.rs:272-274 — the data is read back and reported:

      num_nodes: Some(count(data.nodes.len())),
      num_edges: Some(count(data.edges.len())),

Test coverage of the surrounding behaviour that must stay green: tests/all/import_task_lifecycle.rs:499-500 (the status/conflict gate rows), tests/all/graph_reset.rs:248-264 (seeds nodes and edges into a stored graph through `GraphData::insert_node`/`insert_edge`, proving a stored graph can hold data), tests/all/graph_restore_from_snapshot.rs (restore carries the captured data). No test pins "an import into a non-empty graph is accepted".

## Steps to Reproduce

1. Read the model sentence: smithy_query(query="doc",shape="StartImportTask") -> "The graph needs to be empty and in the AVAILABLE state."
2. Open services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/start_import_task.rs and read lines 299-315: the AVAILABLE half of that sentence is enforced; the empty half is skipped with the comment "this emulator has no graph data plane, so a stored graph holds no data for an import to collide with".
3. Grep the same crate for the data plane the comment says does not exist: `grep -n 'pub data: GraphData\|insert_node\|insert_edge\|data.clear' src/state.rs src/ops/*.rs` -> state.rs:286, state.rs:313, state.rs:424, ops/reset_graph.rs:240, ops/restore_graph_from_snapshot.rs:379.
4. Create a graph, seed nodes/edges into its `GraphData` (as tests/all/graph_reset.rs:248-264 does), CreateGraphSnapshot of it, then RestoreGraphFromSnapshot — the new graph settles AVAILABLE holding those nodes and edges (GetGraphSummary reports numNodes > 0).
5. Call StartImportTask against that restored, non-empty graph: it answers 201 and mints an import task, with no refusal for the documented emptiness precondition.

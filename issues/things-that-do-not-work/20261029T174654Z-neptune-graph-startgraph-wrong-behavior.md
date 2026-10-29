Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# Thirty handlers sample the clock BEFORE taking the store lock, so the settle/reap decisions are taken against an instant that can predate the lock — the three tagging handlers of the same crate sample it under the lock

_Filed: 2026-10-29T17:46:54Z_
_Operation: StartGraph | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every lifecycle decision in this crate is taken by comparing a handler-supplied `now` against a stored due instant (`Graph::settle`, `reap_deleted_graphs`, `materialise_snapshots`, `materialise_private_endpoints`, `reap_terminated_queries`, `ExportTask::settle`, `ImportTask::settle`). Thirty of the thirty-three implemented handlers read that instant with `Instant::now()` BEFORE they contend for the store lock, so the instant a handler settles and reaps with is the instant it STARTED waiting, not the instant it holds the store. Three handlers in the same crate — `TagResource`, `UntagResource`, `ListTagsForResource` — do it the other way round: they lock first and sample the clock afterwards, immediately before the reap they need it for.

While a handler waits for the lock, a transition the clock has made due is therefore invisible to it, and the handler then reads and WRITES the store as the world stood before the wait:

* `StartGraph`: a graph whose `STOPPING -> STOPPED` move fell due during the wait is not settled by `settled_graph_mut(.., stale_now)`, so the handler refuses the start with `ConflictException` "Unable to start the graph because it is in the STOPPING state" — contradicting its own comment two lines below the sample, which says the lookup settles first "so a graph whose stop has come due is startable".
* The same stale instant is handed to `reap_deleted_graphs`, so a graph whose `DELETION_SPAN` fell due during the wait is still found: `UpdateGraph` mutates a record that the next read no longer holds (a lost write), `ResetGraph`/`DeleteGraph` can mint a final snapshot of a graph that is already gone, and `CreateGraph`/`RestoreGraphFromSnapshot`/`CreateGraphUsingImportTask` still refuse its `graphName` as taken.
* In the other direction it turns an error into a success: `GetQuery`/`CancelQuery` pass the stale instant to `live_query`/`live_query_mut`, which reap on `has_terminated(now)`, so a query whose `QUERY_RUN_SPAN` or accepted cancellation ran out during the wait is still reported 200 and still accepts a second cancellation.

The fix is the ordering the three tagging handlers already use: take the lock, then sample the clock.

## Evidence

The thirty-handler pattern, e.g. services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/start_graph.rs:176-182 — the clock is read at 177, the lock only at 178, and the comment under it states the rule the stale sample breaks:
```
        let region = crate::state::request_region(ctx);
        let now = ::std::time::Instant::now();
        let mut store = state.lock();
        // The lookup settles the transitions the clock has already made due first, so a
        // graph whose stop has come due is startable. A graph lives in exactly one
        // Region, so one of another Region is not found here.
        let Some(graph) = store.settled_graph_mut(&input.graph_identifier, &region, now) else {
```

The counter-example in the SAME crate — src/ops/tag_resource.rs:165-173 locks first and samples the clock under the lock, immediately before the reap it is needed for:
```
        let region = crate::state::request_region(ctx);
        let mut store = state.lock();
        // A graph or snapshot whose deletion has already fallen due is no longer a
        // resource this service holds, so its record is reclaimed before the ARN is
        // resolved: tagging it answers the same 404 as tagging an ARN that never named
        // anything.
        let now = ::std::time::Instant::now();
        store.reap_deleted_graphs(now);
        store.materialise_snapshots(now);
```
src/ops/untag_resource.rs:146 (`let mut store = state.lock();`) then :151 (`let now = ::std::time::Instant::now();`) and src/ops/list_tags_for_resource.rs:149 then :156 do the same.

The clock-before-lock order in the other handlers, by file:line of `let now = ...` / `let mut store = state.lock();`: start_graph.rs:177/178, stop_graph.rs:176/177, update_graph.rs:183/184, delete_graph.rs:181/182, reset_graph.rs:181/182, get_graph.rs:171/172, create_graph.rs:238/239, create_graph_snapshot.rs:214/215, restore_graph_from_snapshot.rs:236/237, create_graph_using_import_task.rs:324/325, start_import_task.rs:270/271, start_export_task.rs:255/256, cancel_import_task.rs:178/179, cancel_export_task.rs:180/181, get_query.rs:165/166, cancel_query.rs:181/182, list_queries.rs:188/189, get_graph_summary.rs:169/170, plus the remaining reads and listings.

What the stale instant is used for — src/state.rs:1871 (`settled_graph_mut`): `self.reap_deleted_graphs(now);` then `graph.settle(now);`, src/state.rs:1783 (`live_query`): `self.reap_terminated_queries(now);`, and src/state.rs:1844-1853 (`reap_deleted_graphs`) filtering on `graph.deletion_complete(now)` — whose own doc at src/state.rs:1838-1841 says "each of those graphs was gone at its `deletes_at` instant whether or not anybody called", which a pre-lock sample cannot honour.

Test coverage: the lifecycle tests drive one request at a time (tests/all/graph_run_state.rs, tests/all/graph_lifecycle.rs, tests/all/query_cancellation.rs), so no test contends for the lock and nothing pins the pre-lock sample.

## Steps to Reproduce

1. Boot the emulator from this checkout and create a graph (CreateGraph, provisionedMemory 16), wait out PROVISIONING_SPAN, then StopGraph it.
2. Issue two concurrent requests so that one holds the store lock while the other is inside `handle` but past its `Instant::now()` — e.g. a GetGraphSummary on a graph holding data (which walks every node, edge and property under the lock before dropping it at get_graph_summary.rs:206) concurrently with StartGraph on the stopped graph, timed so the StartGraph sample lands just before the STOPPING->STOPPED due instant and the lock is granted just after it.
3. Observe StartGraph answer 409 ConflictException "Unable to start the graph because it is in the STOPPING state" although the record it wrote under was due to be STOPPED at the instant it held the lock — the state its own comment at start_graph.rs:179-181 says the lookup settles first.
4. Repeat the same shape with DeleteGraph (skipSnapshot=true) on a graph whose DELETION_SPAN falls due in the window, followed by CreateGraph with that graph's name: the name is still refused as a conflict by graph_id_named because reap_deleted_graphs ran on the pre-lock instant.
5. Compare with TagResource/UntagResource/ListTagsForResource, which sample the clock after `state.lock()` (tag_resource.rs:166/171) and therefore reap against the instant they actually hold the store.

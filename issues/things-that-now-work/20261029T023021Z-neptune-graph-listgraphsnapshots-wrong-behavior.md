Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e

# A snapshot (or task) outlives the graph it came from, but listing it under that graph answers 404 — the crate's own "stays listed ... even once that graph is gone" rule is unreachable

_Filed: 2026-10-29T02:30:21Z_
_Operation: ListGraphSnapshots | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListGraphSnapshots` resolves an optional `graphIdentifier` by looking the GRAPH up in the store and answering `ResourceNotFoundException` when no graph of this Region holds that id. Because `DeleteGraph` reaps the graph record once `DELETION_SPAN` falls due, that 404 fires for every graph the caller has deleted — so the only way to list a graph's snapshots stops working exactly when the snapshots are all that is left of it.

This contradicts the rule the state module states twice about the very helper the handler calls. `snapshots_in`'s own doc says a snapshot "stays listed under the graph it was taken from even once that graph is gone", and `Snapshot::source_graph_id` is deliberately stored as a copy of the id "so deleting that graph leaves the snapshot's provenance intact". The filter inside `snapshots_in` honours both — it matches `of_graph` against the stored `source_graph_id` and never consults the `graphs` map — but the handler's own lookup refuses the request before that filter can ever be exercised with a gone graph. The documented capability is dead code.

The case is not hypothetical: `DeleteGraph` with the required `skipSnapshot=false` MINTS a final snapshot of the graph through `mint_final_snapshot` and then begins the graph's deletion in the same handler. The snapshot is named after the graph and carries `source_graph_id` pointing at it, yet once the deletion completes `ListGraphSnapshots?graphIdentifier=<that graph>` answers 404 instead of listing the final snapshot the delete just took.

The same single root cause is copied into the two sibling task listings, whose helpers carry the identical doc sentence, so all three should be reconciled together:
* `ListImportTasks` — `import_tasks_in` doc: "so a task stays listed under the graph it loaded into even once that graph is gone"; `ImportTask::graph_id` doc: "deleting that graph leaves the task's provenance intact". `remove_graph`'s own doc also states "An import task is NOT dropped with its graph: it is read by its own id and keeps the provenance of the load it describes" — yet `ListImportTasks?graphIdentifier=<deleted graph>` is refused 404.
* `ListExportTasks` — `export_tasks_in` doc: "so a task stays listed under the graph it read even once that graph is gone"; `ExportTask::source_graph_id` doc: "kept as that graph's id (not a borrow) so deleting the graph leaves the task's provenance intact".

Note that `GetGraphSnapshot`, `GetImportTask` and `GetExportTask` all read these records back fine after the graph is gone — they resolve the record by its own id and never touch the `graphs` map — so the per-graph listings are the one surface that loses them.

## Evidence

The rule the state module states about the helper this handler calls — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs:1909-1913`:
```
    /// The same shape as [`Self::graphs_in`], for the same reason: `snapshots` is a
    /// `BTreeMap`, so one stable total order underlies every page and a token can name
    /// a position in it. `of_graph` is matched against the snapshot's stored
    /// `source_graph_id`, so a snapshot stays listed under the graph it was taken from
    /// even once that graph is gone.
```
and the field it relies on — `src/state.rs:574-575`:
```
    /// The graph the snapshot was taken from. Kept as the graph's id (not a
    /// borrow), so deleting that graph leaves the snapshot's provenance intact.
```
The filter itself never consults the `graphs` map — `src/state.rs:1924-1930`:
```
        self.snapshots
            .range((lower, ::std::ops::Bound::Unbounded))
            .map(|(_, snapshot)| snapshot)
            .filter(move |snapshot| {
                snapshot.is_in(region)
                    && of_graph.is_none_or(|graph_id| snapshot.source_graph_id == graph_id)
            })
```

The handler makes that unreachable — `src/ops/list_graph_snapshots.rs:190-205`:
```
        // `graphIdentifier` is optional: absent, the listing spans every snapshot of
        // the Region; present, it has to name a graph of this Region, because a listing
        // of the snapshots of a graph that does not exist is a failed lookup rather
        // than an empty page.
        let of_graph = match input.graph_identifier.as_deref() {
            Some(identifier) => {
                let Some(graph) = store.settled_graph(identifier, &region, now) else {
                    return Err(ListGraphSnapshotsErrorModeled::ResourceNotFoundException(
                        crate::state::graph_not_found(identifier),
                    )
                    .into());
                };
                Some(graph.id.clone())
            }
            None => None,
        };
```
and the graph record really is gone once the clock makes the deletion due — `src/state.rs:1835-1845` (`reap_deleted_graphs` -> `remove_graph`), reached from `settled_graph_mut` at `src/state.rs:1862`.

The workflow that creates the orphan is in the delete handler itself — `src/ops/delete_graph.rs:226-248`:
```
        if !input.skip_snapshot {
            store
                .mint_final_snapshot(&graph, &region, now)
...
        deleting.begin_deletion(now);
```
with `mint_final_snapshot` naming the snapshot after the graph and stamping `source_graph_id: graph.id.clone()` — `src/state.rs:1536` and `src/state.rs:1544`.

The two siblings carry the same contradiction:
* `src/state.rs:2004-2006` — "`of_graph` is matched against the task's stored `graph_id`, so a task stays listed under the graph it loaded into even once that graph is gone." vs `src/ops/list_import_tasks.rs:193-204` (the same 404 block).
* `src/state.rs:1434-1436` — "`of_graph` is matched against the task's stored `source_graph_id`, so a task stays listed under the graph it read even once that graph is gone." vs `src/ops/list_export_tasks.rs:182-193` (the same 404 block).
* `src/state.rs:1577-1578` — "An import task is NOT dropped with its graph: it is read by its own id and keeps the provenance of the load it describes."

Test coverage: `tests/all/graph_snapshot_listing.rs:473-505` pins the 404 only for ids that were NEVER created ("a well-formed graph id that was never created", "another well-formed absent graph id"), with the file's stated reason at `tests/all/graph_snapshot_listing.rs:13` ("graph of this Region is a failed lookup (404) rather than an empty page"). No test drives the listing against a graph that WAS created and then deleted, so nothing pins the current behaviour for the case the state module documents. `tests/all/import_task_listing.rs:13`, `tests/all/export_task_listing.rs:13` and their 404 tables (`:690`, `:622`) are the same shape. `tests/all/final_snapshot_rule.rs:24-28` reads the minted final snapshot's id straight out of the store rather than through the listing (its stated reason there — "`ListGraphSnapshots` is a scaffold stub too" — is itself stale, the handler is implemented).

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName="doomed-graph", provisionedMemory=16); poll GetGraph until status is AVAILABLE.
3. GET /snapshots?graphIdentifier=<graph id> — observe HTTP 200 with an empty `graphSnapshots` list (the listing works while the graph exists).
4. DELETE /graphs/<graph id>?skipSnapshot=false — observe HTTP 200 with status DELETING; a final snapshot named "doomed-graph-final-snapshot" has been minted and carries sourceGraphId=<graph id>.
5. Wait out DELETION_SPAN (500ms) and confirm GET /graphs/<graph id> now answers 404, i.e. the graph record is reaped.
6. GET /snapshots?graphIdentifier=<graph id> — observe HTTP 404 ResourceNotFoundException ("No graph found for customer [...] with graph id [...]") instead of a page carrying the final snapshot that the delete just took and that `snapshots_in` is written to keep listed under that graph.
7. GET /snapshots (no graphIdentifier) — the final snapshot IS reported, proving the record exists and only the per-graph listing lost it; GET /snapshots/<snapshot id> also still answers 200.
8. Repeat for the tasks: StartImportTask on a second graph, then DELETE that graph (skipSnapshot=true), wait out DELETION_SPAN, then GET /importtasks?graphIdentifier=<that graph id> — 404, although GET /importtasks lists the task and GET /importtasks/<task id> still answers 200. Same with StartExportTask and GET /exporttasks?graphIdentifier=...

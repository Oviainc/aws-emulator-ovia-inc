Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 772fb5ab5b7496e2df886721dbd75d9882bc460e

# ListImportTasks 404s the whole listing when graphIdentifier names a graph that is gone, making the task↔graph association the store deliberately keeps unobservable

_Filed: 2026-10-28T21:36:48Z_
_Operation: ListImportTasks | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `772fb5ab5b7496e2df886721dbd75d9882bc460e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The store deliberately keeps an import task after the graph it loaded into has been reclaimed, and the listing helper the handler pages over documents that retention as OBSERVABLE THROUGH THE `graphIdentifier` FILTER: `import_tasks_in`'s doc says a task "stays listed under the graph it loaded into even once that graph is gone".

The handler makes that unreachable. When `graphIdentifier` is supplied it resolves the graph FIRST and returns `ResourceNotFoundException` if the store no longer holds it, so `import_tasks_in` is never called with the id of a graph that is gone — the one case its documented rule is about. A caller that deletes a graph and then asks for the import tasks of that graph gets a 404 instead of the tasks the store is still holding for exactly that purpose, even though `GetImportTask` on each of those task ids still answers 200 with `graphId` naming the gone graph (which the crate has a test for).

The same contradiction is spelled three times in this crate, so a fix should settle all three paths together: `ListExportTasks` (`export_tasks_in`: "a task stays listed under the graph it read even once that graph is gone") and `ListGraphSnapshots` (`snapshots_in`: "a snapshot stays listed under the graph it was taken from even once that graph is gone") resolve the filter graph the same way and refuse the same way. Either the helpers' documented rule is observable — the filter has to be matched against the stored `graph_id`/`source_graph_id` without requiring the graph to still exist — or the retention those docs justify has no reachable reader.

## Evidence

The state module states the rule — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs:2002-2006`:
```
    /// The same shape as [`Self::export_tasks_in`], for the same reason: `import_tasks`
    /// is a `BTreeMap`, so one stable total order underlies every page and a token can
    /// name a position in it. `of_graph` is matched against the task's stored `graph_id`,
    /// so a task stays listed under the graph it loaded into even once that graph is
    /// gone.
    pub fn import_tasks_in<'store>(
```
and the retention it depends on — `src/state.rs:1577-1578` (`remove_graph`):
```
    /// so it cannot outlive it. An import task is NOT dropped with its graph: it is
    /// read by its own id and keeps the provenance of the load it describes.
```

The handler, the helper's ONLY caller, refuses before the filter is ever built — `src/ops/list_import_tasks.rs:189-204`:
```
        // `graphIdentifier` is optional: absent, the listing spans every import task of
        // the Region; present, it has to name a graph of this Region, because a listing
        // of the import tasks of a graph that does not exist is a failed lookup rather
        // than an empty page.
        let of_graph = match input.graph_identifier.as_deref() {
            Some(identifier) => {
                let Some(graph) = store.settled_graph(identifier, &region, now) else {
                    return Err(ListImportTasksErrorModeled::ResourceNotFoundException(
                        crate::state::graph_not_found(identifier),
                    )
                    .into());
                };
```
`settled_graph` reaps first (`src/state.rs:1862`: `self.reap_deleted_graphs(now);`), so once `DELETION_SPAN` has passed the graph is gone and this arm always fires.

The two siblings carry the identical pair: `src/state.rs:1434-1436` ("`of_graph` is matched against the task's stored `source_graph_id`, so a task stays listed under the graph it read even once that graph is gone") against `src/ops/list_export_tasks.rs:178-193`, and `src/state.rs:1911-1913` ("so a snapshot stays listed under the graph it was taken from even once that graph is gone") against `src/ops/list_graph_snapshots.rs:190-205`.

Test coverage: `tests/all/import_task_read.rs:192` is `an_import_task_is_served_by_its_own_id_once_its_graph_is_gone`, which pins the record surviving the graph (`"a task must outlive the graph it loaded into"`, line 343) — but it only reads the task by id. No test in `tests/all/import_task_listing.rs`, `export_task_listing.rs` or `graph_snapshot_listing.rs` exercises the filtered listing of a graph that is gone, so nothing pins the 404 the handler answers.

## Steps to Reproduce

1. CreateGraph with graphName `import-target`, provisionedMemory 16; wait out PROVISIONING_SPAN so it reports AVAILABLE.
2. StartImportTask on that graph id with a valid roleArn and source; note the returned `taskId`.
3. DeleteGraph on that graph id with `skipSnapshot: true`, then wait out DELETION_SPAN so the record is reclaimed (GetGraph answers 404).
4. GET /importtasks/{taskId} (GetImportTask) — 200, and `graphId` still names the deleted graph, so the association is stored.
5. GET /importtasks?graphIdentifier={graphId} (ListImportTasks) — observe HTTP 404 ResourceNotFoundException instead of a page containing that task, contradicting `import_tasks_in`'s "a task stays listed under the graph it loaded into even once that graph is gone".
6. Repeat with ListExportTasks (after StartExportTask) and ListGraphSnapshots (after CreateGraphSnapshot) against a deleted graph — same refusal, same contradicted helper doc.

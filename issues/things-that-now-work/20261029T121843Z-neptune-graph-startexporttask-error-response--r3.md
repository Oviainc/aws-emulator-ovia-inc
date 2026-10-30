Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 45e969ed3c6a96399c623276452bb212f0ee3327

# StartExportTask reaches its full-store ceiling only after the graph-status gate, inverting its three sibling minting handlers, and crate::state still documents that refusal as a ValidationException

_Filed: 2026-10-29T12:18:43Z_
_Operation: StartExportTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `45e969ed3c6a96399c623276452bb212f0ee3327`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two symptoms of how StartExportTask reports a full export-task store.

(1) Precedence inverted against every sibling. Three other handlers in this crate that mint a record into a bounded store check the STORE CEILING before the graph-status precondition, and two of them carry a comment saying why the store/identity check goes first. StartExportTask — the mirror of StartImportTask — checks the graph's status FIRST and only then the ceiling. So for a request that breaks both conditions at once (a full export-task store AND a graph that is not AVAILABLE) StartImportTask answers the full-store message while StartExportTask answers the graph-status message, even though the two handlers are written as each other's counterpart and both route the ceiling through the same `crate::state::store_ceiling_conflict` builder.

(2) The crate's own documentation of that ceiling still names the wrong modelled error. `MAX_EXPORT_TASKS`'s doc comment says reaching it is reported as a `ValidationException`. The handler under it returns a `ConflictException`, `crate::state::store_ceiling_conflict`'s own doc says the opposite in the same file, and the integration test asserts `ConflictException` and explicitly says "never as a `ValidationException`". The doc is the only place in the crate still stating the refused classification, so the next reader of `MAX_EXPORT_TASKS` is told to produce the error the code and the test forbid.

## Evidence

Sibling handlers put the ceiling BEFORE the graph-status gate:

- `src/ops/start_import_task.rs:287-301` — ceiling first: "// The store is shared and grows by one task per call, and a task is never reclaimed, so it has a ceiling (`crate::state::MAX_IMPORT_TASKS`)." with `if store.import_tasks.len() >= crate::state::MAX_IMPORT_TASKS { return Err(StartImportTaskErrorModeled::ConflictException(...` — and only then, at `src/ops/start_import_task.rs:311-319`, `if graph_status != crate::state::GraphStatus::Available { return Err(...graph_status_precondition_conflict("start the import task", graph_status)...`
- `src/ops/create_graph_snapshot.rs:248-250` — "// Checked before the source graph's status so a duplicate name is reported as the name conflict it is, whatever the graph is doing." then ceiling at `src/ops/create_graph_snapshot.rs:267` (`if store.snapshots.len() >= crate::state::MAX_SNAPSHOTS`) and the graph gate last at `src/ops/create_graph_snapshot.rs:297` (`if !source_graph_status.is_at_rest()`).
- `src/ops/create_private_graph_endpoint.rs:214-215` — "// Checked before the graph's status so a duplicate is reported as the conflict it is, whatever the graph is doing." then ceiling at `src/ops/create_private_graph_endpoint.rs:234` and the graph gate last at `src/ops/create_private_graph_endpoint.rs:259`.

StartExportTask inverts it, with no comment explaining the inversion:

- `src/ops/start_export_task.rs:278-286`: `if source_graph_status != crate::state::GraphStatus::Available { return Err(StartExportTaskErrorModeled::ConflictException(crate::state::graph_status_precondition_conflict("start the export task", source_graph_status)).into()); }`
- `src/ops/start_export_task.rs:294-302` (AFTER it): `if store.export_tasks.len() >= crate::state::MAX_EXPORT_TASKS { return Err(StartExportTaskErrorModeled::ConflictException(crate::state::store_ceiling_conflict("export tasks", crate::state::MAX_EXPORT_TASKS)).into()); }`

The stale doc, both sides quoted from the same crate:

- `src/state.rs:2320-2325` (doc of `MAX_EXPORT_TASKS`): "`StartExportTask` models no quota error ... so reaching it is reported as the `ValidationException` that names this bound — not as throttling, which would promise that a retry can succeed."
- `src/ops/start_export_task.rs:290-293` (the code under it): "This operation models no quota error, so reaching the ceiling is reported with the `ConflictException` it does model, by the one builder every full-store refusal in this crate reaches (`crate::state::store_ceiling_conflict`)" — and it returns `ConflictException` (`src/ops/start_export_task.rs:295`).
- `src/state.rs:2951-2954` (doc of `store_ceiling_conflict`, same file as the stale doc): "A full store is never a `ValidationException`: every member of such a request is well formed and accepted, so nothing about the INPUT was invalid".

Test coverage that pins the current (ConflictException) answer and so contradicts the doc: `tests/all/full_task_store_refusals.rs:7-9` — "no quota error reports a store with no room left through the `ConflictException` ... and never as a `ValidationException`" — and `tests/all/full_task_store_refusals.rs:410-419`, which asserts `status == 409` and `error_code(&body) == "ConflictException"` for both the import and the export store. No existing test sends a request that breaks the graph-status gate AND meets the ceiling at once, so re-ordering the two checks changes no current assertion.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. Create a graph, wait for it to reach AVAILABLE, then call StopGraph and wait for STOPPED.
3. Fill the export-task store to crate::state::MAX_EXPORT_TASKS entries (see tests/all/full_task_store_refusals.rs:254-276 for how the test seeds them).
4. Call StartExportTask against the STOPPED graph. Observe 409 ConflictException with the message 'Unable to start the export task because the graph is in the STOPPED state.' — the full store is not reported.
5. Do the same with StartImportTask against a STOPPED graph with the import-task store full. Observe 409 ConflictException with the message 'this emulator holds at most 100 import tasks' — the opposite member of the same pair of conditions is reported.
6. Read src/state.rs:2320-2325 and compare it with src/ops/start_export_task.rs:294-302 and tests/all/full_task_store_refusals.rs:415-419: the doc names ValidationException, the code and the test name ConflictException.

Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7b6bbea9d5777d5e3200cc7b9f4654c9e364acc7

# The five quota-reporting handlers hand-write the store-ceiling clause instead of the crate's "ONE clause" builder, and two of them already word the identical MAX_GRAPHS condition differently

_Filed: 2026-10-29T15:11:33Z_
_Operation: CreateGraph | Class: error-response_

> _Note: this issue was identified against the code base at commit `7b6bbea9d5777d5e3200cc7b9f4654c9e364acc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` hosts a single builder for the sentence this service refuses a reached store ceiling with, and documents it as the only place that sentence exists so that "a caller cannot tell one full store from another by the wording of its refusal". `store_ceiling_conflict`, which wraps it, additionally names the five operations that report a ceiling through the modelled `ServiceQuotaExceededException` (402) — `CreateGraph`, `CreateGraphSnapshot`, `RestoreGraphFromSnapshot`, `CreateGraphUsingImportTask`, `CreatePrivateGraphEndpoint` — and asserts the clause is "Worded by `store_ceiling_clause`, so the ceiling reads the same whichever store was full".

None of those five handlers reaches either builder. Each hand-writes its own `format!` for the clause, and the copies have ALREADY drifted: `CreateGraph` and `RestoreGraphFromSnapshot` guard the *identical* predicate (`store.graphs.len() >= crate::state::MAX_GRAPHS`) yet answer two different sentences — "delete a graph before creating another" vs "delete a graph before restoring another" — so the same full-graph-store condition is distinguishable by its wording, exactly what `store_ceiling_clause`'s doc says must not happen. `CreateGraphUsingImportTask` duplicates `CreateGraph`'s graph sentence verbatim AND hand-writes an import-task clause (`"this emulator holds at most {} import tasks"`) that is byte-for-byte what `store_ceiling_clause("import tasks", MAX_IMPORT_TASKS)` already produces for `StartImportTask`.

Meanwhile the two handlers that model no quota error (`StartImportTask`, `StartExportTask`), plus `insert_query` and `mint_final_snapshot`, DO go through the shared builder. So the crate has one rule stated once, honoured on the 409 paths and bypassed on all five 402 paths.

## Evidence

The rule the crate states, in its own words:

- `src/state.rs:2931-2939` (doc + body of `store_ceiling_clause`): "/// The ONE clause this service words a reached store ceiling with: how many records of that kind this emulator keeps at most. /// ... `records` names what the ceiling counts, in the plural the refusal reads in (\"import tasks\", \"export tasks\", \"graph snapshots\") ... /// Written once here so a caller cannot tell one full store from another by the wording of its refusal." — `fn store_ceiling_clause(records: &str, ceiling: usize) -> String { format!("this emulator holds at most {ceiling} {records}") }`
- `src/state.rs:2942-2957` (doc of `store_ceiling_conflict`): "/// Which modelled error reports a reached ceiling follows from the operation's own error set: an operation that models `ServiceQuotaExceededException` (402) reports it there (`CreateGraph`, `CreateGraphSnapshot`, `RestoreGraphFromSnapshot`, `CreateGraphUsingImportTask`, `CreatePrivateGraphEndpoint`) ... /// Worded by [`store_ceiling_clause`], so the ceiling reads the same whichever store was full."

The five 402 handlers that break it — none of them calls `store_ceiling_clause` / `store_ceiling_conflict`:

- `src/ops/create_graph.rs:265-272`: `if store.graphs.len() >= crate::state::MAX_GRAPHS {` ... `message: format!("this emulator holds at most {} graphs; delete a graph before creating another", crate::state::MAX_GRAPHS),`
- `src/ops/restore_graph_from_snapshot.rs:303-311`: the SAME predicate `if store.graphs.len() >= crate::state::MAX_GRAPHS {` ... `message: format!("this emulator holds at most {} graphs; delete a graph before restoring another", crate::state::MAX_GRAPHS),` — the drift: one condition, two sentences.
- `src/ops/create_graph_snapshot.rs:267-275`: `if store.snapshots.len() >= crate::state::MAX_SNAPSHOTS {` ... `message: format!("this emulator holds at most {} graph snapshots; delete a snapshot before creating another", crate::state::MAX_SNAPSHOTS),`
- `src/ops/create_graph_using_import_task.rs:349-366`: the table rows `(store.graphs.len() >= crate::state::MAX_GRAPHS, "Graph", format!("this emulator holds at most {} graphs; delete a graph before creating another", crate::state::MAX_GRAPHS))` and `(store.import_tasks.len() >= crate::state::MAX_IMPORT_TASKS, "ImportTask", format!("this emulator holds at most {} import tasks", crate::state::MAX_IMPORT_TASKS))` — the second is a hand-copy of `store_ceiling_clause`'s exact output.
- `src/ops/create_private_graph_endpoint.rs:234-242`: `if store.private_endpoint_count() >= crate::state::MAX_PRIVATE_GRAPH_ENDPOINTS {` ... `message: format!("this emulator holds at most {} private graph endpoints; delete an endpoint before creating another", crate::state::MAX_PRIVATE_GRAPH_ENDPOINTS),`

The paths that DO honour the rule, for contrast, in the same crate:

- `src/ops/start_import_task.rs:293-300`: `if store.import_tasks.len() >= crate::state::MAX_IMPORT_TASKS { return Err(StartImportTaskErrorModeled::ConflictException(crate::state::store_ceiling_conflict("import tasks", crate::state::MAX_IMPORT_TASKS))...`
- `src/ops/start_export_task.rs:294-301`: `crate::state::store_ceiling_conflict("export tasks", crate::state::MAX_EXPORT_TASKS)`
- `src/state.rs:1536-1543` (`mint_final_snapshot`) and `src/state.rs:1744` (`insert_query`) both build their message from `store_ceiling_clause(...)`.

Test coverage: the only test that pins a ceiling message is `tests/all/full_task_store_refusals.rs:310-316`, which asserts the SHARED clause (`format!("this emulator holds at most {} {}", store.ceiling(), store.records())`) for `StartImportTask`/`StartExportTask` — i.e. the 409 paths. A repo-wide grep for "holds at most" under `tests/` matches only that file, so no test asserts any of the five hand-written 402 sentences; routing them through the shared builder changes no existing assertion.

## Steps to Reproduce

1. Read `src/state.rs:2931-2939` and `src/state.rs:2942-2957`: the crate declares one clause for a reached store ceiling, written once, and names `CreateGraph` and `RestoreGraphFromSnapshot` among the operations that report it.
2. Grep the crate for `store_ceiling_clause` and `store_ceiling_conflict`: the only callers are `src/state.rs` (`mint_final_snapshot`, `insert_query`), `src/ops/start_import_task.rs` and `src/ops/start_export_task.rs`. None of the five handlers the doc names appears.
3. Compare `src/ops/create_graph.rs:265-272` with `src/ops/restore_graph_from_snapshot.rs:303-311`: the guard is the identical `store.graphs.len() >= crate::state::MAX_GRAPHS`, but the messages end '…delete a graph before creating another' and '…delete a graph before restoring another'.
4. Boot the emulator, fill the graph store to `crate::state::MAX_GRAPHS` graphs, then call CreateGraph once and RestoreGraphFromSnapshot once. Both answer 402 ServiceQuotaExceededException for the same full store, with two different messages — so a caller can tell the two refusals apart by wording, which `store_ceiling_clause`'s doc says it must not be able to do.
5. Compare `src/ops/create_graph_using_import_task.rs:360-365` with `src/state.rs:2938-2939`: the hand-written import-task message is exactly what `store_ceiling_clause("import tasks", MAX_IMPORT_TASKS)` returns, duplicated rather than called.

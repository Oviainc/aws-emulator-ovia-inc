Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36

# A task-listing nextToken is not scoped to the listing that minted it, so ListImportTasks' cursor silently truncates (or empties) a ListExportTasks page

_Filed: 2026-10-29T01:12:35Z_
_Operation: ListExportTasks | Class: pagination_

> _Note: this issue was identified against the code base at commit `41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The five paginated listings of this crate each gate their `nextToken` by handing the shared codec their own id-space predicate, and `crate::state::decode_page_token`'s doc states that this id-space check is what keeps one listing's token out of another listing's pages. That holds for the graph, snapshot and VPC id spaces — but NOT for the two task listings: `is_export_task_id` and `is_import_task_id` are literally the same function (`is_task_id`), because the model gives an import task and an export task one `^t-[a-z0-9]{10}$` pattern.

So a `nextToken` that `ListImportTasks` minted is accepted verbatim by `ListExportTasks`, and vice versa. The two records live in two separate `BTreeMap`s fed by two separate counters (`import_task_id_counter`, `export_task_id_counter`), so the import id the token carries names an ARBITRARY cut point in the export map's order: `export_tasks_in` resumes at `Bound::Excluded(that id)` and silently drops every export task ordered at or before it — a truncated page, or an empty page with no `nextToken`, reported as a successful end-of-listing. Nothing tells the caller the cursor belonged to a different listing, even though the crate's own refusal for a token it never issued (`unissued_page_token`) exists for exactly that case.

The crate elsewhere treats the two task namespaces as distinct — a task id that is "well formed, stored, and still not found by this lookup" in the sibling namespace is refused 404 — so the pagination gate is the one place where a value from one task namespace is honoured by the other.

## Evidence

The rule the shared codec states about itself — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs:2342-2356`:
```
/// The position a token resumes after, or `None` when it is not a token
/// [`encode_page_token`] minted for the id space `in_id_space` accepts.
///
/// `in_id_space` is the listing's own id-space predicate, one of the `is_*_id`
/// predicates below: a decoded value naming no position in it is refused rather than
/// treated as the start of the list, which is also what stops one listing's token from
/// paging another's id space.
...
pub fn decode_page_token(token: &str, in_id_space: impl Fn(&str) -> bool) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    in_id_space(&decoded).then_some(decoded)
}
```

The two predicates the two task listings pass are one predicate — `src/state.rs:2462-2472`:
```
/// Whether `value` matches the modelled `ExportTaskId` pattern `^t-[a-z0-9]{10}$` —
/// the same check as [`is_task_id`], under the name the export surface reads it by.
pub fn is_export_task_id(value: &str) -> bool {
    is_task_id(value)
}

/// Whether `value` matches the modelled `TaskId` pattern `^t-[a-z0-9]{10}$` - the same
/// check as [`is_task_id`], under the name the import surface reads it by.
pub fn is_import_task_id(value: &str) -> bool {
    is_task_id(value)
}
```
reached by `src/ops/list_export_tasks.rs:212-214` (`crate::state::decode_page_token(token, crate::state::is_export_task_id)`) and `src/ops/list_import_tasks.rs:223-225` (`crate::state::decode_page_token(token, crate::state::is_import_task_id)`).

The accepted value is then used as a cut point in the OTHER collection — `src/ops/list_export_tasks.rs:197-200`:
```
        let mut remaining = store
            .export_tasks_in(&region, of_graph.as_deref(), start_after.as_deref())
            .map(ExportTaskSummary::from);
        let (tasks, next_token) = take_page(&mut remaining, page_size, |summary| &summary.task_id);
```
over a map keyed independently of the import map — `src/state.rs:1290-1292` and `:1303-1305` (`pub import_tasks: BTreeMap<String, ImportTask>` / `pub export_tasks: BTreeMap<String, ExportTask>`), fed by two counters, `src/state.rs:1316-1319` (`pub import_task_id_counter: u64,` / `pub export_task_id_counter: u64,`).

The crate states that the two task namespaces are distinct everywhere else — `tests/all/task_namespace_refusals.rs:11-20`:
```
//! Import tasks and export tasks share ONE modelled identifier space (`TaskId` and
//! `ExportTaskId` are both `^t-[a-z0-9]{10}$`) while living in two namespaces, so this
//! rule is what distinguishes their refusals.
...
//!   id of a task that really exists IN THE SIBLING NAMESPACE, which is the one value
//!   that is well formed, stored, and still not found by this lookup.
```

Test coverage: `tests/all/unissued_page_token_refusal.rs:197-203` asserts the rule only for the OTHER spaces, and both task listings declare the same `space: "task"` (lines 158 and 166), so the cross pair is filtered out of the table —
```
        // Every OTHER id space of the service: one listing's token never pages another's.
        tokens.extend(
            ID_SPACES
                .iter()
                .filter(|(space, _)| *space != listing.space)
```
and `tests/all/listing_paging_rule.rs` only ever feeds a listing the token that listing itself issued, so nothing pins the current cross-acceptance.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName="import-host", provisionedMemory=16) and poll GetGraph until AVAILABLE; do the same for a second graph "export-host".
3. StartImportTask on "import-host" (valid roleArn, any source) three times — wait out PROVISIONING_SPAN between calls so each is accepted — then StartExportTask on "export-host" (valid roleArn, kmsKeyIdentifier, destination s3://bucket/prefix) three times.
4. GET /importtasks?maxResults=1 — note the returned `nextToken` (it is hex('t-xxxxxxxxxx') for an IMPORT task).
5. GET /exporttasks?nextToken=<that token> — observe HTTP 200 with a page of export tasks that silently starts after an id no export task ever had: depending on how the two counters' ids sort, the page omits some or all of the three export tasks and may carry no `nextToken` at all, reporting end-of-listing.
6. GET /exporttasks (no token) — observe all three export tasks, proving the previous page dropped entries rather than being refused as a token this listing never issued (`nextToken '<token>' is not a pagination token this service issued`).
7. Repeat in the other direction: take ListExportTasks' token into GET /importtasks?nextToken=... — same silent truncation.

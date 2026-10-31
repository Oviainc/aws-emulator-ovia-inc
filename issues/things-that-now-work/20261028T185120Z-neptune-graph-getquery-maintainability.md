Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 293c15726352cf981422bfe1d7cee5f6a5389f12

# Hoist the per-query record projection that GetQuery and ListQueries each hand-copy into the crate::state record-output macros that every other record of this crate already uses

_Filed: 2026-10-28T18:51:20Z_
_Operation: GetQuery | Class: maintainability_

> _Note: this issue was identified against the code base at commit `293c15726352cf981422bfe1d7cee5f6a5389f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The query record is the ONE resource of this crate whose output projection is written out by hand, twice, in two different handler files.

`GetQueryOutput` and `QuerySummary` are structurally identical shapes — the model gives both exactly `id`, `queryString`, `waited`, `elapsed`, `state` (`smithy_query(query="shape",shape="GetQueryOutput,QuerySummary")`). `get_query.rs` builds the first by naming all five members inline in the handler body; `list_queries.rs` builds the second by naming the same five members, in the same order, with the same five expressions, in a private `fn summarise`. Nothing links the two, so a change to how one of the five facts is derived (e.g. the clock arithmetic behind `waited`/`elapsed`, or whether a member is reported at all) lands in one copy and silently not in the other — which is exactly the drift `crate::state` says it hosts its record-output macros to prevent for every OTHER record this service stores (graph, snapshot, import task, export task, private endpoint).

The fix is the pattern the crate already has: express the query projection once in `crate::state` (a `From<&Query>` rendered for both `GetQueryOutput` and `QuerySummary` by a `query_record_output!` macro alongside the existing five, taking `now` the way `ExportTask::get_output(now)` does), and have both handlers call it. Every response byte and every existing assertion stays identical — the two copies already agree today, so consolidating them is a no-op on the wire.

## Evidence

Copy 1 — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/get_query.rs:202-210`:
```
        // Every member is read off the stored record, and the two durations are read
        // off the clock against the instants that record holds.
        Ok(GetQueryOutput {
            id: Some(query.id.clone()),
            query_string: Some(query.query_string.clone()),
            state: Some(query.state.as_wire().to_string()),
            waited: Some(query.waited_millis(now)),
            elapsed: Some(query.elapsed_millis(now)),
        })
```

Copy 2 — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/list_queries.rs:253-264`, which even names the duplication in its own doc comment:
```
/// One stored query as the listing reports it: the same five facts `GetQuery` reports
/// about one query, with both durations read off the clock at `now` against the instants
/// the record holds.
fn summarise(query: &crate::state::Query, now: ::std::time::Instant) -> QuerySummary {
    QuerySummary {
        id: Some(query.id.clone()),
        query_string: Some(query.query_string.clone()),
        state: Some(query.state.as_wire().to_string()),
        waited: Some(query.waited_millis(now)),
        elapsed: Some(query.elapsed_millis(now)),
    }
}
```

The rule this crate states for every other record — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs:3206-3208`:
```
/// The `*Output` shapes that carry a whole graph record are structurally
/// identical, so one macro renders the stored graph into each rather than one
/// hand-copied field list per operation, which could drift apart.
```
and again for the task records at `src/state.rs:3351-3355`:
```
/// The `*Output` shapes that carry a whole export-task record differ only in the
/// members they add, so the members they SHARE are rendered by one macro rather than
/// one hand-copied field list per operation, which could drift apart. The listing
/// projection (`ExportTaskSummary`) is rendered here too, so a listed task cannot drift
/// from what a single-task read answers;
```
The five macros that implement it (`src/state.rs:3235-3244`, `3291-3296`, `3324-3349`, `3383-3394`, `3417-3422`) cover `CreateGraphOutput`/`GetGraphOutput`/.../`GraphSummary`, `GraphSnapshotSummary`, `ImportTaskSummary`, `ExportTaskSummary` and `PrivateGraphEndpointSummary` — every stored record EXCEPT the query.

The model proving the two shapes are the same member set (`smithy_query(query="shape",shape="QuerySummary,GetQueryOutput")`): `QuerySummary` members = `id`, `queryString`, `waited`, `elapsed`, `state`; `GetQueryOutput` members = `id`, `queryString`, `waited`, `elapsed`, `state`.

Test coverage that must stay green and unchanged: `tests/all/query_status_reporting.rs:43` pins GetQuery's projection — `const OUTPUT_MEMBERS: &[&str] = &["id", "queryString", "state", "waited", "elapsed"];` — plus the clock assertions at lines 99-137 and the monotonic-`elapsed` check at 284-311; `tests/all/query_listing.rs:48` pins the listing's — `const SUMMARY_MEMBERS: &[&str] = &["id", "queryString", "waited", "elapsed", "state"];` — plus lines 138-160; `tests/all/query_cancellation.rs:278-321` asserts both report the same facts across a cancel.

## Steps to Reproduce

1. Open `src/ops/get_query.rs` and read the `Ok(GetQueryOutput { .. })` literal at lines 204-210: five members, each derived from the stored `Query` and `now`.
2. Open `src/ops/list_queries.rs` and read `fn summarise` at lines 256-264: the same five members, same order, same five expressions, into `QuerySummary`.
3. Confirm with `smithy_query(query="shape",shape="GetQueryOutput,QuerySummary")` that both shapes declare exactly `id`, `queryString`, `waited`, `elapsed`, `state` — so neither copy carries a member the other lacks.
4. Open `src/state.rs:3206` and `src/state.rs:3351` and read the stated reason the graph, snapshot, import-task, export-task and private-endpoint projections are rendered by macros there: 'rather than one hand-copied field list per operation, which could drift apart'. The query record is the only stored record not rendered that way.
5. Change how one of the five facts is derived in one file only (e.g. report `waited: None` in `get_query.rs`) and observe that `tests/all/query_listing.rs` still passes while `tests/all/query_status_reporting.rs` fails — the two copies are free to disagree.

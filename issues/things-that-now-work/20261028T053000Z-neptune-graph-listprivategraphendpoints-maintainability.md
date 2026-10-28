Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# Hoist the page-size resolution and next-token decision the five listings each re-implement (one has already drifted a clamp the others lack)

_Filed: 2026-10-28T05:30:00Z_
_Operation: ListPrivateGraphEndpoints | Class: maintainability_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of this crate's five paginated listings re-implements the same two-part paging rule in its own handler: (1) resolve the page size as `maxResults` when it is supplied and convertible, else this listing's own `DEFAULT_PAGE_SIZE` const, and (2) mint a `nextToken` only when entries remain after the page, keyed off the LAST entry reported. The token FORMAT was already hoisted into `crate::state` (`encode_page_token` / `decode_page_token`, the fix for the `ListGraphs` maintainability issue 20261025T040952Z), but the page-size resolution, the five identical `DEFAULT_PAGE_SIZE` consts, and the "token only when more remain" decision were left copied per handler.

The copies have already drifted: `ListPrivateGraphEndpoints` adds `.clamp(1, DEFAULT_PAGE_SIZE)` to the resolved page size — with a comment explaining the invariant it protects (a page of at least one entry, so a page can never come back empty while endpoints remain) — and the other four do not, so four of five listings no longer hold the invariant the fifth documents as local. Hoisting the rule once beside the token codec it already calls removes that drift surface.

Responses are unchanged by the fix: `maxResults` is modelled `MaxResults` with `range 1..100` on all five listings, so the boundary validator refuses every value the clamp would change, and the next-token decision is byte-identical across the five copies.

## Evidence

Copy 1 (the drifted one) — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/list_private_graph_endpoints.rs:182-228, 245:

        // The modelled `maxResults` range 1..100 is refused by the boundary validator
        // before this handler runs, so reaching here with a value outside it is not
        // possible; the clamp keeps the invariant the paging below relies on LOCAL — a
        // page of at least one entry — so a page can never come back empty while
        // endpoints remain.
        let page_size = input
            .max_results
            .and_then(|requested| usize::try_from(requested).ok())
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, DEFAULT_PAGE_SIZE);
        ...
        let next_token = if remaining.next().is_some() {
            page.last()
                .map(|endpoint| encode_page_token(&endpoint.vpc_id))
        } else {
            None
        };
    ...
    const DEFAULT_PAGE_SIZE: usize = 100;

Copy 2 — src/ops/list_graphs.rs:169-197, 206 (no clamp):

        let page_size = input
            .max_results
            .and_then(|requested| usize::try_from(requested).ok())
            .unwrap_or(DEFAULT_PAGE_SIZE);
        ...
        let next_token = if remaining.next().is_some() {
            graphs.last().map(|summary| encode_page_token(&summary.id))
        } else {
            None
        };
    ...
    const DEFAULT_PAGE_SIZE: usize = 100;

Copy 3 — src/ops/list_graph_snapshots.rs:178-225, 237; Copy 4 — src/ops/list_export_tasks.rs:166-214, 223; Copy 5 — src/ops/list_import_tasks.rs:177-225, 234: character-for-character the same `page_size` expression and the same `if remaining.next().is_some() { <page>.last().map(|summary| encode_page_token(&summary.<key>)) } else { None }`, each with its own `const DEFAULT_PAGE_SIZE: usize = 100;` carrying the same doc sentence ("The page size when `maxResults` is absent: the largest the member allows").

That the bound is modelled on all five, so no response can change — `smithy_query(query="operation",operation="ListGraphs,ListGraphSnapshots,ListImportTasks,ListExportTasks,ListPrivateGraphEndpoints")`:

    "maxResults": {"target": "MaxResults", "httpQuery": "maxResults", "range": {"min": 1, "max": 100}, "type": "integer"}

The precedent in this same crate, already fixed, for hoisting the other half of this rule — issues/.../20261025T040952Z-neptune-graph-listgraphs-maintainability.md ("Hoist the triplicated pagination-token codec into crate::state beside the id-space predicates it already hosts"), whose result is the `use crate::state::encode_page_token;` line at the head of all five listings.

Test coverage that must stay green and unchanged: tests/all/private_graph_endpoint_listing.rs, tests/all/graph_snapshot_listing.rs, tests/all/import_task_listing.rs, tests/all/export_task_listing.rs, tests/all/graph_lifecycle.rs (ListGraphs paging) and tests/all/pagination_token_codec.rs, plus the per-handler `page_tokens_round_trip_and_reject_foreign_tokens` unit tests in each of the five files.

## Steps to Reproduce

1. Open src/ops/list_graphs.rs:169-172, src/ops/list_graph_snapshots.rs:178-181, src/ops/list_import_tasks.rs:177-180, src/ops/list_export_tasks.rs:166-169 and src/ops/list_private_graph_endpoints.rs:187-191 side by side: the same page-size resolution five times, with a `.clamp(1, DEFAULT_PAGE_SIZE)` on the last one only.
2. Open the next-token block of each (list_graphs.rs:193-197, list_graph_snapshots.rs:219-225, list_import_tasks.rs:219-225, list_export_tasks.rs:208-214, list_private_graph_endpoints.rs:223-228): the same decision, differing only in the field the last reported entry is keyed by.
3. Note the five identical `const DEFAULT_PAGE_SIZE: usize = 100;` declarations (list_graphs.rs:206, list_graph_snapshots.rs:237, list_import_tasks.rs:234, list_export_tasks.rs:223, list_private_graph_endpoints.rs:245).
4. Compare with the already-hoisted half of the same rule in src/state.rs:2324-2342 (`encode_page_token` / `decode_page_token`), which all five handlers now call instead of spelling the hex codec themselves.

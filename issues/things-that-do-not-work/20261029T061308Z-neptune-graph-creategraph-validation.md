Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2a30e9f7a31e09aa2b907716a7c9b6be0706cfea

# vectorSearchConfiguration.dimension of 65536 is accepted and persisted although the member documentation states the maximum is 65,535

_Filed: 2026-10-29T06:13:08Z_
_Operation: CreateGraph | Class: validation_

> _Note: this issue was identified against the code base at commit `2a30e9f7a31e09aa2b907716a7c9b6be0706cfea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `@range` on `VectorSearchDimension` is `min: 1, max: 65536` — exactly one ABOVE the maximum the member's own documentation publishes ("Max = 65,535"). The boundary validator therefore accepts `vectorSearchConfiguration = {"dimension": 65536}`, and CreateGraph's handler stores the supplied structure verbatim with no bound of its own, so the out-of-range dimension is persisted on the graph record and then reported back by CreateGraph, GetGraph, ListGraphs (via the graph-record output macro), captured into every snapshot's `source_configuration`, and restored by RestoreGraphFromSnapshot.

This is the ONE caller-sized constraint this crate leaves unenforced: every other bound the model under-constrains relative to its documentation is enforced by hand at the handler, with a comment saying so — `ListQueries` enforces the published `maxResults` minimum because "The model puts no range on the member", `StartExportTask` enforces the S3 form of `destination` because "its FORM is not" refused at the boundary, `StartImportTask`/`CreateGraphUsingImportTask` enforce the documented `format`/`blankNodeHandling` dependency and the `source` bounds, and `CreateGraph`/`CreateGraphSnapshot`/`RestoreGraphFromSnapshot` enforce the name pattern the regex engine cannot compile. `vectorSearchConfiguration` is the only member whose documented bound is wider than the enforced one and that nothing re-checks.

The same unchecked store happens in the sibling minting handler `CreateGraphUsingImportTask` (`src/ops/create_graph_using_import_task.rs:428`), so the fix belongs in one shared `crate::state` bound reached by both, in the same shape as `crate::state::import_source_requirement` / `crate::state::blank_node_handling_clause`.

## Evidence

The documented maximum, from the model itself — `smithy_query(query="doc",shape="CreateGraphInput",member="vectorSearchConfiguration")`:

> "Specifies the number of dimensions for vector embeddings that will be loaded into the graph. The value is specified as `dimension=`value. Max = 65,535"

reproduced verbatim in the handler's own contract header — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/create_graph.rs:71-72:
```
//!   dimensions for vector embeddings that will be loaded into the graph. The value is
//!   specified as dimension=value. Max = 65,535
```

The modelled trait that is actually enforced is one HIGHER — `smithy_query(query="shape",shape="VectorSearchConfiguration")`:
```
"dimension": {"target": "VectorSearchDimension", "required": true, "range": {"min": 1, "max": 65536}, "type": "integer"}
```
so 65536 passes the boundary constraint walk and reaches the handler.

The handler stores it unchecked — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/create_graph.rs:310:
```
            vector_search_configuration: input.vector_search_configuration,
```
(no clause between the `graph_name_is_well_formed` refusal at create_graph.rs:206-216 and the `bound_tags` refusal at create_graph.rs:224-232 mentions `vectorSearchConfiguration`). The sibling does the same — src/ops/create_graph_using_import_task.rs:428: `vector_search_configuration: input.vector_search_configuration,`.

The crate's own test names 65535 as the bound and never drives 65536 — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/tests/all/final_snapshot_rule.rs:73-76:
```
    /// `vectorSearchConfiguration.dimension`, within the documented maximum of 65535
    ...
    vector_dimension: Option<i64>,
```
with the largest row at tests/all/final_snapshot_rule.rs:453 `vector_dimension: Some(65535),`. The other dimension values any test sends are 1, 128 and 1536 (tests/all/graph_lifecycle.rs:173,191; tests/all/graph_restore_from_snapshot.rs:253,286; tests/all/create_graph_using_import_task.rs:316), so no existing test or comment in the crate justifies accepting 65536 — `grep -rn "65536"` over the crate matches nothing outside the raw model JSON.

The established pattern this member is the exception to — src/ops/list_queries.rs:241-246:
```
/// The model puts no range on the member (`smithy_query(query="operation",
/// operation="ListQueries")` -> `maxResults` targets the prelude `Integer`), so the
/// minimum the AWS API reference publishes for it stands as the bound enforced here.
const MIN_MAX_RESULTS: i32 = 1;
```
and src/ops/start_export_task.rs:215-219:
```
        // `destination` is modelled as a plain `String` of 1..1024 characters, so its
        // length is refused at the boundary but its FORM is not: ...
```

## Steps to Reproduce

1. Boot the emulator from this checkout and point a botocore `neptune-graph` client at it.
2. Call `CreateGraph(graphName='vec-max', provisionedMemory=16, vectorSearchConfiguration={'dimension': 65536})`.
3. Observe HTTP 201 and a response whose `vectorSearchConfiguration.dimension` is 65536 — a value one above the maximum the member's documentation publishes ("Max = 65,535"), where the handler-enforced bounds of every other doc-only constraint in this crate (ListQueries' maxResults minimum, StartExportTask's destination form, StartImportTask's source bounds, the graphName pattern) would have produced a 400 ValidationException.
4. Call `GetGraph(graphIdentifier=<id>)` and observe the same 65536 read back from the stored record; `CreateGraphSnapshot` of that graph then captures it into `source_configuration`, and `RestoreGraphFromSnapshot` brings a new graph up reporting it.
5. Repeat step 2 against `CreateGraphUsingImportTask(graphName='vec-max-2', source='s3://bucket/prefix/', roleArn=<same-account role>, vectorSearchConfiguration={'dimension': 65536})` — the sibling minting handler accepts it the same way.

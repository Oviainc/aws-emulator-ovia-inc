Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# ListQueries serves a graph whose data plane the crate says is not serving, while GetGraphSummary on the same host prefix refuses it

_Filed: 2026-10-28T05:30:00Z_
_Operation: ListQueries | Class: validation_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The four operations the model binds to the per-graph data-plane endpoint (`@endpoint(hostPrefix: "{graphIdentifier}.")`) are GetGraphSummary, ListQueries, GetQuery, CancelQuery (and the still-stubbed ExecuteQuery). `GetGraphSummary` states the rule this crate applies to that endpoint — it serves only while the graph is serving — and refuses any non-`AVAILABLE` graph with the modelled `ValidationException` (400) naming the status it read.

`ListQueries` is bound to the SAME host prefix and models the SAME `ValidationException`, but it checks only that the graph EXISTS: a graph that is `CREATING`, `UPDATING`, `IMPORTING`, `SNAPSHOTTING`, `STOPPED`, `STOPPING`, `RESETTING` or `DELETING` is served, and the answer is a success — HTTP 200 with an empty `queries` list — for a request the crate's own rule says has no serving data plane to answer it. The caller cannot tell "this graph is not serving, ask again later" from "this graph is serving and has no queries in flight". `GetQuery` (get_query.rs:168-176) and `CancelQuery` (cancel_query.rs:187-195) skip the same gate and answer the same existence-only lookup, so the inconsistency is the whole query surface, not one listing; this issue is filed against `ListQueries` because that is the one that answers a 200.

The defect is the divergence inside one crate between the rule one handler on the data plane states and enforces and the three on the same data plane that do not. Which refusal to use is not invented here: the sibling already answers the modelled `ValidationException` that `ListQueries` also declares.

## Evidence

The rule, stated and enforced — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/get_graph_summary.rs:182-203:

        // A summary is read from the graph's OWN data endpoint — the operation is
        // bound to the host prefix `{graphIdentifier}.`
        // (`smithy_query(query="operation",operation="GetGraphSummary")` -> endpoint) —
        // and that endpoint serves only while the graph is serving. A graph that is
        // provisioning, reconfiguring, importing, snapshotting, stopped or on its way
        // out has no data plane to summarise, which this operation reports with the
        // one client error it models for a request it cannot serve: it declares no
        // `ConflictException` (`smithy_query` -> errors), so the refusal is the
        // modelled `ValidationException` (400) naming the status that was read.
        if graph.status != crate::state::GraphStatus::Available {
            return Err(GetGraphSummaryErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "graph {} is in status {} and has no serving data plane to summarise",
                        graph.id,
                        graph.status.as_wire()
                    ),
                    reason: Some("BAD_REQUEST".to_string()),
                },
            )
            .into());
        }

The sibling on the same endpoint, existence only, no status gate anywhere in the handler — src/ops/list_queries.rs:188-216:

        // A query is addressed as a query OF a graph, so the identifier has to name a
        // graph of THIS Region: a listing of the queries of a graph that does not exist
        // is a failed lookup rather than an empty list. ...
        let Some(graph) = store.settled_graph(&input.graph_identifier, &region, now) else {
            return Err(ListQueriesErrorModeled::ValidationException(ValidationException {
                message: crate::state::graph_not_found(&input.graph_identifier).message,
                reason: Some("ILLEGAL_ARGUMENT".to_string()),
            })
            .into());
        };
        let graph_id = graph.id.clone();
        ...
        Ok(ListQueriesOutput { queries })

That both operations are on one data plane is the model's own binding — `smithy_query(query="operation",operation="GetGraphSummary,ListQueries,GetQuery,CancelQuery")`:

    "GetGraphSummary": ... "endpoint": {"hostPrefix": "{graphIdentifier}."}
    "ListQueries":    ... "endpoint": {"hostPrefix": "{graphIdentifier}."}, errors include {"shape": "ValidationException", "httpError": 400}
    "GetQuery":       ... "endpoint": {"hostPrefix": "{graphIdentifier}."}
    "CancelQuery":    ... "endpoint": {"hostPrefix": "{graphIdentifier}."}

The two siblings that skip it the same way — src/ops/get_query.rs:165-176 and src/ops/cancel_query.rs:182-195 both end their graph handling at

        if store
            .settled_graph(&input.graph_identifier, &region, now)
            .is_none()
        { /* 404 graph_not_found */ }

and read the query store next, with no reference to `graph.status`.

Test coverage: tests/all/graph_summary.rs exercises the GetGraphSummary gate; tests/all/query_listing.rs and tests/all/query_status_reporting.rs drive ListQueries/GetQuery only through `graph_in_service(..)` (an AVAILABLE graph — see tests/all/query_cancellation.rs), so no test pins the non-AVAILABLE answer of the query surface either way.

## Steps to Reproduce

1. Start the emulator and `POST /graphs` with `{"graphName": "gate-demo", "provisionedMemory": 16}`; note the returned `id` (the graph reports CREATING for crate::state::PROVISIONING_SPAN).
2. Immediately call `GET /summary?mode=BASIC` with header `graphIdentifier: <id>` — observe HTTP 400 ValidationException "graph <id> is in status CREATING and has no serving data plane to summarise" (get_graph_summary.rs:191-203).
3. Immediately call `GET /queries?maxResults=10` with the same `graphIdentifier: <id>` header — observe HTTP 200 and `{"queries": []}`: the same per-graph data plane, the same graph status, a success instead of the refusal the crate states for that endpoint.
4. Wait for AVAILABLE, then `POST /graphs/<id>/stop` and repeat both calls while the graph reports STOPPING/STOPPED — GetGraphSummary refuses, ListQueries still answers 200 with an empty list (and GetQuery/CancelQuery still answer the query-level 404 of get_query.rs:178-183).

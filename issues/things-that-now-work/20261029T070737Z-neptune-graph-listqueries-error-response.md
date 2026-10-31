Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 3a384b4a12755a7fa8fedde9fe2fff5778558e98

# A boundary-caught constraint violation answers a ValidationException with no `reason`, while the handler-caught violation of the same constraint class carries one — the thing the handlers say a caller cannot tell apart

_Filed: 2026-10-29T07:07:37Z_
_Operation: ListQueries | Class: error-response_

> _Note: this issue was identified against the code base at commit `3a384b4a12755a7fa8fedde9fe2fff5778558e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler in this crate that enforces a constraint the boundary validator cannot check reports it through the service's OWN `ValidationException`, filling the modelled `reason` member with `CONSTRAINT_VIOLATION`, and each of those sites states that it is worded by the same renderer the boundary uses "so a caller cannot tell which side of the boundary caught the violation".

A caller can tell, from the body alone: the boundary path goes through `EmulatedService::override_refusal`, whose return type (`smithy_server_runtime::Refused`) carries only `code` and `message`, so a refusal found at the boundary renders a `ValidationException` body with `__type` + `message` and NO `reason` at all.

ListQueries and ListGraphs make the divergence exact, because they refuse the SAME constraint class on the same member name through the two different paths. `ListQueries$maxResults` targets the unconstrained prelude `Integer`, so its minimum is enforced in the handler and the refusal carries `reason: "CONSTRAINT_VIOLATION"`. `ListGraphs$maxResults` targets `MaxResults` with `@range(1..100)`, so the identical "below the minimum" violation is caught at the boundary and the refusal carries no `reason`. Two listings of one service answer `maxResults=0` with two different `ValidationException` bodies.

The crate already treats a missing `reason` as a defect rather than a choice: its own test calls a `ValidationException` body without `reason` "the synthetic framework shape rather than this service's own", `ListGraphs`' module doc says the service's own shape "carries the `reason` the synthetic framework one does not", and the fixed issue 20261028T023537Z-neptune-graph-listgraphs-error-response was filed for exactly this member being dropped on one refusal path. The boundary path is the remaining path that drops it, and it is the path taken by every modelled `@required`/`@length`/`@range`/`@pattern`/enum violation of all 33 implemented operations.

## Evidence

The claim the handlers make — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/list_queries.rs:160-175`:
```
        // `maxResults` carries no range in the model, so the published minimum is
        // enforced here rather than at the boundary: a request asking for fewer than one
        // result names no number of queries to fetch. The refusal is worded by the same
        // builder the boundary refusal renderer uses (`crate::service`), so a caller
        // cannot tell which side of the boundary caught the violation.
        if input.max_results < MIN_MAX_RESULTS {
            return Err(ListQueriesErrorModeled::ValidationException(ValidationException {
                message: crate::state::validation_envelope(&[crate::state::constraint_clause(
                    "maxResults",
                    Some(&input.max_results.to_string()),
                    &crate::state::min_value_requirement(MIN_MAX_RESULTS),
                )]),
                reason: Some("CONSTRAINT_VIOLATION".to_string()),
            })
            .into());
        }
```
(the same sentence appears at `src/ops/create_graph.rs:198-205`, `src/ops/create_graph_snapshot.rs:169-175`, `src/ops/restore_graph_from_snapshot.rs:195-200` and `src/ops/create_graph_using_import_task.rs:248-251`, each followed by a `reason: Some("CONSTRAINT_VIOLATION".to_string())`).

The renderer they claim parity with can only restate two fields — `src/service.rs:229-242`:
```
        // The code stays the modelled `ValidationException` (HTTP 400), which is what
        // the framework would have answered; `override_refusal` replaces the code and
        // the message together, so it is restated rather than changed.
        ...
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
            _ => None,
        }
```
and `Refused` has no third member — `vendor/smithy-server-runtime/src/pre_dispatch.rs:163-169`:
```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// Error code / `__type` shape name.
    pub code: String,
    /// Human-readable message, unescaped — each protocol escapes it itself.
    pub message: String,
}
```

The member that is therefore absent is modelled and documented — `smithy_query(query="shape",shape="ValidationException")`: `"reason": {"target": "ValidationExceptionReason", "type": "enum", "doc": "<p>The reason that the resource could not be validated.</p>"}`.

The crate's own statement that a body without `reason` is the wrong shape — `tests/all/unissued_page_token_refusal.rs:225-233`:
```
            // The modelled shape's second member: present, and carrying the value that
            // classifies a bad argument. A body missing it is the synthetic framework
            // shape rather than this service's own.
            let reason = answer["reason"].as_str();
            assert_eq!(
                reason,
                Some(ILLEGAL_ARGUMENT),
```
and `src/ops/list_graphs.rs:73-77`:
```
//! The service's own `ValidationException` (HTTP 400) is answered besides those: this
//! listing takes a `nextToken` from the caller, and a token it cannot resolve to a
//! position in the graph-id space is client input it has to refuse — reported through
//! the service's own shape, which carries the `reason` the synthetic framework one does
//! not (see [`crate::state::unissued_page_token`]).
```

Test coverage: no existing assertion pins the absence, so filling `reason` on the boundary path leaves every current expectation (status 400 and `__type` == ValidationException, plus the exact `message`) unchanged — `tests/all/graph_lifecycle.rs:586-594` (in `list_graphs_pages_over_every_graph_without_repeats`) asserts only the status for the boundary-refused `maxResults`:
```
    // maxResults outside its modelled 1..100 range is refused at the boundary.
    for max_results in ["0", "101"] {
        let (status, body) =
            call(&router, "GET", &format!("/graphs?maxResults={max_results}"), None).await;
        assert_eq!(
            status, 400,
            "case maxResults {max_results}: expected 400, got {status}: {body}"
        );
    }
```
and `tests/all/graph_lifecycle.rs:421-429` (`create_graph_refuses_values_outside_the_modelled_constraints`) and `tests/all/error_message_rendering.rs:98-101,113-140` assert only `status`, `__type` and `message`.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName="reason-probe", provisionedMemory=16) and poll GetGraph until AVAILABLE.
3. GET /queries?maxResults=0 with the graphIdentifier header set to that graph id — observe 400 ValidationException whose body carries "reason": "CONSTRAINT_VIOLATION" beside "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
4. GET /graphs?maxResults=0 — observe 400 ValidationException whose body carries the analogous message but NO "reason" member at all, so the same constraint class on the same member name yields two different bodies.
5. GET /graphs/not-a-valid-id (a `@pattern` violation caught at the boundary) — again a ValidationException with no "reason"; compare with POST /graphs {"graphName":"g-reserved","provisionedMemory":16}, the same pattern enforced in the handler, which answers with "reason": "CONSTRAINT_VIOLATION".

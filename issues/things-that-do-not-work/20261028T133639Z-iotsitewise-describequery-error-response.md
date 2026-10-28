Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# DescribeQuery skips the workspace-existence check its own siblings (CancelQuery, DescribeSearch, GetQueryResults) state as the crate-wide rule

_Filed: 2026-10-28T13:36:39Z_
_Operation: DescribeQuery | Class: error-response_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeQuery addresses a query by (workspaceName, queryId) through `crate::state::query_key` and refuses with a 404 that names the QUERY. It never resolves the workspace, so a request naming a workspace this store does not hold is reported as a missing query instead of a missing workspace. Every other handler on the same resource does the opposite and says so in a comment: CancelQuery resolves the workspace first ("as every handler in this crate resolves the resource its path names before the one inside it"), GetQueryResults does the same for the same (workspaceName, queryId) key, and DescribeSearch — the sibling read of the other workspace-scoped async resource — resolves the workspace first too. The result is that two reads of one query disagree about which resource is absent: GET /workspaces/no-such-ws/queries/{id} answers "Query with ID ... not found" while PUT .../cancel on the identical path answers "Workspace with ID no-such-ws not found".

## Evidence

The rule, stated and implemented by the cancel of the same resource:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/cancel_query.rs:161-169
    "// The workspace the path names is resolved FIRST, as every handler in this\n // crate resolves the resource its path names before the one inside it, so a\n // request that names neither reports the workspace that is absent.\n if !store.workspaces.contains_key(&input.workspace_name) { return Err(not_found(crate::state::RESOURCE_WORKSPACE, &input.workspace_name)); }"

The same rule in the two other readers:
  src/ops/get_query_results.rs:181-190 — "// The results belong to a query OF a workspace, so the workspace the path names is resolved first, as every handler in this crate resolves the resource its path names before anything else ... if !store.workspaces.contains_key(&input.workspace_name) { return Err(not_found(crate::state::not_found_message(crate::state::RESOURCE_WORKSPACE, &input.workspace_name))); }"
  src/ops/describe_search.rs:181-190 — "// A search lives in ONE workspace, so a request naming a workspace this account does not hold addresses nothing: the not-found refusal names the workspace, the way every other operation scoped to one does ... if !store.workspaces.contains_key(&input.workspace_name) { ... RESOURCE_WORKSPACE ... }"

The handler that skips it:
  src/ops/describe_query.rs:169-178 — "let key = crate::state::query_key(&input.workspace_name, &input.query_id); ... let record = store.queries.get(&key).ok_or_else(|| not_found(&input.query_id))?;"
  src/ops/describe_query.rs:191-198 — "/// HTTP 404 `ResourceNotFoundException` naming the query the request addressed. A query is addressed within a workspace, so this is also what a query of ANOTHER workspace answers. ... message: crate::state::not_found_message(crate::state::RESOURCE_QUERY, query_id)"

Test coverage: no suite sends DescribeQuery an unknown workspace. tests/all/workspace_delete.rs:520-529 asserts only `(404, "ResourceNotFoundException")` for a query whose workspace was deleted — it does not pin the message — so resolving the workspace first leaves it green, and tests/all/refusal_message_rule.rs:274-287 already pins "Workspace with ID {UNKNOWN_WORKSPACE} not found" as this crate's answer for other workspace-scoped entry points (ListTimeSeries, DeleteTimeSeries).

## Steps to Reproduce

1. CreateWorkspace "ws"; StartQuery in it and note the queryId.
2. GET /workspaces/no-such-ws/queries/{queryId} (DescribeQuery) → 404 ResourceNotFoundException "Query with ID {queryId} not found".
3. PUT /workspaces/no-such-ws/queries/{queryId}/cancel (CancelQuery) → 404 ResourceNotFoundException "Workspace with ID no-such-ws not found".
4. GET /workspaces/no-such-ws/searches/{searchId} (DescribeSearch) likewise names the workspace — the query read is the only one of the four that names the inner resource.

Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# A different StartSearch reusing a spent clientToken is answered with the FIRST search's id, so GetSearchResults answers a query that was never run

_Filed: 2026-10-29T00:26:49Z_
_Operation: StartSearch | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartSearch recognises a reused `clientToken` by the token and workspace only: `search_for_client_token(&workspace, token)` returns the first search of that workspace carrying the token and the handler answers from it. `queryStatement`, `searchType`, `searchFilters` and `groupId` are never compared, although `SearchRecord` stores every one of them. A second request that reuses the token with a DIFFERENT `queryStatement` therefore starts no search and is handed the earlier search's id — and `GetSearchResults` on that id then reports the results of the first query, presented as the answer to the second. The crate's own idempotency rule (and the modelled doc, which speaks of repeating *a call*) makes a different request under a spent token the modelled `ConflictingOperationException`, which StartSearch models (409).

## Evidence

Handler — services/.../src/ops/start_search.rs:219-225:
```
        if let Some(client_token) = input.client_token.as_deref() {
            if let Some(existing) =
                store.search_for_client_token(&input.workspace_name, client_token)
            {
                return Ok(started(existing, now));
            }
        }
```
The helper compares only workspace + token — state.rs:4898-4907:
```
        self.searches.values().find(|record| {
            record.workspace_name == workspace_name
                && record.client_token.as_deref() == Some(client_token)
        })
```
The record holds what would distinguish two requests — start_search.rs:246-257: `query_statement: input.query_statement.clone(), search_type: ..., search_filters: input.search_filters.clone(), group_id: input.group_id.clone(), client_token: input.client_token.clone()`.

The rule it skips — create_task.rs:193-198: "a retry — the SAME request under that token — replays what that call left behind and writes nothing, while a DIFFERENT request reusing it belongs to another call and is the modelled conflict rather than a second task"; generic implementation in state.rs:9338-9342 (`TokenVerdict::Replay` only when `call.request == request`, else `TokenVerdict::Spent`). Sibling create_application.rs:229-232 hand-rolls the same lookup and does compare every member.
StartSearch models the conflict: smithy_query StartSearch errors include `{"shape": "ConflictingOperationException", "httpError": 409}`.

## Steps to Reproduce

1. CreateWorkspace W.
2. StartSearch in W with queryStatement Q1 and clientToken T; note searchId S1.
3. StartSearch in W with queryStatement Q2 (different) and the same clientToken T.
4. Observe the response carries searchId S1 and status of the first search; ListSearches shows one search only, whose queryStatement is Q1.
5. GetSearchResults(S1) returns results for Q1 while the caller believes it asked for Q2 — instead of the modelled ConflictingOperationException.

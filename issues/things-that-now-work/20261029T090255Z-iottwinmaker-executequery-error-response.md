Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# ExecuteQuery judges the nextToken before it resolves the workspace, so a query against an unknown workspace is refused for the token instead of for the workspace every sibling names

_Filed: 2026-10-29T09:02:55Z_
_Operation: ExecuteQuery | Class: error-response_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other workspace-scoped paginated operation of this crate resolves the SCOPE (the workspace, then the entity/component inside it) before it looks at the request's own page members, and `ListComponentTypes` states that order is deliberate and names `ListEntities` as the operation it follows. `ExecuteQuery` does the reverse: it decodes `nextToken` at src/ops/execute_query.rs:251-255 and only reaches the store — and the workspace lookup — at src/ops/execute_query.rs:272-287.

Because the token envelope is scoped to the workspace id, a token can never decode for a workspace that does not exist. So `ExecuteQuery(workspaceId=<unknown>, nextToken=<anything non-empty>)` is always refused with `nextToken is not a token this listing minted`, never with the workspace message `crate::state::workspace_not_found_message` builds — the opposite of what the seven sibling handlers do for the identical pair of bad inputs, and the opposite of the rule the crate's own test for that rule pins (its `unmintedToken` row asserts the refusal names the workspace, not the token).

The only ordering reason `ExecuteQuery` states for itself concerns the STATEMENT ("Parsed and planned BEFORE the store is touched: a statement that is not a query needs no state to be refused", src/ops/execute_query.rs:236-238); nothing in the file states a reason for deciding the token before the scope, and its module doc lists the unknown workspace and the un-minted token together as one error class without ordering them.

## Evidence

THE RULE A SIBLING STATES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_component_types.rs:227-237:
```
        // The SCOPE is resolved before the request's own members, so a listing
        // of a workspace this service does not hold is refused for the workspace
        // whatever else the request carries — the order `ListEntities` resolves
        // them in. ...
        let Some(workspace) = state.workspaces.get_mut(&input.workspace_id) else {
            return Err(invalid(crate::state::workspace_not_found_message(
                &input.workspace_id,
            ))
            .into());
```
and the test that pins it, tests/all/component_type_listing.rs:288-312:
```
/// RULE: the listing is scoped to a workspace this service holds — a request
/// naming one it does not is refused with the modelled `ValidationException`
/// naming that workspace, whatever page or filter members it carries, because
/// the scope is resolved before the request's own members.
...
        // A token this listing never minted: same.
        ("unmintedToken", json!({"nextToken": "not-a-token"})),
```
with the assertion "the refusal must name the workspace it could not resolve" (component_type_listing.rs:320-327).

EVERY OTHER WORKSPACE-SCOPED PAGINATED HANDLER DOES SCOPE FIRST, TOKEN SECOND (workspace-lookup line / token-decode line):
- src/ops/list_entities.rs:200 / 223-227
- src/ops/list_scenes.rs:209 / 221-227
- src/ops/list_sync_jobs.rs:198 / 211-217
- src/ops/list_sync_resources.rs:227 and 237 / 302-308
- src/ops/list_properties.rs:219, 224, 230 / 238-244
- src/ops/list_components.rs:228, 234 / 263-269
- src/ops/list_component_types.rs:233 / 258-264

THE HANDLER THAT REVERSES IT — src/ops/execute_query.rs:247-255 (token FIRST):
```
        // A `nextToken` carries the row offset the page resumes at, and is bound to BOTH
        // the workspace and the statement it was minted for: ...
        let resume_at = match input.next_token.as_deref() {
            None | Some("") => 0,
            Some(token) => decode_next_token(token, &input.workspace_id, &input.query_statement)
                .ok_or_else(|| invalid(crate::state::invalid_next_token_message()))?,
        };
```
and src/ops/execute_query.rs:280-287 (workspace SECOND):
```
            // The model declares no `ResourceNotFoundException` for this operation, so an
            // unknown workspace is refused the way `ListEntities` refuses one.
            let Some(workspace) = state.workspaces.get(&input.workspace_id) else {
                return Err(invalid(crate::state::workspace_not_found_message(
                    &input.workspace_id,
                ))
                .into());
            };
```
That very comment claims this refusal follows `ListEntities` — the operation whose order it does not follow.

THE ONLY ORDERING REASON THE FILE GIVES, WHICH IS ABOUT THE STATEMENT AND NOT THE TOKEN — src/ops/execute_query.rs:236-238:
```
        // Parsed and planned BEFORE the store is touched: a statement that is not a
        // query needs no state to be refused, and the store's lock below is held for as
        // little as possible.
```

TEST COVERAGE: tests/all/next_token_refusal.rs seeds the workspaces it queries (`seed` at next_token_refusal.rs:50-57 creates WORKSPACE and OTHER_WORKSPACE), and tests/all/execute_query.rs:780-803 varies only the STATEMENT a token was minted for — so no existing row sends ExecuteQuery an unknown workspace together with a token, and nothing pins the current order.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. ListComponentTypes(workspaceId="noSuchWs", nextToken="not-a-token") -> 400 ValidationException whose message names noSuchWs (the rule tests/all/component_type_listing.rs:288-327 pins).
3. ExecuteQuery(workspaceId="noSuchWs", queryStatement="SELECT e.entityId FROM EntityGraph MATCH (e)", nextToken="not-a-token") -> 400 ValidationException whose message is "nextToken is not a token this listing minted"; the workspace it could not resolve is never named, because src/ops/execute_query.rs:251-255 runs before the lookup at src/ops/execute_query.rs:282.
4. Repeat step 3 against the six other workspace-scoped paginated operations (ListEntities, ListScenes, ListSyncJobs, ListSyncResources, ListProperties, ListComponents) with the same unknown workspace and the same forged token: each names the workspace instead.

Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# A different ExecuteAction request reusing a spent clientToken is answered 202 with the FIRST action's id and executes nothing

_Filed: 2026-10-29T00:26:49Z_
_Operation: ExecuteAction | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ExecuteAction recognises a reused `clientToken` by the TOKEN ALONE: `store.action_for_client_token(token)` scans every stored action for one carrying that token and immediately returns its `actionId`. The members that say WHAT action to take — `targetResource`, `actionDefinitionId`, `actionPayload`, `resolveTo` — are never compared, even though the `ActionRecord` this crate writes stores all four. So a second request that carries the same token but names a different computation model, a different action definition or a different payload is answered 202 with the earlier action's id, records no `ActionRecord` and inserts no `ExecutionRecord`: the action the caller asked for is never executed, and the caller is told it was. That inverts the crate's own idempotency rule, which every handler built on `idempotent_verdict` states: the same request under a spent token replays, a DIFFERENT request under a spent token is the modelled `ConflictingOperationException` (which this operation models, HTTP 409). The modelled doc for the token covers only a retry, not a different request.

## Evidence

Handler, token-only replay — services/.../src/ops/execute_action.rs:188-196:
```
        if let Some(recorded) = input
            .client_token
            .as_deref()
            .and_then(|token| store.action_for_client_token(token))
        {
            return Ok(ExecuteActionOutput {
                action_id: recorded.action_id.clone(),
            });
        }
```
The helper it calls compares nothing but the token — state.rs:4965-4969:
```
    pub fn action_for_client_token(&self, client_token: &str) -> Option<&ActionRecord> {
        self.actions
            .values()
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }
```
Yet the record stores every member needed to tell two requests apart — execute_action.rs:316-327: `action_definition_id: input.action_definition_id.clone(), target_resource: input.target_resource.clone(), action_payload: input.action_payload.clone(), resolve_to: input.resolve_to.clone(), client_token: input.client_token.clone()`.

The crate-wide rule this skips, stated in a sibling — create_task.rs:193-198: "a token identifies the call that spent it, so a retry — the SAME request under that token — replays what that call left behind and writes nothing, while a DIFFERENT request reusing it belongs to another call and is the modelled conflict rather than a second task."
And implemented generically in state.rs:9338-9342: `Some(call) if call.request == request => TokenVerdict::Replay(...)`, `Some(call) => TokenVerdict::Spent(...)`.
A sibling that hand-rolls the same lookup does compare every member — create_application.rs:229-232: "The retry has to be the SAME request, so the members that identify the application are compared too; a token already spent on a different request collides with the application it made, rather than answering from it."
ExecuteAction models the error such a collision needs: smithy_query ExecuteAction errors include `{"shape": "ConflictingOperationException", "httpError": 409}`.

## Steps to Reproduce

1. CreateComputationModel and wait for it to reach ACTIVE; note its id and one of its action definition ids.
2. ExecuteAction with targetResource = that computation model, that actionDefinitionId, some actionPayload and clientToken = T. Note the returned actionId A1.
3. ExecuteAction again with clientToken = T but a DIFFERENT actionDefinitionId or a different actionPayload.
4. Observe 202 with actionId = A1: no new action is recorded and no execution is created (ListActions/ListExecutions still show only the first one), instead of the modelled ConflictingOperationException for a token already spent on another request.

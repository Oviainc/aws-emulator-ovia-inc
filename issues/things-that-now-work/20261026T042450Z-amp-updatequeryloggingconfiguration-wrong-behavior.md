Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b17317436abe1a5dae1bd1120020acb35c8cff22

# An update carrying the clientToken the create used is answered as a replay, so the new destinations are silently dropped

_Filed: 2026-10-26T04:24:50Z_
_Operation: UpdateQueryLoggingConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b17317436abe1a5dae1bd1120020acb35c8cff22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateQueryLoggingConfiguration` stores its `clientToken` in BOTH `create_token` and `client_token`, but `client_token` is documented as the token of the call that last CHANGED the configuration and is the field `UpdateQueryLoggingConfiguration` reads to recognise a replay of ITS own call. So an `UpdateQueryLoggingConfiguration` that carries the same token string the create carried is mistaken for a replay of that update: the handler returns 202 with the stored status and never applies `destinations`. The caller gets a success for a change that was not persisted. The same field is also overwritten by `DeleteQueryLoggingConfiguration` (delete_query_logging_configuration.rs:222), so an update replaying that token is likewise answered 202 instead of the DELETING conflict the handler models immediately below.

The sibling rules-and-alerting logging configuration does NOT have this problem: it keeps the create token and the update token in two fields, and `UpdateLoggingConfiguration` only ever matches the update one.

## Evidence

state.rs:775-778 documents the field the update replay check reads:
```
    /// The `clientToken` of the call that last CHANGED this configuration (an update or
    /// the start of a deletion). Replaying that exact token is the SAME request, so a
    /// handler answers from the stored record instead of applying the change twice.
    pub client_token: Option<String>,
```
But the CREATE writes it too — src/ops/create_query_logging_configuration.rs:229-230:
```
            create_token: input.client_token.clone(),
            client_token: input.client_token,
```
and src/ops/update_query_logging_configuration.rs:197-204 reads exactly that field, returning without applying the request:
```
        // `clientToken` carries `@idempotencyToken`: replaying the very call that last
        // changed this configuration is that same call, so it answers from the stored
        // record instead of applying the change twice (and restarting its span).
        if input.client_token.is_some() && configuration.client_token == input.client_token {
            return Ok(UpdateQueryLoggingConfigurationOutput {
                status: crate::state::query_logging_status(configuration),
            });
        }
```
The sibling keeps the two apart — state.rs:708-712:
```
    /// The `clientToken` of the call that last UPDATED it. Replaying that exact token is
    /// the SAME request, so `UpdateLoggingConfiguration` answers from the stored record
    /// instead of applying the change twice. Kept apart from [`Self::client_token`] for
    /// the same reason `QueryLoggingConfiguration` keeps its create token apart.
    pub update_token: Option<String>,
```
and src/ops/update_logging_configuration.rs:214 checks `logging.update_token == input.client_token`, never the create token.

Test coverage: tests/all/query_logging_configuration.rs:324-335 only ever updates under fresh tokens (`format!("update-{index}")`), and lines 537-566 only check the reverse direction (a create token replayed after an update). No test sends an update under the create's token, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /workspaces/{workspaceId}/logging/query with {"destinations": [... logGroupArn A, qspThreshold 100], "clientToken": "tok-1"} — 202, configuration created with destination A
2. PATCH/PUT the update route /workspaces/{workspaceId}/logging/query with {"destinations": [... logGroupArn B, qspThreshold 7], "clientToken": "tok-1"}
3. Observe 202 with the stored status
4. GET /workspaces/{workspaceId}/logging/query — the destinations are still A/100: the update was answered successfully but persisted nothing

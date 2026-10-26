Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b17317436abe1a5dae1bd1120020acb35c8cff22

# A create while the stored logging configuration is DELETING is answered 202 as a replay, and the record is then removed, so the create persists nothing

_Filed: 2026-10-26T04:24:50Z_
_Operation: CreateLoggingConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b17317436abe1a5dae1bd1120020acb35c8cff22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLoggingConfiguration` treats a call as a replay of the create whenever the stored configuration carries the same `clientToken` OR the same `logGroupArn`, without looking at the stored configuration's status. A configuration whose `DeleteLoggingConfiguration` is still pending reports `DELETING` and is removed by `materialize_due` once its `transition_due` passes. So a create sent in that window is answered 202 (reporting status `DELETING`) and nothing is created: moments later the workspace holds no logging configuration at all and `DescribeLoggingConfiguration` answers 404, although the caller's create was accepted.

The crate's own sibling create for the other single-valued workspace child excludes a record on its way out from the replay path for exactly this reason.

## Evidence

src/ops/create_logging_configuration.rs:189-202 — the replay test never consults the stored status:
```
        if let Some(existing) = workspace.logging_configuration.as_ref() {
            ...
            let replays_create = (input.client_token.is_some()
                && existing.client_token == input.client_token)
                || existing.log_group_arn == input.log_group_arn;
            if replays_create {
                return Ok(CreateLoggingConfigurationOutput {
                    status: crate::state::logging_configuration_status(existing),
                });
            }
```
The sibling does — src/ops/create_alert_manager_definition.rs:231-242:
```
            // `clientToken` carries `@idempotencyToken`: re-sending the very call that
            // created this definition — the same token AND the same document — is that
            // same call, so it answers with the definition it already created, as long as
            // that definition is still the record that call left.
            if input.client_token.is_some()
                && existing.create_token == input.client_token
                && existing.data == input.data
                && existing.status_code != crate::state::DEFINITION_STATUS_DELETING
            {
                return Ok(response_for(existing));
            }
```
That the DELETING record is really dropped is state.rs:996-1002 (`workspace.logging_configuration = None` once `LoggingConfiguration::materialize_due` reports the removal, state.rs:723-733).

Test coverage: tests/all/logging_configuration.rs:412-460 pins the replay rows only for a configuration that is ACTIVE; tests/all/logging_configuration_deletion.rs covers delete, replayed delete and the post-removal 404, but no test issues a CREATE while the removal is owed, so nothing pins this path.

## Steps to Reproduce

1. POST /workspaces/{workspaceId}/logging with {"logGroupArn": A} and wait for the configuration to settle to ACTIVE
2. DELETE /workspaces/{workspaceId}/logging — 202, the record reports DELETING with a removal owed
3. Immediately POST /workspaces/{workspaceId}/logging with {"logGroupArn": A} again — answered 202 with status DELETING, nothing created
4. Wait past the owed transition, then GET /workspaces/{workspaceId}/logging — 404 ResourceNotFoundException: the accepted create left the workspace with no logging configuration

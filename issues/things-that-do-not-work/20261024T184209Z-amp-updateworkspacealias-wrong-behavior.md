Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# UpdateWorkspaceAlias never puts the workspace in UPDATING, so the modelled transient status is unobservable

_Filed: 2026-10-24T18:42:09Z_
_Operation: UpdateWorkspaceAlias | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`WorkspaceStatusCode` models `UPDATING` — "Workspace is being updated. Updates are allowed only when status is ACTIVE." — which is the status a workspace carries while an in-flight update settles; the real service accepts the rename asynchronously (the operation is modelled 204/`@idempotent` on an eventually-consistent resource) and the workspace reads back `UPDATING` before returning to `ACTIVE`.

Expected: `UpdateWorkspaceAlias` on an ACTIVE workspace moves its status to `UPDATING`, with the clock owing the move back to `ACTIVE` (exactly the shape this crate already uses for `UpdateWorkspaceConfiguration`, `PutRuleGroupsNamespace`, `UpdateLoggingConfiguration` and `PutResourcePolicy`), so a DescribeWorkspace immediately after the rename reports `UPDATING` and a second concurrent rename is refused `ConflictException`.

Actual: `src/ops/update_workspace_alias.rs:224` writes the new alias and returns; the workspace status is left at `ACTIVE` and nothing in the crate ever sets a workspace to `UPDATING` (`src/state.rs:31-35` declares only `CREATING`/`ACTIVE`/`DELETING` for a workspace). The rename is instantaneous and `WorkspaceStatusCode$UPDATING` is dead in the emulator, so a client that polls for UPDATING → ACTIVE after a rename, or that expects its second rename to conflict, sees behaviour the real service does not produce.

## Evidence

Spec: `com.amazonaws.amp#WorkspaceStatusCode$UPDATING` doc ("Workspace is being updated. Updates are allowed only when status is ACTIVE.") — a status only a workspace update can produce, and no other operation in the API writes a workspace status. Handler: `src/ops/update_workspace_alias.rs:200-226` checks ACTIVE then assigns the alias with no status/`transition_due` write; `src/state.rs:19-35` enumerates the three statuses this crate uses, omitting UPDATING. Tests: `tests/all/workspace_lifecycle.rs:853-871` asserts only the 204 and the new alias; no test asserts the post-rename status, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /workspaces with {"alias":"before"} and wait until DescribeWorkspace reports ACTIVE.
2. POST /workspaces/{workspaceId}/alias with {"alias":"after"} — 204.
3. GET /workspaces/{workspaceId} immediately: expected status.statusCode = UPDATING (settling to ACTIVE shortly after), actual ACTIVE.
4. Immediately issue a second POST /workspaces/{workspaceId}/alias with a different alias and a different clientToken: expected ConflictException while the first update is in flight, actual 204.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# PutLoggingOptions stores a logging scope for a workspaceName no workspace holds, skipping the existence check every other workspace-scoped write makes

_Filed: 2026-10-29T00:26:49Z_
_Operation: PutLoggingOptions | Class: validation_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PutLoggingOptions passes `input.workspace_name` straight into `set_logging_options` without ever asking whether that workspace exists. A put naming a workspace nobody created is accepted (200) and a logging scope is inserted under that name, which `DescribeLoggingOptions` on the same name then reports back as a configured level. Every other workspace-scoped operation of this crate resolves the workspace first against `store.workspaces` and refuses an unknown one with the modelled `ResourceNotFoundException` — 41 call sites, including the creates of tasks, pipelines, applications, datasets, enrichment jobs, searches and queries. PutLoggingOptions models that same error (404). The state module is explicit that a workspace logging scope belongs to the workspace and dies with it, so a scope that belongs to no workspace is state nothing can ever reach or clean up.

## Evidence

Handler writes without resolving the workspace — services/.../src/ops/put_logging_options.rs:172-180:
```
        // The whole setting is replaced, for the named scope only: ...
        state.lock().set_logging_options(
            input.workspace_name.as_deref(),
            input.logging_options,
            ::std::time::SystemTime::now(),
        );
```
(no `store.workspaces.contains_key(...)` anywhere in the file; grep for `workspace` in put_logging_options.rs matches only lines 6, 55-56 of the docs and line 177.)

The rule it skips, stated by a sibling — create_task.rs:245-258:
```
        // The task is created IN the workspace the path names, so a workspace this
        // store holds no record of is the same absence every other workspace-scoped
        // operation of this service reports ...
        if !store.workspaces.contains_key(&input.workspace_name) {
            return Err(... ResourceNotFoundException { message: crate::state::not_found_message(crate::state::RESOURCE_WORKSPACE, &input.workspace_name) } ...);
        }
```
And the state module treats a workspace logging scope as owned by the workspace — state.rs:5605-5617: "Forget a workspace and everything keyed off it: ... and the logging scope set for it. Each of those is addressable ONLY inside the workspace, so a workspace this service no longer holds can leave none of them behind for a later read" (implemented at state.rs:5634 `self.logging_options.remove(workspace_name);`).
The modelled error is available — smithy_query PutLoggingOptions errors include `{"shape": "ResourceNotFoundException", "httpError": 404}`.

## Steps to Reproduce

1. Do NOT create any workspace named "ghost".
2. PutLoggingOptions with workspaceName = "ghost" and loggingOptions.level = ERROR.
3. Observe HTTP 200 instead of the modelled ResourceNotFoundException naming the absent workspace.
4. DescribeLoggingOptions?workspaceName=ghost returns level ERROR — a configured logging scope for a workspace that does not exist, which no DeleteWorkspace can ever sweep.

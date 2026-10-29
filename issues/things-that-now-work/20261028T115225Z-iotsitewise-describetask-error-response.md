Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# DescribeTask skips the workspace-existence check its sibling listing documents as the rule for every workspace-scoped read

_Filed: 2026-10-28T11:52:25Z_
_Operation: DescribeTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeTask addresses a task by (workspaceName, taskName) through `task_key` and refuses with a 404 naming the TASK. It never resolves the workspace, so a request naming a workspace this store does not hold is reported as a missing task instead of a missing workspace. The crate's sibling workspace-scoped reads state the opposite rule in a comment and implement it: ListTasks resolves the workspace first and reports the workspace as the absent resource, and the crate's refusal-message test pins that behaviour for other workspace-scoped entry points (ListTimeSeries, DeleteTimeSeries). CreateTask resolves the workspace too, so a task can only ever be stored under a workspace the registry holds — which is exactly why the read should report the workspace. UpdateTask (src/ops/update_task.rs:164-169) has the same shape.

## Evidence

The rule, stated in a sibling of the same resource:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_tasks.rs:164-175
    "// The listing is scoped to ONE workspace, so that workspace has to exist: a name this service does not hold is the modelled refusal rather than an empty page... Resolved BEFORE the paging arguments are read, as every workspace-scoped read in this crate resolves the resource its path names first, so a request that is wrong in both ways reports the resource that is absent.
     if !store.workspaces.contains_key(&input.workspace_name) { return Err(not_found(crate::state::not_found_message(crate::state::RESOURCE_WORKSPACE, &input.workspace_name))); }"

The handler that skips it:
  src/ops/describe_task.rs:172-179
    "let key = crate::state::task_key(&input.workspace_name, &input.task_name);
     let store = state.lock();
     let record = store.tasks.get(&key).ok_or_else(|| not_found(&input.task_name))?;"
  src/ops/describe_task.rs:208-212 — "message: crate::state::not_found_message(crate::state::RESOURCE_TASK, task_name)"

The same crate applies the workspace check on the create of the resource (src/ops/create_task.rs:192 "if !store.workspaces.contains_key(&input.workspace_name) {") and pins the "absent workspace is reported as the workspace" answer for other workspace-scoped entry points in tests/all/refusal_message_rule.rs:274-287 ("ListTimeSeries, a workspace name" / "DeleteTimeSeries, a workspace name" both expect "Workspace with ID {UNKNOWN_WORKSPACE} not found").

Tests for this operation (tests/all/describe_task_reads.rs, tests/all/task_lifecycle.rs) do not exercise an unknown workspace, so nothing pins the current wording.

## Steps to Reproduce

1. CreateWorkspace "ws" and CreateTask "t" in it.
2. GET /workspaces/no-such-ws/tasks/t (DescribeTask).
3. Observed: 404 ResourceNotFoundException "Task with ID t not found".
4. GET /workspaces/no-such-ws/tasks (ListTasks) for the same unknown workspace: 404 ResourceNotFoundException "Workspace with ID no-such-ws not found" — the two reads of the same workspace disagree about which resource is absent.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# DeleteWorkflow removes the workflow synchronously, so the modeled DELETING status is never observable

_Filed: 2026-10-24T00:24:00Z_
_Operation: DeleteWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: DeleteWorkflow starts an asynchronous teardown ('Amazon Managed Workflows for Apache Airflow Serverless ensures that all associated resources are properly cleaned up, including stopping any running executions...'), and the workflow reports WorkflowStatus=DELETING ('workflow is being deleted') while that cleanup runs — which is why the WorkflowStatus enum carries only READY and DELETING. A client that deletes and then describes/lists sees DELETING before the resource disappears.

Actual: the handler removes the record from `store.workflows` inside the same request (plus its name index and idempotency tokens), so the workflow goes straight from READY to absent. `WORKFLOW_STATUS_READY` is the only status any handler ever writes — DELETING is unreachable in this emulator — and the immediately following GetWorkflow/ListWorkflows report the workflow as already gone. A caller or SDK waiter that polls for the DELETING phase (or expects a describe to still succeed right after the delete call) sees behavior AWS does not produce.

## Evidence

Smithy model: WorkflowStatus enum = READY|DELETING with DELETING documented as 'workflow is being deleted'; DeleteWorkflow doc describes a cleanup process over associated resources. Handler: src/ops/delete_workflow.rs lines ~271-277 (`store.workflows.remove(&arn)` / `workflow_names.remove` / `forget_tokens_for`) — no status transition. State: src/state.rs defines only `WORKFLOW_STATUS_READY`; grep for DELETING in the crate's src and tests returns no matches. Test coverage: tests/all/workflow_lifecycle.rs `delete_removes_the_workflow_its_versions_and_its_runs` asserts immediate disappearance; nothing asserts a DELETING phase.

## Steps to Reproduce

1. CreateWorkflow and note the WorkflowArn.
2. Call DeleteWorkflow for that ARN (no WorkflowVersion).
3. Immediately call GetWorkflow (and ListWorkflows) for the same ARN.
4. Observe the lookup miss (ResourceNotFoundException) right away and no response anywhere reporting WorkflowStatus=DELETING, where the real service reports the workflow in DELETING until cleanup finishes.

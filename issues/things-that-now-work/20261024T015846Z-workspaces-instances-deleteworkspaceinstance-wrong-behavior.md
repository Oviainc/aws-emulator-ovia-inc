Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# Deletion is synchronous: an instance never passes through DEALLOCATING, so the modelled deallocation states are unreachable

_Filed: 2026-10-24T01:58:46Z_
_Operation: DeleteWorkspaceInstance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteWorkspaceInstance removes the record from the store inside the request, so the very next GetWorkspaceInstance answers ResourceNotFoundException and ListWorkspaceInstances no longer carries the instance.

Expected (real service): deletion of a WorkSpace Instance is asynchronous teardown. The call returns 200 and the instance moves to ProvisionState DEALLOCATING (and, on failure, ERROR_DEALLOCATING / DEALLOCATED) before it stops being reported; a Get immediately after the Delete reports the instance with a deallocation state rather than a 404, and ListWorkspaceInstances with ProvisionStates=["DEALLOCATING"] can select it.

Actual: the handler does `state.instances.remove(...)` and returns. No record ever holds DEALLOCATING, DEALLOCATED, ERROR_ALLOCATING or ERROR_DEALLOCATING — the only transition the emulator models is ALLOCATING -> ALLOCATED (state.rs `WorkspaceInstanceRecord::settle`, which only flips ALLOCATING to ALLOCATED). Four of the six modelled ProvisionStateEnum values are therefore unreachable, and the ProvisionStates filter can never match them.

## Evidence

Spec: ProvisionStateEnum models ALLOCATED | ALLOCATING | DEALLOCATED | DEALLOCATING | ERROR_ALLOCATING | ERROR_DEALLOCATING (smithy_query shape=ProvisionStateEnum); the DEALLOC* values can only be produced by the delete path, and DeleteWorkspaceInstance's doc is an asynchronous teardown ("Usage of this API will result in deletion of the resource in question"). Handler: src/ops/delete_workspace_instance.rs lines ~151-169 removes the record outright. State machine: src/state.rs `settle()` only performs ALLOCATING -> ALLOCATED, and `PROVISION_STATE_*` constants only exist for those two. Tests pin the current behaviour rather than the service's: tests/all/workspace_instance_lifecycle.rs `delete_removes_the_instance_from_every_view`, `provision_states_filters_on_the_stored_state` (expects an empty page for ProvisionStates=["DEALLOCATING","DEALLOCATED"]), and tests/all/wire_error_responses.rs `a_deleted_instance_is_answered_the_same_way_as_one_that_never_existed`.

## Steps to Reproduce

1. POST / with X-Amz-Target: EUCMIFrontendAPIService.CreateWorkspaceInstance and body {"ManagedInstance":{"InstanceType":"t3.medium"}} — note the returned WorkspaceInstanceId
2. Call DeleteWorkspaceInstance with that id (200, empty body)
3. Immediately call GetWorkspaceInstance with that id: the emulator answers ResourceNotFoundException instead of reporting the instance with ProvisionState DEALLOCATING
4. Call ListWorkspaceInstances with {"ProvisionStates":["DEALLOCATING"]}: always an empty list, because no code path can ever store a deallocation state

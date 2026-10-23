Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: f76fdd8ab8ab91800165900d63e48ff1fef7e5fc

# DeleteDeployment reports DELETE_IN_PROGRESS immediately, so the modeled DELETE_INITIATING status is never observable

_Filed: 2026-10-23T23:09:15Z_
_Operation: DeleteDeployment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f76fdd8ab8ab91800165900d63e48ff1fef7e5fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A delete of a live deployment sets the record straight to DELETE_IN_PROGRESS and returns that value as the operation's `status`. Expected (real Launch Wizard, and the reason the modeled DeploymentStatus enum carries both values): the synchronous DeleteDeployment response reports the just-initiated state, DELETE_INITIATING, and the deployment then advances to DELETE_IN_PROGRESS before reaching DELETED. Actual: DELETE_INITIATING is unreachable from every code path in this crate — it appears only inside the `is_deleting_or_deleted()` predicate — so a client polling GetDeployment/ListDeployments after a delete can never observe it, and the first status both the delete response and the next read report is DELETE_IN_PROGRESS.

## Evidence

Spec: Smithy model — DeploymentStatus enumerates DELETE_INITIATING, DELETE_IN_PROGRESS, DELETE_FAILED and DELETED as distinct members, and DeleteDeploymentOutput$status is documented as the status of the deployment.
Handler: src/ops/delete_deployment.rs lines 170-204 — `deployment.status = crate::state::STATUS_DELETE_IN_PROGRESS` is assigned on the first delete and returned as the output status; the only clock-owed step scheduled afterwards is DELETED (lines 186-198), so no step ever yields DELETE_INITIATING. src/state.rs defines STATUS_DELETE_INITIATING (line 45) but it is referenced only by Deployment::is_deleting_or_deleted (lines 492-497).
Tests: tests/all/deployment_lifecycle.rs::delete_is_idempotent_and_observable_from_every_reader asserts the emulator's current DELETE_IN_PROGRESS → DELETED sequence, so nothing asserts the DELETE_INITIATING step.

## Steps to Reproduce

1. Create a deployment and wait for it to reach a non-deleting status.
2. Call DeleteDeployment(deploymentId=<id>) and read the returned `status` — expect DELETE_INITIATING; observe DELETE_IN_PROGRESS.
3. Immediately call GetDeployment(deploymentId=<id>) and observe the stored status is also DELETE_IN_PROGRESS, so DELETE_INITIATING is never visible to any reader.

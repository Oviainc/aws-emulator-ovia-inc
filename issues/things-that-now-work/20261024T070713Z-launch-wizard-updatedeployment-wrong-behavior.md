Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 43c3b52fd22125f2604949f65e29a8c5612a98db

# UpdateDeployment accepts an update of a deployment whose create/update is still in flight and silently cancels that lifecycle

_Filed: 2026-10-24T07:07:13Z_
_Operation: UpdateDeployment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `43c3b52fd22125f2604949f65e29a8c5612a98db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateDeployment guards only against a deployment that is going away: `if deployment.is_deleting_or_deleted()` -> ValidationException. Every other status is accepted, including the non-terminal CREATING / IN_PROGRESS (a deployment that is still being created) and UPDATE_IN_PROGRESS (an update already under way).

Worse, the accepted update REPLACES the deployment's queue of clock-owed status steps (`deployment.pending = VecDeque::from(vec![UPDATE_COMPLETED step])`), so the steps the create still owed (IN_PROGRESS, then COMPLETED) are discarded. A deployment created and then updated before its create finished therefore walks CREATING -> UPDATE_IN_PROGRESS -> UPDATE_COMPLETED and NEVER reports the COMPLETED status its creation was heading for; a poller that was waiting for the creation to finish sees the create silently cancelled.

Expected: a control-plane resource that is mid-provision (or mid-update) does not accept a configuration update — the request is refused with the operation's modeled ValidationException (HTTP 400), leaving the in-flight lifecycle intact, exactly the way this handler already refuses an update of a deployment whose delete has started. Actual: HTTP 200, the record's settings and versions are overwritten, the status is forced to UPDATE_IN_PROGRESS and the pending create steps are dropped.

## Evidence

Spec/semantics: the model declares the UPDATE_* statuses as a separate lifecycle from the create pair (DeploymentStatus: CREATING/IN_PROGRESS/COMPLETED vs UPDATE_IN_PROGRESS/UPDATE_COMPLETED/UPDATE_FAILED/UPDATE_ROLLBACK_*) and UpdateDeployment declares ValidationException (HTTP 400); refusing a mutation of a resource that is still being created is standard AWS control-plane behavior (CloudFormation UpdateStack on CREATE_IN_PROGRESS, RDS/ECS/EKS equivalents all refuse).
Handler: services/service-com-amazonaws-launchwizard-launchwizard/src/ops/update_deployment.rs — the only state guard is `is_deleting_or_deleted()` (around lines 205-215, whose own comment explains that a deployment with no settings left to apply is refused rather than updated); the pending-step queue is then unconditionally replaced (around lines 327-339), discarding whatever the create or a previous update still owed. `crate::state::Deployment::is_deleting_or_deleted` (src/state.rs) matches only DELETE_INITIATING / DELETE_IN_PROGRESS / DELETED, so CREATING, IN_PROGRESS and UPDATE_IN_PROGRESS all fall through to the success path.
Tests: tests/all/deployment_update.rs::an_update_of_a_deployment_that_is_absent_or_going_away_is_refused covers only the absent and deleting cases; no test exercises an update while the create is still in flight, so nothing pins the current acceptance as intended.

## Steps to Reproduce

1. Call CreateDeployment(workloadName='SAP', deploymentPatternName='SapHanaSingle', name='mid-flight', specifications={<the schema the pattern publishes>}, dryRun=false) and note the deploymentId.
2. Immediately (before the create's clock-owed steps come due) call GetDeployment and confirm the status is still the non-terminal CREATING.
3. Call UpdateDeployment(deploymentId=<id>, specifications={<same map>}, dryRun=false, force=false).
4. Expect the update to be refused with ValidationException (HTTP 400) because the deployment is still being created; observe HTTP 200 with deployment.status = UPDATE_IN_PROGRESS.
5. Poll GetDeployment until the status settles and observe it reaches UPDATE_COMPLETED without ever passing through IN_PROGRESS/COMPLETED — the creation lifecycle was discarded; ListDeploymentEvents shows no DeploymentInProgress/DeploymentCompleted event.

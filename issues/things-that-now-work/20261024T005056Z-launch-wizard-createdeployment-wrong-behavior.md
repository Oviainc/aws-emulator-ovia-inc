Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# A created deployment never reports the modeled VALIDATING status, so the pre-provisioning phase is unobservable

_Filed: 2026-10-24T00:50:56Z_
_Operation: CreateDeployment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateDeployment schedules the whole status lifecycle of the new deployment: the record is stored in CREATING and the handler queues exactly two clock-owed steps, IN_PROGRESS then COMPLETED. Launch Wizard validates a deployment's settings before it provisions anything, and `DeploymentStatus` models that phase as `VALIDATING`; a real deployment therefore passes through VALIDATING between being accepted and resources being created, which is what a client polling GetDeployment/ListDeployments (or filtering ListDeployments with `DEPLOYMENT_STATUS=VALIDATING`) observes.

Expected: the status sequence a created deployment walks includes the modeled VALIDATING step before IN_PROGRESS, with a matching deployment event, so that a poller and a DEPLOYMENT_STATUS filter can ever see it.

Actual: VALIDATING is never written to any deployment by any code path — `state.rs` does not even define a constant for it (only CREATING, IN_PROGRESS, COMPLETED, DELETE_INITIATING, DELETE_IN_PROGRESS, DELETED) — so ListDeployments(filters=[{name:'DEPLOYMENT_STATUS', values:['VALIDATING']}]) can only ever return an empty list and ListDeploymentEvents never records a validation event.

## Evidence

Spec: Smithy model `com.amazonaws.launchwizard#DeploymentStatus` enum includes `VALIDATING` alongside CREATING/IN_PROGRESS/COMPLETED, and it is a value `DeploymentData$status`, `DeploymentDataSummary$status` and the `DEPLOYMENT_STATUS` DeploymentFilter can carry; CreateDeployment's `dryRun` doc and GetWorkloadDeploymentPattern's published specification schema exist because Launch Wizard validates deployment settings before deploying.
Handler: services/service-com-amazonaws-launchwizard-launchwizard/src/ops/create_deployment.rs lines 272-306 build the pending transition queue with exactly `STATUS_IN_PROGRESS` then `STATUS_COMPLETED` and store the record with `STATUS_CREATING`; src/state.rs lines 37-47 define no `STATUS_VALIDATING` constant, and `Deployment::materialize_due` (lines 471-483) only applies queued steps, so no other path can produce it.
Tests: tests/all/deployment_lifecycle.rs::a_created_deployment_is_read_back_from_state_and_advances_on_the_clock (lines 157-272) asserts exactly three events (create + two steps) and the CREATING→IN_PROGRESS→COMPLETED walk, so no test asserts the VALIDATING phase.
Parallel: the already-filed DeleteDeployment/wrong-behavior issue covers the delete half (DELETE_INITIATING never observable); this is the create half and a different operation + class pair.

## Steps to Reproduce

1. Boot the emulator and call CreateDeployment(workloadName='SAP', deploymentPatternName='SapHanaSingle', name='status-walk', specifications={...}, dryRun=false).
2. Poll GetDeployment on the returned deploymentId every ~100ms until it reports COMPLETED and record every distinct status seen.
3. Expect the recorded sequence to include the modeled VALIDATING phase before IN_PROGRESS; observe only CREATING, IN_PROGRESS, COMPLETED.
4. Call ListDeployments(filters=[{name:'DEPLOYMENT_STATUS', values:['VALIDATING']}]) at any point during the walk and observe it always returns an empty list.
5. Call ListDeploymentEvents on the deployment and observe no validation event is recorded (only DeploymentCreated / DeploymentInProgress / DeploymentCompleted).

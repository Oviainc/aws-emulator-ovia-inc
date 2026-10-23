Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: f76fdd8ab8ab91800165900d63e48ff1fef7e5fc

# GetDeployment never reports resourceGroup, so DeploymentData omits the resource group a completed deployment owns

_Filed: 2026-10-23T23:09:15Z_
_Operation: GetDeployment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f76fdd8ab8ab91800165900d63e48ff1fef7e5fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeploymentData$resourceGroup is modeled as "The resource group of the deployment". Real Launch Wizard creates a Resource Groups group for each deployment (named after the deployment, `LaunchWizard-<deploymentName>`) and reports it on GetDeployment once the deployment has progressed past creation. In the emulator the field is hardcoded absent for every deployment at every status: `Deployment::resource_group` is initialised to None by CreateDeployment and no code path ever assigns it, so a client that reads `deployment.resourceGroup` after a deployment reaches COMPLETED finds the member missing rather than a group name derived from the deployment it just created.

## Evidence

Spec: Smithy model — DeploymentData$resourceGroup (String), doc "The resource group of the deployment"; GetDeployment returns DeploymentData.
Handler/state: src/ops/get_deployment.rs lines 151-167 reports `deployment.to_data()`; src/state.rs Deployment::to_data line 510 passes `resource_group` straight through, and the field is set to `None` at creation (src/ops/create_deployment.rs line 309) with no later assignment anywhere in the crate (grep: `resource_group` appears only in the struct definition, to_data, and that one `None`).
Tests: tests/all/deployment_lifecycle.rs asserts the deployment projection it expects but never asserts a resourceGroup value, so no test pins the current absence as intended.

## Steps to Reproduce

1. Create a deployment via CreateDeployment and wait until GetDeployment reports status COMPLETED.
2. Call GetDeployment(deploymentId=<id>) and inspect the `deployment` structure — expect a `resourceGroup` naming the group created for the deployment; observe the member is absent at every status, including COMPLETED.

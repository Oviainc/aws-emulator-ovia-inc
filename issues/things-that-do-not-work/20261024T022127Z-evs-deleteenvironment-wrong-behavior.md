Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# Deleting an environment does not cascade to its connectors and entitlements, which survive as live records

_Filed: 2026-10-24T02:21:27Z_
_Operation: DeleteEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteEnvironment "Deletes an Amazon EVS environment" and the doc is explicit that the environment's dependent resources go away with it ("Environment deletion also deletes the associated Amazon EVS VLAN subnets"). A connector and a VM entitlement are sub-resources of the environment (`Connector$environmentId`, `VmEntitlement$environmentId`), and the model carries the terminal states for exactly this: `ConnectorState` includes `DELETING`/`DELETED` and `EntitlementStatus` includes `DELETED` and `ENTITLEMENT_REMOVED`.

Expected: once DeleteEnvironment is accepted, the environment's connectors and entitlements stop being live — they move to their terminal states as the environment moves to DELETING/DELETED (or the lists against the deleted environment answer the not-found refusal the `ResourceNotFoundException` doc sanctions, "it may have a `state` of `DELETED`").

Actual: the handler stamps only the environment record and its VLANs. The `connectors`, `entitlements` and `hosts` maps are left exactly as they were, and the list operations do not filter a DELETED environment, so after the deletion settles ListEnvironmentConnectors still reports the connectors with state ACTIVE and modifiedAt from before the delete, and ListVmEntitlements still reports CREATED entitlements with a `startedAt` (i.e. usage tracking that never stopped) for an environment GetEnvironment reports as DELETED. The create/update paths DO treat a DELETED environment as gone (create_environment_connector.rs line 241, create_entitlement.rs line 214, update_environment_connector.rs line 221), so the two surfaces contradict each other.

## Evidence

Spec: DeleteEnvironment doc (environment deletion also deletes associated resources), `ConnectorState` = CREATING|CREATE_FAILED|ACTIVE|UPDATING|UPDATE_FAILED|DELETING|DELETED and `EntitlementStatus` = CREATING|CREATED|DELETED|AT_RISK|ENTITLEMENT_REMOVED|CREATE_FAILED (`smithy_query(query="shape", shape="ConnectorState,EntitlementStatus")`), plus `ResourceNotFoundException` doc ("it may have a `state` of `DELETED`"). Handler: src/ops/delete_environment.rs lines 217-230 — only `environment.state`, `modified_at_ms` and each VLAN are changed; nothing touches `environment.connectors` / `environment.entitlements`. src/ops/list_environment_connectors.rs lines 208-214 and src/ops/list_vm_entitlements.rs lines 206-212 resolve the environment with `environment_in_region` and no DELETED filter. Tests: tests/all/environment_lifecycle.rs lines 486-537 cover the environment's own DELETING→DELETED walk and tests/all/connectors.rs lines 813-823 cover create/update refusals in a DELETED environment, but no test lists the connectors or entitlements of a deleted environment.

## Steps to Reproduce

1. Create an environment, wait for CREATED, then CreateEnvironmentConnector (type=VCENTER) and CreateEntitlement (entitlementType=WINDOWS_SERVER, vmIds=["vm-1"]) in it.
2. Wait for the connector to reach ACTIVE and the entitlement to reach CREATED.
3. Call DeleteEnvironment and wait until GetEnvironment reports environmentState=DELETED.
4. Call ListEnvironmentConnectors for that environmentId — the connector is still reported with state ACTIVE instead of DELETED (or a not-found refusal).
5. Call ListVmEntitlements for the same environmentId and connectorId — the entitlement is still CREATED with a startedAt and no stoppedAt.

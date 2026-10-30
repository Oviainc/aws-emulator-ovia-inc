Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ee0cc881641f9455cbcb1e70f91e773fdf19df64

# A logging configuration is created on a workspace whose deletion is already owed, so the call is accepted (202) and the store then holds nothing for it

_Filed: 2026-10-29T15:00:24Z_
_Operation: CreateLoggingConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `ee0cc881641f9455cbcb1e70f91e773fdf19df64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLoggingConfiguration` resolves the `{workspaceId}` label and never looks at the workspace's STATUS. A workspace whose `DeleteWorkspace` has been accepted is still in the store while its removal is owed (it reports `DELETING`), so a create in that window is accepted with 202 and a `CREATING` configuration is written onto it. The clock then drops the workspace and takes the configuration with it, so moments later `DescribeLoggingConfiguration` answers 404 for a create that was accepted — the accepted-call-that-persisted-nothing outcome this crate's own suite says nothing does, and which was already filed and fixed for the twin case (a create while the CONFIGURATION's own removal was owed).

Every other handler in this crate that adds a child to a workspace refuses this: `CreateAnomalyDetector` refuses a `DELETING` workspace with `ConflictException`, and `CreateAlertManagerDefinition` and `CreateRuleGroupsNamespace` refuse any workspace that is not `ACTIVE`. The two mutators of this very record also gate on the workspace (`crate::state::logging_mutation_allowed` requires the workspace to be `ACTIVE`), so only the create skips the parent's status. The handler already owns a refusal shape for exactly this class of state conflict (`crate::state::request_validation_failed`, used two lines above for the DELETING-configuration case), so nothing new has to be invented.

## Evidence

THE HANDLER NEVER READS THE WORKSPACE'S STATUS — src/ops/create_logging_configuration.rs:177-195 (the only use of the workspace, and the record is written at :255):
```
        // The configuration belongs to a workspace (`/workspaces/{workspaceId}/logging`),
        // so an id naming no stored workspace is the modelled 404 — as is a Region this
        // account holds nothing in — ...
        let Some(workspace) = store
            .held_region_mut(&region)
            .and_then(|store| store.workspaces.get_mut(&input.workspace_id))
        else { ... };
```
```
255:        workspace.logging_configuration = Some(logging);
```
THE SIBLING CHILD-CREATE REFUSES IT — src/ops/create_anomaly_detector.rs:272-286:
```
        // Adding a child to a workspace whose deletion has already started would leave
        // state that cannot hold together — which is what `ConflictException` (409) is
        // modelled for ("The request would cause an inconsistent state").
        if workspace.status_code == crate::state::STATUS_DELETING {
```
and src/ops/create_alert_manager_definition.rs:216-236 (`if workspace.status_code != crate::state::STATUS_ACTIVE`, "Adding a definition to a workspace that is still being created, or being deleted, would leave an inconsistent state"), src/ops/create_rule_groups_namespace.rs:277-290 (same gate for a namespace).

THE SAME RECORD'S OWN MUTATORS GATE ON THE WORKSPACE — src/ops/update_logging_configuration.rs:232 and src/ops/delete_logging_configuration.rs:250 both call `crate::state::logging_mutation_allowed(&workspace_status, &logging.status_code)`, defined at state.rs:1360-1362 as `workspace_status == STATUS_ACTIVE && logging_status == LOGGING_STATUS_ACTIVE`.

THE OUTCOME THE CRATE SAYS IT NEVER PRODUCES — tests/all/create_while_a_removal_is_owed.rs:6-12:
```
//! The rule these suites pin: an idempotent create answers as a replay only from a
//! stored record that is still the one its original call left, ... — a create in that
//! window either refuses, where the resource is single-valued and the one it would
//! create is being deleted, or creates a new resource, where the resource is keyed or
//! minted.
//! Nothing accepts the call and then leaves the store holding nothing for it.
```
That the record really goes with the workspace — state.rs:1106-1108 and 1123-1128:
```
            if workspace.status_code == STATUS_DELETING {
                // Dropping the workspace drops its logging configuration with it.
                deleted.push(workspace.workspace_id.clone());
```
```
        for workspace_id in deleted {
            self.workspaces.remove(&workspace_id);
```
Test coverage: tests/all/create_while_a_removal_is_owed.rs:150 covers only the configuration's own pending removal; tests/all/logging_configuration.rs:282-307 creates on a workspace that is still CREATING, and no test creates while the WORKSPACE's removal is owed, so nothing pins this path.

## Steps to Reproduce

1. POST /workspaces (CreateWorkspace) and wait for the workspace to report ACTIVE
2. DELETE /workspaces/{workspaceId} — 202, the workspace now reports DELETING with its removal owed on the clock
3. Immediately POST /workspaces/{workspaceId}/logging with {"logGroupArn": "arn:aws:logs:...:log-group:/aws/prometheus/x:*"} — answered 202 with statusCode CREATING
4. Wait past the owed transition
5. GET /workspaces/{workspaceId}/logging — 404: the accepted create left the store holding nothing for it (the workspace and its configuration are both gone)

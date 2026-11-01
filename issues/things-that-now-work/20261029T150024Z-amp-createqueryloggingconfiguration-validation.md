Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ee0cc881641f9455cbcb1e70f91e773fdf19df64

# A query logging configuration is created on a workspace whose deletion is already owed, so the call is accepted (202) and the store then holds nothing for it

_Filed: 2026-10-29T15:00:24Z_
_Operation: CreateQueryLoggingConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `ee0cc881641f9455cbcb1e70f91e773fdf19df64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateQueryLoggingConfiguration` only asks whether the Region's store CONTAINS the workspace (`store.workspaces.contains_key`) and never looks at the workspace's status. A workspace whose `DeleteWorkspace` has been accepted is still held while its removal is owed (it reports `DELETING`), so a create in that window is accepted with 202 and a `CREATING` configuration is inserted into `query_logging` under that workspace id. `materialize_due` then removes the workspace and `self.query_logging.remove(&workspace_id)` with it, so `DescribeQueryLoggingConfiguration` answers 404 moments after a create that was accepted.

This is the same hole as on the workspace's other single-valued logging child, and the opposite of what every other child-create in this crate does: `CreateAnomalyDetector` refuses a `DELETING` workspace with `ConflictException`, `CreateAlertManagerDefinition` and `CreateRuleGroupsNamespace` refuse any workspace that is not `ACTIVE`. The handler already owns the refusal shape for this class of state conflict (`crate::state::request_validation_failed`, used fifteen lines above for the case where this configuration's OWN removal is owed), so no new behaviour has to be designed.

## Evidence

THE HANDLER NEVER READS THE WORKSPACE'S STATUS — src/ops/create_query_logging_configuration.rs:178-192 (the only use of the workspace; the record is inserted at :267):
```
        let Some(store) = store
            .held_region_mut(&region)
            .filter(|store| store.workspaces.contains_key(&input.workspace_id))
        else { ... ResourceNotFoundException { message: crate::state::WORKSPACE_NOT_FOUND_MESSAGE ... } };
```
```
267:        store.query_logging.insert(input.workspace_id, configuration);
```
THE SIBLING CHILD-CREATE REFUSES IT — src/ops/create_anomaly_detector.rs:272-286:
```
        // Adding a child to a workspace whose deletion has already started would leave
        // state that cannot hold together — which is what `ConflictException` (409) is
        // modelled for ("The request would cause an inconsistent state").
        if workspace.status_code == crate::state::STATUS_DELETING {
```
(also src/ops/create_alert_manager_definition.rs:222 and src/ops/create_rule_groups_namespace.rs:277, both `!= crate::state::STATUS_ACTIVE`).

THE RECORD IS DROPPED WITH THE WORKSPACE — state.rs:1123-1129:
```
        for workspace_id in deleted {
            self.workspaces.remove(&workspace_id);
            self.client_tokens.retain(|_, id| id != &workspace_id);
            // A query logging configuration belongs to its workspace, so it goes when
            // the workspace does.
            self.query_logging.remove(&workspace_id);
```
THE OUTCOME THE CRATE SAYS IT NEVER PRODUCES — tests/all/create_while_a_removal_is_owed.rs:12:
```
//! Nothing accepts the call and then leaves the store holding nothing for it.
```
and the handler's own refusal for the twin case — src/ops/create_query_logging_configuration.rs:202-211:
```
            if existing.status_code == crate::state::QUERY_LOGGING_STATUS_DELETING {
                return Err(
                    CreateQueryLoggingConfigurationErrorModeled::ValidationException(
                        crate::state::request_validation_failed(
                            crate::state::QUERY_LOGGING_CONFIGURATION_DELETING_MESSAGE,
```
Test coverage: tests/all/create_while_a_removal_is_owed.rs:267 covers only the configuration's own pending removal; no test in tests/all/query_logging_configuration.rs creates while the WORKSPACE's removal is owed.

## Steps to Reproduce

1. POST /workspaces (CreateWorkspace) and wait for the workspace to report ACTIVE
2. DELETE /workspaces/{workspaceId} — 202, the workspace reports DELETING with its removal owed
3. Immediately POST /workspaces/{workspaceId}/logging/query with one `destinations` entry — answered 202 with statusCode CREATING
4. Wait past the owed transition
5. GET /workspaces/{workspaceId}/logging/query — 404: the accepted create left the store holding nothing for it

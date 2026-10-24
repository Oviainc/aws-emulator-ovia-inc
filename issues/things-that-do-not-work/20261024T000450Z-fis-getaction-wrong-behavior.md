Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# GetAction's action catalog holds only ~20 entries, so documented FIS action ids answer ResourceNotFoundException instead of the action

_Filed: 2026-10-24T00:04:50Z_
_Operation: GetAction | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetAction reports from the constant table `state.rs::ACTION_CATALOG`, which the code itself describes as "a SUBSET of the published catalog". Expected (real FIS): GetAction(id) returns the Action (id, arn, description, parameters, targets, tags) for every action FIS publishes — the catalog has dozens of entries. Actual: only the ~20 ids hard-coded in the table resolve; every other published action id answers ResourceNotFoundException (404). Examples of published actions absent from the table: aws:ec2:api-error, aws:eks:pod-delete, aws:eks:pod-cpu-stress, aws:eks:inject-kubernetes-custom-resource, aws:ecs:task-cpu-stress, aws:ecs:task-kill-process, aws:dynamodb:global-table-pause-replication, aws:s3:bucket-pause-replication, aws:elasticache:replicationgroup-interrupt-az-power, aws:network:route-table-disrupt-connectivity, aws:network:transit-gateway-disrupt-cross-region-connectivity, aws:lambda:invocation-add-delay, aws:lambda:invocation-error, aws:rds:failover-db-cluster variants for Multi-AZ clusters. A client that reads an action's parameter/target contract before building a template gets a 404 for an action AWS describes, and CreateExperimentTemplate cannot be validated against the catalog for those ids either.

## Evidence

Smithy model: GetAction -> Action {id, description, parameters: map<string,ActionParameter>, targets: map<string,ActionTarget>, arn, tags}; errors ResourceNotFoundException (404), ValidationException (400). Action is a SERVICE-owned catalog resource: FIS publishes the full list in the FIS User Guide ("Actions reference"), which is considerably larger than the table here.
Handler/state: src/ops/get_action.rs resolves through `crate::state::action_catalog_entry(&input.id)` and answers ResourceNotFoundException on a miss; src/state.rs ACTION_CATALOG holds ~20 entries and its own doc comment states "It is a SUBSET of the published catalog, not the whole of it: an id this table does not hold answers ResourceNotFoundException rather than a fabricated action."
Tests: tests/all/action_catalog.rs asserts GetAction for the rows that ARE in the table (and the ARN region rule); no test asserts coverage of the published catalog.

## Steps to Reproduce

1. Boot the emulator and point a botocore `fis` client at it.
2. Call get_action(id='aws:eks:pod-delete') — observe ResourceNotFoundException (404) instead of 200 with the action's parameters and targets.
3. Repeat with 'aws:lambda:invocation-error', 'aws:dynamodb:global-table-pause-replication' and 'aws:network:route-table-disrupt-connectivity' — all 404.
4. Compare with get_action(id='aws:fis:wait'), which succeeds, showing the catalog is only partially populated.

Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# UpdateTable refuses autoScalingSpecification (and replicaSpecifications / warmThroughputSpecification) with an unmodeled 501 NotImplemented

_Filed: 2026-10-24T00:49:58Z_
_Operation: UpdateTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateTable's handler starts with a guard that answers 501 `NotImplemented` whenever the request carries `autoScalingSpecification`, `replicaSpecifications`, `cdcSpecification` or `warmThroughputSpecification`, and nothing is stored.

Expected (AWS): all four are ordinary optional members of `UpdateTableRequest`, and UpdateTable is THE operation that changes a table's auto scaling after creation ("Adds new columns to the table or updates one of the table's settings, for example capacity mode, auto scaling, encryption at rest, or point-in-time recovery"). A well-formed call that supplies `autoScalingSpecification` on a PROVISIONED table must return HTTP 200 with `resourceArn`, move the table to UPDATING, and leave `GetTableAutoScalingSettings` reporting the NEW minimumUnits / maximumUnits / autoScalingDisabled / targetTrackingScalingPolicyConfiguration.targetValue. UpdateTable models exactly six errors (AccessDenied 403, Conflict 409, InternalServer 500, ResourceNotFound 404, ServiceQuotaExceeded 402, Validation 400) — no not-implemented error of any status, so no SDK branch matches the code the emulator returns.

Actual (emulator): HTTP 501, error code `NotImplemented`, message "UpdateTable is implemented in this emulator, but not its autoScalingSpecification member(s): ..."; the table is unchanged. Because CreateTable now ACCEPTS `autoScalingSpecification` (stored on `crate::state::Table.auto_scaling_specification`) and GetTableAutoScalingSettings reports it, the refusal is a pure asymmetry: auto scaling can be set at create time and read back, but can never be changed, enabled or disabled afterwards. The handler comment justifying the guard is stale — it claims GetTableAutoScalingSettings is "still scaffolded", but that operation is implemented.

Also in scope of this same guard (same root cause, same fix site): `replicaSpecifications` (per-Region capacity overrides for a multi-Region table) and `warmThroughputSpecification`. `cdcSpecification` is already tracked by the open issue "Keyspaces CDC never enabled ..." and is out of scope here.

## Evidence

Spec: Smithy model `UpdateTableRequest` members `autoScalingSpecification` (AutoScalingSpecification), `replicaSpecifications` (ReplicaSpecificationList), `warmThroughputSpecification`; `UpdateTable` error set contains no not-implemented shape; `GetTableAutoScalingSettingsResponse$autoScalingSpecification` is what reports the setting back.
Handler: services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/update_table.rs lines 223-257 (the `unimplemented` member list returning `crate::scaffolded::Scaffolded::NotImplemented`), vs create_table.rs lines 285-289 + 421 which DO resolve and store `autoScalingSpecification`, and get_table_auto_scaling_settings.rs lines 216-235 which report it.
Tests: services/.../tests/all/update_table.rs lines 900-948 (`a_setting_this_slice_cannot_report_is_refused_with_the_scaffold_501`) asserts the 501 for autoScalingSpecification / replicaSpecifications / cdcSpecification / warmThroughputSpecification — i.e. the divergence is locked in by the crate's own tests, and no test asserts the AWS behaviour.

## Steps to Reproduce

1. Boot the emulator and point a boto3 keyspaces client at it.
2. create_keyspace(keyspaceName='ks1')
3. create_table(keyspaceName='ks1', tableName='tbl1', schemaDefinition={'allColumns':[{'name':'pk','type':'text'}],'partitionKeys':[{'name':'pk'}]}, capacitySpecification={'throughputMode':'PROVISIONED','readCapacityUnits':5,'writeCapacityUnits':5}, autoScalingSpecification={'readCapacityAutoScaling':{'autoScalingDisabled':False,'minimumUnits':5,'maximumUnits':50,'scalingPolicy':{'targetTrackingScalingPolicyConfiguration':{'targetValue':60.0}}},'writeCapacityAutoScaling':{'autoScalingDisabled':False,'minimumUnits':5,'maximumUnits':50,'scalingPolicy':{'targetTrackingScalingPolicyConfiguration':{'targetValue':60.0}}}}) and wait for status ACTIVE
4. update_table(keyspaceName='ks1', tableName='tbl1', autoScalingSpecification={'writeCapacityAutoScaling':{'autoScalingDisabled':False,'minimumUnits':10,'maximumUnits':100,'scalingPolicy':{'targetTrackingScalingPolicyConfiguration':{'targetValue':70.0}}}})
5. Observe HTTP 501 / code NotImplemented instead of HTTP 200 with resourceArn.
6. get_table_auto_scaling_settings(keyspaceName='ks1', tableName='tbl1') still reports the creation-time min 5 / max 50 / targetValue 60.0 — the update was never applied.
7. Repeat step 4 with replicaSpecifications=[{'region':'us-east-1','readCapacityUnits':5}] or warmThroughputSpecification={'readUnitsPerSecond':12000} for the same 501.

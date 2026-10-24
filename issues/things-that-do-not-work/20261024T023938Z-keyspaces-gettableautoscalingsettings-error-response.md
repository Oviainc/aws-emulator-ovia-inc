Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 047af81d647cd24bcec0ef3f34fd68e55311879a

# GetTableAutoScalingSettings answers 404 ResourceNotFoundException for a table that exists but has no auto scaling configured

_Filed: 2026-10-24T02:39:38Z_
_Operation: GetTableAutoScalingSettings | Class: error-response_

> _Note: this issue was identified against the code base at commit `047af81d647cd24bcec0ef3f34fd68e55311879a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the named keyspace and table both exist but the stored table has no `auto_scaling_specification`, the handler returns HTTP 404 `ResourceNotFoundException` with message "Table <name> has no auto scaling settings." and `resourceArn` set to the existing table's ARN.

Expected (AWS): `ResourceNotFoundException` is documented as "The operation tried to access a keyspace, table, or type that doesn't exist. The resource might not be specified correctly, or its status might not be ACTIVE." The table here demonstrably exists and is ACTIVE — the very same request to GetTable returns 200 — so a 404 claiming the resource does not exist is the wrong failure mode. AWS answers such a call either with HTTP 200 carrying the identity members (`keyspaceName`, `tableName`, `resourceArn`, all @required) and no `autoScalingSpecification`, or with a 400 `ValidationException` saying auto scaling is not configured / not available for the table's capacity mode (auto scaling applies only to `throughputMode:PROVISIONED`). Either way the response must not report the table as nonexistent, and a client that distinguishes "table missing" from "auto scaling not configured" cannot do so against this emulator.

A table created in PAY_PER_REQUEST mode (the modeled default, i.e. any CreateTable that omits `capacitySpecification`) is the common case that hits this path, so a plain create -> get-auto-scaling-settings sequence returns a 404 for a live table.

## Evidence

Spec (Smithy model): `ResourceNotFoundException` doc "The operation tried to access a keyspace, table, or type that doesn't exist. The resource might not be specified correctly, or its status might not be ACTIVE"; `GetTableAutoScalingSettingsResponse` makes `keyspaceName`, `tableName` and `resourceArn` @required while `autoScalingSpecification` is OPTIONAL — a response shape that only makes sense if a table without auto scaling can be reported with the member absent; `AutoScalingSpecification` doc "The optional auto scaling capacity settings for a table in provisioned capacity mode"; the operation also models `ValidationException` (400).
Handler: services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/get_table_auto_scaling_settings.rs lines 213-229 (`let Some(auto_scaling_specification) = table.auto_scaling_specification.clone() else { return Err(... ResourceNotFoundException ...) }`) — reached only after the keyspace and table were both resolved from state, i.e. the resource exists.
Tests: services/.../tests/all/column_types_and_auto_scaling.rs lines 710-742 asserts exactly this 404 for a table created without `autoScalingSpecification`, so the emulator's own tests lock the divergence in; no test asserts the AWS response.

## Steps to Reproduce

1. Boot the emulator and point a boto3 keyspaces client at it.
2. create_keyspace(keyspaceName='ks1')
3. create_table(keyspaceName='ks1', tableName='no_auto_scaling', schemaDefinition={'allColumns':[{'name':'pk','type':'text'}],'partitionKeys':[{'name':'pk'}]})  # PAY_PER_REQUEST default, no autoScalingSpecification
4. Wait for get_table(keyspaceName='ks1', tableName='no_auto_scaling') to report status ACTIVE — proving the table exists.
5. get_table_auto_scaling_settings(keyspaceName='ks1', tableName='no_auto_scaling')
6. Observe HTTP 404 ResourceNotFoundException naming the existing table's ARN, instead of a 200 carrying keyspaceName/tableName/resourceArn with no autoScalingSpecification (or a 400 ValidationException about auto scaling not being configured).

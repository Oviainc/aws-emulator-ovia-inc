Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047af81d647cd24bcec0ef3f34fd68e55311879a

# CreateTable refuses replicaSpecifications and warmThroughputSpecification with an unmodeled 501 NotImplemented

_Filed: 2026-10-24T02:39:38Z_
_Operation: CreateTable | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047af81d647cd24bcec0ef3f34fd68e55311879a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateTable's handler opens with a guard that answers HTTP 501 with error code `NotImplemented` whenever the request carries `replicaSpecifications`, `cdcSpecification` or `warmThroughputSpecification`, and nothing is stored.

Expected (AWS): both `replicaSpecifications` (list<ReplicaSpecification>, len 1..) and `warmThroughputSpecification` (WarmThroughputSpecification) are ordinary optional members of `CreateTableRequest`. A well-formed call that supplies them must return HTTP 200 with `resourceArn`, create the table (status CREATING -> ACTIVE), persist the per-Region read capacity / read auto-scaling overrides and the warm-throughput read/write units per second, and GetTable must then report them back as `replicaSpecifications` (list<ReplicaSpecificationSummary>) and `warmThroughputSpecification` (WarmThroughputSpecificationSummary: readUnitsPerSecond, writeUnitsPerSecond, status AVAILABLE|UPDATING). `replicaSpecifications` is the documented way to give a multi-Region table per-Region capacity, which pairs with the MULTI_REGION keyspaces this emulator already supports in CreateKeyspace/GetKeyspace.

Actual (emulator): HTTP 501, code `NotImplemented`, message "CreateTable is implemented in this emulator, but not its replicaSpecifications, warmThroughputSpecification member(s): omit them, or the table would be stored with settings no operation here can report." No table is created at all, so the failure is not a dropped setting but a refused create. CreateTable models exactly six errors (AccessDenied 403, Conflict 409, InternalServer 500, ResourceNotFound 404, ServiceQuotaExceeded 402, Validation 400) and no not-implemented shape of any status, so no SDK error branch matches the code returned. The `crate::state::Table` record has no replica or warm-throughput field, which is why the guard exists.

Scope note: `cdcSpecification` on the same guard is already tracked by the open issue "Keyspaces CDC never enabled ...", and the UpdateTable half of the same guard by the open issue "UpdateTable refuses autoScalingSpecification (and replicaSpecifications / warmThroughputSpecification) ...". This issue is the CreateTable half for the two non-CDC members, plus the GetTable reporting of `warmThroughputSpecification` that must follow once they can be stored (GetTable's `replicaSpecifications` hardcode is tracked by its own open issue).

## Evidence

Spec (Smithy model): `CreateTableRequest` members `replicaSpecifications` -> ReplicaSpecificationList (len 1..) and `warmThroughputSpecification` -> WarmThroughputSpecification (readUnitsPerSecond/writeUnitsPerSecond); `WarmThroughputSpecificationSummary` (readUnitsPerSecond, writeUnitsPerSecond, status all @required) and `ReplicaSpecificationSummary` are GetTable output members; CreateTable's modeled error set contains no not-implemented error.
Handler: services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/create_table.rs lines 246-274 (the `unimplemented` member list returning `crate::scaffolded::Scaffolded::NotImplemented`); the stored record `crate::state::Table` (src/state.rs lines 594-645) carries no replica/warm-throughput field; src/ops/get_table.rs lines 260-267 hardcodes `warm_throughput_specification: None` for the same reason.
Tests: the crate's own tests assert the 501 refusal for these members (tests/all/update_table.rs has the UpdateTable analogue, and tests/all/column_types_and_auto_scaling.rs exercises only the accepted members), so the divergence is locked in by tests; nothing asserts the AWS behaviour.

## Steps to Reproduce

1. Boot the emulator and point a boto3 keyspaces client at it (sign for us-west-2).
2. create_keyspace(keyspaceName='mrks', replicationSpecification={'replicationStrategy':'MULTI_REGION','regionList':['us-west-2','eu-west-1']})
3. create_table(keyspaceName='mrks', tableName='tbl1', schemaDefinition={'allColumns':[{'name':'pk','type':'text'}],'partitionKeys':[{'name':'pk'}]}, capacitySpecification={'throughputMode':'PROVISIONED','readCapacityUnits':5,'writeCapacityUnits':5}, replicaSpecifications=[{'region':'eu-west-1','readCapacityUnits':10}])
4. Observe HTTP 501 with error code NotImplemented instead of HTTP 200 with resourceArn; list_tables(keyspaceName='mrks') shows no table was created.
5. Repeat step 3 with warmThroughputSpecification={'readUnitsPerSecond':12000,'writeUnitsPerSecond':4000} instead — same 501.
6. After a successful create (AWS), get_table(keyspaceName='mrks', tableName='tbl1') reports warmThroughputSpecification with readUnitsPerSecond/writeUnitsPerSecond/status; the emulator never reports that member.

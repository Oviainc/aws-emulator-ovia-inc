Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Timestream InfluxDB emulator

_Filed: 2026-10-21T21:18:33Z_
_Operation: service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 24 operations in `services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), holding no state. Nothing works end-to-end.

This issue scopes the FIRST buildable slice: the DB instance lifecycle, which is the primary resource of Amazon Timestream for InfluxDB. Wire protocol is `awsJson1_0`: `POST /` with `X-Amz-Target: AmazonTimestreamInfluxDB.<OperationName>` (see OPERATIONS.md "Wire protocol").

Implement these 5 operations, in this order, all computing their responses from persisted shared state (`crate::state::service_state(&*self.engine)`):

1. **CreateDbInstance** — required input: `name` (3..40, `^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$`), `password` (8..64), `dbInstanceType` (enum db.influx.medium ... db.influx.24xlarge), `vpcSubnetIds` (1..6), `vpcSecurityGroupIds` (1..5), `allocatedStorage` (20..15360), `port` (1024..65535, default 8086). Optional and must be STORED, not dropped: `username`, `organization`, `bucket`, `publiclyAccessible`, `dbStorageType`, `dbParameterGroupIdentifier`, `deploymentType`, `networkType`, `logDeliveryConfiguration`, `tags`, `kmsKeyId`. Must mint a service-generated `id` matching `^[a-zA-Z0-9]+$` (3..64), an `arn` of the form `arn:aws:timestream-influxdb:<region>:<account12>:db-instance/<id>`, an `endpoint` derived from the name/id, and `status`. A second CreateDbInstance with the same `name` must fail `ConflictException` (409) — names are unique per customer per region.
2. **GetDbInstance** — input `identifier` (the stored `id`); must return the full record exactly as created (every stored field, same `id`/`arn`/`name`/`port`/`vpcSubnetIds`/`allocatedStorage`/`dbInstanceType`). Unknown identifier → `ResourceNotFoundException` (404), never a canned record.
3. **ListDbInstances** — `@paginated(inputToken=nextToken, outputToken=nextToken, items=items)`; must return one `DbInstanceSummary` per live instance (`id`, `name`, `arn`, `status`, `endpoint`, `port`, `networkType`, `dbInstanceType`, `dbStorageType`, `allocatedStorage`, `deploymentType`), honour `maxResults` (1..100) and round-trip `nextToken` so paging through the list yields every instance exactly once.
4. **UpdateDbInstance** — input `identifier` plus the mutable fields `dbInstanceType`, `port`, `allocatedStorage`, `dbStorageType`, `deploymentType`, `dbParameterGroupIdentifier`, `logDeliveryConfiguration`, `maintenanceSchedule`; must mutate the stored record so a subsequent GetDbInstance reflects the new values, and return the updated record. Unknown identifier → `ResourceNotFoundException` (404).
5. **DeleteDbInstance** — input `identifier`; must remove the instance from state and return the final record with `status` = `DELETING`/`DELETED`. A following GetDbInstance must fail `ResourceNotFoundException` (404), and ListDbInstances must no longer include it. Deleting an unknown identifier → 404.

Rules for this slice:
- NEVER STUBS: no hardcoded ids/arns/endpoints, no echoing the request back without storing it, no random id without a stored record. Every response field must come from the persisted record.
- Modeled constraints (lengths, patterns, enums, ranges) are refused by the framework before `handle` runs; handler-level refusals (duplicate name, missing resource) must use the modeled errors `ConflictException` (409) / `ResourceNotFoundException` (404) with the correct awsJson error shape.

## State Model

In the service state struct (`crate::state`):
- `db_instances: BTreeMap<String /* id */, DbInstance>` — insertion-ordered or id-ordered map, the single source of truth for the lifecycle ops.
- `db_instance_names: BTreeMap<String /* name */, String /* id */>` — uniqueness index backing the `ConflictException` on duplicate `name`.
- `DbInstance { id: String, name: String, arn: String, status: Status, endpoint: String, port: i32 /* default 8086 */, network_type: Option<NetworkType>, db_instance_type: DbInstanceType, db_storage_type: Option<DbStorageType>, allocated_storage: i32, deployment_type: Option<DeploymentType>, vpc_subnet_ids: Vec<String>, vpc_security_group_ids: Vec<String>, publicly_accessible: Option<bool>, db_parameter_group_identifier: Option<String>, availability_zone: Option<String>, secondary_availability_zone: Option<String>, log_delivery_configuration: Option<LogDeliveryConfiguration>, maintenance_schedule: Option<MaintenanceSchedule>, influx_auth_parameters_secret_arn: Option<String>, kms_key_id: Option<String>, username: Option<String>, organization: Option<String>, bucket: Option<String>, tags: BTreeMap<String,String>, created_at: SystemTime }`
- A monotonic id/sequence counter used to mint `id` values matching `^[a-zA-Z0-9]{3,64}$`, plus the region/account values used to format `arn`.

Out of scope for this slice (stay 501, to be filed as individual gap issues later): all DB cluster ops (CreateDbCluster, GetDbCluster, UpdateDbCluster, DeleteDbCluster, ListDbClusters, ListDbInstancesForCluster, RebootDbCluster), backups (CreateDbBackup, GetDbBackup, ListDbBackups, DeleteDbBackup, RestoreFromDbBackup), parameter groups (CreateDbParameterGroup, GetDbParameterGroup, ListDbParameterGroups), RebootDbInstance, and the tagging ops (TagResource, UntagResource, ListTagsForResource) — though CreateDbInstance should already persist `tags` so tagging is cheap to add next.

## Evidence

OPERATIONS.md for this crate (24 operations, `awsJson1_0`, `X-Amz-Target: AmazonTimestreamInfluxDB.<Op>`) lists CreateDbInstance (required `name`, `password`, `dbInstanceType`, `vpcSubnetIds`, `vpcSecurityGroupIds`, `allocatedStorage`, `port` default 8086; output required `id`, `name`, `arn`, `vpcSubnetIds`), GetDbInstance (`identifier` required; errors include ResourceNotFoundException 404), ListDbInstances (`@paginated` nextToken/maxResults 1..100, output `items` required), UpdateDbInstance and DeleteDbInstance (`identifier` required, ConflictException 409 / ResourceNotFoundException 404). `smithy_query shape=DbInstanceSummary` confirms the summary members required in list output (`id`, `name`, `arn` required). `src/ops/create_db_instance.rs` lines 4-6 and 291 show the SCAFFOLDED marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDbInstance"))`; all 24 handler files in `src/ops/` are the same generated stub, so no state exists and no test asserts any behavior.

## Steps to Reproduce

1. Start the emulator and point the AWS SDK/CLI for `timestream-influxdb` at it.
2. CreateDbInstance with name=my-influx, password=supersecret1, dbInstanceType=db.influx.medium, allocatedStorage=20, vpcSubnetIds=[subnet-aaa,subnet-bbb], vpcSecurityGroupIds=[sg-aaa], port omitted — expect 200 with a generated `id`, an `arn` ending in `db-instance/<id>`, `name`=my-influx, `port`=8086; today: HTTP 501 NotImplemented.
3. GetDbInstance with identifier=<id> — expect the same id/arn/name/port/allocatedStorage/dbInstanceType/vpcSubnetIds that were created.
4. ListDbInstances — expect `items` to contain exactly one summary whose `id`/`name`/`arn` match; repeat with maxResults=1 after creating a second instance and follow `nextToken` to see both exactly once.
5. CreateDbInstance again with name=my-influx — expect ConflictException (HTTP 409).
6. UpdateDbInstance identifier=<id>, dbInstanceType=db.influx.large, allocatedStorage=40 — expect 200, then GetDbInstance shows the new type and storage.
7. DeleteDbInstance identifier=<id> — expect 200 with status DELETING/DELETED, then GetDbInstance with the same identifier returns ResourceNotFoundException (404) and ListDbInstances no longer lists it.
8. GetDbInstance with identifier=doesnotexist — expect ResourceNotFoundException (404), not a fabricated instance.

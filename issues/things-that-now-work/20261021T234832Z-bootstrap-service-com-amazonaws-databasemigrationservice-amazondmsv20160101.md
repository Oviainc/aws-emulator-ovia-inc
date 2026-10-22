Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9e4273988d175f89e8a017aa0737ee7107d62a89

# Bootstrap the DatabaseMigrationService (DMS) emulator

_Filed: 2026-10-21T23:48:32Z_
_Operation: service-com-amazonaws-databasemigrationservice-amazondmsv20160101 | Class: missing_

> _Note: this issue was identified against the code base at commit `9e4273988d175f89e8a017aa0737ee7107d62a89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding no state. No DMS client workflow can run.

Implement a FIRST, SMALL, stateful slice: the core DMS migration-task happy path. Six operations, in this order:

1. **CreateReplicationInstance** — input `ReplicationInstanceIdentifier` (required), `ReplicationInstanceClass` (required, len 0..30), `AllocatedStorage`, `EngineVersion`, `MultiAZ`, `PubliclyAccessible`, `AvailabilityZone`, `Tags`. Mint `ReplicationInstanceArn` = `arn:aws:dms:<region>:<account>:rep:<generated-id>`, store the instance, and return a `ReplicationInstance` echoing the stored identifier/class/storage plus server-computed `ReplicationInstanceStatus` (`available`) and `InstanceCreateTime`. A second create with the same identifier must fail `ResourceAlreadyExistsFault`.
2. **CreateEndpoint** (called twice: source and target) — input `EndpointIdentifier` (required), `EndpointType` (required, `source`|`target`), `EngineName` (required), plus `ServerName`, `Port`, `DatabaseName`, `Username`, `SslMode`, `Tags`. Mint `EndpointArn` = `arn:aws:dms:<region>:<account>:endpoint:<generated-id>`, persist, return the `Endpoint` structure with `Status` = `active`. Duplicate `EndpointIdentifier` ⇒ `ResourceAlreadyExistsFault`.
3. **CreateReplicationTask** — input `ReplicationTaskIdentifier` (required), `SourceEndpointArn` (required), `TargetEndpointArn` (required), `ReplicationInstanceArn` (required), `MigrationType` (required, `cdc`|`full-load`|`full-load-and-cdc`), `TableMappings` (required), `ReplicationTaskSettings`, `TaskData`. The three ARNs must resolve against persisted endpoints/instance — unknown ARN ⇒ `ResourceNotFoundFault`. Mint `ReplicationTaskArn` = `arn:aws:dms:<region>:<account>:task:<generated-id>`, persist, return a `ReplicationTask` with `Status` = `ready` and `ReplicationTaskCreationDate` set from the clock.
4. **DescribeReplicationTasks** — paginated (`inputToken=Marker`, `outputToken=Marker`, `MaxRecords`). Must return the tasks actually persisted by step 3 (ARN, identifier, both endpoint ARNs, instance ARN, migration type, table mappings, current status), NOT a canned list. Honour `MaxRecords` by truncating and emitting an opaque `Marker` that resumes the scan; omit `Marker` on the final page. Honour at least the `replication-task-arn` / `replication-task-id` `Filters` entries, and drop `ReplicationTaskSettings` from the response when `WithoutSettings` is true. `DescribeEndpoints` and `DescribeReplicationInstances` should follow the same read-from-state pattern for the resources created in steps 1–2.
5. **StartReplicationTask** — input `ReplicationTaskArn` (required), `StartReplicationTaskType` (required, `start-replication`|`resume-processing`|`reload-target`), `CdcStartTime`/`CdcStartPosition`/`CdcStopPosition`. Must look up the stored task: unknown ARN ⇒ `ResourceNotFoundFault`; a task not in a startable state (e.g. already `running`) ⇒ `InvalidResourceStateFault`. On success transition the persisted task's `Status` to `starting`/`running`, record `ReplicationTaskStartDate`, and return the updated `ReplicationTask`. A following DescribeReplicationTasks must observe the new status.
6. **DeleteReplicationTask** — input `ReplicationTaskArn` (required). Unknown ARN ⇒ `ResourceNotFoundFault`; a `running` task ⇒ `InvalidResourceStateFault` (StopReplicationTask first). On success remove the task from state and return the final `ReplicationTask` snapshot; a subsequent DescribeReplicationTasks must no longer list it.

Every one of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — no canned structs, no echoing the input back without storing it, no minting an ARN that is never recorded. All identifiers/ARNs/timestamps/statuses returned by a describe must be the ones a prior create wrote.

## State Model

In `src/state.rs`, a single lockable struct for this service:

- `replication_instances: BTreeMap<String /* ReplicationInstanceArn */, ReplicationInstance { arn, identifier, instance_class, allocated_storage, engine_version, multi_az, publicly_accessible, availability_zone, status, create_time, kms_key_id, tags }>` plus an `instance_arn_by_identifier: BTreeMap<String, String>` index so duplicate-identifier creates can be rejected.
- `endpoints: BTreeMap<String /* EndpointArn */, Endpoint { arn, identifier, endpoint_type, engine_name, server_name, port, database_name, username, ssl_mode, extra_connection_attributes, status, tags }>` plus `endpoint_arn_by_identifier: BTreeMap<String, String>`.
- `replication_tasks: BTreeMap<String /* ReplicationTaskArn */, ReplicationTask { arn, identifier, source_endpoint_arn, target_endpoint_arn, replication_instance_arn, migration_type, table_mappings, replication_task_settings, task_data, status, stop_reason, last_failure_message, creation_date, start_date, cdc_start_position, cdc_stop_position, recovery_checkpoint, stats, tags }>` plus `task_arn_by_identifier: BTreeMap<String, String>`.
- `account_id: String` and `region: String` used for ARN construction, and a monotonically increasing `next_resource_id` counter (or UUID source) for the ARN suffixes.
- Ordering: the `BTreeMap`s give a deterministic scan order, which the `Marker` pagination can encode as "resume after this key".

Done = a client script can run CreateReplicationInstance → CreateEndpoint(source) → CreateEndpoint(target) → CreateReplicationTask → DescribeReplicationTasks → StartReplicationTask → DescribeReplicationTasks (status advanced) → DeleteReplicationTask → DescribeReplicationTasks (gone) and see correct, stateful bodies at every step.

Everything else in the service (Fleet Advisor, metadata-model/schema-conversion, data migrations, serverless replications, event subscriptions, certificates, tagging, assessment runs, ModifyEndpoint/ModifyReplicationTask, TestConnection, etc.) stays 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md in the crate root defines each operation of this slice: CreateReplicationInstance (line 111: `ReplicationInstanceIdentifier` and `ReplicationInstanceClass` required; output `ReplicationInstance`; errors include `ResourceAlreadyExistsFault`), CreateEndpoint (line 75: `EndpointIdentifier`/`EndpointType`/`EngineName` required; output `Endpoint`), CreateReplicationTask (line 123: `ReplicationTaskIdentifier`/`SourceEndpointArn`/`TargetEndpointArn`/`ReplicationInstanceArn`/`MigrationType`/`TableMappings` required; errors `ResourceNotFoundFault`, `ResourceAlreadyExistsFault`), DescribeReplicationTasks (line 522: `@paginated(inputToken=Marker, outputToken=Marker)`, input `Filters`/`MaxRecords`/`Marker`/`WithoutSettings`, output `Marker` + `ReplicationTasks`), StartReplicationTask (line 735: `ReplicationTaskArn` + `StartReplicationTaskType` required; errors `InvalidResourceStateFault`, `ResourceNotFoundFault`), DeleteReplicationTask (line 207: `ReplicationTaskArn` required; errors `InvalidResourceStateFault`, `ResourceNotFoundFault`). Smithy shapes `ReplicationInstance`, `Endpoint` and `ReplicationTask` confirm the server-computed members cited above (`ReplicationInstanceArn`, `ReplicationInstanceStatus`, `InstanceCreateTime`; `EndpointArn`, `Status`; `ReplicationTaskArn`, `Status`, `ReplicationTaskCreationDate`, `ReplicationTaskStartDate`, `RecoveryCheckpoint`). All 120 files under src/ops/ match `^//! SCAFFOLDED, NOT IMPLEMENTED:` (grep), including create_replication_instance.rs, create_endpoint.rs, create_replication_task.rs, describe_replication_tasks.rs, start_replication_task.rs and delete_replication_task.rs — so there is no existing behaviour and no test coverage for any of it.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI DMS client at it.
2. `aws dms create-replication-instance --replication-instance-identifier my-ri --replication-instance-class dms.t3.medium` — expect a ReplicationInstance with a minted ReplicationInstanceArn and status; actual: HTTP 501 NotImplemented.
3. `aws dms create-endpoint --endpoint-identifier src-ep --endpoint-type source --engine-name mysql --server-name db1 --port 3306` and again with `--endpoint-identifier tgt-ep --endpoint-type target --engine-name postgres` — expect two Endpoints with minted EndpointArns.
4. `aws dms create-replication-task --replication-task-identifier my-task --source-endpoint-arn <src> --target-endpoint-arn <tgt> --replication-instance-arn <ri> --migration-type full-load --table-mappings file://tm.json` — expect a ReplicationTask with minted ReplicationTaskArn and Status=ready.
5. `aws dms describe-replication-tasks` — expect exactly the task just created, with the same ARNs/identifier/migration type; also check `--max-records 1` returns a Marker that pages correctly.
6. `aws dms start-replication-task --replication-task-arn <task> --start-replication-task-type start-replication` — expect the task's Status to advance and ReplicationTaskStartDate to be set; re-run describe-replication-tasks and confirm the new status is observed.
7. `aws dms start-replication-task --replication-task-arn arn:aws:dms:us-east-1:000000000000:task:DOESNOTEXIST ...` — expect ResourceNotFoundFault (HTTP 400), not 501.
8. `aws dms delete-replication-task --replication-task-arn <task>` then `aws dms describe-replication-tasks` — expect the task returned once by the delete and then absent from the list.

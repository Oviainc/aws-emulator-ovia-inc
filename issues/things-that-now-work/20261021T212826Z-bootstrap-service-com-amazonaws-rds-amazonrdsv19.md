Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the rds emulator

_Filed: 2026-10-21T21:28:26Z_
_Operation: service-com-amazonaws-rds-amazonrdsv19 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The RDS (com-amazonaws-rds-amazonrdsv19, awsQuery protocol, sigv4 name `rds`) emulator is a fresh scaffold: all 168 handlers under `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answer HTTP 501 `NotImplemented`, and `ComAmazonawsRdsAmazonrdsv19State` has no fields. Implement a FIRST, SMALL stateful slice around the primary resource (the DB instance) so a realistic client script works end to end.

### Happy path to implement (in order)

1. **CreateDBInstance** — input `DBInstanceIdentifier` (required), `DBInstanceClass` (required), `Engine` (required), plus commonly used optionals that must be PERSISTED, not dropped: `DBName`, `AllocatedStorage`, `MasterUsername`, `EngineVersion`, `Port`, `MultiAZ`, `StorageType`, `StorageEncrypted`, `BackupRetentionPeriod`, `PubliclyAccessible`, `DeletionProtection`, `Tags`. Output: `DBInstance` with at minimum `DBInstanceIdentifier`, `DBInstanceClass`, `Engine`, `EngineVersion`, `DBInstanceStatus` (`creating`, then `available` on subsequent reads), `DBInstanceArn` (`arn:aws:rds:<region>:<account>:db:<id>`), `DbiResourceId`, `Endpoint{Address,Port,HostedZoneId}`, `AllocatedStorage`, `MasterUsername`, `MultiAZ`, `StorageType`, `AvailabilityZone`, `InstanceCreateTime`, `TagList`. Re-creating the same identifier must fail with `DBInstanceAlreadyExistsFault` (HTTP 400).
2. **DescribeDBInstances** — input `DBInstanceIdentifier`, `Filters`, `MaxRecords`, `Marker`; output `DBInstances` (list) + `Marker`. With no identifier it returns every stored instance; with an identifier it returns exactly that one, or `DBInstanceNotFoundFault` (HTTP 404) when unknown. `MaxRecords`/`Marker` must actually page (operation is `@paginated(inputToken=Marker, outputToken=Marker, items=DBInstances)`): return a `Marker` only when more items remain, and resume from it.
3. **ModifyDBInstance** — input `DBInstanceIdentifier` (required) plus at least `DBInstanceClass`, `AllocatedStorage`, `BackupRetentionPeriod`, `MasterUserPassword`, `MultiAZ`, `EngineVersion`, `DeletionProtection`, `ApplyImmediately`, `NewDBInstanceIdentifier`. Supplied fields must be written into the stored instance so a following DescribeDBInstances shows the new values (when `ApplyImmediately` is false it is acceptable to apply immediately for the first slice, but the stored record must change, never an echo). Unknown id → `DBInstanceNotFoundFault` (404); `NewDBInstanceIdentifier` colliding with an existing instance → `DBInstanceAlreadyExistsFault` (400).
4. **CreateDBSnapshot** — input `DBSnapshotIdentifier` (required), `DBInstanceIdentifier` (required), `Tags`; output `DBSnapshot` whose `Engine`, `EngineVersion`, `AllocatedStorage`, `MasterUsername`, `AvailabilityZone`, `Port`, `StorageType`, `InstanceCreateTime` are COPIED from the stored source instance (not defaulted), with `Status` `available`, `SnapshotCreateTime`, `SnapshotType` `manual`, `DBSnapshotArn`. Unknown source instance → `DBInstanceNotFoundFault` (404); duplicate snapshot id → `DBSnapshotAlreadyExistsFault` (400). Pair with **DescribeDBSnapshots** (filter by `DBInstanceIdentifier` / `DBSnapshotIdentifier`) so the snapshot is readable back.
5. **DeleteDBInstance** — input `DBInstanceIdentifier` (required), `SkipFinalSnapshot`, `FinalDBSnapshotIdentifier`, `DeleteAutomatedBackups`; output the `DBInstance` with `DBInstanceStatus` `deleting`. The instance must be REMOVED from state so the next DescribeDBInstances for that id returns `DBInstanceNotFoundFault` (404) and the list no longer contains it. A second delete of the same id → `DBInstanceNotFoundFault`. When `SkipFinalSnapshot` is false and `FinalDBSnapshotIdentifier` is given, store that snapshot (visible via DescribeDBSnapshots). If the stored instance has `DeletionProtection=true`, fail with `InvalidDBInstanceStateFault` (400).

### Rules

- NEVER STUBS: every one of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the input back without storing it, no random identifiers that are not saved.
- Errors must be the modeled awsQuery faults with their modeled HTTP statuses (e.g. `DBInstanceNotFoundFault` 404, `DBInstanceAlreadyExistsFault` 400), not generic 500s.
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line in each handler file you implement.

## State Model

In `ComAmazonawsRdsAmazonrdsv19State` (guarded by the engine's lock):

- `db_instances: BTreeMap<String /* DBInstanceIdentifier */, DbInstanceRecord>` — ordered so DescribeDBInstances pagination is stable. `DbInstanceRecord { identifier, arn, dbi_resource_id, db_instance_class, engine, engine_version, status, db_name, allocated_storage, master_username, endpoint_address, endpoint_port, hosted_zone_id, multi_az, storage_type, storage_encrypted, kms_key_id, availability_zone, backup_retention_period, preferred_backup_window, preferred_maintenance_window, publicly_accessible, deletion_protection, db_subnet_group_name, db_parameter_group_name, vpc_security_group_ids: Vec<String>, instance_create_time, tags: Vec<Tag> }`.
- `db_snapshots: BTreeMap<String /* DBSnapshotIdentifier */, DbSnapshotRecord>` — `DbSnapshotRecord { snapshot_identifier, snapshot_arn, db_instance_identifier, dbi_resource_id, engine, engine_version, allocated_storage, master_username, availability_zone, port, storage_type, status, snapshot_type, instance_create_time, snapshot_create_time, tags: Vec<Tag> }`.
- `counter`/clock helpers for deterministic ARNs, resource ids and endpoint addresses (e.g. `<id>.cluster-emu.<region>.rds.amazonaws.com`).

Every other RDS operation (clusters, parameter groups, proxies, global clusters, blue/green, exports, …) stays 501 for now and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-rds-amazonrdsv19/OPERATIONS.md` line 3: "168 operations", protocol `awsQuery`, SDK id `RDS`. Field lists and error sets quoted above come from OPERATIONS.md §CreateDBInstance (l.152-156), §DescribeDBInstances (l.464-469, including the `@paginated(inputToken=Marker, outputToken=Marker, items=DBInstances)` trait), §ModifyDBInstance (l.789-793), §CreateDBSnapshot (l.194-198) and §DeleteDBInstance (l.278-282). Stub proof: `src/ops/create_db_instance.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, and every file in `src/ops/` is generated the same way (README.md l.36-49). `src/state.rs` l.7-9 shows `ComAmazonawsRdsAmazonrdsv19State { /* TODO: add service-specific state fields */ }`, i.e. zero persisted state today. No tests exist beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point the AWS CLI/SDK at it with `--endpoint-url`.
2. `aws rds create-db-instance --db-instance-identifier emu-db-1 --db-instance-class db.t3.micro --engine postgres --allocated-storage 20 --master-username admin --master-user-password secret99 --backup-retention-period 7` → expect 200 with a DBInstance echoing the identifier/class/engine/storage and carrying an ARN, endpoint and status; today: HTTP 501 NotImplemented.
3. `aws rds describe-db-instances --db-instance-identifier emu-db-1` → expect exactly the instance just created with the same AllocatedStorage/MasterUsername/Engine.
4. `aws rds describe-db-instances --max-records 20` → expect the instance present in DBInstances; with more instances than MaxRecords expect a Marker that resumes correctly.
5. `aws rds modify-db-instance --db-instance-identifier emu-db-1 --allocated-storage 50 --backup-retention-period 14 --apply-immediately` then `aws rds describe-db-instances --db-instance-identifier emu-db-1` → expect AllocatedStorage 50 and BackupRetentionPeriod 14 persisted.
6. `aws rds create-db-snapshot --db-instance-identifier emu-db-1 --db-snapshot-identifier emu-snap-1` then `aws rds describe-db-snapshots --db-instance-identifier emu-db-1` → expect the snapshot with Engine/AllocatedStorage(50)/MasterUsername copied from the instance.
7. `aws rds create-db-instance --db-instance-identifier emu-db-1 ...` again → expect 400 DBInstanceAlreadyExistsFault.
8. `aws rds delete-db-instance --db-instance-identifier emu-db-1 --skip-final-snapshot` → expect 200 with DBInstanceStatus `deleting`.
9. `aws rds describe-db-instances --db-instance-identifier emu-db-1` → expect 404 DBInstanceNotFoundFault, and the instance absent from the unfiltered list.
10. `aws rds describe-db-instances --db-instance-identifier does-not-exist` → expect 404 DBInstanceNotFoundFault, not 501 or 500.

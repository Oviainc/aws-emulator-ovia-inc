Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the docdb emulator

_Filed: 2026-10-21T21:58:06Z_
_Operation: service-com-amazonaws-docdb-amazonrdsv19 | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 55 operations in `services/service-com-amazonaws-docdb-amazonrdsv19/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST, SMALL stateful slice: the Amazon DocumentDB cluster lifecycle, which is the service's primary resource and core workflow.

### Happy path to implement (in order)

1. **CreateDBCluster** — input `DBClusterIdentifier` (required), `Engine` (required, `docdb`), `MasterUsername`, `MasterUserPassword`, `EngineVersion`, `Port`, `DBSubnetGroupName`, `DBClusterParameterGroupName`, `BackupRetentionPeriod`, `StorageEncrypted`, `DeletionProtection`, `AvailabilityZones`, `VpcSecurityGroupIds`, `Tags`. Output `DBCluster`. Must INSERT a new cluster record keyed by the (lowercased) identifier and return the stored record: `DBClusterIdentifier`, `DbClusterResourceId`, `DBClusterArn` (`arn:aws:rds:<region>:<account>:cluster:<id>`), `Status` (`available` is acceptable for the emulator), `Engine`, `EngineVersion` (default a real DocDB version when absent), `Port` (default 27017), `MasterUsername`, `Endpoint` (`<id>.cluster-<resid>.<region>.docdb.amazonaws.com`), `ReaderEndpoint` (`<id>.cluster-ro-...`), `ClusterCreateTime`, `BackupRetentionPeriod` (default 1), `StorageEncrypted`, `DeletionProtection` (default false), `DBClusterMembers` (empty), `AvailabilityZones`. Re-creating an existing identifier must fail `DBClusterAlreadyExistsFault` (HTTP 400).
2. **CreateDBInstance** — input `DBInstanceIdentifier`, `DBInstanceClass`, `Engine`, `DBClusterIdentifier` (all required), plus `AvailabilityZone`, `PromotionTier`, `Tags`. Output `DBInstance`. Must look the cluster up in state and fail `DBClusterNotFoundFault` (404) when it is absent; on success store the instance AND append a `DBClusterMember{DBInstanceIdentifier, IsClusterWriter, PromotionTier, DBClusterParameterGroupStatus}` to that cluster (first member is the writer), so the next DescribeDBClusters shows it. Duplicate identifier → `DBInstanceAlreadyExistsFault` (400).
3. **DescribeDBClusters** — input `DBClusterIdentifier`, `Filters`, `MaxRecords`, `Marker`; output `DBClusters` + `Marker`. Must return the clusters actually in state. With `DBClusterIdentifier` set, return exactly that one cluster or fail `DBClusterNotFoundFault` (404); without it, return all clusters, honouring `MaxRecords` and round-tripping `Marker` per the `@paginated(inputToken=Marker, outputToken=Marker, items=DBClusters)` trait (omit `Marker` on the last page).
4. **DescribeDBInstances** — same shape for instances (`DBInstances` + `Marker`, `DBInstanceNotFoundFault` 404 for an unknown identifier). Each returned `DBInstance` must carry the `DBClusterIdentifier` it was created against, its `DBInstanceClass`, `DBInstanceStatus`, `Endpoint` and `InstanceCreateTime` from state.
5. **ModifyDBCluster** — input `DBClusterIdentifier` (required) plus the mutable fields (`BackupRetentionPeriod`, `PreferredBackupWindow`, `PreferredMaintenanceWindow`, `Port`, `MasterUserPassword`, `DeletionProtection`, `NewDBClusterIdentifier`). Must MUTATE the persisted record (including re-keying on `NewDBClusterIdentifier`) and return the updated `DBCluster`; a following DescribeDBClusters must show the new values. Unknown id → `DBClusterNotFoundFault` (404).
6. **DeleteDBInstance** / **DeleteDBCluster** — `DeleteDBInstance` removes the instance from state and from its cluster's `DBClusterMembers`, returning the deleted `DBInstance` with `DBInstanceStatus: deleting`. `DeleteDBCluster` (input `DBClusterIdentifier`, `SkipFinalSnapshot`, `FinalDBSnapshotIdentifier`) removes the cluster and returns it with `Status: deleting`; it must fail `InvalidDBClusterStateFault` (400) when `DeletionProtection` is enabled or when member instances still exist, and `DBClusterNotFoundFault` (404) for an unknown id. After deletion, DescribeDBClusters for that id must 404.

Every one of these must compute its response from persisted state — no canned, hardcoded or echoed-input responses, and no random-id-without-storage. A create followed by a describe must return exactly what was created, and a delete must make the resource unobservable.

## State Model

In `src/state.rs` (`ComAmazonawsDocdbAmazonrdsv19State`, reached via `crate::state::service_state(&*self.engine)`):

- `clusters: BTreeMap<String /* lowercased DBClusterIdentifier */, DbClusterRecord>` where `DbClusterRecord { identifier, resource_id, arn, status, engine, engine_version, port, master_username, endpoint, reader_endpoint, cluster_create_time, backup_retention_period, preferred_backup_window, preferred_maintenance_window, storage_encrypted, kms_key_id, deletion_protection, availability_zones: Vec<String>, vpc_security_group_ids: Vec<String>, db_subnet_group: Option<String>, db_cluster_parameter_group: Option<String>, members: Vec<ClusterMember>, tags: Vec<(String,String)> }`
- `ClusterMember { db_instance_identifier: String, is_cluster_writer: bool, promotion_tier: i32 }`
- `instances: BTreeMap<String /* DBInstanceIdentifier */, DbInstanceRecord>` where `DbInstanceRecord { identifier, arn, dbi_resource_id, instance_class, engine, status, cluster_identifier, availability_zone, promotion_tier, endpoint_address, endpoint_port, instance_create_time, tags }`
- `next_resource_id: u64` — monotonic counter used to mint `cluster-XXXX` / `db-XXXX` resource ids deterministically.

Both maps are ordered so list/describe output and pagination markers are stable across calls.

All other operations (snapshots, parameter groups, subnet groups, global clusters, event subscriptions, tagging, failover, restore, start/stop) stay 501 for now and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-docdb-amazonrdsv19/OPERATIONS.md` lists 55 operations from `smithy-model/com-amazonaws-docdb-amazonrdsv19.json` (awsQuery protocol, SDK id `DocDB`, sigv4 signing name `rds`). It gives the exact contracts cited above: CreateDBCluster (line 56-60, `DBClusterIdentifier`/`Engine` required, output `DBCluster`, `DBClusterAlreadyExistsFault` 400), CreateDBInstance (line 74-78, `DBInstanceIdentifier`/`DBInstanceClass`/`Engine`/`DBClusterIdentifier` required, `DBClusterNotFoundFault` 404, `DBInstanceAlreadyExistsFault` 400), DescribeDBClusters (line 161-166, `@paginated(inputToken=Marker, outputToken=Marker, items=DBClusters)`, `DBClusterNotFoundFault` 404), DescribeDBInstances (line 188-193, same paginated shape, `DBInstanceNotFoundFault` 404), ModifyDBCluster (line 267), DeleteDBInstance (line 116-120) and DeleteDBCluster (line 98-102, `SkipFinalSnapshot`/`FinalDBSnapshotIdentifier`, `InvalidDBClusterStateFault` 400). Every handler file under `src/ops/` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and ends in `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))` — verified in `src/ops/create_db_cluster.rs:4` and `:291`. No tests assert any behaviour beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Point an AWS SDK/CLI docdb client at the emulator endpoint (awsQuery, signing name `rds`).
2. `aws docdb create-db-cluster --db-cluster-identifier my-cluster --engine docdb --master-username admin --master-user-password secret99` → currently HTTP 501 NotImplemented; must return a `DBCluster` with Status, Endpoint, ReaderEndpoint, Port 27017 and ClusterCreateTime.
3. `aws docdb create-db-instance --db-instance-identifier my-instance-1 --db-instance-class db.r5.large --engine docdb --db-cluster-identifier my-cluster` → must return a `DBInstance` bound to that cluster.
4. `aws docdb describe-db-clusters --db-cluster-identifier my-cluster` → must return the one cluster created in step 2, with `my-instance-1` present in `DBClusterMembers` and `IsClusterWriter: true`.
5. `aws docdb describe-db-instances` → must list `my-instance-1` with its DBClusterIdentifier and instance class as stored.
6. `aws docdb modify-db-cluster --db-cluster-identifier my-cluster --backup-retention-period 7 --deletion-protection` then `describe-db-clusters` → must show BackupRetentionPeriod 7 and DeletionProtection true.
7. `aws docdb delete-db-cluster --db-cluster-identifier my-cluster --skip-final-snapshot` → must fail InvalidDBClusterStateFault (HTTP 400) while deletion protection is on and member instances exist.
8. `aws docdb modify-db-cluster --db-cluster-identifier my-cluster --no-deletion-protection`, `aws docdb delete-db-instance --db-instance-identifier my-instance-1`, then `aws docdb delete-db-cluster --db-cluster-identifier my-cluster --skip-final-snapshot` → must succeed and return Status `deleting`.
9. `aws docdb describe-db-clusters --db-cluster-identifier my-cluster` → must now fail DBClusterNotFoundFault (HTTP 404).
10. Create 3 clusters and call `aws docdb describe-db-clusters --max-records 2` → must return 2 clusters plus a `Marker`; passing that Marker back must return the remaining cluster with no Marker.

Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the Neptune emulator

_Filed: 2026-10-21T22:08:13Z_
_Operation: service-com-amazonaws-neptune-amazonrdsv19 | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 70 operations in `services/service-com-amazonaws-neptune-amazonrdsv19/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Neptune client can do anything. Implement a FIRST, SMALL slice: the DB cluster lifecycle, statefully, end-to-end.

### Happy path to implement (in order)

1. **CreateDBCluster** — input `DBClusterIdentifier` (required), `Engine` (required, e.g. `neptune`), plus optional `EngineVersion`, `Port`, `DBSubnetGroupName`, `DBClusterParameterGroupName`, `BackupRetentionPeriod`, `StorageEncrypted`, `KmsKeyId`, `DeletionProtection`, `Tags`. Must persist a new cluster record and return the `DBCluster` structure built from it: `DBClusterIdentifier`, `Status` (`available`), `Engine`, `EngineVersion`, `Endpoint` (`<id>.cluster-<rand>.<region>.neptune.amazonaws.com`), `ReaderEndpoint` (`<id>.cluster-ro-...`), `Port` (default 8182), `DBClusterArn` (`arn:aws:rds:<region>:<account>:cluster:<id>`), `DbClusterResourceId`, `ClusterCreateTime`, `AllocatedStorage` (1), `DBClusterMembers` (empty at create), `AvailabilityZones`, `StorageEncrypted`, `DeletionProtection`, `BackupRetentionPeriod`. A second create with the same identifier must fail with `DBClusterAlreadyExistsFault` (HTTP 400).
2. **CreateDBInstance** — input `DBInstanceIdentifier` (required), `DBInstanceClass` (required), `Engine` (required), `DBClusterIdentifier` (required). Must look the cluster up in state and fail `DBClusterNotFoundFault` (404) when absent; on success persist the instance, add it to the owning cluster's `DBClusterMembers` (with `IsClusterWriter=true` for the first member, `DBInstanceIdentifier`, `DBClusterParameterGroupStatus`, `PromotionTier`) and return the `DBInstance` structure computed from state: `DBInstanceIdentifier`, `DBInstanceClass`, `Engine`, `DBInstanceStatus` (`available`), `DBClusterIdentifier`, `Endpoint{Address,Port,HostedZoneId}`, `DBInstanceArn`, `DbiResourceId`, `InstanceCreateTime`, `AvailabilityZone`. Duplicate identifier -> `DBInstanceAlreadyExistsFault` (400).
3. **DescribeDBClusters** — input `DBClusterIdentifier`, `MaxRecords`, `Marker`; output `DBClusters` (list<DBCluster>) + `Marker`. With no identifier, list every persisted cluster; with one, return exactly that cluster or `DBClusterNotFoundFault` (404). Honour `MaxRecords` by truncating and returning an opaque `Marker` that a follow-up call resumes from. The returned `DBCluster` must reflect everything persisted, including members added by step 2.
4. **DescribeDBInstances** — same contract for `DBInstances`/`Marker`, with `DBInstanceNotFoundFault` (404) for an unknown `DBInstanceIdentifier`, and support filtering by `db-cluster-id` so a client can list a cluster's members.
5. **DeleteDBInstance** — input `DBInstanceIdentifier` (required), `SkipFinalSnapshot`. Remove the instance from state and from its cluster's `DBClusterMembers`; return the `DBInstance` with `DBInstanceStatus=deleting`. Unknown id -> `DBInstanceNotFoundFault` (404).
6. **DeleteDBCluster** — input `DBClusterIdentifier` (required), `SkipFinalSnapshot`, `FinalDBSnapshotIdentifier`. Remove the cluster from state and return the `DBCluster` with `Status=deleting`. Unknown id -> `DBClusterNotFoundFault` (404); a cluster with `DeletionProtection=true` -> `InvalidDBClusterStateFault` (400). After deletion, DescribeDBClusters for that id must return `DBClusterNotFoundFault`.

### Rules

- NEVER STUBS: every response field above must be computed from the persisted record, not canned, echoed or randomly generated without being stored. A create followed by a describe must return exactly what was created (identifiers, engine, port, endpoints, ARNs, timestamps, members, tags).
- Errors must use the modeled fault shapes and their modeled HTTP statuses over the `awsQuery` wire protocol (see OPERATIONS.md "Wire protocol": awsQuery, sigv4 name `rds`).
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line in each implemented op file.
- Operations outside this slice (snapshots, parameter groups, subnet groups, global clusters, endpoints, events, tags, failover, restore, start/stop, modify*) legitimately stay 501 and will be filed as individual gap issues later.

## State Model

In `src/state.rs`, on the shared `ComAmazonawsNeptuneAmazonrdsv19State`:

- `clusters: BTreeMap<String /* DBClusterIdentifier */, DbClusterRecord>` where `DbClusterRecord { identifier: String, arn: String, resource_id: String, engine: String, engine_version: String, status: String, endpoint: String, reader_endpoint: String, port: i32, availability_zones: Vec<String>, db_subnet_group: Option<String>, db_cluster_parameter_group: String, backup_retention_period: i32, preferred_backup_window: String, preferred_maintenance_window: String, storage_encrypted: bool, kms_key_id: Option<String>, deletion_protection: bool, members: Vec<ClusterMember>, tags: Vec<(String,String)>, cluster_create_time: SystemTime, allocated_storage: i32, storage_type: Option<String>, network_type: Option<String> }`
- `ClusterMember { db_instance_identifier: String, is_cluster_writer: bool, promotion_tier: i32, db_cluster_parameter_group_status: String }`
- `instances: BTreeMap<String /* DBInstanceIdentifier */, DbInstanceRecord>` where `DbInstanceRecord { identifier: String, arn: String, dbi_resource_id: String, instance_class: String, engine: String, engine_version: String, status: String, cluster_identifier: String, endpoint_address: String, endpoint_port: i32, hosted_zone_id: String, availability_zone: String, publicly_accessible: bool, promotion_tier: i32, tags: Vec<(String,String)>, instance_create_time: SystemTime, deletion_protection: bool }`
- `pagination: BTreeMap<String /* Marker */, usize /* next offset */>` (or encode the offset in the opaque marker) shared by the Describe* operations.

## Done when

An integration test drives CreateDBCluster -> CreateDBInstance -> DescribeDBClusters -> DescribeDBInstances -> DeleteDBInstance -> DeleteDBCluster -> DescribeDBClusters(404) with full body fidelity, not merely 2xx.

## Evidence

`services/service-com-amazonaws-neptune-amazonrdsv19/OPERATIONS.md` lists 70 operations for sdkId `Neptune`, protocol `awsQuery`, including CreateDBCluster (`DBClusterIdentifier`/`Engine` required, output `DBCluster`, error `DBClusterAlreadyExistsFault` 400), CreateDBInstance (`DBInstanceIdentifier`/`DBInstanceClass`/`Engine`/`DBClusterIdentifier` required, output `DBInstance`, errors `DBClusterNotFoundFault` 404 / `DBInstanceAlreadyExistsFault` 400), DescribeDBClusters and DescribeDBInstances (both `@paginated(inputToken=Marker, outputToken=Marker, items=DBClusters|DBInstances)`, errors `DBClusterNotFoundFault`/`DBInstanceNotFoundFault` 404), DeleteDBInstance and DeleteDBCluster (output the deleted resource, errors `...NotFoundFault` 404 and `InvalidDBClusterStateFault` 400). The `DBCluster` and `DBInstance` response member lists above come from the Smithy model shapes (`smithy_query shape=DBCluster,DBInstance`). Every handler file under `src/ops/` still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` — verified in `src/ops/create_db_cluster.rs` lines 3-12 — so there is no behavior and no state today, and no tests assert any of the above.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Neptune (rds endpoint) client at it.
2. `aws neptune create-db-cluster --db-cluster-identifier my-graph --engine neptune` -> expect a DBCluster with Status=available, Endpoint, Port 8182 and an ARN; actual: HTTP 501 NotImplemented.
3. `aws neptune create-db-instance --db-instance-identifier my-graph-1 --db-instance-class db.r5.large --engine neptune --db-cluster-identifier my-graph` -> expect DBInstance available and attached to the cluster.
4. `aws neptune describe-db-clusters` -> expect exactly the cluster created in step 2 with my-graph-1 listed in DBClusterMembers (IsClusterWriter=true).
5. `aws neptune describe-db-instances --db-instance-identifier my-graph-1` -> expect the persisted instance; `--db-instance-identifier nope` -> expect DBInstanceNotFoundFault (404).
6. `aws neptune delete-db-instance --db-instance-identifier my-graph-1 --skip-final-snapshot` then `aws neptune delete-db-cluster --db-cluster-identifier my-graph --skip-final-snapshot` -> expect status `deleting` echoed from state.
7. `aws neptune describe-db-clusters --db-cluster-identifier my-graph` -> expect DBClusterNotFoundFault (404).

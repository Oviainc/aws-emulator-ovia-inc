Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the redshift emulator

_Filed: 2026-10-21T21:28:20Z_
_Operation: service-com-amazonaws-redshift-redshiftserviceversion20121201 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 145 operations in `services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds zero state. Nothing works end-to-end today.

This issue defines the FIRST, SMALL slice: the provisioned-cluster lifecycle, which is Redshift's primary resource and the loop every client script starts with. Implement exactly these 6 operations (awsQuery protocol, sigv4 signing name `redshift`), each computing its response from persisted state:

1. **CreateCluster** — required input `ClusterIdentifier`, `NodeType`, `MasterUsername`; commonly supplied optional input `DBName` (default `dev`), `ClusterType` (`single-node`/`multi-node`), `NumberOfNodes` (default 1), `MasterUserPassword`, `Port` (default 5439), `ClusterVersion`, `AvailabilityZone`, `PubliclyAccessible`, `Encrypted`, `AutomatedSnapshotRetentionPeriod`, `Tags`. Output: `Cluster`. Must insert a new Cluster record into state and return it with `ClusterStatus="creating"`, `ClusterCreateTime` set, `Endpoint{Address,Port}` derived from the identifier, and `MasterUserPassword` NEVER echoed back. A second CreateCluster with the same `ClusterIdentifier` must fail `ClusterAlreadyExistsFault` (HTTP 400).

2. **DescribeClusters** — input `ClusterIdentifier`, `MaxRecords`, `Marker`, `TagKeys`, `TagValues`; output `Clusters` (list<Cluster>) + `Marker`. Must list clusters read from state, filter by `ClusterIdentifier` when given, and raise `ClusterNotFoundFault` (HTTP 404) for an unknown identifier. Honour `MaxRecords`/`Marker` (the op is `@paginated(inputToken=Marker, outputToken=Marker, items=Clusters)`) — truncate the page and return a `Marker` only when more remain. A cluster created in step 1 must appear here with the same field values. A cluster that has transitioned to `available` must report `ClusterStatus="available"` (a simple deterministic creating -> available transition on first describe is acceptable for the bootstrap).

3. **ModifyCluster** — required input `ClusterIdentifier`; at minimum apply `NodeType`, `NumberOfNodes`, `ClusterType`, `AutomatedSnapshotRetentionPeriod`, `PreferredMaintenanceWindow`, `AllowVersionUpgrade`, `PubliclyAccessible`, `Port`, and `NewClusterIdentifier` (which re-keys the stored cluster). Output `Cluster`. The mutation must persist: a following DescribeClusters must show the new values. Unknown identifier -> `ClusterNotFoundFault` (404); `NewClusterIdentifier` colliding with an existing cluster -> `ClusterAlreadyExistsFault` (400).

4. **CreateClusterSnapshot** — required input `SnapshotIdentifier`, `ClusterIdentifier`; optional `ManualSnapshotRetentionPeriod`, `Tags`. Output `Snapshot`. Must copy the identifying attributes off the stored cluster (NodeType, NumberOfNodes, MasterUsername, DBName, ClusterVersion, Port, AvailabilityZone, ClusterCreateTime) into a new Snapshot record with `SnapshotType="manual"`, `Status="available"`, `SnapshotCreateTime` set. Unknown cluster -> `ClusterNotFoundFault` (404); duplicate snapshot id -> `ClusterSnapshotAlreadyExistsFault` (400).

5. **DescribeClusterSnapshots** — input `ClusterIdentifier`, `SnapshotIdentifier`, `SnapshotType`, `MaxRecords`, `Marker`; output `Snapshots` + `Marker`. Must return snapshots from state, filtered by `ClusterIdentifier`/`SnapshotIdentifier`/`SnapshotType`, paginated via `Marker`/`MaxRecords`. Unknown `SnapshotIdentifier` -> `ClusterSnapshotNotFoundFault` (404).

6. **DeleteCluster** — required input `ClusterIdentifier`; optional `SkipFinalClusterSnapshot`, `FinalClusterSnapshotIdentifier`, `FinalClusterSnapshotRetentionPeriod`. Output `Cluster` with `ClusterStatus="deleting"`. Must remove the cluster from state so a later DescribeClusters for it returns `ClusterNotFoundFault` (404). When `SkipFinalClusterSnapshot` is false/absent a `FinalClusterSnapshotIdentifier` is required (otherwise `InvalidParameterCombination`/`InvalidClusterStateFault`), and the final snapshot must be created in state and visible via DescribeClusterSnapshots with `SnapshotType="manual"`.

No canned, hardcoded, or echoed responses: every one of these six must read/write the shared store, so that create -> describe -> modify -> snapshot -> delete -> describe-gone is observably consistent across calls. Random IDs returned without storage count as a failure.

## State Model

A single service store (behind the crate's existing state lock), holding:

- `clusters: Map<String /*ClusterIdentifier*/, Cluster>` where `Cluster { cluster_identifier, node_type, cluster_status, cluster_availability_status, modify_status, master_username, db_name, endpoint: Endpoint{address, port}, cluster_create_time, automated_snapshot_retention_period, manual_snapshot_retention_period, cluster_security_groups: Vec<ClusterSecurityGroupMembership>, vpc_security_groups: Vec<VpcSecurityGroupMembership>, cluster_parameter_groups: Vec<ClusterParameterGroupStatus>, cluster_subnet_group_name, vpc_id, availability_zone, preferred_maintenance_window, cluster_version, allow_version_upgrade, number_of_nodes, publicly_accessible, encrypted, kms_key_id, cluster_type, iam_roles, tags: Vec<Tag>, master_user_password (stored, never serialized) }`.
- `snapshots: Map<String /*SnapshotIdentifier*/, Snapshot>` where `Snapshot { snapshot_identifier, cluster_identifier, snapshot_create_time, status, port, availability_zone, cluster_create_time, master_username, cluster_version, snapshot_type, node_type, number_of_nodes, db_name, encrypted, owner_account, manual_snapshot_retention_period, total_backup_size_in_mega_bytes, tags }`.
- Insertion order preserved (e.g. `IndexMap` or an auxiliary `Vec` of keys) so that `Marker` pagination over clusters and snapshots is stable and resumable; the `Marker` is an opaque offset/key cursor.
- `account_id` / `region` taken from the request context so endpoint addresses and ARNs are rendered consistently.

All other 139 operations stay 501 for now and will be filed as individual gap issues later; that is expected and correct for this first slice.

## Evidence

`services/.../OPERATIONS.md` line 3 ("145 operations"), line 10 (`Protocol: awsQuery`, SDK id `Redshift`, signing name `redshift`) and the per-operation entries cited above: CreateCluster (line 98-102: required `ClusterIdentifier`/`NodeType`/`MasterUsername`, output `Cluster`, error `ClusterAlreadyExistsFault` 400), CreateClusterSnapshot (116-120: required `SnapshotIdentifier`/`ClusterIdentifier`, output `Snapshot`, errors `ClusterNotFoundFault` 404 / `ClusterSnapshotAlreadyExistsFault` 400), DeleteCluster (218-222: `SkipFinalClusterSnapshot`/`FinalClusterSnapshotIdentifier`, output `Cluster`), DescribeClusters (377-382: `@paginated(inputToken=Marker, outputToken=Marker, items=Clusters)`, error `ClusterNotFoundFault` 404), DescribeClusterSnapshots (391-396: `@paginated(... items=Snapshots)`, error `ClusterSnapshotNotFoundFault` 404), ModifyCluster (724-728: `NewClusterIdentifier`, output `Cluster`, errors `ClusterNotFoundFault` 404 / `ClusterAlreadyExistsFault` 400). All-stub status confirmed by `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/*.rs`, which matches every handler file including create_cluster.rs, describe_clusters.rs, modify_cluster.rs, create_cluster_snapshot.rs, describe_cluster_snapshots.rs and delete_cluster.rs. No tests exist for these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `redshift` client at it.
2. `aws redshift create-cluster --cluster-identifier demo-cluster --node-type ra3.xlplus --master-username admin --master-user-password Passw0rd! --cluster-type single-node` -> expect HTTP 200 with `Cluster.ClusterIdentifier=demo-cluster`, `ClusterStatus=creating`, `Endpoint.Port=5439`, and no password echoed. Actual: HTTP 501 NotImplemented.
3. `aws redshift describe-clusters --cluster-identifier demo-cluster` -> expect the cluster just created, with the same NodeType/MasterUsername/DBName. Actual: HTTP 501.
4. `aws redshift modify-cluster --cluster-identifier demo-cluster --automated-snapshot-retention-period 7` then describe-clusters again -> expect the retention period to have persisted as 7. Actual: HTTP 501.
5. `aws redshift create-cluster-snapshot --snapshot-identifier demo-snap --cluster-identifier demo-cluster` -> expect `Snapshot.Status=available`, `SnapshotType=manual`, NodeType/MasterUsername copied from the cluster. Actual: HTTP 501.
6. `aws redshift describe-cluster-snapshots --cluster-identifier demo-cluster` -> expect `demo-snap` listed. Actual: HTTP 501.
7. `aws redshift delete-cluster --cluster-identifier demo-cluster --final-cluster-snapshot-identifier demo-final` -> expect 200 with `ClusterStatus=deleting`; then `describe-clusters --cluster-identifier demo-cluster` must fail `ClusterNotFoundFault` (404) and `describe-cluster-snapshots` must list `demo-final`. Actual: HTTP 501 at every step.
8. Re-run `create-cluster` with the same identifier while it exists -> expect `ClusterAlreadyExistsFault` (HTTP 400), not 501/200.

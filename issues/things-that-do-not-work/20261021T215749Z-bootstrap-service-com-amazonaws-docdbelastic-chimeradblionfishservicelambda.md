Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the DocDB Elastic (docdb-elastic) emulator

_Filed: 2026-10-21T21:57:49Z_
_Operation: service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 19 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. Implement a FIRST SLICE: the elastic-cluster lifecycle, stateful end to end. Protocol is `restJson1`, sigv4 name `docdb-elastic`.

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateCluster** — `POST /cluster`. Input: `clusterName`, `authType` (PLAIN_TEXT|SECRET_ARN), `adminUserName`, `adminUserPassword`, `shardCapacity`, `shardCount` (all required), plus optional `vpcSecurityGroupIds`, `subnetIds`, `kmsKeyId`, `preferredMaintenanceWindow`, `preferredBackupWindow`, `backupRetentionPeriod`, `shardInstanceCount`, `tags`, `clientToken`. Must MINT and PERSIST a new cluster record keyed by a generated `clusterArn` (`arn:aws:docdb-elastic:<region>:<account>:cluster/<uuid>`), set `createTime` (UTC string), `clusterEndpoint` (a derived host name), and `status` (a Status value such as CREATING, settling to ACTIVE). Output: `cluster` (Cluster) built from the stored record — every @required Cluster member populated (`clusterName`, `clusterArn`, `status`, `clusterEndpoint`, `createTime`, `adminUserName`, `authType`, `shardCapacity`, `shardCount`, `vpcSecurityGroupIds`, `subnetIds`, `preferredMaintenanceWindow`, `kmsKeyId`). `adminUserPassword` must never appear in any response.
2. **GetCluster** — `GET /cluster/{clusterArn}`. Looks the ARN up in the store and returns the SAME `cluster` structure that CreateCluster persisted (same name, shardCapacity, shardCount, subnetIds, createTime). Unknown ARN -> `ResourceNotFoundException` (HTTP 404).
3. **ListClusters** — `GET /clusters`. Returns `clusters` as a list of `ClusterInList{clusterName, clusterArn, status}` for every stored cluster, honouring `maxResults` and the opaque `nextToken` (@paginated inputToken/outputToken=nextToken, items=clusters): page size must be respected and `nextToken` must be absent on the last page.
4. **UpdateCluster** — `PUT /cluster/{clusterArn}`. Applies only the members present (`shardCapacity`, `shardCount`, `shardInstanceCount`, `authType`, `vpcSecurityGroupIds`, `subnetIds`, `adminUserPassword`, `preferredMaintenanceWindow`, `preferredBackupWindow`, `backupRetentionPeriod`) to the stored record — omitted members keep their previous values — and returns the updated `cluster`. The change must be visible to a following GetCluster. Unknown ARN -> 404.
5. **DeleteCluster** — `DELETE /cluster/{clusterArn}`. Removes the cluster from the store and returns the final `cluster` (status DELETING). A following GetCluster must then fail with `ResourceNotFoundException` (404), and ListClusters must no longer include it. Second DeleteCluster on the same ARN -> 404.
6. **CreateClusterSnapshot / GetClusterSnapshot** (stretch, same slice) — `POST /cluster-snapshot` with `clusterArn` + `snapshotName` persists a snapshot record keyed by a generated `snapshotArn`, copying `subnetIds`, `vpcSecurityGroupIds`, `adminUserName`, `kmsKeyId`, `clusterCreationTime` from the referenced cluster and setting `snapshotCreationTime`, `status`, `snapshotType=manual`; unknown `clusterArn` -> 404. `GET /cluster-snapshot/{snapshotArn}` returns that stored record.

NEVER STUBS: each of these must compute its response from persisted state. Hardcoded structures, echoing the request back without storing it, or minting an ARN that is never recorded are failures — a create followed by a get/list must return what was created, and a delete must make the resource genuinely unreachable.

## State Model

In `src/state.rs` (the per-service store reached via `crate::state::service_state(&*self.engine)`):

- `clusters: BTreeMap<String /* clusterArn */, Cluster>` where `Cluster { cluster_name, cluster_arn, status, cluster_endpoint, create_time, admin_user_name, admin_user_password (never serialized), auth_type, shard_capacity, shard_count, shard_instance_count, vpc_security_group_ids: Vec<String>, subnet_ids: Vec<String>, preferred_maintenance_window, preferred_backup_window, backup_retention_period, kms_key_id, shards: Vec<Shard>, tags: BTreeMap<String,String> }`
- `cluster_names: BTreeMap<String /* clusterName */, String /* clusterArn */>` so a duplicate `clusterName` can raise `ConflictException` (409)
- `snapshots: BTreeMap<String /* snapshotArn */, ClusterSnapshot>` where `ClusterSnapshot { snapshot_name, snapshot_arn, cluster_arn, snapshot_creation_time, cluster_creation_time, status, snapshot_type, admin_user_name, kms_key_id, subnet_ids, vpc_security_group_ids, tags }`
- `tags: BTreeMap<String /* resourceArn */, BTreeMap<String,String>>` (or the per-record `tags` field above) so TagResource/ListTagsForResource can be layered on next

Operations outside this slice (CopyClusterSnapshot, DeleteClusterSnapshot, ListClusterSnapshots, RestoreClusterFromSnapshot, StartCluster, StopCluster, Tag/Untag/ListTagsForResource, the PendingMaintenanceAction trio) stay at 501 and are filed as individual gap issues later.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/` matches all 19 handler files (create_cluster.rs, get_cluster.rs, list_clusters.rs, update_cluster.rs, delete_cluster.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501 with zero persisted state. Contracts above are from the crate's OPERATIONS.md (routes, required inputs, @paginated traits, modeled errors incl. ResourceNotFoundException 404 / ConflictException 409) and from the Smithy shapes `Cluster`, `ClusterInList`, `ClusterSnapshot`, `ClusterSnapshotInList` (required-member lists via smithy_query).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK docdb-elastic client at it.
2. CreateCluster(clusterName='demo', authType='PLAIN_TEXT', adminUserName='admin', adminUserPassword='Secret123!', shardCapacity=2, shardCount=1) -> expect 200 with cluster.clusterArn, status, clusterEndpoint, createTime; actual: HTTP 501 NotImplemented.
3. GetCluster(clusterArn=<arn from step 2>) -> expect the same clusterName/shardCapacity/shardCount/createTime that were persisted.
4. ListClusters(maxResults=1) -> expect the cluster present as ClusterInList{clusterName, clusterArn, status} and a nextToken only when more pages remain.
5. UpdateCluster(clusterArn=<arn>, shardCount=2) -> expect 200, then GetCluster shows shardCount=2 and all other fields unchanged.
6. CreateClusterSnapshot(clusterArn=<arn>, snapshotName='snap1') then GetClusterSnapshot(snapshotArn=<arn from create>) -> expect the stored snapshot echoing the cluster's subnetIds/adminUserName/kmsKeyId.
7. DeleteCluster(clusterArn=<arn>) -> expect 200 with status DELETING; then GetCluster(<arn>) -> expect ResourceNotFoundException (HTTP 404) and ListClusters no longer lists it.
8. GetCluster with a never-created ARN -> expect ResourceNotFoundException (HTTP 404), not 501.

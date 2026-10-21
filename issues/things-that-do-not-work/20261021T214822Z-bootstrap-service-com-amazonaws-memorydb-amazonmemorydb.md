Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the MemoryDB emulator

_Filed: 2026-10-21T21:48:22Z_
_Operation: service-com-amazonaws-memorydb-amazonmemorydb | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 45 operations in `services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no MemoryDB client workflow can run at all. This issue defines the FIRST, SMALL slice: the cluster lifecycle, implemented statefully end-to-end. Everything else stays 501 and is filed later as individual gap issues.

Wire protocol (README.md / OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: AmazonMemoryDB.<OperationName>`, sigv4 signing name `memorydb`, ARN namespace `memorydb`.

## Happy path to implement (in order)

1. **CreateACL** — in: `ACLName` (required), `UserNames`, `Tags`. Out: `ACL` { `Name`, `Status`, `UserNames`, `ARN`, `Clusters` }. Must insert a new ACL into state keyed by name and return it with `Status` = `active` (or `creating` then settling to `active`) and a generated `ARN` of the form `arn:aws:memorydb:<region>:<account>:acl/<ACLName>`. Re-creating an existing name must fail with `ACLAlreadyExistsFault` (HTTP 400).
2. **CreateCluster** — in: `ClusterName` (required), `NodeType` (required), `ACLName` (required), plus `Description`, `NumShards`, `NumReplicasPerShard`, `SubnetGroupName`, `ParameterGroupName`, `Port`, `TLSEnabled`, `SnapshotRetentionLimit`, `MaintenanceWindow`, `SnapshotWindow`, `Engine`, `EngineVersion`, `Tags`. Out: `Cluster` { `Name`, `Status`, `NodeType`, `NumberOfShards`, `ACLName`, `ClusterEndpoint`, `ARN`, `TLSEnabled`, `ParameterGroupName`, `SubnetGroupName`, `Description`, `EngineVersion`, `MaintenanceWindow`, `SnapshotRetentionLimit`, `Shards` }. Must persist the cluster keyed by `ClusterName`, echo back ONLY values actually stored, default `NumberOfShards` to 1 when `NumShards` is absent, synthesize a `ClusterEndpoint` ({Address, Port} with Port defaulting to 6379), and set `Status` = `creating`. Must reject a duplicate name with `ClusterAlreadyExistsFault` (400) and an unknown `ACLName` with `ACLNotFoundFault` (404).
3. **DescribeClusters** — in: `ClusterName`, `MaxResults`, `NextToken`, `ShowShardDetails`. Out: `Clusters` (list<Cluster>), `NextToken`. Must read from state: no `ClusterName` lists every stored cluster; a given `ClusterName` returns exactly that one, and an unknown one returns `ClusterNotFoundFault` (404). The cluster returned must carry the fields stored by CreateCluster/UpdateCluster, not canned values. `Status` should have settled to `available` by the time a client polls (either immediately or after the first describe). `Shards` populated only when `ShowShardDetails` is true. Honour `MaxResults` + `NextToken` for pagination (`@paginated(items=Clusters)`).
4. **UpdateCluster** — in: `ClusterName` (required) plus any of `Description`, `NodeType`, `MaintenanceWindow`, `SnapshotWindow`, `SnapshotRetentionLimit`, `ParameterGroupName`, `ACLName`, `ShardConfiguration`, `EngineVersion`. Out: the mutated `Cluster`. Must apply the supplied fields to the stored cluster so a following DescribeClusters shows the new values; unknown cluster → `ClusterNotFoundFault` (404); an update with no mutating member → `NoOperationFault` (400).
5. **DeleteCluster** — in: `ClusterName` (required), optional `FinalSnapshotName`. Out: the `Cluster` as it was, with `Status` = `deleting`. Must remove the cluster from state so the next DescribeClusters for that name returns `ClusterNotFoundFault` (404); deleting a name that does not exist returns `ClusterNotFoundFault` (404).
6. **DescribeACLs** — in: `ACLName`, `MaxResults`, `NextToken`. Out: `ACLs`, `NextToken`. Must list ACLs from state, unknown name → `ACLNotFoundFault` (404), and each ACL's `Clusters` member must list the names of clusters currently referencing it (so step 2 is observable here).

Every one of these must compute its response from persisted state. No canned responses, no blind echo of the request, and no generated ARN/endpoint that is not stored and returned identically by the subsequent describe.

## State Model

In `src/state.rs` (reached via `crate::state::service_state(&*self.engine)`, one shared engine per crate):

- `clusters: BTreeMap<String /*ClusterName*/, Cluster>` where `Cluster { name, description, status: String ("creating"|"available"|"modifying"|"deleting"), node_type, number_of_shards: i32, num_replicas_per_shard: i32, acl_name, parameter_group_name, parameter_group_status, subnet_group_name, security_group_ids: Vec<String>, maintenance_window, snapshot_window, snapshot_retention_limit: i32, sns_topic_arn, tls_enabled: bool, engine, engine_version, port: i32, endpoint_address: String, arn: String, shards: Vec<Shard>, created_at }`.
- `Shard { name: String ("0001", "0002", ...), status, slots: String, number_of_nodes: i32, nodes: Vec<Node{name, status, availability_zone, endpoint{address, port}, create_time}> }` — generated from `number_of_shards`/`num_replicas_per_shard` at create time and persisted, not recomputed differently per call.
- `acls: BTreeMap<String /*ACLName*/, Acl { name, status, user_names: Vec<String>, minimum_engine_version, arn, clusters: derived from `clusters` by `acl_name` }>`.
- `tags: BTreeMap<String /*ResourceArn*/, Vec<(String, String)>>` populated by the `Tags` member on CreateACL/CreateCluster, so a later ListTags/TagResource slice can build on it.
- Deterministic ARN/endpoint helpers: `arn:aws:memorydb:<region>:<account>:cluster/<name>` and `<name>.<suffix>.clustercfg.memorydb.<region>.amazonaws.com`, using the region/account from the request context.

## Done when

A realistic client script (AWS CLI or SDK) runs CreateACL → CreateCluster → DescribeClusters → UpdateCluster → DescribeClusters → DeleteCluster → DescribeClusters end-to-end with correct, stateful bodies: the described cluster matches what was created, the update is visible in the next describe, and the final describe reports the cluster gone with `ClusterNotFoundFault`.

## Evidence

OPERATIONS.md for this crate lists all 45 operations with their input/output members and errors: CreateACL (ACLName required → ACL, ACLAlreadyExistsFault 400, UserNotFoundFault 404), CreateCluster (ClusterName/NodeType/ACLName required → Cluster, ClusterAlreadyExistsFault 400, ACLNotFoundFault 404, SubnetGroupNotFoundFault 404), DescribeClusters (@paginated inputToken=NextToken, items=Clusters; ClusterNotFoundFault 404), UpdateCluster (ClusterName required, NoOperationFault 400, ClusterNotFoundFault 404), DeleteCluster (ClusterName required → Cluster, ClusterNotFoundFault 404), DescribeACLs (@paginated items=ACLs, ACLNotFoundFault 404). The Cluster, ACL and Shard output shapes were read from the Smithy model (`smithy_query shape=Cluster,ACL,Shard`) and supply the field names used in the State Model above — notably ACL.Clusters ("A list of clusters associated with the ACL") and Cluster.ClusterEndpoint/ARN/Shards. All 45 files under src/ops/ currently match `^//! SCAFFOLDED, NOT IMPLEMENTED:` and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501; there are no tests asserting any MemoryDB behaviour.

## Steps to Reproduce

1. Start the emulator and point the AWS CLI at it: export AWS_ENDPOINT_URL=http://localhost:<port>
2. aws memorydb create-acl --acl-name demo-acl  → expect 200 with ACL{Name=demo-acl, Status=active/creating, ARN}; actual: HTTP 501 NotImplemented
3. aws memorydb create-cluster --cluster-name demo --node-type db.t4g.small --acl-name demo-acl --num-shards 2  → expect 200 with Cluster{Name=demo, Status=creating, NodeType=db.t4g.small, NumberOfShards=2, ACLName=demo-acl, ClusterEndpoint, ARN}
4. aws memorydb describe-clusters --cluster-name demo  → expect the same cluster read back from state (NodeType/NumberOfShards/ACLName/ARN identical), Status available
5. aws memorydb describe-acls --acl-name demo-acl  → expect ACL.Clusters to contain "demo"
6. aws memorydb update-cluster --cluster-name demo --description 'updated' --snapshot-retention-limit 3  → expect 200, then describe-clusters --cluster-name demo shows Description=updated and SnapshotRetentionLimit=3
7. aws memorydb update-cluster --cluster-name demo  (no mutating field) → expect NoOperationFault, HTTP 400
8. aws memorydb describe-clusters --cluster-name nope  → expect ClusterNotFoundFault, HTTP 404
9. aws memorydb delete-cluster --cluster-name demo  → expect 200 with Cluster.Status=deleting
10. aws memorydb describe-clusters --cluster-name demo  → expect ClusterNotFoundFault, HTTP 404 (cluster actually removed from state)

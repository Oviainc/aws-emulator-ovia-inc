Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: b77b17147924de9dec9883e27401834d58e9f348

# Bootstrap the ElastiCache emulator

_Filed: 2026-10-21T22:44:02Z_
_Operation: service-com-amazonaws-elasticache-amazonelasticachev9 | Class: stub_

> _Note: this issue was identified against the code base at commit `b77b17147924de9dec9883e27401834d58e9f348`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 77 operations in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. No client workflow works today.

Implement a FIRST, SMALL stateful slice: the cache-cluster lifecycle, which is ElastiCache's primary resource. Wire protocol is `awsQuery` (SDK id `ElastiCache`, signing name `elasticache`) per OPERATIONS.md, so responses must be the Query-protocol XML wrappers the generated serializer already produces.

Happy path, in order (field names from OPERATIONS.md):

1. **CreateCacheCluster** — input `CacheClusterId` (required), `Engine`, `EngineVersion`, `CacheNodeType`, `NumCacheNodes`, `Port`, `PreferredAvailabilityZone`, `CacheSubnetGroupName`, `PreferredMaintenanceWindow`, `SnapshotRetentionLimit`, `SnapshotWindow`, `AutoMinorVersionUpgrade`, `Tags`. Store a new cluster record and return `CacheCluster` built from it: `CacheClusterId`, `CacheClusterStatus` = `creating`, `Engine`, `EngineVersion`, `CacheNodeType`, `NumCacheNodes`, `CacheClusterCreateTime`, `ConfigurationEndpoint` (Address/Port, synthesized once and stored), `ARN` (`arn:aws:elasticache:<region>:<account>:cluster:<CacheClusterId>`), `PreferredAvailabilityZone`, `CacheParameterGroup`, `CacheSubnetGroupName`. A second create with the same id must fail `CacheClusterAlreadyExistsFault` (HTTP 400).
2. **DescribeCacheClusters** — input `CacheClusterId`, `MaxRecords`, `Marker`, `ShowCacheNodeInfo`. With no filter return every stored cluster; with `CacheClusterId` return just that one, or `CacheClusterNotFoundFault` (404) if absent. Honour `MaxRecords` by truncating and returning an opaque `Marker` that the next call resumes from. When `ShowCacheNodeInfo` is true populate `CacheNodes` from the stored node list, otherwise omit it. Cluster status must have transitioned to `available` on a subsequent describe (store the create timestamp and derive the status from it — no unconditional canned `available`, and no hidden mutation other than materialising the clock-driven transition).
3. **ModifyCacheCluster** — input `CacheClusterId` (required), `NumCacheNodes`, `CacheNodeType`, `EngineVersion`, `PreferredMaintenanceWindow`, `SnapshotRetentionLimit`, `SnapshotWindow`, `AutoMinorVersionUpgrade`, `ApplyImmediately`. Must mutate the stored record (when `ApplyImmediately` is true) or record the change under `PendingModifiedValues` (when false/absent), return the updated `CacheCluster`, and the change must be visible to the next DescribeCacheClusters. Unknown id → `CacheClusterNotFoundFault` (404).
4. **DeleteCacheCluster** — input `CacheClusterId` (required). Must mark the stored cluster `deleting` and remove it, returning the final `CacheCluster` snapshot with `CacheClusterStatus` = `deleting`. Unknown id → `CacheClusterNotFoundFault` (404). A following DescribeCacheClusters for that id must return `CacheClusterNotFoundFault` (404) and the unfiltered describe must no longer list it.
5. **CreateCacheSubnetGroup** + **DescribeCacheSubnetGroups** (supporting pair, both small) — input `CacheSubnetGroupName`, `CacheSubnetGroupDescription`, `SubnetIds` (all required). Persist the group so that CreateCacheCluster's `CacheSubnetGroupName` can be validated against it (`CacheSubnetGroupNotFoundFault`, HTTP 400, when unknown) and so Describe echoes what was created, not the input of the current call. Duplicate name → `CacheSubnetGroupAlreadyExistsFault` (400).

Every one of these must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)`. No canned responses, no echoing the request back, no generating an id/ARN without storing it. A create followed by a describe must return what was created.

## State Model

In `src/state.rs`, on `ComAmazonawsElasticacheAmazonelasticachev9State`:

- `cache_clusters: BTreeMap<String /*CacheClusterId*/, CacheClusterRecord>` — insertion/sorted-by-id order drives Describe output order.
  - `CacheClusterRecord { cache_cluster_id: String, engine: String /*default "redis"*/, engine_version: String, cache_node_type: String, num_cache_nodes: i32, status: String /*creating|available|modifying|deleting*/, created_at: SystemTime, endpoint_address: String, endpoint_port: i32, arn: String, preferred_availability_zone: Option<String>, preferred_maintenance_window: String, cache_subnet_group_name: Option<String>, cache_parameter_group_name: String, snapshot_retention_limit: Option<i32>, snapshot_window: Option<String>, auto_minor_version_upgrade: Option<bool>, nodes: Vec<CacheNodeRecord>, pending_modified_values: PendingValues, tags: Vec<(String, String)> }`
  - `CacheNodeRecord { cache_node_id: String /*"0001"../"000N"*/, status: String, create_time: SystemTime, endpoint_address: String, endpoint_port: i32, availability_zone: Option<String>, parameter_group_status: String }`
  - `PendingValues { num_cache_nodes: Option<i32>, cache_node_type: Option<String>, engine_version: Option<String>, cache_node_ids_to_remove: Vec<String> }`
- `cache_subnet_groups: BTreeMap<String /*CacheSubnetGroupName*/, CacheSubnetGroupRecord { name: String, description: String, subnet_ids: Vec<String>, vpc_id: String, arn: String }>`
- `describe_markers: BTreeMap<String /*opaque marker*/, usize /*offset into the ordered cluster list*/>` (or encode the offset in the marker string) so `MaxRecords`/`Marker` pagination is real and resumable.

All other operations (replication groups, serverless caches, snapshots, users/user groups, global replication groups, parameter groups, migration and update-action APIs) stay 501 for now and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-elasticache-amazonelasticachev9/OPERATIONS.md` lists all 77 operations with their input/output members and errors: CreateCacheCluster (`CacheClusterId` required → `CacheCluster`, errors incl. `CacheClusterAlreadyExistsFault`), DescribeCacheClusters (`@paginated(inputToken=Marker, outputToken=Marker, items=CacheClusters)`, errors incl. `CacheClusterNotFoundFault` 404), ModifyCacheCluster, DeleteCacheCluster, CreateCacheSubnetGroup/DescribeCacheSubnetGroups. The `CacheCluster` output shape in the Smithy model (`smithy_query shape=CacheCluster`) defines `CacheClusterId`, `CacheClusterStatus` (documented values available/creating/deleting/modifying/...), `ConfigurationEndpoint`, `CacheNodes`, `PendingModifiedValues`, `CacheClusterCreateTime`, `ARN`. Proof of the scaffold: `src/ops/create_cache_cluster.rs` lines 4 and 312 — `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented` and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateCacheCluster"))`; every file under `src/ops/` is the same shape. No integration test exercises any cluster behaviour (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI elasticache client at it.
2. CreateCacheSubnetGroup CacheSubnetGroupName=demo-subnets CacheSubnetGroupDescription=demo SubnetIds=subnet-1,subnet-2 → 200 with CacheSubnetGroup echoing stored values.
3. DescribeCacheSubnetGroups → the demo-subnets group is listed.
4. CreateCacheCluster CacheClusterId=demo-cluster Engine=redis CacheNodeType=cache.t3.micro NumCacheNodes=1 CacheSubnetGroupName=demo-subnets → 200 with CacheCluster{CacheClusterId=demo-cluster, CacheClusterStatus=creating, ARN, ConfigurationEndpoint}.
5. CreateCacheCluster with the same CacheClusterId again → HTTP 400 CacheClusterAlreadyExistsFault.
6. CreateCacheCluster with CacheSubnetGroupName=nope → HTTP 400 CacheSubnetGroupNotFoundFault.
7. DescribeCacheClusters CacheClusterId=demo-cluster ShowCacheNodeInfo=true → the stored cluster, status available, CacheNodes populated with 1 node.
8. ModifyCacheCluster CacheClusterId=demo-cluster NumCacheNodes=3 ApplyImmediately=true → 200; DescribeCacheClusters then reports NumCacheNodes=3 and 3 CacheNodes.
9. DescribeCacheClusters MaxRecords=1 with several clusters created → exactly 1 cluster plus a Marker; re-issuing with that Marker returns the next page with no repeats.
10. DeleteCacheCluster CacheClusterId=demo-cluster → 200 with CacheClusterStatus=deleting.
11. DescribeCacheClusters CacheClusterId=demo-cluster → HTTP 404 CacheClusterNotFoundFault, and the unfiltered describe no longer lists it.
12. ModifyCacheCluster / DeleteCacheCluster on an unknown id → HTTP 404 CacheClusterNotFoundFault.

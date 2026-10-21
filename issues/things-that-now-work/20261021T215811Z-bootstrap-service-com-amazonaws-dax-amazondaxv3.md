Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the DAX (AmazonDAXV3) emulator

_Filed: 2026-10-21T21:58:11Z_
_Operation: service-com-amazonaws-dax-amazondaxv3 | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 21 operations in `services/service-com-amazonaws-dax-amazondaxv3/src/ops/` are unmodified scaffolds: each carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and answers HTTP 501 `NotImplemented`, holding zero state. Nothing about the DAX control plane works today.

Implement a FIRST minimal, stateful slice: the cluster lifecycle of the primary resource (a DAX cluster), plus the subnet group it needs. Wire protocol is `awsJson1_1`, routed as `POST /` with `X-Amz-Target: AmazonDAXV3.<OperationName>` (README/OPERATIONS.md).

Happy path, in order:

1. **CreateSubnetGroup** — input `SubnetGroupName` (required), `SubnetIds` (required list<string>), `Description`. Output `SubnetGroup` built from what was stored (`SubnetGroupName`, `Description`, `VpcId`, `Subnets` derived from `SubnetIds`). Re-creating the same name must fail with `SubnetGroupAlreadyExistsFault`.
2. **CreateCluster** — input `ClusterName` (required), `NodeType` (required), `ReplicationFactor` (required, default 0), `IamRoleArn` (required), `SubnetGroupName`, `Description`, `SecurityGroupIds`, `AvailabilityZones`, `PreferredMaintenanceWindow`, `ParameterGroupName`, `Tags`, `SSESpecification`, `ClusterEndpointEncryptionType`, `NetworkType`. Output `Cluster` with `ClusterName`, `ClusterArn` (`arn:aws:dax:<region>:<account>:cache/<ClusterName>`), `Description`, `NodeType`, `Status`, `TotalNodes`/`ActiveNodes` == `ReplicationFactor`, `Nodes` (one `Node` per node: `NodeId`, `Endpoint{Address,Port,URL}`, `NodeCreateTime`, `AvailabilityZone`, `NodeStatus`, `ParameterGroupStatus`), `ClusterDiscoveryEndpoint`, `SubnetGroup`, `SecurityGroups`, `IamRoleArn`, `ParameterGroup`, `PreferredMaintenanceWindow`, `SSEDescription`, `ClusterEndpointEncryptionType`, `NetworkType`. Duplicate name → `ClusterAlreadyExistsFault`; unknown `SubnetGroupName` → `SubnetGroupNotFoundFault`.
3. **DescribeClusters** — input `ClusterNames`, `MaxResults`, `NextToken`. Must return exactly the clusters previously created (filtered by `ClusterNames` when given), reading from persisted state; a name that was never created (or was deleted) → `ClusterNotFoundFault`. Honour `MaxResults`/`NextToken` for a stable page/continuation.
4. **UpdateCluster** — input `ClusterName` (required) plus `Description`, `PreferredMaintenanceWindow`, `NotificationTopicArn`, `NotificationTopicStatus`, `ParameterGroupName`, `SecurityGroupIds`. Must MUTATE the stored cluster (only the members supplied) and return the updated `Cluster`; a following DescribeClusters must show the new values. Unknown cluster → `ClusterNotFoundFault`.
5. **DeleteCluster** — input `ClusterName` (required). Output the `Cluster` as it was, with `Status` set to a deleting state, and remove it from state so a later DescribeClusters for that name returns `ClusterNotFoundFault`. Unknown cluster → `ClusterNotFoundFault`.
6. (same slice, cheap once 2–5 exist) **IncreaseReplicationFactor** — input `ClusterName`, `NewReplicationFactor`, `AvailabilityZones`. Must add nodes to the stored cluster so `TotalNodes`/`ActiveNodes`/`Nodes.len()` equal `NewReplicationFactor` on the next DescribeClusters.

EVERY one of these must compute its response from persisted state held in the shared engine via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back, no generating an ARN/NodeId that is never stored. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line only when the handler really reads/writes state.

## State Model

In `src/state.rs` (`ComAmazonawsDaxAmazondaxv3State`), behind the existing lock:

- `clusters: BTreeMap<String /*ClusterName*/, ClusterRecord>` where `ClusterRecord { cluster_name, cluster_arn, description: Option<String>, node_type, status: String, replication_factor: i32, nodes: Vec<NodeRecord>, discovery_endpoint: EndpointRecord, subnet_group: Option<String>, security_group_ids: Vec<String>, availability_zones: Vec<String>, preferred_maintenance_window: Option<String>, notification_topic_arn: Option<String>, notification_topic_status: Option<String>, parameter_group_name: Option<String>, sse_enabled: bool, cluster_endpoint_encryption_type: Option<String>, network_type: Option<String>, tags: Vec<(String,String)>, create_time }`
- `NodeRecord { node_id: String, address: String, port: i32, url: String, create_time, availability_zone: String, node_status: String, parameter_group_status: String }`
- `subnet_groups: BTreeMap<String /*SubnetGroupName*/, SubnetGroupRecord>` where `SubnetGroupRecord { name, description: Option<String>, vpc_id: String, subnet_ids: Vec<String> }`
- `next_node_seq: u64` — monotonic counter so node ids are unique and reproducible (`<ClusterName>-<n>`).

All other operations (parameter groups, DescribeEvents, DescribeParameters/DefaultParameters, DecreaseReplicationFactor, RebootNode, Tag/Untag/ListTags, DeleteSubnetGroup, UpdateSubnetGroup, DescribeSubnetGroups) stay 501 for now and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-dax-amazondaxv3/OPERATIONS.md` lists all 21 operations with the exact input/output members and declared errors cited above (CreateCluster/DescribeClusters/UpdateCluster/DeleteCluster/CreateSubnetGroup/IncreaseReplicationFactor), and the wire protocol section gives `awsJson1_1` + `X-Amz-Target: AmazonDAXV3.<Op>`. `smithy_query shape=Cluster,Node,Endpoint` confirms the `Cluster` members (ClusterArn, TotalNodes, ActiveNodes, Nodes, ClusterDiscoveryEndpoint, SubnetGroup, SecurityGroups, IamRoleArn, ParameterGroup, SSEDescription, ClusterEndpointEncryptionType, NetworkType) and `Node`/`Endpoint` members the state model must carry. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 21 handler files, so no DAX operation has behavior today; there are no tests beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI DAX client at it.
2. `CreateSubnetGroup` with SubnetGroupName=dax-subnets, SubnetIds=[subnet-1,subnet-2] → expect 200 with the SubnetGroup echoing the stored subnets (today: HTTP 501 NotImplemented).
3. `CreateCluster` with ClusterName=demo, NodeType=dax.r4.large, ReplicationFactor=3, IamRoleArn=arn:aws:iam::123456789012:role/DAXRole, SubnetGroupName=dax-subnets → expect 200 with Cluster.ClusterArn, Status, TotalNodes=3 and 3 Nodes each with an Endpoint.
4. `DescribeClusters` with no filter → expect exactly the `demo` cluster with the same ARN, NodeType, SubnetGroup and 3 nodes that CreateCluster returned.
5. `IncreaseReplicationFactor` ClusterName=demo, NewReplicationFactor=5, then `DescribeClusters` → expect TotalNodes=5 and 5 distinct NodeIds.
6. `UpdateCluster` ClusterName=demo, Description="updated", PreferredMaintenanceWindow="sun:01:00-sun:09:00", then `DescribeClusters` → expect the new Description and window persisted.
7. `DeleteCluster` ClusterName=demo → expect 200 with the cluster in a deleting status.
8. `DescribeClusters` with ClusterNames=[demo] → expect `ClusterNotFoundFault` (HTTP 404).
9. `CreateCluster` with ClusterName=demo again after delete → expect success; and creating it twice without deleting → expect `ClusterAlreadyExistsFault` (HTTP 400).

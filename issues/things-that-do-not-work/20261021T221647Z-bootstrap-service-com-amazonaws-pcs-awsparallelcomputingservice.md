Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7d7bdea90bc70459c4b589287302dc4eaf39117d

# Bootstrap the AWS Parallel Computing Service (PCS) emulator

_Filed: 2026-10-21T22:16:47Z_
_Operation: service-com-amazonaws-pcs-awsparallelcomputingservice | Class: stub_

> _Note: this issue was identified against the code base at commit `7d7bdea90bc70459c4b589287302dc4eaf39117d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 19 PCS operations in `src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state. The service is therefore unusable by any client.

This issue defines the FIRST, SMALL slice: the cluster lifecycle happy path, implemented statefully over the awsJson1_0 wire (`POST /` with `X-Amz-Target: AWSParallelComputingService.<OperationName>`).

### Happy path to implement (in order)

1. **CreateCluster** — input `clusterName` (3..40, `^(?!pcs_)^[A-Za-z][A-Za-z0-9-]+$`, required), `scheduler` (SchedulerRequest{type, version}, required), `size` (SMALL|MEDIUM|LARGE, required), `networking` (NetworkingRequest{subnetIds, securityGroupIds, networkType}, required), optional `slurmConfiguration`, `clientToken`, `tags`. Must allocate a new unique cluster id of the form `pcs_<alnum>`, build an ARN `arn:aws:pcs:<region>:<account>:cluster/<id>`, persist the record, and return `cluster` (Cluster) with all `@required` members populated: `name`, `id`, `arn`, `status` (e.g. `CREATING`), `createdAt`, `modifiedAt`, `scheduler`, `size`, `networking` — echoed from the *stored* record, not from the request struct. A second CreateCluster with the same `clusterName` must fail `ConflictException` (HTTP 409); invalid name/size/missing required members must fail `ValidationException` (HTTP 400).
2. **GetCluster** — input `clusterIdentifier` (accepts either the cluster name or the `pcs_...` id, per pattern `^(pcs_[a-zA-Z0-9]+|[A-Za-z][A-Za-z0-9-]{2,40})$`). Must look the cluster up in state and return exactly what CreateCluster stored (same `id`, `arn`, `createdAt`, `scheduler`, `size`, `networking`). Unknown identifier → `ResourceNotFoundException` (HTTP 404).
3. **ListClusters** — `@paginated(inputToken=nextToken, outputToken=nextToken, items=clusters)`, `maxResults` 1..100 default 10. Must return one `ClusterSummary{name, id, arn, createdAt, modifiedAt, status}` per persisted cluster, honour `maxResults` by truncating and emitting an opaque `nextToken` that resumes where the previous page ended, and omit `nextToken` on the last page. Must NOT return a fixed/empty list.
4. **CreateComputeNodeGroup** — input `clusterIdentifier`, `computeNodeGroupName` (3..25), `subnetIds`, `customLaunchTemplate`, `iamInstanceProfileArn`, `scalingConfiguration`, `instanceConfigs` (all required), optional `amiId`, `purchaseOption`, `spotOptions`, `slurmConfiguration`, `nodeLifecycleActions`, `clientToken`, `tags`. Must reject an unknown `clusterIdentifier` with `ResourceNotFoundException` (404), allocate a `pcs_...` id + ARN, store the group under its parent cluster, and return `computeNodeGroup` (ComputeNodeGroup) with the required members `name, id, arn, clusterId, createdAt, modifiedAt, status, subnetIds, customLaunchTemplate, iamInstanceProfileArn, scalingConfiguration, instanceConfigs` read back from state.
5. **CreateQueue** — input `clusterIdentifier`, `queueName` (3..25), optional `computeNodeGroupConfigurations`, `slurmConfiguration`, `clientToken`, `tags`. Unknown cluster → 404; duplicate queue name within the cluster → `ConflictException` (409). Returns `queue` (Queue) with `name, id, arn, clusterId, createdAt, modifiedAt, status, computeNodeGroupConfigurations` from the persisted record. A following **GetQueue** / **ListQueues** (same cluster) must see it.
6. **DeleteCluster** — input `clusterIdentifier`. Must remove the cluster (and its queues / compute node groups) from state so a subsequent GetCluster returns `ResourceNotFoundException` (404) and ListClusters no longer lists it. Deleting an unknown cluster → 404.

Every one of these must compute its response from persisted state. Hardcoded bodies, echoing the request back without storing it, or minting an id/ARN without saving the record are failures of this issue: create → get must round-trip, and delete → get must 404.

## State Model

In `src/state.rs` (`ComAmazonawsPcsAwsparallelcomputingserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `clusters: HashMap<String /* cluster id, "pcs_..." */, Cluster>` plus `cluster_names: HashMap<String /* clusterName */, String /* id */>` (or an index helper) so `clusterIdentifier` resolves by name OR id.
- `struct Cluster { id: String, name: String, arn: String, status: ClusterStatus, created_at: DateTime, modified_at: DateTime, scheduler: Scheduler{type, version}, size: Size, networking: Networking{subnet_ids, security_group_ids, network_type}, slurm_configuration: Option<ClusterSlurmConfiguration>, endpoints: Vec<Endpoint>, error_info: Vec<ErrorInfo>, tags: HashMap<String,String>, client_token: Option<String>, compute_node_groups: HashMap<String /* cng id */, ComputeNodeGroup>, queues: HashMap<String /* queue id */, Queue> }`
- `struct ComputeNodeGroup { id, name, arn, cluster_id, status, created_at, modified_at, ami_id: Option<String>, subnet_ids: Vec<String>, purchase_option: Option<PurchaseOption>, custom_launch_template, iam_instance_profile_arn, scaling_configuration, instance_configs: Vec<InstanceConfig>, spot_options, slurm_configuration, node_lifecycle_actions, tags }`
- `struct Queue { id, name, arn, cluster_id, status, created_at, modified_at, compute_node_group_configurations: Vec<ComputeNodeGroupConfiguration>, slurm_configuration, tags }`
- `id_counter` (or a random `pcs_<alnum>` generator) to mint ids; store the ARN alongside each record so TagResource/ListTagsForResource can later key tags by ARN.
- Optional for idempotency: `client_tokens: HashMap<String /* clientToken */, String /* resource id */>` so repeating a `@idempotencyToken` create returns the first result instead of a conflict.

All other operations (UpdateCluster, Update/Delete/Get/List ComputeNodeGroup, UpdateQueue, DeleteQueue, GetQueue beyond the read-back above, RegisterComputeNodeGroupInstance, TagResource, UntagResource, ListTagsForResource) stay 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md (crate root) lists all 19 operations with their input/output members and errors: CreateCluster (required clusterName/scheduler/size/networking → `cluster`), GetCluster (@readonly, `clusterIdentifier` → `cluster`, ResourceNotFoundException 404), ListClusters (@paginated inputToken=nextToken/outputToken=nextToken/items=clusters, maxResults 1..100 default 10, required output `clusters`), CreateComputeNodeGroup, CreateQueue and DeleteCluster. README.md documents the wire protocol (awsJson1_0, `X-Amz-Target: AWSParallelComputingService.<Op>`) and that every scaffold answers HTTP 501 until the `SCAFFOLDED, NOT IMPLEMENTED` marker is deleted. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 19 handler files (create_cluster.rs, get_cluster.rs, list_clusters.rs, create_compute_node_group.rs, create_queue.rs, delete_cluster.rs, ...), so no operation has any behaviour today. Smithy shapes Cluster/ClusterSummary/Queue/ComputeNodeGroup confirm the required response members (name, id, arn, clusterId, status, createdAt, modifiedAt, ...) that must come from persisted state. No tests exist beyond the generated tests/state_roundtrip.rs.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `pcs` client at it.
2. CreateCluster with clusterName="demo-cluster", scheduler={type:SLURM, version:"24.11"}, size=SMALL, networking={subnetIds:["subnet-0123456789abcdef0"]} — expect 200 with cluster.id like `pcs_...`, cluster.arn, status CREATING, createdAt set. Today: HTTP 501 NotImplemented.
3. GetCluster with clusterIdentifier = the returned cluster.id (and again with "demo-cluster") — expect the identical name/id/arn/scheduler/size/networking that CreateCluster returned.
4. ListClusters with maxResults=1 after creating two clusters — expect one ClusterSummary plus a nextToken, and the second cluster on the follow-up call with that token.
5. CreateComputeNodeGroup in "demo-cluster" with computeNodeGroupName="demo-cng", subnetIds, customLaunchTemplate, iamInstanceProfileArn, scalingConfiguration{minInstanceCount:0,maxInstanceCount:4}, instanceConfigs=[{instanceType:"c6i.large"}] — expect 200 with clusterId equal to the cluster's id.
6. CreateQueue in "demo-cluster" with queueName="demo-queue" and computeNodeGroupConfigurations=[{computeNodeGroupId: <cng id>}] — expect 200; then GetQueue/ListQueues for that cluster must return it.
7. CreateCluster again with clusterName="demo-cluster" — expect ConflictException (HTTP 409).
8. GetCluster with clusterIdentifier="no-such-cluster" — expect ResourceNotFoundException (HTTP 404).
9. DeleteCluster "demo-cluster", then GetCluster "demo-cluster" — expect 404, and ListClusters must no longer include it.

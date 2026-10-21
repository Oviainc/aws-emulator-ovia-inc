Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the EMR containers (emr-containers) emulator

_Filed: 2026-10-21T21:08:45Z_
_Operation: service-com-amazonaws-emrcontainers-awschicagowebservice | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 operations in `src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state. No client workflow works at all.

Implement a FIRST, SMALL, stateful slice: the virtual-cluster + job-run core loop of EMR on EKS (restJson1, signing name `emr-containers`), in this order:

1. **CreateVirtualCluster** — `POST /virtualclusters`. Input: `name` (required), `containerProvider` (required, ContainerProvider{id,type=EKS,info.eksInfo.namespace}), `clientToken` (required, @idempotencyToken), `tags`, `securityConfigurationId`, `sessionEnabled`, `schedulerConfiguration`. Must mint a fresh lowercase-alphanumeric `id` matching `^[0-9a-z]+$`, build `arn` as `arn:aws:emr-containers:{region}:{account}:/virtualclusters/{id}`, persist the cluster with `state=RUNNING` and `createdAt=now`, and return `{id, name, arn}`. Same `clientToken` replayed must return the SAME id (idempotency), not a second cluster.
2. **StartJobRun** — `POST /virtualclusters/{virtualClusterId}/jobruns`. Input: `virtualClusterId` (@httpLabel, required), `clientToken` (required), `name`, `executionRoleArn`, `releaseLabel`, `jobDriver`, `configurationOverrides`, `tags`, `jobTemplateId`, `jobTemplateParameters`, `retryPolicyConfiguration`. Must reject an unknown/deleted `virtualClusterId` with `ResourceNotFoundException` (HTTP 400), mint a job-run `id` (`^[0-9a-z]+$`), store the FULL submitted job run (name, executionRoleArn, releaseLabel, jobDriver, configurationOverrides, tags, retryPolicyConfiguration, clientToken, createdAt, state=`PENDING`→`RUNNING`), and return `{id, name, arn, virtualClusterId}` with `arn` = `.../virtualclusters/{vcId}/jobruns/{id}`.
3. **DescribeJobRun** — `GET /virtualclusters/{virtualClusterId}/jobruns/{id}`. Must return `jobRun` (JobRun) reconstructed from the stored record — `id`, `name`, `virtualClusterId`, `arn`, `state`, `clientToken`, `executionRoleArn`, `releaseLabel`, `jobDriver`, `configurationOverrides`, `createdAt`, `tags` — so that what StartJobRun submitted comes back byte-for-byte. Unknown id or id under the wrong cluster ⇒ `ResourceNotFoundException`.
4. **ListJobRuns** — `GET /virtualclusters/{virtualClusterId}/jobruns`. Must list the job runs actually stored for that cluster (newest first), honour the `states`, `name`, `createdBefore`/`createdAfter` query filters, and honour `maxResults` + opaque `nextToken` paging per the `@paginated(items=jobRuns)` trait — returning `nextToken` only when more remain, and resuming exactly where the token left off with no repeats or gaps.
5. **CancelJobRun** — `DELETE /virtualclusters/{virtualClusterId}/jobruns/{id}`. Must flip the STORED job run to `CANCELLED` (via `CANCEL_PENDING` is acceptable) and set `finishedAt`, return `{id, virtualClusterId}`, and a following DescribeJobRun must observe the cancelled state. Unknown id ⇒ `ValidationException`/`ResourceNotFoundException` rather than a silent 200.
6. **DescribeVirtualCluster / DeleteVirtualCluster** — `GET`/`DELETE /virtualclusters/{id}`. Describe returns the persisted `virtualCluster` (id, name, arn, state, containerProvider, createdAt, tags). Delete marks it `TERMINATED` and returns `{id}`; a subsequent DescribeVirtualCluster must show `TERMINATED` (not a 404 — EMR on EKS keeps terminated clusters visible) and StartJobRun against it must fail.

None of these may be canned, hardcoded or echoed: every response field must be computed from the persisted record. A create followed by a describe must return what was created, and a random id with no stored object behind it is a failure.

## State Model

Store in the single shared engine via `crate::state::service_state(&*self.engine)`, under the generated `ComAmazonawsEmrcontainersAwschicagowebserviceState`:

- `virtual_clusters: HashMap<String /*id*/, VirtualClusterRecord>` where `VirtualClusterRecord { id, name, arn, state: VirtualClusterState (RUNNING|TERMINATING|TERMINATED|ARRESTED), container_provider: ContainerProvider, created_at: DateTime, tags: HashMap<String,String>, security_configuration_id: Option<String>, session_enabled: Option<bool>, scheduler_configuration: Option<SchedulerConfiguration>, client_token: String }`
- `job_runs: HashMap<String /*jobRunId*/, JobRunRecord>` where `JobRunRecord { id, name: Option<String>, virtual_cluster_id, arn, state: JobRunState (PENDING|SUBMITTED|RUNNING|CANCEL_PENDING|CANCELLED|FAILED|COMPLETED), client_token, execution_role_arn, release_label, job_driver: Option<JobDriver>, configuration_overrides: Option<ConfigurationOverrides>, retry_policy_configuration, created_at, finished_at: Option<DateTime>, state_details: Option<String>, tags }` plus an index `job_runs_by_cluster: HashMap<String /*vcId*/, Vec<String /*jobRunId*/>>` preserving creation order for ListJobRuns paging.
- `idempotency: HashMap<(OperationName, ClientToken), String /*resource id*/>` so replayed `clientToken` values return the original id.
- A monotonic `id_counter` used to mint ids that satisfy `^[0-9a-z]+$`, and an `account_id`/`region` source for ARN construction.

Done when: a client can CreateVirtualCluster → StartJobRun → DescribeJobRun (sees the exact submitted jobDriver/releaseLabel/tags) → ListJobRuns (sees the run, filters and paginates) → CancelJobRun (state observably CANCELLED) → DeleteVirtualCluster (state observably TERMINATED, further StartJobRun rejected), all without a 501 and with every field read back from state.

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): CreateJobTemplate/DescribeJobTemplate/DeleteJobTemplate/ListJobTemplates, Create/Describe/Delete/ListManagedEndpoints, GetManagedEndpointSessionCredentials, Create/Describe/Delete/ListSecurityConfigurations, UpdateVirtualCluster, ListVirtualClusters, TagResource/UntagResource/ListTagsForResource.

## Evidence

OPERATIONS.md (services/service-com-amazonaws-emrcontainers-awschicagowebservice/OPERATIONS.md) lists all 25 operations with their routes, required inputs, outputs and errors — CreateVirtualCluster (POST /virtualclusters -> {id,name,arn}), StartJobRun (POST /virtualclusters/{virtualClusterId}/jobruns -> {id,name,arn,virtualClusterId}, ResourceNotFoundException), DescribeJobRun (GET .../jobruns/{id} -> jobRun), ListJobRuns (@paginated(inputToken=nextToken, outputToken=nextToken, items=jobRuns), maxResults/states/name/createdBefore/createdAfter query params), CancelJobRun (DELETE .../jobruns/{id} -> {id,virtualClusterId}), DescribeVirtualCluster/DeleteVirtualCluster (GET/DELETE /virtualclusters/{id}). The Smithy model's VirtualCluster and JobRun shapes (queried) give the exact fields each describe must return, including VirtualCluster.state and JobRun.state/createdAt/finishedAt/stateDetails. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 25 operation files (every file except mod.rs), and README.md confirms the scaffold answers HTTP 501 until the marker is deleted. No handler holds any state; there are no tests asserting any of this behaviour beyond the generated tests/state_roundtrip.rs example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI emr-containers client at it.
2. aws emr-containers create-virtual-cluster --name demo-vc --container-provider '{"id":"demo-eks","type":"EKS","info":{"eksInfo":{"namespace":"emr"}}}' --client-token tok-1 — expect 200 with {id, name, arn}; currently HTTP 501 NotImplemented.
3. Repeat the identical call with --client-token tok-1 — expect the SAME virtual cluster id returned, not a new cluster.
4. aws emr-containers start-job-run --virtual-cluster-id <id> --name demo-job --client-token tok-2 --execution-role-arn arn:aws:iam::123456789012:role/EMRJobRole --release-label emr-6.10.0-latest --job-driver '{"sparkSubmitJobDriver":{"entryPoint":"s3://bucket/job.py"}}' — expect 200 with {id, name, arn, virtualClusterId}.
5. aws emr-containers start-job-run --virtual-cluster-id doesnotexist ... — expect ResourceNotFoundException (HTTP 400), not 200 and not 501.
6. aws emr-containers describe-job-run --virtual-cluster-id <id> --id <jobRunId> — expect jobRun with the exact name, executionRoleArn, releaseLabel, jobDriver and createdAt that were submitted, and a live state.
7. aws emr-containers list-job-runs --virtual-cluster-id <id> --max-results 1 — expect exactly 1 jobRun plus a nextToken; pass that nextToken back and expect the next page with no repeats.
8. aws emr-containers cancel-job-run --virtual-cluster-id <id> --id <jobRunId> — expect {id, virtualClusterId}; then describe-job-run again and expect state CANCELLED with finishedAt set.
9. aws emr-containers describe-virtual-cluster --id <id> — expect the persisted cluster (state RUNNING, containerProvider, createdAt, tags).
10. aws emr-containers delete-virtual-cluster --id <id> — expect {id}; then describe-virtual-cluster and expect state TERMINATED, and a further start-job-run on it to be rejected.

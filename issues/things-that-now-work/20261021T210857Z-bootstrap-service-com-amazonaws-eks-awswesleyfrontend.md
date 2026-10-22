Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the eks emulator

_Filed: 2026-10-21T21:08:57Z_
_Operation: service-com-amazonaws-eks-awswesleyfrontend | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 70 operations in `services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. No client workflow works today.

Implement a FIRST, SMALL, stateful slice: the cluster lifecycle plus tagging, in this order (restJson1, sigv4 name `eks`, endpoint prefix `eks`):

1. **CreateCluster** — `POST /clusters`. Required input: `name` (1..100, `^[0-9A-Za-z][A-Za-z0-9\-_]*$`), `roleArn`, `resourcesVpcConfig` (VpcConfigRequest). Optional and must be persisted when supplied: `version` (default to a plausible value such as `1.31` when absent), `tags`, `logging`, `kubernetesNetworkConfig`, `encryptionConfig`, `accessConfig`, `clientRequestToken`, `deletionProtection`. Output: `cluster` (Cluster) with server-generated `arn` (`arn:aws:eks:<region>:<account>:cluster/<name>`), `createdAt`, `endpoint`, `platformVersion`, `certificateAuthority`, `identity`, `status` (`CREATING` or `ACTIVE`), and the echo-back of every stored field. Creating a name that already exists must fail `ResourceInUseException` (409); a name violating the pattern/length must fail `InvalidParameterException` (400).
2. **DescribeCluster** — `GET /clusters/{name}`. Output: `cluster` (Cluster) read back from the store — byte-for-byte consistent with what CreateCluster stored (same arn, createdAt, tags, roleArn, resourcesVpcConfig, version). Unknown name => `ResourceNotFoundException` (404).
3. **ListClusters** — `GET /clusters`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=clusters)`. Output: `clusters` (list of names actually in the store) + `nextToken`. Honour `maxResults` (1..100) by truncating and returning an opaque `nextToken` that resumes the listing; omit `nextToken` on the last page.
4. **TagResource** — `POST /tags/{resourceArn}`, required `tags` (1..50). Must look the ARN up in the store (cluster ARN for this slice) and merge the tags into that cluster's tag map; unknown ARN => `NotFoundException` (404). Empty output body.
5. **ListTagsForResource** — `GET /tags/{resourceArn}`. Must return the merged tag map from state (create-time tags plus TagResource additions), not an echo of the request.
6. **DeleteCluster** — `DELETE /clusters/{name}`. Removes the cluster from the store and returns the deleted `cluster` (Cluster) with `status` `DELETING`. A second DeleteCluster or a subsequent DescribeCluster for the same name must fail `ResourceNotFoundException` (404), and ListClusters must no longer include it. When `deletionProtection` is `true`, reject with `InvalidRequestException` (400) instead of deleting.

NEVER STUBS: each of these six must compute its response from persisted state held in the single shared engine reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the input back without storing it, no random ARN/ID that is not written to the store. A create followed by a describe/list must return exactly what was created.

## State Model

In `src/state.rs` (`ComAmazonawsEksAwswesleyfrontendState`):

- `clusters: BTreeMap<String /* cluster name */, ClusterRecord>` — BTreeMap so ListClusters has a stable, sortable order for pagination.
- `ClusterRecord { name: String, arn: String, created_at: SystemTime, version: String, platform_version: String, endpoint: String, role_arn: String, resources_vpc_config: VpcConfigResponse, kubernetes_network_config: Option<KubernetesNetworkConfigResponse>, logging: Option<Logging>, identity: Option<Identity>, certificate_authority: Option<Certificate>, status: ClusterStatus, tags: BTreeMap<String, String>, encryption_config: Vec<EncryptionConfig>, access_config: Option<AccessConfigResponse>, deletion_protection: bool, client_request_token: Option<String> }`
- `arn_index: BTreeMap<String /* arn */, String /* cluster name */>` — so TagResource/ListTagsForResource can resolve `resourceArn` to the owning record without scanning.
- Reserved for the next slice (leave empty but present): `nodegroups: BTreeMap<(String /* cluster */, String /* nodegroup */), NodegroupRecord>`, `updates: BTreeMap<String /* updateId */, UpdateRecord>`.

Pagination tokens for ListClusters should be derived from the last returned key (e.g. base64 of the cluster name), not an index into a transient vector.

Everything else (addons, nodegroups, fargate profiles, access entries, pod identity associations, identity providers, updates, EKS Anywhere subscriptions, capabilities, certificate authorities, insights) stays 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-eks-awswesleyfrontend/OPERATIONS.md` lists 70 operations from `smithy-model/com-amazonaws-eks-awswesleyfrontend.json` with protocol `restJson1`, sdkId `EKS`, sigv4 name `eks`. It gives the exact contracts used above: CreateCluster `POST /clusters` (required `name` with len 1..100 and pattern `^[0-9A-Za-z][A-Za-z0-9\-_]*$`, `roleArn`, `resourcesVpcConfig`; output `cluster`; errors ResourceInUseException 409, InvalidParameterException 400), DescribeCluster `GET /clusters/{name}` (output `cluster`; ResourceNotFoundException 404), ListClusters `GET /clusters` (`@paginated` items=clusters, `maxResults` range 1..100, `nextToken`), TagResource `POST /tags/{resourceArn}` (required `tags` len 1..50; NotFoundException 404), DeleteCluster `DELETE /clusters/{name}` (output `cluster`; ResourceNotFoundException 404, InvalidRequestException 400, ResourceInUseException 409). `smithy_query shape=Cluster` confirms the Cluster members to populate (`name`, `arn`, `createdAt`, `version`, `endpoint`, `roleArn`, `resourcesVpcConfig`, `status`, `certificateAuthority`, `platformVersion`, `tags`, `deletionProtection`, ...). A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` under `src/ops/` matches every operation file, including `create_cluster.rs`, `describe_cluster.rs`, `list_clusters.rs`, `tag_resource.rs`, `list_tags_for_resource.rs` and `delete_cluster.rs` — so there is no existing behavior and no test asserting any of it (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Point an AWS SDK/CLI at the emulator endpoint with `--region us-east-1` and the `eks` service.
2. `aws eks create-cluster --name demo --role-arn arn:aws:iam::123456789012:role/eksRole --resources-vpc-config subnetIds=subnet-1,subnet-2 --tags env=dev` — expect 200 with `cluster.name=demo`, a generated `cluster.arn` ending in `cluster/demo`, `createdAt`, and `tags.env=dev`.
3. `aws eks describe-cluster --name demo` — expect 200 returning the same arn/createdAt/roleArn/resourcesVpcConfig/tags that were stored (not a canned body).
4. `aws eks list-clusters` — expect `clusters` to contain `demo`; repeat with `--max-results 1` after creating a second cluster and confirm a `nextToken` is returned and resumes correctly.
5. `aws eks tag-resource --resource-arn <cluster arn> --tags team=platform` then `aws eks list-tags-for-resource --resource-arn <cluster arn>` — expect both `env=dev` and `team=platform`.
6. `aws eks describe-cluster --name nope` — expect HTTP 404 `ResourceNotFoundException`.
7. `aws eks create-cluster --name demo ...` again — expect HTTP 409 `ResourceInUseException`.
8. `aws eks delete-cluster --name demo` — expect 200 with `cluster.status=DELETING`; then `aws eks describe-cluster --name demo` must return 404 and `aws eks list-clusters` must not list `demo`.
9. Confirm no response in this workflow is HTTP 501 and that no field returned was fabricated per-request rather than read from the store.

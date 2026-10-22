Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the dsql emulator

_Filed: 2026-10-21T21:57:48Z_
_Operation: service-com-amazonaws-dsql-dsql | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 16 operations in `services/service-com-amazonaws-dsql-dsql/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this emulator. Implement a FIRST, SMALL slice: the cluster lifecycle, which is the primary resource of Aurora DSQL (`restJson1`, sigv4 name `dsql`).

## Happy path to implement (in order)

1. **CreateCluster** — `POST /cluster`. Input: `deletionProtectionEnabled` (bool, default `true`), `kmsEncryptionKey`, `tags`, `clientToken` (@idempotencyToken), `multiRegionProperties`, `policy`, `bypassPolicyLockoutSafetyCheck` (default `false`). Must MINT and PERSIST a new cluster: generate a fresh `identifier` matching `^[a-z0-9]{26}$`, build `arn` as `arn:aws:dsql:{region}:{account}:cluster/{identifier}` (from the request context, matching the modeled ARN pattern), set `creationTime` to now, `status` to a value of `ClusterStatus` (e.g. `CREATING` then settling to `ACTIVE`, or `ACTIVE` directly), echo back the effective `deletionProtectionEnabled`, and store `tags`, `kmsEncryptionKey`/`encryptionDetails`, `multiRegionProperties` and `endpoint`. Output: `identifier`, `arn`, `status`, `creationTime`, `deletionProtectionEnabled` (all required) plus `multiRegionProperties`, `encryptionDetails`, `endpoint`.
2. **GetCluster** — `GET /cluster/{identifier}`. Must look the cluster up in the store and return exactly what CreateCluster persisted: `identifier`, `arn`, `status`, `creationTime`, `deletionProtectionEnabled`, `multiRegionProperties`, `tags`, `encryptionDetails`, `endpoint`. Unknown identifier -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListClusters** — `GET /cluster`. Input: `maxResults` (`max-results`, 1..100, default 20), `nextToken` (`next-token`). Must enumerate the persisted clusters as `ClusterSummary` items (`identifier`, `arn`), honour `maxResults` as a real page size and emit/accept an opaque `nextToken` that resumes where the previous page stopped; the last page must omit `nextToken`.
4. **UpdateCluster** — `POST /cluster/{identifier}`. Input: `deletionProtectionEnabled`, `kmsEncryptionKey`, `clientToken`, `multiRegionProperties`. Must MUTATE the stored cluster (only the members present in the request) so a following GetCluster observes the change, and return `identifier`, `arn`, `status`, `creationTime`. Unknown identifier -> `ResourceNotFoundException` (404).
5. **DeleteCluster** — `DELETE /cluster/{identifier}` with optional `client-token`. Must refuse with `ConflictException` (409) when the stored cluster has `deletionProtectionEnabled = true`, otherwise remove it (or mark it `DELETING`/`DELETED`) from the store so a following GetCluster returns `ResourceNotFoundException` (404) and ListClusters no longer lists it. Return `identifier`, `arn`, `status` (a deleting/deleted `ClusterStatus`), `creationTime`.

No canned, hardcoded or echoed responses: every field above must be computed from persisted state. A random identifier returned without storing the cluster, or a GetCluster that reflects its own input, counts as a failure of this issue. Validation of the `@httpLabel` `identifier` against `^[a-z0-9]{26}$` should produce `ValidationException` (400) where the operation models it.

## State Model

In `src/state.rs` (`ComAmazonawsDsqlDsqlState`, reached via `crate::state::service_state(&*self.engine)`):

- `clusters: HashMap<String /* identifier, 26 lowercase alnum */, Cluster>`
- `Cluster { identifier: String, arn: String, status: ClusterStatus, creation_time: SystemTime, deletion_protection_enabled: bool, kms_encryption_key: Option<String>, encryption_details: Option<EncryptionDetails>, multi_region_properties: Option<MultiRegionProperties>, endpoint: Option<String>, tags: BTreeMap<String, String>, policy: Option<String>, policy_version: Option<String> }`
- `idempotency: HashMap<String /* clientToken */, String /* identifier */>` so a repeated `clientToken` on CreateCluster/DeleteCluster returns the first result instead of creating a second cluster.
- Ordering for ListClusters comes from a deterministic sort (e.g. by `identifier`) so the `nextToken` cursor — an opaque encoding of the last-returned `identifier` — is stable across pages.

Out of scope for this first slice (stay 501, filed separately later): PutClusterPolicy, GetClusterPolicy, DeleteClusterPolicy, CreateStream, GetStream, ListStreams, DeleteStream, GetVpcEndpointServiceName, TagResource, UntagResource, ListTagsForResource.

Done = a client script can create a cluster, get it back with the same identifier/arn/creationTime, see it in ListClusters, flip `deletionProtectionEnabled` via UpdateCluster, delete it, and then get a 404 — all against the same running emulator process.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-dsql-dsql/src/ops/` matches all 16 handler files (create_cluster.rs, get_cluster.rs, list_clusters.rs, update_cluster.rs, delete_cluster.rs, and the 11 others), so every operation answers HTTP 501 and the service holds zero state. Operation routes, required input/output members, defaults and modeled errors are taken from services/service-com-amazonaws-dsql-dsql/OPERATIONS.md: CreateCluster (`POST /cluster`, lines 50-55), GetCluster (`GET /cluster/{identifier}`, lines 57-63), UpdateCluster (`POST /cluster/{identifier}`, lines 65-70), DeleteCluster (`DELETE /cluster/{identifier}`, lines 72-78), ListClusters (`GET /cluster`, @paginated inputToken=nextToken/outputToken=nextToken/items=clusters, lines 80-86). The ARN and identifier patterns (`^[a-z0-9]{26}$`, `^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}$`) and the `ClusterStatus` enum values are from the same file; deletion protection blocking a delete with `ConflictException` is the documented Aurora DSQL behaviour and ConflictException (409) is modeled on DeleteCluster.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI dsql client at it (restJson1, signing name `dsql`).
2. `CreateCluster` with `deletionProtectionEnabled=false` and `tags={"env":"test"}` — expect 200 with a 26-char lowercase-alnum `identifier`, a well-formed `arn`, a `status` from ClusterStatus, a `creationTime` and `deletionProtectionEnabled=false`. Currently HTTP 501 NotImplemented.
3. `GetCluster` with that `identifier` — expect the same `identifier`, `arn`, `creationTime` and the `tags` supplied at create time. Currently 501.
4. `ListClusters` with `max-results=1` — expect the new cluster present as a ClusterSummary, and a `nextToken` only when more clusters remain; follow the token and expect no duplicates or repeats. Currently 501.
5. `UpdateCluster` on that `identifier` with `deletionProtectionEnabled=true`, then `GetCluster` — expect the updated value to be observable. Currently 501.
6. `DeleteCluster` on that `identifier` while deletion protection is enabled — expect `ConflictException` (409). Disable protection via UpdateCluster, delete again — expect 200.
7. `GetCluster` on the deleted `identifier` — expect `ResourceNotFoundException` (HTTP 404), and expect it absent from `ListClusters`.
8. `GetCluster` with an identifier that was never created — expect `ResourceNotFoundException` (404), not 501.

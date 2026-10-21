Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the odb (Oracle Database@AWS) emulator

_Filed: 2026-10-21T23:28:09Z_
_Operation: service-com-amazonaws-odb-odb | Class: stub_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 80 operations in `services/service-com-amazonaws-odb-odb/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state. Nothing a client does works end-to-end today.

This issue defines the FIRST, SMALL slice: the ODB network lifecycle, which is the primary resource of this service (every Exadata infrastructure / VM cluster / autonomous database hangs off an ODB network, so this is the right foundation). Implement these 5 operations statefully, in this order (wire protocol is `awsJson1_0`, `POST /` with `X-Amz-Target: Odb.<OperationName>`):

1. **CreateOdbNetwork** — input `displayName` (required, len 1..255, pattern `^[a-zA-Z_](?!.*--)[a-zA-Z0-9_-]*$`), `clientSubnetCidr` (required), optional `availabilityZone`, `availabilityZoneId`, `backupSubnetCidr`, `customDomainName`, `defaultDnsPrefix`, `s3Access`/`zeroEtlAccess`/`stsAccess`/`kmsAccess` (`ENABLED|DISABLED`), the matching `*PolicyDocument` strings, `tags`, and `clientToken` (`@idempotencyToken`). Must mint a new `odbNetworkId` (and a matching `odbNetworkArn` of the form `arn:aws:odb:<region>:<account>:odb-network/<id>`), persist the whole record, and return `odbNetworkId` (required), `displayName`, `status`, `statusReason`. Status should start `PROVISIONING` and become `AVAILABLE` (immediately or on first read) — not a hardcoded constant divorced from the stored record. The operation is `@idempotent`: a repeat call with the same `clientToken` must return the SAME `odbNetworkId` instead of creating a second network.
2. **GetOdbNetwork** — input `odbNetworkId` (required; accept either the bare id or the ARN, per the `ResourceIdOrArn` pattern). Returns the `odbNetwork` structure built from the persisted record: `odbNetworkId`, `displayName`, `status`, `statusReason`, `odbNetworkArn`, `availabilityZone`, `availabilityZoneId`, `clientSubnetCidr`, `backupSubnetCidr`, `customDomainName`, `defaultDnsPrefix`, `peeredCidrs`, `createdAt`, `percentProgress`, `managedServices`. Unknown id ⇒ `ResourceNotFoundException` (HTTP 404).
3. **ListOdbNetworks** — returns `odbNetworks` (required list of `OdbNetworkSummary`, same fields as the Get payload) for every live network, honouring `maxResults` and round-tripping `nextToken` (the op is `@paginated(inputToken=nextToken, outputToken=nextToken, items=odbNetworks)`). A created network must appear here; a deleted one must not.
4. **UpdateOdbNetwork** — input `odbNetworkId` (required) plus `displayName`, `peeredCidrsToBeAdded`/`peeredCidrsToBeRemoved`, `s3Access`/`zeroEtlAccess`/`stsAccess`/`kmsAccess`, `*PolicyDocument`, `crossRegionS3RestoreSourcesToEnable`/`...ToDisable`. Must MUTATE the stored record (add/remove entries in `peeredCidrs`, replace access flags/policy docs) so a following GetOdbNetwork reflects the change, and return `odbNetworkId`, `displayName`, `status`, `statusReason`. Unknown id ⇒ `ResourceNotFoundException`.
5. **DeleteOdbNetwork** — input `odbNetworkId` (required) and `deleteAssociatedResources` (required boolean). Removes the record; a later GetOdbNetwork must return `ResourceNotFoundException` (404) and ListOdbNetworks must omit it. Unknown id ⇒ `ResourceNotFoundException`.

No canned, echoed or hardcoded responses: every field above must be computed from the persisted record. In particular, Get/List must NOT echo the request, and Create must not mint an id it fails to store. Validation errors (missing required member, `displayName` pattern/length violation) must surface as `ValidationException` (HTTP 400), not 500 or 200.

## State Model

In `src/state.rs` (`ComAmazonawsOdbOdbState`, reached via `crate::state::service_state(&*self.engine)` and its lock guard):

- `odb_networks: BTreeMap<String /* odbNetworkId */, OdbNetworkRecord>`
- `odb_network_tokens: HashMap<String /* clientToken */, String /* odbNetworkId */>` — for `@idempotent` Create replay
- `OdbNetworkRecord { odb_network_id: String, odb_network_arn: String, display_name: String, status: ResourceStatus, status_reason: Option<String>, availability_zone: Option<String>, availability_zone_id: Option<String>, client_subnet_cidr: String, backup_subnet_cidr: Option<String>, custom_domain_name: Option<String>, default_dns_prefix: Option<String>, peered_cidrs: Vec<String>, s3_access: Access, zero_etl_access: Access, sts_access: Access, kms_access: Access, s3_policy_document: Option<String>, sts_policy_document: Option<String>, kms_policy_document: Option<String>, cross_region_s3_restore_sources: Vec<String>, tags: BTreeMap<String,String>, created_at: SystemTime, percent_progress: f32 }`

All other 75 operations (Exadata infrastructure, VM clusters, autonomous databases, DB nodes, tagging, metadata list ops, OCI onboarding) stay at 501 for now and are tracked as individual gap issues.

## Evidence

Scaffold census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-odb-odb/src/ops/` matches all 80 handler files, including `create_odb_network.rs`, `get_odb_network.rs`, `list_odb_networks.rs`, `update_odb_network.rs`, `delete_odb_network.rs` — each returns `Scaffolded::not_implemented(..)` (HTTP 501). Contract grounding: services/service-com-amazonaws-odb-odb/OPERATIONS.md sections `CreateOdbNetwork` (required `displayName`, `clientSubnetCidr`; `@idempotent` with `clientToken` `@idempotencyToken`; output required `odbNetworkId`), `GetOdbNetwork` (`@readonly`, output `odbNetwork`, declares `ResourceNotFoundException` 404), `ListOdbNetworks` (`@paginated` on `nextToken`/`odbNetworks`), `UpdateOdbNetwork` (`peeredCidrsToBeAdded`/`Removed`, 404), `DeleteOdbNetwork` (required `deleteAssociatedResources`, 404), plus README.md 'Wire protocol' (awsJson1_0, `X-Amz-Target: Odb.<Op>`). Field lists for the stored record come from the Smithy shapes `OdbNetwork` / `OdbNetworkSummary`. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an SDK/CLI client for `odb` at it.
2. CreateOdbNetwork with displayName="my_odb_net", clientSubnetCidr="10.0.1.0/24", availabilityZone="us-east-1a", s3Access=ENABLED, clientToken="tok-1" — expect 200 with a fresh odbNetworkId and status PROVISIONING/AVAILABLE (today: HTTP 501 NotImplemented).
3. Repeat the identical CreateOdbNetwork with clientToken="tok-1" — expect the SAME odbNetworkId, and only one network in state.
4. GetOdbNetwork with that odbNetworkId — expect odbNetwork.displayName="my_odb_net", clientSubnetCidr="10.0.1.0/24", availabilityZone="us-east-1a", a populated odbNetworkArn and createdAt.
5. ListOdbNetworks — expect the network present in odbNetworks; with maxResults=1 and two networks created, expect a nextToken that pages to the remainder.
6. UpdateOdbNetwork with displayName="my_odb_net2" and peeredCidrsToBeAdded=["10.9.0.0/16"], then GetOdbNetwork — expect the new displayName and "10.9.0.0/16" in peeredCidrs.
7. DeleteOdbNetwork with deleteAssociatedResources=true, then GetOdbNetwork on the same id — expect ResourceNotFoundException (HTTP 404) and the id absent from ListOdbNetworks.
8. GetOdbNetwork with an unknown id, and CreateOdbNetwork with displayName="-bad--name" — expect ResourceNotFoundException (404) and ValidationException (400) respectively.

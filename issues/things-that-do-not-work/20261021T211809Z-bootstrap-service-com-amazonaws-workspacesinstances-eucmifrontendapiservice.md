Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the workspaces-instances (EUCMIFrontendAPIService) emulator

_Filed: 2026-10-21T21:18:09Z_
_Operation: service-com-amazonaws-workspacesinstances-eucmifrontendapiservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 14 operations in this crate is an untouched scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds zero state. No client can do anything with it.

Implement a FIRST, SMALL slice: the WorkSpace Instance lifecycle plus tagging, backed by real persisted state in `crate::state` reached through `self.engine` (never a fresh engine). Wire protocol is awsJson1_0: `POST /` with `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`.

Happy path, in order:

1. **CreateWorkspaceInstance** — input `ManagedInstance` (ManagedInstanceRequest, required), optional `Tags`, `BillingConfiguration`, `ClientToken` (@idempotencyToken; the op is @idempotent). Must mint a fresh id matching `^wsinst-[0-9a-zA-Z]{8,63}$` (len 15..70), store the full record (ManagedInstance config, tags, billing config, client token) and return `WorkspaceInstanceId`. A repeat call with the same `ClientToken` must return the SAME id and not create a second instance.
2. **GetWorkspaceInstance** — input `WorkspaceInstanceId` (required). Must look the record up and return `WorkspaceInstanceId`, `ProvisionState` (ProvisionStateEnum: ALLOCATING/ALLOCATED/DEALLOCATING/DEALLOCATED/ERROR_*), `EC2ManagedInstance{InstanceId}` and the `BillingConfiguration` that was supplied at create time. An unknown id must be `ResourceNotFoundException` (HTTP 404), never a hollow 200.
3. **ListWorkspaceInstances** — must return `WorkspaceInstances` (required list of `WorkspaceInstance{WorkspaceInstanceId, ProvisionState, EC2ManagedInstance}`) built from the stored map, honouring `MaxResults` (1..25) and the opaque `NextToken` round-trip (@paginated), and filtering by `ProvisionStates` when supplied.
4. **TagResource** — input `WorkspaceInstanceId` + `Tags` (required, 0..50). Must merge the tags onto the stored instance (same key overwrites), 404 for an unknown instance.
5. **ListTagsForResource** — must return exactly the tags persisted by create/TagResource/UntagResource, not an echo of the request.
6. **DeleteWorkspaceInstance** — must remove the instance from state so a subsequent GetWorkspaceInstance returns `ResourceNotFoundException` (404) and ListWorkspaceInstances no longer includes it. Deleting an unknown id is 404.

Rules for the slice: no canned, hardcoded or echoed responses; every field returned must be computed from the stored record. Minting an id without storing it is a failure. Required-member absence and the modeled `pattern`/`length`/`range` constraints must produce `ValidationException` (HTTP 400), not a 500 or a success.

## State Model

In `src/state.rs`, on the service state struct (behind the existing lock):

- `instances: HashMap<String /* wsinst-… id */, WorkspaceInstanceRecord>` where
  `WorkspaceInstanceRecord { id: String, provision_state: ProvisionStateEnum, managed_instance: ManagedInstanceRequest /* as supplied */, ec2_instance_id: String /* minted i-… */, billing_configuration: Option<BillingConfiguration>, tags: BTreeMap<String /*Key*/, String /*Value*/>, created_at: SystemTime }`
- `client_tokens: HashMap<String /* ClientToken */, String /* WorkspaceInstanceId */>` for @idempotent CreateWorkspaceInstance replay.
- `id_counter: u64` used to mint unique `wsinst-…` / `i-…` identifiers.
- (room for the next slice, not required now) `volumes: HashMap<String /* vol-… */, VolumeRecord { id, size_in_gb, volume_type, iops, throughput, availability_zone, attachments: Vec<Attachment{instance_id, device}> }>`.

The remaining operations (CreateVolume, DeleteVolume, ModifyVolume, AssociateVolume, DisassociateVolume, UntagResource, ListInstanceTypes, ListRegions) stay at 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md (crate root, 14 operations, awsJson1_0, target prefix `EUCMIFrontendAPIService.`) gives the contracts cited above: CreateWorkspaceInstance (@idempotent, input ManagedInstance required / ClientToken @idempotencyToken, output WorkspaceInstanceId pattern ^wsinst-[0-9a-zA-Z]{8,63}$), GetWorkspaceInstance (@readonly, outputs ProvisionState/EC2ManagedInstance/BillingConfiguration, declares ResourceNotFoundException 404), ListWorkspaceInstances (@paginated inputToken=NextToken outputToken=NextToken items=WorkspaceInstances, MaxResults 1..25, ProvisionStates filter), TagResource/ListTagsForResource (Tags 0..50, ResourceNotFoundException 404), DeleteWorkspaceInstance (ResourceNotFoundException 404). Shape definitions for WorkspaceInstance{WorkspaceInstanceId, ProvisionState, EC2ManagedInstance}, EC2ManagedInstance{InstanceId} and Tag{Key, Value} come from the Smithy model via smithy_query. All-stub status: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/` matches all 14 handler files (create_workspace_instance.rs, get_workspace_instance.rs, list_workspace_instances.rs, delete_workspace_instance.rs, tag_resource.rs, list_tags_for_resource.rs, …). No tests exist beyond the generated tests/state_roundtrip.rs.

## Steps to Reproduce

1. Start the emulator and point an awsJson1_0 client at it (POST /, header `X-Amz-Target: EUCMIFrontendAPIService.<Op>`, sigv4 signing name `workspaces-instances`).
2. CreateWorkspaceInstance with {"ClientToken":"tok-1","ManagedInstance":{"ImageId":"ami-0123456789","InstanceType":"t3.medium","SubnetId":"subnet-abc123"},"Tags":[{"Key":"env","Value":"dev"}]} — expect 200 with a WorkspaceInstanceId matching ^wsinst-[0-9a-zA-Z]{8,63}$; today 501 NotImplemented.
3. Repeat the identical CreateWorkspaceInstance with ClientToken "tok-1" — expect the SAME WorkspaceInstanceId and no second instance.
4. GetWorkspaceInstance with that id — expect 200 echoing the id, a ProvisionState, EC2ManagedInstance.InstanceId, and the BillingConfiguration/config supplied at create time.
5. ListWorkspaceInstances with MaxResults=1 after creating two instances — expect one WorkspaceInstance entry plus a NextToken, and passing that NextToken back returns the second instance and no duplicate.
6. TagResource {id, Tags:[{"Key":"owner","Value":"alice"}]} then ListTagsForResource {id} — expect both env=dev and owner=alice from persisted state.
7. DeleteWorkspaceInstance {id}, then GetWorkspaceInstance {id} — expect ResourceNotFoundException with HTTP 404, and the instance absent from ListWorkspaceInstances.
8. GetWorkspaceInstance with a well-formed but unknown id (wsinst-00000000) — expect 404 ResourceNotFoundException; with a malformed id ("bogus") — expect 400 ValidationException.

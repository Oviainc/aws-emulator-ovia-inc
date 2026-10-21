Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the FSx (AWSSimbaAPIService_v20180301) emulator

_Filed: 2026-10-21T21:08:49Z_
_Operation: service-com-amazonaws-fsx-awssimbaapiservice-v20180301 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 48 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no FSx client can do anything. Implement a FIRST, SMALL slice: the file-system lifecycle, the service's primary resource, end-to-end and statefully over the `awsJson1_1` wire protocol (`POST /`, `X-Amz-Target: AWSSimbaAPIService_v20180301.<OperationName>`).

Happy path to implement, in order (fields per OPERATIONS.md):

1. **CreateFileSystem** — input `FileSystemType` (required; LUSTRE|ONTAP|OPENZFS|WINDOWS), `SubnetIds` (required), `StorageCapacity`, `StorageType` (HDD|INTELLIGENT_TIERING|SSD), `SecurityGroupIds`, `KmsKeyId`, `Tags`, `FileSystemTypeVersion`, `NetworkType`, `ClientRequestToken` (@idempotencyToken). Output `FileSystem`. Must mint a new `FileSystemId` matching `^(fs-[0-9a-f]{8,})$`, set `CreationTime`, `Lifecycle` (CREATING→AVAILABLE, or AVAILABLE directly), `OwnerId` (12-digit account), `ResourceARN` (`arn:aws:fsx:<region>:<account>:file-system/fs-...`), `DNSName`, and echo back the stored `SubnetIds`/`StorageCapacity`/`StorageType`/`Tags`/`FileSystemType` from the persisted record — not from the request object. Same `ClientRequestToken` replayed must return the SAME file system, not a second one.
2. **DescribeFileSystems** — input `FileSystemIds`, `MaxResults`, `NextToken`; output `FileSystems` + `NextToken`. Must list the file systems actually stored, filter by `FileSystemIds` when given, honour `MaxResults` with a real opaque `NextToken` (base64-shaped, matching the modeled pattern), and raise `FileSystemNotFound` (HTTP 400) when a requested id does not exist.
3. **TagResource** — input `ResourceARN` (required), `Tags` (required). Must resolve the ARN to the stored resource and merge the tags into its tag map (same Key overwrites); unknown ARN → `ResourceNotFound` (HTTP 400).
4. **ListTagsForResource** — input `ResourceARN` (required), `MaxResults`, `NextToken`; output `Tags` + `NextToken`. Must return exactly the tags stored by CreateFileSystem/TagResource for that ARN.
5. **UpdateFileSystem** — input `FileSystemId` (required), `StorageCapacity`, `StorageType`, `FileSystemTypeVersion`, `NetworkType`, per-type configuration structs, `ClientRequestToken`; output `FileSystem`. Must mutate the persisted record so a following DescribeFileSystems observes the new values; unknown id → `FileSystemNotFound` (HTTP 400).
6. **DeleteFileSystem** — input `FileSystemId` (required), `ClientRequestToken`; output `FileSystemId` + `Lifecycle` (DELETING). Must remove (or mark DELETING and then remove) the record so DescribeFileSystems no longer returns it and a second describe of that id yields `FileSystemNotFound`.

NEVER STUBS: each of these six must compute its response from the shared store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back without storing it, no random id without a record behind it.

## State Model

In `src/state.rs` (`ComAmazonawsFsxAwssimbaapiserviceV20180301State`):

- `file_systems: HashMap<String /* FileSystemId */, FileSystemRecord>` where `FileSystemRecord { file_system_id, owner_id, creation_time, file_system_type, lifecycle, storage_capacity, storage_type, vpc_id, subnet_ids: Vec<String>, security_group_ids: Vec<String>, network_interface_ids: Vec<String>, dns_name, kms_key_id, resource_arn, file_system_type_version, network_type, windows_configuration, lustre_configuration, ontap_configuration, open_zfs_configuration, administrative_actions: Vec<AdministrativeAction>, tags: BTreeMap<String,String> }` — mirrors the modeled `FileSystem` shape so Describe can serialize straight from it.
- `tags_by_arn: HashMap<String /* ResourceARN */, BTreeMap<String,String>>` (or the `tags` field on each record plus an ARN→resource index) so TagResource/ListTagsForResource work against the same storage CreateFileSystem wrote.
- `idempotency_tokens: HashMap<String /* ClientRequestToken */, String /* created resource id */>` for the @idempotencyToken replay behaviour on CreateFileSystem/DeleteFileSystem.
- `next_id_counter: u64` (or equivalent) used to mint well-formed `fs-[0-9a-f]{8,}` ids.

Leave the rest (backups, volumes, SVMs, snapshots, file caches, data repository associations/tasks, S3 access points, aliases, shared VPC config) at 501 — they are follow-up gap issues. Done = a client script can create a file system, describe it and see exactly what it created, tag and list tags, update storage capacity and see the new value, delete it, and then get FileSystemNotFound.

## Evidence

OPERATIONS.md for service-com-amazonaws-fsx-awssimbaapiservice-v20180301 lists 48 operations with protocol `awsJson1_1`, target prefix `AWSSimbaAPIService_v20180301`, sigv4 name `fsx`. CreateFileSystem (line 88) requires `FileSystemType` + `SubnetIds` and returns `FileSystem`; DescribeFileSystems (line 207) is @paginated(inputToken=NextToken, outputToken=NextToken) returning `FileSystems`/`NextToken` with `FileSystemNotFound (HTTP 400)`; UpdateFileSystem (line 315) requires `FileSystemId` and returns `FileSystem` with `FileSystemNotFound (HTTP 400)`; DeleteFileSystem (line 146) is @idempotent, requires `FileSystemId`, returns `FileSystemId`+`Lifecycle`; ListTagsForResource (line 260) requires `ResourceARN` and returns `Tags`/`NextToken` with `ResourceNotFound (HTTP 400)`. The `FileSystem` shape in the Smithy model defines FileSystemId (pattern ^(fs-[0-9a-f]{8,})$), OwnerId, CreationTime, Lifecycle (AVAILABLE|CREATING|DELETING|FAILED|MISCONFIGURED|MISCONFIGURED_UNAVAILABLE|UPDATING), StorageCapacity, StorageType, VpcId, SubnetIds, NetworkInterfaceIds, DNSName, KmsKeyId, ResourceARN, Tags, per-type configurations, AdministrativeActions, NetworkType. All 48 files under src/ops/ carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (501) — e.g. src/ops/create_file_system.rs, src/ops/describe_file_systems.rs, src/ops/delete_file_system.rs. No tests cover any of these beyond tests/state_roundtrip.rs.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSSimbaAPIService_v20180301.CreateFileSystem and body {"FileSystemType":"WINDOWS","StorageCapacity":32,"StorageType":"SSD","SubnetIds":["subnet-0123456789abcdef0"],"Tags":[{"Key":"env","Value":"dev"}]} — expect 200 with FileSystem.FileSystemId matching ^fs-[0-9a-f]{8,}$, Lifecycle CREATING or AVAILABLE, CreationTime, ResourceARN, and the submitted SubnetIds/StorageCapacity/Tags; actual: HTTP 501 NotImplemented.
2. POST DescribeFileSystems with {} — expect the created file system in FileSystems; then with {"FileSystemIds":["<id>"]} expect exactly that one, and with {"FileSystemIds":["fs-00000000deadbeef"]} expect FileSystemNotFound (400).
3. POST TagResource with {"ResourceARN":"<arn from step 1>","Tags":[{"Key":"team","Value":"storage"}]}, then ListTagsForResource with the same ARN — expect both env=dev and team=storage.
4. POST UpdateFileSystem with {"FileSystemId":"<id>","StorageCapacity":64}, then DescribeFileSystems — expect StorageCapacity 64 persisted.
5. POST DeleteFileSystem with {"FileSystemId":"<id>"} — expect 200 with that FileSystemId and Lifecycle DELETING; then DescribeFileSystems with {"FileSystemIds":["<id>"]} — expect FileSystemNotFound (400).
6. Replay CreateFileSystem twice with the same ClientRequestToken — expect one file system, the same id both times.

Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the EFS emulator

_Filed: 2026-10-21T21:08:16Z_
_Operation: service-com-amazonaws-efs-magnolioapiservice-v20150201 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 31 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no EFS client workflow can run at all. Implement a first, small, stateful slice covering the core file-system lifecycle (restJson1, signing name `elasticfilesystem`, URI prefix `/2015-02-01`).

### Happy path to implement (in order)

1. **CreateFileSystem** — `POST /2015-02-01/file-systems` -> 201. Input: `CreationToken` (required, @idempotencyToken), optional `PerformanceMode` (default `generalPurpose`), `ThroughputMode` (default `bursting`), `ProvisionedThroughputInMibps`, `Encrypted`, `KmsKeyId`, `AvailabilityZoneName`, `Backup`, `Tags`. Must mint a fresh `FileSystemId` matching `^fs-[0-9a-f]{8,40}$`, build `FileSystemArn` as `arn:aws:elasticfilesystem:<region>:<ownerId>:file-system/<fs-id>`, set `CreationTime` to now, `LifeCycleState` = `available` (or `creating` then observable as `available`), `NumberOfMountTargets` = 0, `SizeInBytes` (`Value`/`Timestamp`), echo stored `PerformanceMode`/`ThroughputMode`/`Encrypted`/`Tags`, and derive `Name` from the `Name` tag when present. Persist the record. Re-calling with an already-used `CreationToken` must return `FileSystemAlreadyExists` (HTTP 409) with the existing `FileSystemId`, not a second file system.
2. **DescribeFileSystems** — `GET /2015-02-01/file-systems` -> 200. Must return the persisted `FileSystems` list (not a canned list), honouring the `FileSystemId` and `CreationToken` query filters, and `FileSystemNotFound` (404) when a requested `FileSystemId` is unknown. `MaxItems`/`Marker` -> `NextMarker` pagination should at minimum truncate consistently and resume from the returned marker.
3. **CreateMountTarget** — `POST /2015-02-01/mount-targets` -> 200. Input: `FileSystemId` (required), `SubnetId` (required, `^subnet-[0-9a-f]{8,40}$`), optional `IpAddress`, `Ipv6Address`, `IpAddressType`, `SecurityGroups`. Must 404 `FileSystemNotFound` for an unknown file system, mint `MountTargetId` matching `^fsmt-[0-9a-f]{8,40}$`, store the mount target against its file system, allocate an `IpAddress` (or honour the supplied one), fill `NetworkInterfaceId`/`AvailabilityZoneName`/`AvailabilityZoneId`/`VpcId`/`OwnerId`, set `LifeCycleState` = `available`, and **increment the owning file system's `NumberOfMountTargets`** so a later DescribeFileSystems reflects it. A second mount target in the same subnet for the same file system must fail with `MountTargetConflict` (409).
4. **DescribeMountTargets** — `GET /2015-02-01/mount-targets` -> 200. Must list persisted mount targets filtered by `FileSystemId` or `MountTargetId`, with `MountTargetNotFound` (404) for an unknown `MountTargetId` and `FileSystemNotFound` (404) for an unknown file system. Each entry must be the stored description from step 3, byte-for-byte consistent with the CreateMountTarget response.
5. **DeleteMountTarget** — `DELETE /2015-02-01/mount-targets/{MountTargetId}` -> 204 (empty body). Must remove the mount target from state, decrement the owning file system's `NumberOfMountTargets`, and return `MountTargetNotFound` (404) for an id that was never created or already deleted. After this, DescribeMountTargets must no longer list it.
6. **DeleteFileSystem** — `DELETE /2015-02-01/file-systems/{FileSystemId}` -> 204 (empty body). Must refuse with `FileSystemInUse` (409) while the file system still has mount targets, remove the record otherwise (freeing its `CreationToken` for reuse), and return `FileSystemNotFound` (404) for an unknown or already-deleted id. A following DescribeFileSystems for that id must 404.

No canned, hardcoded, or echoed responses: every field above must be computed from persisted state, and every read must observe what the preceding write stored. Ids must be minted AND stored — a random id with no backing record is a failure.

## State Model

Service state lives behind `crate::state::service_state(&*self.engine)` (one lock-guarded struct), holding:

- `file_systems: HashMap<String /* fs-id */, FileSystem>` where `FileSystem { file_system_id: String, file_system_arn: String, owner_id: String, creation_token: String, creation_time: SystemTime, life_cycle_state: LifeCycleState, name: Option<String>, performance_mode: PerformanceMode, throughput_mode: ThroughputMode, provisioned_throughput_in_mibps: Option<f64>, encrypted: Option<bool>, kms_key_id: Option<String>, availability_zone_name: Option<String>, availability_zone_id: Option<String>, size_in_bytes: FileSystemSize { value, timestamp, value_in_ia, value_in_standard }, number_of_mount_targets: i32, tags: Vec<Tag>, mount_target_ids: Vec<String> }`
- `creation_tokens: HashMap<String /* CreationToken */, String /* fs-id */>` — backs CreateFileSystem idempotency / `FileSystemAlreadyExists`.
- `mount_targets: HashMap<String /* fsmt-id */, MountTarget>` where `MountTarget { mount_target_id: String, file_system_id: String, subnet_id: String, owner_id: String, life_cycle_state: LifeCycleState, ip_address: Option<String>, ipv6_address: Option<String>, network_interface_id: String, availability_zone_id: String, availability_zone_name: String, vpc_id: String, security_groups: Vec<String> }`
- `next_id_seed: u64` (or equivalent) used to mint `fs-`/`fsmt-` hex ids; plus a fixed 12-digit `owner_id` and region string for ARN construction.

The remaining 25 operations (access points, policies, backup/lifecycle configuration, replication, tagging, account preferences, UpdateFileSystem, …) stay at 501 for this slice and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-efs-magnolioapiservice-v20150201/OPERATIONS.md` lists all 31 operations with routes, input/output members and modeled errors (CreateFileSystem `POST /2015-02-01/file-systems -> 201`, CreateMountTarget `POST /2015-02-01/mount-targets -> 200`, DescribeFileSystems/DescribeMountTargets with `MaxItems`/`Marker`/`NextMarker` pagination, DeleteMountTarget and DeleteFileSystem `-> 204`), and the wire protocol (`restJson1`, sigv4 name `elasticfilesystem`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 31 handler files, and `src/ops/create_file_system.rs:316` ends in `Err(crate::scaffolded::Scaffolded::not_implemented("CreateFileSystem"))` with nothing read from or written to `crate::state::service_state`. The modeled docs in that same file state the idempotency rule (same `CreationToken` returns `FileSystemAlreadyExists` with the existing file system id) and that `NumberOfMountTargets` is the current count of mount targets — plus `DeleteFileSystem` models `FileSystemInUse` (409), which is well-established EFS behaviour while mount targets exist.

## Steps to Reproduce

1. Start the emulator with this service mounted and point an EFS client/`aws efs` at it.
2. `CreateFileSystem` with `CreationToken=boot-1` and `Tags=[{Key:Name,Value:demo}]` — expect 201 with a minted `fs-...` id, `LifeCycleState` available, `NumberOfMountTargets=0`, `Name=demo`. Currently: HTTP 501 NotImplemented.
3. Repeat the same `CreateFileSystem` call with `CreationToken=boot-1` — expect HTTP 409 `FileSystemAlreadyExists` naming the id from step 2, and no second file system.
4. `DescribeFileSystems` — expect exactly the one created file system with the same field values; `DescribeFileSystems FileSystemId=fs-deadbeef` (never created) must be 404 `FileSystemNotFound`.
5. `CreateMountTarget FileSystemId=<id> SubnetId=subnet-0123456789abcdef0` — expect 200 with a minted `fsmt-...` id, available state and an allocated `IpAddress`; then `DescribeFileSystems` must show `NumberOfMountTargets=1`.
6. `CreateMountTarget` again for the same file system and subnet — expect 409 `MountTargetConflict`.
7. `DescribeMountTargets FileSystemId=<id>` — expect the one stored mount target, identical to the create response.
8. `DeleteFileSystem <id>` while the mount target exists — expect 409 `FileSystemInUse`.
9. `DeleteMountTarget <fsmt-id>` — expect 204 empty body; `DescribeMountTargets` now returns an empty list and `NumberOfMountTargets` is back to 0; a repeat delete is 404 `MountTargetNotFound`.
10. `DeleteFileSystem <id>` — expect 204 empty body; `DescribeFileSystems FileSystemId=<id>` is then 404 `FileSystemNotFound` and a repeat delete is 404.

Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 543596051071af06ccfbb58d47ce453a1db8e676

# Bootstrap the S3Files (s3files) emulator

_Filed: 2026-10-21T22:39:03Z_
_Operation: service-com-amazonaws-s3files-s3files | Class: stub_

> _Note: this issue was identified against the code base at commit `543596051071af06ccfbb58d47ce453a1db8e676`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 21 operations in `src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the service holds no state. No client workflow can run.

This issue defines the FIRST, minimal happy-path slice: the file-system lifecycle plus one access point, implemented statefully over the restJson1 wire protocol (SDK id `S3Files`, sigv4 name `s3files`). Operations outside this slice stay 501 and will be filed as individual gap issues later.

### Happy path to implement (in order)

1. **CreateFileSystem** — `PUT /file-systems -> 201`. Input: `bucket` (required, `arn:aws*:s3:::<name>`), `roleArn` (required IAM role ARN), optional `prefix`, `kmsKeyId`, `tags`, `clientToken` (@idempotencyToken), `acceptBucketWarning`. Must mint a new `fileSystemId` matching `^fs-[0-9a-f]{17,40}$` and a matching `fileSystemArn` (`arn:aws:s3files:<region>:<account>:file-system/<fileSystemId>`), set `creationTime` to now, `ownerId` to the 12-digit account, `status` to `available`, echo back the stored `bucket`/`prefix` (default `""`)/`kmsKeyId`/`roleArn`/`tags`/`clientToken` — and PERSIST the record. Repeating the call with the same `clientToken` must be idempotent (return the same file system, not a second one).
2. **GetFileSystem** — `GET /file-systems/{fileSystemId} -> 200`. Must look the record up in state (accepting either the bare `fs-...` id or the file-system ARN) and return exactly what CreateFileSystem stored (`creationTime`, `fileSystemArn`, `bucket`, `prefix`, `kmsKeyId`, `roleArn`, `ownerId`, `status`, `tags`). Unknown id -> `ResourceNotFoundException` (HTTP 404); malformed id -> `ValidationException` (HTTP 400).
3. **ListFileSystems** — `GET /file-systems -> 200`. Returns `fileSystems` (list<ListFileSystemsDescription>, required) built from the stored map, filtered by the optional `bucket` query param, honouring `maxResults` (default 100) and round-tripping `nextToken` for the remainder (@paginated inputToken/outputToken=nextToken, items=fileSystems). A created file system must appear; a deleted one must not.
4. **CreateAccessPoint** — `PUT /access-points -> 200`. Input: `fileSystemId` (required), optional `posixUser`, `rootDirectory`, `tags`, `clientToken`. Must reject an unknown `fileSystemId` with `ResourceNotFoundException` (404), mint `accessPointId` matching `^fsap-[0-9a-f]{17,40}$` plus the required `accessPointArn` (`.../file-system/fs-.../access-point/fsap-...`), set `status` = `available`, `ownerId`, return the stored `posixUser`/`rootDirectory`/`tags`/`clientToken`, and persist the access point under its parent file system.
5. **GetAccessPoint** — `GET /access-points/{accessPointId} -> 200`. Must return the persisted access point (all required members: `accessPointArn`, `accessPointId`, `clientToken`, `fileSystemId`, `status`, `ownerId`) from state; unknown id -> `ResourceNotFoundException` (404).
6. **DeleteFileSystem** — `DELETE /file-systems/{fileSystemId} -> 204` with empty body. Must remove the record from state so a subsequent GetFileSystem returns 404 and ListFileSystems omits it. If the file system still has access points (or mount targets) and `forceDelete` is not true, fail with `ConflictException` (HTTP 409) rather than deleting; with `forceDelete=true` remove the children too. Delete of an unknown id -> `ResourceNotFoundException` (404).

### Non-negotiables

- Every response must be COMPUTED FROM PERSISTED STATE reached via `crate::state::service_state(&*self.engine)` (the shared engine) — never a canned struct, never a bare echo of the input, never a random id that is not stored. A create followed by a get/list must return what was created, and a delete must be observable by the next caller.
- Ids and ARNs must satisfy the model's patterns (`fs-[0-9a-f]{17,40}`, `fsap-[0-9a-f]{17,40}`, the s3files ARN forms) so SDK client-side validation accepts them.
- Modeled errors must use the modeled shapes/statuses: `ValidationException` 400, `ResourceNotFoundException` 404, `ConflictException` 409.
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line in each of the six handler files as it is implemented.

## State Model

In `src/state.rs` (`ComAmazonawsS3filesS3filesState`, reached through the shared engine):

- `file_systems: HashMap<String /* fileSystemId e.g. fs-0a1b... */, FileSystem>`
  - `FileSystem { file_system_id: String, file_system_arn: String, bucket: String, prefix: String /* default "" */, kms_key_id: Option<String>, role_arn: String, owner_id: String, status: LifeCycleState /* available */, status_message: Option<String>, creation_time: SystemTime, client_token: Option<String>, name: Option<String>, tags: Vec<Tag>, policy: Option<String>, sync_config: Option<SynchronizationConfiguration>, access_point_ids: Vec<String>, mount_target_ids: Vec<String> }`
- `access_points: HashMap<String /* fsap-... */, AccessPoint>`
  - `AccessPoint { access_point_id: String, access_point_arn: String, file_system_id: String, client_token: String, owner_id: String, status: LifeCycleState, posix_user: Option<PosixUser>, root_directory: Option<RootDirectory>, name: Option<String>, tags: Vec<Tag> }`
- `idempotency: HashMap<String /* clientToken */, String /* created resource id */>` so a repeated CreateFileSystem/CreateAccessPoint with the same token returns the first result.
- `id_counter: u64` (or a seeded RNG) used to mint pattern-conforming hex suffixes for `fs-`/`fsap-`/`fsmt-` ids.
- Reserved for the later slices (declare now, unused): `mount_targets: HashMap<String /* fsmt-... */, MountTarget>`.

Done = an integration test that runs steps 1-6 against the emulator and asserts the response BODIES (ids, ARNs, bucket/prefix/roleArn/tags, status, 404 after delete), not merely 2xx.

## Evidence

`services/service-com-amazonaws-s3files-s3files/OPERATIONS.md` lists all 21 operations with their routes, required input/output members and modeled errors — CreateFileSystem (`PUT /file-systems -> 201`, required `bucket`+`roleArn`, @idempotent with an @idempotencyToken `clientToken`), GetFileSystem (`GET /file-systems/{fileSystemId} -> 200`, @readonly), ListFileSystems (`GET /file-systems`, @paginated items=fileSystems, `maxResults` default 100), CreateAccessPoint (`PUT /access-points -> 200`, required `fileSystemId`, required output `accessPointArn`/`accessPointId`/`status`/`ownerId`), GetAccessPoint (`GET /access-points/{accessPointId}`), DeleteFileSystem (`DELETE /file-systems/{fileSystemId} -> 204` with `forceDelete` @httpQuery and a modeled `ConflictException` 409). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-s3files-s3files/src/ops/` matches all 21 handler files, each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), so no state exists. README.md documents the restJson1 protocol and the shared-engine state API used above. No tests assert any of this behavior (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `s3files` at it.
2. CreateFileSystem with bucket="arn:aws:s3:::my-bucket", roleArn="arn:aws:iam::123456789012:role/S3FilesRole", prefix="data/", tags=[{Key:env,Value:test}] — expect HTTP 201 with a newly minted fileSystemId matching ^fs-[0-9a-f]{17,40}$, a matching fileSystemArn, creationTime, status=available. Today: HTTP 501 NotImplemented.
3. Repeat the same CreateFileSystem call with the same clientToken — expect the SAME fileSystemId (idempotent), not a second file system.
4. GetFileSystem with that fileSystemId — expect 200 and the stored bucket/prefix/roleArn/tags/status, byte-for-byte what create returned.
5. ListFileSystems (no filter, then bucket="arn:aws:s3:::my-bucket") — expect the created file system present in `fileSystems`; with maxResults=1 and two file systems expect a `nextToken` that returns the remainder.
6. CreateAccessPoint with that fileSystemId and a posixUser/rootDirectory — expect 200, accessPointId matching ^fsap-[0-9a-f]{17,40}$ and accessPointArn containing the parent fs id; CreateAccessPoint with fileSystemId="fs-00000000000000000" must return ResourceNotFoundException (404).
7. GetAccessPoint with the returned accessPointId — expect 200 with the persisted posixUser/rootDirectory/fileSystemId/status.
8. DeleteFileSystem without forceDelete while the access point exists — expect ConflictException (409); then DeleteFileSystem with forceDelete=true — expect 204.
9. GetFileSystem on the deleted id — expect ResourceNotFoundException (404); ListFileSystems must no longer include it.

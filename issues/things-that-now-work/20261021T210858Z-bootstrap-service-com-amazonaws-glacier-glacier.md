Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the glacier emulator

_Filed: 2026-10-21T21:08:58Z_
_Operation: service-com-amazonaws-glacier-glacier | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 33 handlers under `services/service-com-amazonaws-glacier-glacier/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Glacier client can do anything at all. Implement a FIRST, SMALL, stateful slice: the vault + archive + retrieval-job happy path, using the restJson1 wire bindings exactly as OPERATIONS.md documents them (routes, `@httpLabel` path params, `@httpHeader` outputs, `@httpPayload` bodies).

Happy path, in order (all fields per OPERATIONS.md):

1. **CreateVault** — `PUT /{accountId}/vaults/{vaultName}` -> 201. Input `accountId`, `vaultName` (both `@httpLabel`, required). Output header `Location` = `/{accountId}/vaults/{vaultName}`. Must INSERT a vault record into the store keyed by (accountId, vaultName); idempotent re-create of an existing vault returns 201 with the same Location (real Glacier treats CreateVault as idempotent) and must not wipe existing archives.
2. **UploadArchive** — `POST /{accountId}/vaults/{vaultName}/archives` -> 201. Input `body` (blob, `@httpPayload`, required), `archiveDescription` (`x-amz-archive-description`), `checksum` (`x-amz-sha256-tree-hash`). Must STORE the archive bytes and mint a fresh opaque `archiveId`; output headers `x-amz-archive-id`, `x-amz-sha256-tree-hash` (the SHA-256 tree hash computed over the stored body, not the echoed request header), and `Location` = `/{accountId}/vaults/{vaultName}/archives/{archiveId}`. Unknown vault -> `ResourceNotFoundException` (404).
3. **DescribeVault** — `GET /{accountId}/vaults/{vaultName}` -> 200. Output `VaultName`, `VaultARN` (`arn:aws:glacier:<region>:<accountId>:vaults/<vaultName>`), `CreationDate` (ISO-8601 of the CreateVault call), and `NumberOfArchives` / `SizeInBytes` COMPUTED by counting/summing the stored archives — these two are the primary anti-hollow check and must not be hardcoded 0 once an archive exists. Unknown vault -> `ResourceNotFoundException` (404).
4. **InitiateJob** — `POST /{accountId}/vaults/{vaultName}/jobs` -> 202. Input `jobParameters` (`JobParameters`, `@httpPayload`) carrying `Type` (`archive-retrieval` | `inventory-retrieval`), `ArchiveId`, `Description`, `Tier`, `RetrievalByteRange`. Must INSERT a job record and return header `x-amz-job-id` plus `Location`. For `archive-retrieval`, an `ArchiveId` that is not in the vault -> `ResourceNotFoundException` (404); missing `ArchiveId` for archive-retrieval -> `InvalidParameterValueException` (400).
5. **DescribeJob** — `GET /{accountId}/vaults/{vaultName}/jobs/{jobId}` -> 200. Output read back from the job record: `JobId`, `Action` (`ArchiveRetrieval`/`InventoryRetrieval`), `ArchiveId`, `VaultARN`, `CreationDate`, `StatusCode` (`InProgress`/`Succeeded`), `Completed`, `ArchiveSizeInBytes`, `ArchiveSHA256TreeHash`, `SHA256TreeHash`, `CompletionDate`, `Tier`, `JobDescription`. The emulator may complete jobs immediately (or after a short simulated delay), but `Completed`/`StatusCode`/`CompletionDate` must be derived from the stored job, never canned. Unknown jobId -> `ResourceNotFoundException` (404).
6. **GetJobOutput** — `GET /{accountId}/vaults/{vaultName}/jobs/{jobId}/output` -> 200. Must return the ACTUAL stored archive bytes as `body` (`@httpPayload`) for an archive-retrieval job — byte-for-byte equal to what UploadArchive received — with `x-amz-sha256-tree-hash`, `Content-Type`, `Accept-Ranges`, `x-amz-archive-description`, and `status` (`@httpResponseCode`) = 200, or 206 with `Content-Range` when the `Range` request header is present. For inventory-retrieval, return a JSON inventory listing the vault's stored archives (ArchiveId, ArchiveDescription, CreationDate, Size, SHA256TreeHash) built from state.
7. **DeleteArchive** — `DELETE /{accountId}/vaults/{vaultName}/archives/{archiveId}` -> 204. Must REMOVE the archive so DescribeVault's `NumberOfArchives`/`SizeInBytes` drop accordingly; unknown archive -> `ResourceNotFoundException` (404).
8. **DeleteVault** — `DELETE /{accountId}/vaults/{vaultName}` -> 204. Must REMOVE the vault; a subsequent DescribeVault -> `ResourceNotFoundException` (404).

Also in this slice because clients list before they read: **ListVaults** (`GET /{accountId}/vaults`, output `VaultList` of `DescribeVaultOutput` + `Marker`, honouring `limit`/`marker`) and **ListJobs** (`GET /{accountId}/vaults/{vaultName}/jobs`, output `JobList` + `Marker`, honouring `limit`/`marker`/`statuscode`/`completed`) rendered from the same stored records as DescribeVault/DescribeJob.

NEVER STUBS: every one of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). No hardcoded IDs without storage, no echoing the request's `x-amz-sha256-tree-hash` back as the computed checksum, no empty `VaultList`/`JobList`, no canned `Completed: true` on a job that was never recorded. `accountId` of `-` must resolve to the caller's account so SDK defaults work.

## State Model

In `src/state.rs` (`ComAmazonawsGlacierGlacierState`, lock-guarded):

- `vaults: HashMap<(String /*accountId*/, String /*vaultName*/), Vault>`
- `Vault { vault_name: String, vault_arn: String, creation_date: String /*ISO-8601*/, last_inventory_date: Option<String>, archives: HashMap<String /*archiveId*/, Archive>, jobs: HashMap<String /*jobId*/, Job>, tags: HashMap<String,String>, access_policy: Option<String>, notification_config: Option<VaultNotificationConfig>, multipart_uploads: HashMap<String /*uploadId*/, MultipartUpload> }`
- `Archive { archive_id: String, body: Vec<u8>, size_in_bytes: i64, sha256_tree_hash: String, description: Option<String>, creation_date: String }`
- `Job { job_id: String, action: ActionCode, archive_id: Option<String>, description: Option<String>, tier: Option<String>, retrieval_byte_range: Option<String>, status_code: StatusCode, completed: bool, creation_date: String, completion_date: Option<String>, archive_size_in_bytes: Option<i64>, sha256_tree_hash: Option<String>, archive_sha256_tree_hash: Option<String>, output: Vec<u8> /*materialised archive bytes or inventory JSON*/, sns_topic: Option<String> }`
- `MultipartUpload { upload_id: String, part_size: i64, archive_description: Option<String>, creation_date: String, parts: BTreeMap<u64 /*range start*/, (Vec<u8>, String /*part checksum*/)> }` — declared now so the multipart slice (InitiateMultipartUpload/UploadMultipartPart/CompleteMultipartUpload/ListParts) can land next without reshaping state.
- `data_retrieval_policies: HashMap<String /*accountId*/, DataRetrievalPolicy>`, `provisioned_capacity: HashMap<String /*accountId*/, Vec<ProvisionedCapacityDescription>>`
- Counters/generators for `archiveId`, `jobId`, `uploadId` so each minted id is unique AND the minted id is the key under which the object was actually inserted.

Derived, never stored-and-stale: `NumberOfArchives` = `vault.archives.len()`, `SizeInBytes` = sum of `archive.size_in_bytes`.

Everything else in the model (vault lock: InitiateVaultLock/CompleteVaultLock/AbortVaultLock/GetVaultLock; tags: AddTagsToVault/RemoveTagsFromVault/ListTagsForVault; policies: Set/Get/DeleteVaultAccessPolicy, Set/GetDataRetrievalPolicy; notifications: Set/Get/DeleteVaultNotifications; multipart: InitiateMultipartUpload/UploadMultipartPart/CompleteMultipartUpload/AbortMultipartUpload/ListParts/ListMultipartUploads; provisioned capacity; Select-type jobs and `OutputLocation`) stays 501 for now and will be filed as individual gap issues.

Done = an integration test driving the numbered workflow end-to-end passes with full fidelity: not just 2xx, but the retrieved bytes equal the uploaded bytes, the tree hashes match, DescribeVault's archive count/size track uploads and deletes, and the deleted vault is really gone (404).

## Evidence

All 33 operation files under `services/service-com-amazonaws-glacier-glacier/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header on line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, with zero state writes (verified by grep over the whole ops directory: 33/33 files match). Routes, status codes, required members, header bindings and error shapes above are taken verbatim from the crate's generated `OPERATIONS.md` (CreateVault `PUT /{accountId}/vaults/{vaultName}` -> 201 with `Location`; UploadArchive `POST .../archives` -> 201 with `x-amz-archive-id`/`x-amz-sha256-tree-hash`; DescribeVault output `NumberOfArchives`/`SizeInBytes` required with default 0; InitiateJob `POST .../jobs` -> 202 with `x-amz-job-id`; DescribeJob output incl. `Completed` required and `StatusCode`; GetJobOutput `body` `@httpPayload` required plus `status` `@httpResponseCode` and `Content-Range`; DeleteArchive/DeleteVault -> 204; ListVaults/ListJobs `@paginated(inputToken=marker, outputToken=Marker)`), which is rendered from `smithy-model/com-amazonaws-glacier-glacier.json`. Protocol is `restJson1`, sigv4 name `glacier`. Well-established Glacier semantics ground the behavioural requirements: archives are retrieved only via an asynchronous retrieval job (InitiateJob -> poll DescribeJob -> GetJobOutput), the SHA-256 tree hash is computed over the archive content, and operations against a non-existent vault/archive/job fail with `ResourceNotFoundException` (404). No tests exist beyond the scaffold's `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Glacier client at it (`aws glacier --endpoint-url <emulator> --account-id -`).
2. `create-vault --vault-name demo-vault` -> expect HTTP 201 with a `Location` header; actual: HTTP 501 NotImplemented.
3. `upload-archive --vault-name demo-vault --body ./hello.txt` -> expect 201 with `x-amz-archive-id` and the tree hash of the file's bytes; record the archiveId.
4. `describe-vault --vault-name demo-vault` -> expect 200 with `NumberOfArchives` 1 and `SizeInBytes` equal to hello.txt's length (not 0).
5. `list-vaults` -> expect `VaultList` containing demo-vault with the same computed counts.
6. `initiate-job --vault-name demo-vault --job-parameters '{"Type":"archive-retrieval","ArchiveId":"<archiveId>"}'` -> expect 202 with `x-amz-job-id`.
7. `describe-job --vault-name demo-vault --job-id <jobId>` -> expect 200 with StatusCode Succeeded / Completed true and the archive's size and tree hash; `list-jobs --vault-name demo-vault` must include the same job.
8. `get-job-output --vault-name demo-vault --job-id <jobId> out.txt` -> expect 200 and `out.txt` byte-identical to `hello.txt`; repeat with `--range bytes=0-3` and expect 206 plus a `Content-Range` header.
9. `delete-archive --vault-name demo-vault --archive-id <archiveId>` -> 204, then `describe-vault` must report `NumberOfArchives` 0 and `SizeInBytes` 0.
10. `delete-vault --vault-name demo-vault` -> 204, then `describe-vault --vault-name demo-vault` -> expect HTTP 404 `ResourceNotFoundException`, and `list-vaults` must no longer list it.

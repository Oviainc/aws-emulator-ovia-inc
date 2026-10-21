Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the com-amazonaws-backup-cryocontrollerusermanager emulator

_Filed: 2026-10-21T21:19:11Z_
_Operation: service-com-amazonaws-backup-cryocontrollerusermanager | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 115 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state. Nothing works end to end.

This issue scopes the FIRST buildable slice: the AWS Backup vault + backup-job happy path (restJson1, SDK id `Backup`, signing name `backup`). Implement these 6 operations, in this order, so a realistic client script runs green:

1. **CreateBackupVault** — `PUT /backup-vaults/{BackupVaultName}` -> 200. In: `BackupVaultName` (@httpLabel, required, pattern `^[a-zA-Z0-9\-\_]{2,50}$`), `BackupVaultTags`, `EncryptionKeyArn`, `CreatorRequestId`. Out: `BackupVaultName`, `BackupVaultArn` (synthesized `arn:aws:backup:<region>:<account>:backup-vault:<name>`), `CreationDate`. Must INSERT a vault record into state; a second create of the same name with different parameters must return `AlreadyExistsException` (HTTP 400). Reject a name violating the pattern with `InvalidParameterValueException`.
2. **DescribeBackupVault** — `GET /backup-vaults/{BackupVaultName}` -> 200. In: `BackupVaultName` (@httpLabel, required), `BackupVaultAccountId` (@httpQuery). Out: `BackupVaultName`, `BackupVaultArn`, `CreationDate`, `CreatorRequestId`, `EncryptionKeyArn`, `VaultType` (`BACKUP_VAULT`), `VaultState` (`AVAILABLE`), `NumberOfRecoveryPoints` (required, default 0) — the count must be COMPUTED from the recovery points actually stored in that vault, not hardcoded 0. Unknown vault -> `ResourceNotFoundException` (HTTP 400).
3. **StartBackupJob** — `PUT /backup-jobs` -> 200. In: `BackupVaultName` (required), `ResourceArn` (required), `IamRoleArn` (required), `IdempotencyToken`, `StartWindowMinutes`, `CompleteWindowMinutes`, `Lifecycle`, `RecoveryPointTags`, `BackupOptions`, `Index`. Out: `BackupJobId` (freshly generated), `RecoveryPointArn` (generated, e.g. `arn:aws:backup:<region>:<account>:recovery-point:<uuid>`), `CreationDate`, `IsParent` (required, false). Must create BOTH a persisted backup-job record and a persisted recovery point inside the named vault. Unknown vault -> `ResourceNotFoundException`; missing required member -> `MissingParameterValueException`. Same `IdempotencyToken` replayed must return the same `BackupJobId`/`RecoveryPointArn` without creating a duplicate.
4. **DescribeBackupJob** — `GET /backup-jobs/{BackupJobId}` -> 200. Out (from the stored job, not echoed input): `BackupJobId`, `BackupVaultName`, `BackupVaultArn`, `RecoveryPointArn`, `ResourceArn`, `IamRoleArn`, `CreationDate`, `CompletionDate`, `State` (`COMPLETED` once the emulated job has settled; `CREATED`/`RUNNING` are acceptable intermediate values as long as the transition is driven by stored state, not by a coin flip), `PercentDone`, `BackupSizeInBytes`, `IsEncrypted` (required, default false), `IsParent` (required, false), `BackupOptions`. Unknown id -> `ResourceNotFoundException`.
5. **ListRecoveryPointsByBackupVault** — `GET /backup-vaults/{BackupVaultName}/recovery-points` -> 200, @paginated(inputToken=NextToken, outputToken=NextToken, items=RecoveryPoints). Must return the recovery points StartBackupJob stored in that vault as `RecoveryPoints` (list<RecoveryPointByBackupVault>) with `RecoveryPointArn`, `BackupVaultName`, `BackupVaultArn`, `ResourceArn`, `ResourceType`, `CreationDate`, `BackupSizeInBytes`, `Status` (`COMPLETED`), `IamRoleArn`. Honour `MaxResults` (range 1..1000) and round-trip `NextToken`; apply at least the `ByResourceArn` filter. Unknown vault -> `ResourceNotFoundException`.
6. **DeleteBackupVault** — `DELETE /backup-vaults/{BackupVaultName}` -> 200, empty body. Must REMOVE the vault from state; a vault that still holds recovery points must be refused with `InvalidRequestException` (AWS requires the vault be empty). Unknown vault -> `ResourceNotFoundException`. After a successful delete, DescribeBackupVault and ListRecoveryPointsByBackupVault for that name must 400 `ResourceNotFoundException` and ListBackupVaults must not include it.

Hard requirement — NEVER STUBS: every one of these six must compute its response from the shared engine state reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the request back, no generating an id that is never stored. A create followed by a describe must return exactly what was created; a delete followed by a describe must 400. Do not construct a fresh engine inside a handler (see README warning) or writes will vanish between requests.

## State Model

In `src/state.rs`, on `ComAmazonawsBackupCryocontrollerusermanagerState`:

- `vaults: BTreeMap<String /*BackupVaultName*/, BackupVault>` where
  `BackupVault { name: String, arn: String, encryption_key_arn: Option<String>, creator_request_id: Option<String>, creation_date: SystemTime, vault_type: VaultType /*BACKUP_VAULT*/, vault_state: VaultState /*AVAILABLE*/, tags: BTreeMap<String,String>, locked: bool, min_retention_days: Option<i64>, max_retention_days: Option<i64> }`
- `recovery_points: BTreeMap<String /*RecoveryPointArn*/, RecoveryPoint>` where
  `RecoveryPoint { arn: String, backup_vault_name: String, backup_vault_arn: String, resource_arn: String, resource_type: String, iam_role_arn: String, status: RecoveryPointStatus /*COMPLETED*/, creation_date: SystemTime, completion_date: Option<SystemTime>, backup_size_in_bytes: i64, lifecycle: Option<Lifecycle>, is_encrypted: bool, is_parent: bool, tags: BTreeMap<String,String>, created_by: Option<RecoveryPointCreator> }`
  plus an index `recovery_points_by_vault: BTreeMap<String /*vault name*/, Vec<String /*rp arn*/>>` (insertion order) so the list op and `NumberOfRecoveryPoints` are derived, never stored twice inconsistently.
- `backup_jobs: BTreeMap<String /*BackupJobId*/, BackupJob>` where
  `BackupJob { job_id: String, backup_vault_name: String, backup_vault_arn: String, recovery_point_arn: String, resource_arn: String, resource_type: String, iam_role_arn: String, state: BackupJobState, status_message: Option<String>, percent_done: String, backup_size_in_bytes: i64, bytes_transferred: i64, creation_date: SystemTime, completion_date: Option<SystemTime>, start_by: Option<SystemTime>, backup_options: BTreeMap<String,String>, is_parent: bool, is_encrypted: bool }`
- `idempotency_tokens: BTreeMap<String /*IdempotencyToken*/, String /*BackupJobId*/>` for StartBackupJob replay.
- `account_id: String` and `region: String` used to synthesize all ARNs consistently so the ARNs returned by StartBackupJob match those keyed in `recovery_points`.

Out-of-scope operations (backup plans, selections, frameworks, report plans, legal holds, copy/restore/scan jobs, tiering, access points, tagging, notifications, vault policies, settings) stay at 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md at the crate root (`services/service-com-amazonaws-backup-cryocontrollerusermanager/OPERATIONS.md`) documents 115 operations with protocol restJson1 / SDK id `Backup`, and gives each contract cited above: CreateBackupVault (line 65: `PUT /backup-vaults/{BackupVaultName}`, out BackupVaultName/BackupVaultArn/CreationDate, errors AlreadyExists/InvalidParameterValue/LimitExceeded/MissingParameterValue), DescribeBackupVault (line 254, out includes `NumberOfRecoveryPoints` long default 0 required), StartBackupJob (line 781: `PUT /backup-jobs`, required BackupVaultName/ResourceArn/IamRoleArn, out BackupJobId/RecoveryPointArn/CreationDate/IsParent), DescribeBackupJob (line 246: `GET /backup-jobs/{BackupJobId}`, out State of BackupJobState enum), ListRecoveryPointsByBackupVault (line 622: @paginated inputToken=NextToken outputToken=NextToken items=RecoveryPoints, MaxResults range 1..1000, By* filters), DeleteBackupVault (line 159: `DELETE /backup-vaults/{BackupVaultName}`, errors include InvalidRequestException and ResourceNotFoundException). All-stub status verified: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches every operation file in the crate (115 files), and README.md lines 36-49 confirm a scaffold answers HTTP 501 until the marker is removed. The emptiness/vault-must-be-empty rule for DeleteBackupVault and the NumberOfRecoveryPoints semantics are well-established AWS Backup behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `backup` client at it.
2. `CreateBackupVault(BackupVaultName="demo-vault")` -> expect 200 with BackupVaultName="demo-vault", a backup-vault ARN and a CreationDate; actual: HTTP 501 NotImplemented.
3. `DescribeBackupVault(BackupVaultName="demo-vault")` -> expect 200 echoing the stored vault with NumberOfRecoveryPoints=0.
4. `ListBackupVaults()` / repeat `CreateBackupVault("demo-vault")` -> expect the vault listed once, and the duplicate create to fail with AlreadyExistsException (400).
5. `StartBackupJob(BackupVaultName="demo-vault", ResourceArn="arn:aws:dynamodb:us-east-1:123456789012:table/demo", IamRoleArn="arn:aws:iam::123456789012:role/service-role/AWSBackupDefaultServiceRole", IdempotencyToken="tok-1")` -> expect 200 with a new BackupJobId and RecoveryPointArn; replay with the same IdempotencyToken and expect the SAME ids.
6. `DescribeBackupJob(BackupJobId=<from above>)` -> expect 200 whose BackupVaultName, ResourceArn, IamRoleArn and RecoveryPointArn match what was submitted and whose State comes from stored job state.
7. `ListRecoveryPointsByBackupVault(BackupVaultName="demo-vault")` -> expect the one recovery point created by the job, with the same RecoveryPointArn and ResourceArn; re-run `DescribeBackupVault` and expect NumberOfRecoveryPoints=1.
8. `ListRecoveryPointsByBackupVault(BackupVaultName="demo-vault", MaxResults=1)` with two jobs started -> expect 1 item plus a NextToken that returns the second item and no NextToken.
9. `DeleteBackupVault(BackupVaultName="demo-vault")` while it still holds recovery points -> expect 400 InvalidRequestException.
10. `DeleteRecoveryPoint`-free cleanup path: start from a fresh empty vault, `DeleteBackupVault` it -> expect 200, then `DescribeBackupVault` on it -> expect 400 ResourceNotFoundException and absence from ListBackupVaults.
11. `DescribeBackupVault(BackupVaultName="never-created")` -> expect 400 ResourceNotFoundException, not 501.

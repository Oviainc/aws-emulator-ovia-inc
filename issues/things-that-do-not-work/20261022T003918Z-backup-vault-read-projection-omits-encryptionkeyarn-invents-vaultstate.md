Issue-Source: trace-sweep-divergence
Issue-Trace: backup-backup-vault-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-backup-vault-lifecycle backup-backup-vault-protected-resources-lifecycle backup-mpa-approval-team-association-errors backup-recovery-point-error-paths-on-live-vault backup-vault-lifecycle-and-listing backup-vault-lock-configuration-lifecycle backup-backup-vault-lock-configuration
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|DescribeBackupVault|2xx->2xx|$.EncryptionKeyArn
Issue-Key: backup|DescribeBackupVault|2xx->2xx|$.EncryptionKeyType
Issue-Key: backup|DescribeBackupVault|2xx->2xx|$.VaultState
Issue-Key: backup|DescribeBackupVault|2xx->2xx|$.LockDate
Issue-Key: backup|DescribeBackupVault|2xx->2xx|$.Locked
Issue-Key: backup|DescribeBackupVault|2xx->2xx|$.MaxRetentionDays
Issue-Key: backup|DescribeBackupVault|2xx->2xx|$.MinRetentionDays
Issue-Key: backup|ListBackupVaults|2xx->2xx|$.BackupVaultList[*]
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Backup vault read projection omits EncryptionKeyArn, invents VaultState and EncryptionKeyType, and loses vault-lock configuration

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackupVault

## Description
The emulator's vault read model does not match what AWS Backup projects for a standard BACKUP_VAULT: it drops the KMS key ARN, emits two enum members AWS leaves unset, and does not retain vault-lock configuration at all. This issue bundles THREE causes of that one projection/state fault; all three must be closed.

Cause 1 of 3 — DescribeBackupVault projection of a freshly created vault.
AWS: Smithy: DescribeBackupVault GET /backup-vaults/{BackupVaultName} -> DescribeBackupVaultOutput with members BackupVaultName, BackupVaultArn, VaultType, VaultState (enum CREATING|AVAILABLE|FAILED), EncryptionKeyArn (ARN), CreationDate, CreatorRequestId, NumberOfRecoveryPoints, Locked, MinRetentionDays, MaxRetentionDays, LockDate, EncryptionKeyType (enum AWS_OWNED_KMS_KEY|CUSTOMER_MANAGED_KMS_KEY). Recorded behavior for a default BACKUP_VAULT: EncryptionKeyArn IS returned (a kms key ARN), while VaultState and EncryptionKeyType are left UNSET (recorded behavior only — the model permits them but AWS does not populate them for this vault type).
Emulator: returns 200 with EncryptionKeyArn missing, and adds VaultState='AVAILABLE' and EncryptionKeyType='AWS_OWNED_KMS_KEY'.
Why: a client that reads the vault's encryption key (to check or reuse the CMK, or to compare vaults) gets nothing, and a client that treats presence of VaultState/EncryptionKeyType as meaningful sees fabricated values.
Fix: the DescribeBackupVault handler's output projection and the vault record in the service state (store the encryption key ARN at create time; leave VaultState/EncryptionKeyType unset for a standard vault).
Traces: backup-backup-vault-lifecycle#task#2 (also backup-backup-vault-protected-resources-lifecycle#task#2, backup-mpa-approval-team-association-errors#task#3, backup-recovery-point-error-paths-on-live-vault#task#3, backup-vault-lifecycle-and-listing#task#3, backup-vault-lock-configuration-lifecycle#task#5).

Cause 2 of 3 — DescribeBackupVault after PutBackupVaultLockConfiguration: lock state not persisted.
AWS: Smithy: DescribeBackupVaultOutput exposes Locked (boolean), MinRetentionDays, MaxRetentionDays and LockDate; after a vault-lock configuration is applied, the recorded response reports Locked=true with the configured MinRetentionDays/MaxRetentionDays and a LockDate.
Emulator: returns 200 with Locked=false and MinRetentionDays, MaxRetentionDays and LockDate all absent (and the same EncryptionKeyArn/VaultState/EncryptionKeyType divergence as cause 1).
Why: vault lock is a compliance control; a client that applies a lock and then verifies it is told the vault is unlocked, and retention bounds it set are invisible.
Fix: the PutBackupVaultLockConfiguration handler must record the lock fields onto the vault in the service state, and DescribeBackupVault must project them.
Traces: backup-backup-vault-lock-configuration#task#3 (also backup-vault-lock-configuration-lifecycle#task#3).

Cause 3 of 3 — ListBackupVaults entries carry the same wrong projection.
AWS: Smithy: ListBackupVaults GET /backup-vaults -> BackupVaultList. Recorded entries carry EncryptionKeyArn and leave VaultState/EncryptionKeyType unset, matching the Describe projection.
Emulator: each list entry omits EncryptionKeyArn and adds VaultState='AVAILABLE' and EncryptionKeyType='AWS_OWNED_KMS_KEY', so the list and describe views are wrong in the same way.
Why: a client enumerating vaults cannot see which key encrypts each vault, and sees fabricated state values.
Fix: share one vault-to-wire projection between the ListBackupVaults and DescribeBackupVault handlers so a single fix covers both.
Traces: backup-backup-vault-lifecycle#task#3 (also backup-vault-lifecycle-and-listing#task#4).

## Evidence
Cause 1 (DescribeBackupVault, backup-backup-vault-lifecycle#task#2): status 200 both sides; $.EncryptionKeyArn expected an arn:aws:kms:...:key/... value, got None; $.EncryptionKeyType expected unset, got 'AWS_OWNED_KMS_KEY'; $.VaultState expected unset, got 'AVAILABLE'. Cluster covers 6 fixture steps.
Cause 2 (DescribeBackupVault, backup-backup-vault-lock-configuration#task#3): status 200 both sides; $.Locked expected True, got False; $.MinRetentionDays expected 7, got None; $.MaxRetentionDays expected 30, got None; $.LockDate expected a timestamp, got None; plus the cause-1 EncryptionKeyArn/EncryptionKeyType/VaultState divergence.
Cause 3 (ListBackupVaults, backup-backup-vault-lifecycle#task#3): status 200 both sides; $.BackupVaultList[0] expected to carry EncryptionKeyArn with VaultState and EncryptionKeyType unset, got an entry with no EncryptionKeyArn and with VaultState='AVAILABLE', EncryptionKeyType='AWS_OWNED_KMS_KEY'.
Smithy citation: DescribeBackupVaultOutput members EncryptionKeyArn (ARN), VaultState (enum CREATING|AVAILABLE|FAILED), EncryptionKeyType (enum AWS_OWNED_KMS_KEY|CUSTOMER_MANAGED_KMS_KEY), Locked, MinRetentionDays, MaxRetentionDays, LockDate.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (trace set simparity-wire-aws-v1).
2. Cause 1: fixture backup-backup-vault-lifecycle, step task#2 — PUT /backup-vaults/<name> then GET /backup-vaults/<name>; compare EncryptionKeyArn, VaultState, EncryptionKeyType against the recording.
3. Cause 2: fixture backup-backup-vault-lock-configuration, step task#3 — apply a vault lock configuration, then GET /backup-vaults/<name>; compare Locked, MinRetentionDays, MaxRetentionDays, LockDate.
4. Cause 3: fixture backup-backup-vault-lifecycle, step task#3 — GET /backup-vaults and compare the list entry's projection with the recording.

Issue-Source: trace-sweep-divergence
Issue-Trace: backup-logically-air-gapped-vault-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-logically-air-gapped-vault-lifecycle
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.BackupVaultArn
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.BackupVaultName
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.CreationDate
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.CreatorRequestId
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.EncryptionKeyArn
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.EncryptionKeyType
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.LockDate
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.Locked
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.MaxRetentionDays
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.MinRetentionDays
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.NumberOfRecoveryPoints
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.VaultState
Issue-Key: backup|DescribeBackupVault|2xx->4xx|$.VaultType
Issue-Key: backup|DescribeBackupVault|2xx->4xx|status
Issue-Key: backup|DeleteBackupVault|4xx->4xx|$.Code
Issue-Key: backup|DeleteBackupVault|4xx->4xx|$.Message
Issue-Key: backup|DeleteBackupVault|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|DeleteBackupVault|2xx->4xx|status
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Logically air-gapped vault creation does not persist the vault, so its whole lifecycle fails

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackupVault

## Description
Creating a LOGICALLY_AIR_GAPPED_BACKUP_VAULT answers on the wire without the vault ever entering the service's vault store: every subsequent operation on that vault name resolves to nothing and is refused as ResourceNotFoundException. This issue bundles THREE causes rooted in that one missing state write; all three must be closed.

Cause 1 of 3 — DescribeBackupVault cannot read back the created air-gapped vault.
AWS: Smithy: DescribeBackupVault GET /backup-vaults/{BackupVaultName} -> 200 with VaultType and VaultState (enum CREATING|AVAILABLE|FAILED). Recorded behavior: after the air-gapped vault is created the vault is readable and its VaultState progresses to 'AVAILABLE', with VaultType='LOGICALLY_AIR_GAPPED_BACKUP_VAULT', Locked=true and the configured MinRetentionDays/MaxRetentionDays.
Emulator: HTTP 400 {'__type': 'ResourceNotFoundException', 'Message': 'Backup vault <name> not found'} on every poll attempt, so the whole output structure is absent and the VaultState='AVAILABLE' wait never converges (poll capped at 60s after 7 attempts).
Why: an air-gapped vault a client just created is invisible; any workflow that creates one and then waits for it to become AVAILABLE never proceeds, and times out instead of failing fast.
Fix: the CreateLogicallyAirGappedBackupVault handler must insert the vault (with VaultType, lock fields and an initial CREATING state that settles to AVAILABLE) into the service's vault state that DescribeBackupVault reads.
Traces: backup-logically-air-gapped-vault-lifecycle#task#3.

Cause 2 of 3 — DeleteBackupVault on an air-gapped vault still creating returns the wrong error class.
AWS: Smithy: DeleteBackupVault DELETE /backup-vaults/{BackupVaultName} models InvalidRequestException and ResourceNotFoundException (both httpError 400), each with Code/Message/Type/Context members. Recorded response while the air-gapped vault is in Creating state: HTTP 400 with x-amzn-errortype='InvalidRequestException', Code='ERROR_9100' and Message="Backup vault cannot be deleted because it's still in Creating state." (fixed AWS text, quoted as an example; the requirements are the InvalidRequestException error type and Code ERROR_9100).
Emulator: HTTP 400 with x-amzn-errortype='ResourceNotFoundException', no Code, and Message='Backup vault <name> not found' — because the vault was never stored, the state-dependent refusal is replaced by a not-found.
Why: a client that treats InvalidRequestException/ERROR_9100 as 'retry the delete once creation finishes' instead sees a permanent not-found and abandons the vault.
Fix: as cause 1, plus the DeleteBackupVault handler's vault-state guard, which must reject a CREATING vault with InvalidRequestException and Code ERROR_9100.
Traces: backup-logically-air-gapped-vault-lifecycle#task#2.

Cause 3 of 3 — DeleteBackupVault of the air-gapped vault fails instead of succeeding.
AWS: Smithy: DeleteBackupVault returns HTTP 200 with an empty body (output Unit) when the vault is deletable. Recorded response: 200.
Emulator: HTTP 400 (ResourceNotFoundException), since the vault is absent from state.
Why: the teardown path of any air-gapped-vault workflow reports failure for a delete AWS accepts, leaving client code unable to clean up.
Fix: as cause 1 — once the vault is persisted, the delete handler's normal success path applies.
Traces: backup-logically-air-gapped-vault-lifecycle#task#5.

## Evidence
Cause 1 (DescribeBackupVault, backup-logically-air-gapped-vault-lifecycle#task#3): status expected 200, got 400; every output member missing (BackupVaultArn, BackupVaultName, VaultType expected 'LOGICALLY_AIR_GAPPED_BACKUP_VAULT', VaultState expected 'AVAILABLE', Locked expected True, MaxRetentionDays expected 365, MinRetentionDays expected 7, NumberOfRecoveryPoints expected 0); body {'__type': 'ResourceNotFoundException', 'Message': 'Backup vault <name> not found'}; poll: VaultState not in ['AVAILABLE'] after 7 attempt(s), capped at 60s.
Cause 2 (DeleteBackupVault, backup-logically-air-gapped-vault-lifecycle#task#2): expected 400 with $.Code='ERROR_9100', errortype 'InvalidRequestException', Message "Backup vault cannot be deleted because it's still in Creating state."; actual 400 with $.Code missing, errortype 'ResourceNotFoundException', Message 'Backup vault <name> not found'.
Cause 3 (DeleteBackupVault, backup-logically-air-gapped-vault-lifecycle#task#5): status expected 200, got 400.
Note: the create step of this fixture did not itself diverge on the wire — the vault simply never appears in the emulator's state afterwards.
Smithy citation: DeleteBackupVault errors InvalidRequestException / ResourceNotFoundException (httpError 400, members Code/Message/Type/Context), success code 200 with output Unit; VaultState enum = [CREATING, AVAILABLE, FAILED].

## Steps to Reproduce
1. Replay the wire corpus against the emulator (trace set simparity-wire-aws-v1).
2. Fixture backup-logically-air-gapped-vault-lifecycle: create the logically air-gapped vault, then step task#3 — GET /backup-vaults/<name> and poll for VaultState=AVAILABLE; observe 400 ResourceNotFoundException on every attempt.
3. Step task#2 — DELETE /backup-vaults/<name> while the vault should be in Creating state; expect 400 InvalidRequestException with Code ERROR_9100, observe 400 ResourceNotFoundException.
4. Step task#5 — DELETE /backup-vaults/<name> once deletable; expect 200, observe 400.

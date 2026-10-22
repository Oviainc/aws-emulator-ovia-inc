Issue-Source: trace-sweep-divergence
Issue-Trace: backup-backup-vault-lifecycle
Issue-Step: teardown#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-backup-vault-lifecycle backup-backup-vault-protected-resources-lifecycle backup-cross-vault-recovery-point-account-scoped-listing backup-operate-on-nonexistent-vault backup-protected-resources-listing backup-vault-lifecycle-and-listing backup-vault-not-found-errors backup-logically-air-gapped-vault-lifecycle
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|DescribeBackupVault|4xx->4xx|$.Message
Issue-Key: backup|DescribeBackupVault|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|DescribeBackupVault|4xx->4xx|status
Issue-Key: backup|ListRecoveryPointsByBackupVault|4xx->4xx|$.Message
Issue-Key: backup|ListRecoveryPointsByBackupVault|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|ListRecoveryPointsByBackupVault|4xx->4xx|status
Issue-Key: backup|DescribeBackupVault|4xx->4xx|$
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Vault reads outside the caller's visibility answer 400 ResourceNotFoundException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBackupVault

## Description
When a vault-scoped read names a vault the caller cannot see — one that has been deleted, or one addressed with a BackupVaultAccountId the caller does not own — AWS Backup refuses the request with HTTP 403 AccessDeniedException rather than disclosing that the vault does not exist. The emulator instead resolves the vault in its own store, misses it, and answers HTTP 400 ResourceNotFoundException. This issue bundles THREE causes of that one authorization-before-existence fault; all three must be closed.

Cause 1 of 3 — DescribeBackupVault on a deleted vault.
AWS: Smithy: DescribeBackupVault GET /backup-vaults/{BackupVaultName} models InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException (all httpError 400) and ServiceUnavailableException (500); AccessDeniedException is NOT in the operation's modeled error list, so this is recorded behavior only. Recorded response after the vault is deleted: HTTP 403, header x-amzn-errortype='AccessDeniedException', body Message='Insufficient privileges to perform this action.' (the message is a fixed AWS string — quoted here as an example; the required part is the 403 status and the AccessDeniedException error type).
Emulator: HTTP 400 with x-amzn-errortype='ResourceNotFoundException' and Message='Backup vault <name> not found'.
Why: a client distinguishing authorization failure from a missing resource takes the wrong branch — it retries or reports 'not found' where AWS denies access, and the status class (403 vs 400) is wrong for every generic error handler and retry policy.
Fix: the vault-resolution step shared by the vault-scoped read handlers must apply the access decision before the existence lookup and raise the AccessDenied path (403 + AccessDeniedException errortype) for a vault not visible to the caller.
Traces: backup-backup-vault-lifecycle#teardown#6 (also backup-backup-vault-protected-resources-lifecycle#teardown#5, backup-cross-vault-recovery-point-account-scoped-listing#teardown#6, backup-operate-on-nonexistent-vault#task#1, backup-protected-resources-listing#teardown#7, backup-vault-lifecycle-and-listing#teardown#6).

Cause 2 of 3 — ListRecoveryPointsByBackupVault on a vault scoped to another account.
AWS: Smithy: ListRecoveryPointsByBackupVault GET /backup-vaults/{BackupVaultName}/recovery-points with BackupVaultAccountId as httpQuery 'backupVaultAccountId'; modeled errors are the same 400 set plus 500, so again recorded behavior only. Recorded response: HTTP 403 with x-amzn-errortype='AccessDeniedException' and Message='Insufficient privileges to perform this action.' (example text).
Emulator: HTTP 400 ResourceNotFoundException naming the vault.
Why: cross-account recovery-point listing is a documented access path; a client is told the vault is absent instead of that it lacks privileges, and leaks the name of a vault it may not address.
Fix: the same pre-existence access decision in the ListRecoveryPointsByBackupVault handler, including the BackupVaultAccountId scoping.
Traces: backup-cross-vault-recovery-point-account-scoped-listing#task#3 (also backup-vault-not-found-errors#task#1).

Cause 3 of 3 — DescribeBackupVault polled after deleting a logically air-gapped vault.
AWS: recorded behavior: once the vault is gone the poll settles on HTTP 403 with x-amzn-errortype AccessDeniedException and the same 'Insufficient privileges to perform this action.' message example.
Emulator: every poll attempt answers HTTP 400 ResourceNotFoundException, so the fixture's until-status 403 condition never becomes true and the step burns its full retry budget before failing.
Why: besides the wrong status and error type, a client that waits for the 403 terminal condition never converges and hangs until its own timeout.
Fix: as cause 1 — the shared vault-resolution/access path, which also fixes the polled form.
Traces: backup-logically-air-gapped-vault-lifecycle#task#6.

## Evidence
Cause 1 (DescribeBackupVault, backup-backup-vault-lifecycle#teardown#6): status expected 403, got 400; $headers.x-amzn-errortype expected 'AccessDeniedException', got 'ResourceNotFoundException'; $.Message expected 'Insufficient privileges to perform this action.' (example), got 'Backup vault <name> not found'. Cluster covers 6 fixture steps.
Cause 2 (ListRecoveryPointsByBackupVault, backup-cross-vault-recovery-point-account-scoped-listing#task#3): status expected 403, got 400; errortype expected 'AccessDeniedException', got 'ResourceNotFoundException'. Cluster covers 2 fixture steps.
Cause 3 (DescribeBackupVault, backup-logically-air-gapped-vault-lifecycle#task#6): poll 'status 400 not in [403] after 13 attempt(s)', capped at 60s; body {'__type': 'ResourceNotFoundException', ...} where the recording has {'Message': 'Insufficient privileges to perform this action.'}.
Smithy citation: DescribeBackupVault and ListRecoveryPointsByBackupVault model only InvalidParameterValueException / MissingParameterValueException / ResourceNotFoundException (400) and ServiceUnavailableException (500); the 403 AccessDeniedException behavior is recorded, not modeled.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (trace set simparity-wire-aws-v1).
2. Cause 1: fixture backup-backup-vault-lifecycle, step teardown#6 — DELETE /backup-vaults/<name>, then GET /backup-vaults/<name>; expect 403 AccessDeniedException, observe 400 ResourceNotFoundException.
3. Cause 2: fixture backup-cross-vault-recovery-point-account-scoped-listing, step task#3 — GET /backup-vaults/<name>/recovery-points?backupVaultAccountId=<other-account>; expect 403 AccessDeniedException.
4. Cause 3: fixture backup-logically-air-gapped-vault-lifecycle, step task#6 — poll GET /backup-vaults/<name> until 403; observe every attempt return 400.

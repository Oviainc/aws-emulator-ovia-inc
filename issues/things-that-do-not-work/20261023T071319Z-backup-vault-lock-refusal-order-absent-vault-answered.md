Issue-Source: trace-sweep-divergence
Issue-Trace: backup-operate-on-nonexistent-vault
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-operate-on-nonexistent-vault
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|PutBackupVaultLockConfiguration|4xx->4xx|$.Message
Issue-Key: backup|PutBackupVaultLockConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|PutBackupVaultLockConfiguration|4xx->4xx|status
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# Backup vault-lock refusal order: absent vault answered 400 ResourceNotFoundException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBackupVaultLockConfiguration

## Description
Cause 1 of 1: PutBackupVaultLockConfiguration against a backup vault that does not exist is refused with the wrong status and the wrong error code, because the emulator resolves the vault before it evaluates the caller's privilege for the vault-lock action.

AWS: recorded behavior only. The Smithy model for PutBackupVaultLockConfiguration (input BackupVaultName is an @httpLabel, pattern ^[a-zA-Z0-9\-\_]{2,50}$; modeled errors InvalidParameterValueException, InvalidRequestException, MissingParameterValueException, ResourceNotFoundException (all 400) and ServiceUnavailableException (500)) does not model AccessDeniedException at all, so the model says nothing about this path. The recording shows AWS answering HTTP 403 with header x-amzn-errortype: AccessDeniedException for a vault name that was never created; the recorded message is an example only ("Insufficient privileges to perform this action."). Hypothesis: vault-lock is an account/privilege-gated action, so AWS runs the authorization decision before any vault lookup and therefore never discloses whether the named vault exists.

Emulator: answers HTTP 400 with x-amzn-errortype: ResourceNotFoundException, i.e. it performs the vault lookup first and reports the lookup failure (recorded message example: "Backup vault <name> not found").

Why: a client sees the opposite refusal class. 403/AccessDeniedException is a terminal, non-retryable authorization failure that SDK retry policies and error handlers treat as a permissions problem, while 400/ResourceNotFoundException is read as "create the vault and retry". Code that branches on ResourceNotFoundException to provision a vault will loop or provision resources it has no privilege to lock, and the emulator also leaks vault existence on an unauthorized call where AWS deliberately does not.

Fix: the PutBackupVaultLockConfiguration handler in the backup service crate — move the privilege/authorization gate for the vault-lock action ahead of the vault-state lookup, and emit 403 with errortype AccessDeniedException on that path instead of falling through to the 400 ResourceNotFoundException the state lookup produces.

Traces: backup-operate-on-nonexistent-vault#task#4 (expected 403 AccessDeniedException, got 400 ResourceNotFoundException).

## Evidence
Fixture backup-operate-on-nonexistent-vault, step task#4, operation PutBackupVaultLockConfiguration.
Expected (recorded AWS): status 403; $headers.x-amzn-errortype = AccessDeniedException; $.Message example "Insufficient privileges to perform this action."
Actual (emulator): status 400; $headers.x-amzn-errortype = ResourceNotFoundException; $.Message "Backup vault sim2real-probe-backup-never-created-vault-8d4e8634 not found".
Replay divergence_key: service=backup|PutBackupVaultLockConfiguration|status=403->400|keys=$.Message,$headers.x-amzn-errortype,status (verdict mismatch, 1 affected step).
Smithy: PutBackupVaultLockConfiguration -> PUT /backup-vaults/{BackupVaultName}/vault-lock; modeled errors are 400-class plus ServiceUnavailableException 500; AccessDeniedException is not modeled, so the 403 is recorded behavior.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture backup-operate-on-nonexistent-vault and let it reach step task#4, which calls PutBackupVaultLockConfiguration on a vault name that was never created (PUT /backup-vaults/<never-created-vault>/vault-lock).
3. Observe the emulator answers 400 with x-amzn-errortype ResourceNotFoundException; the recording expects 403 with x-amzn-errortype AccessDeniedException.

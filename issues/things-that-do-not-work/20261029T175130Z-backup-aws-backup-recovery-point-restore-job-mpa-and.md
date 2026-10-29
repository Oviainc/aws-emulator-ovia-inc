Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.vault-lock-min-retention-enforced backup.backup-vault-lock-compliance-mode-server-enforced backup.deploy-backup-vault-cfn-lock-enforced backup.delete-recovery-point-wrong-state-rejected backup.e2e-legal-hold-blocks-deletion-then-release backup.mpa-approval-team-association-enforced backup.e2e-restore-job-validation-flow backup.put-backup-vault-access-policy-malformed-policy-rejected backup.concurrent-create-tiering-configuration-duplicate
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup.vault-lock-min-retention-enforced|run#27|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: backup.backup-vault-lock-compliance-mode-server-enforced|run#14|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: backup.deploy-backup-vault-cfn-lock-enforced|run#21|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: backup.delete-recovery-point-wrong-state-rejected|run#21|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: backup.e2e-legal-hold-blocks-deletion-then-release|run#12|AssertionError|assert describe_code == "ResourceNotFoundException", f"unexpected code: {describe_code}"
Issue-Key: backup.mpa-approval-team-association-enforced|run#4|AssertionError|assert code == "InvalidParameterValueException", (
Issue-Key: backup.e2e-restore-job-validation-flow|run#13|AssertionError|assert start_restore_code == "InvalidParameterValueException", f"unexpected code: {start_restore_code}"
Issue-Key: backup.put-backup-vault-access-policy-malformed-policy-rejected|run#6|AssertionError|assert code == "InvalidParameterValueException", f"expected InvalidParameterValueException, got {code!r}"
Issue-Key: backup.concurrent-create-tiering-configuration-duplicate|run#11|AssertionError|assert len(successes) == 1, f"expected exactly one winning create, got {len(successes)}"
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# AWS Backup recovery-point, restore-job, MPA and vault-policy refusals are unreachable: six operations answer 501 NotImplemented, so Vault Lock enforcement and every not-found boundary is unobservable (6 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRecoveryPoint

## Description
Six distinct root causes behind nine net-new failing checks in the `backup` group. Each check reached a 501 `NotImplemented` scaffold stub on a call it expected to be refused with a specific modeled error, so the suite reported it as a MISMATCH (wrong error code) rather than marking it ALREADY FILED. Two of the operations below (DescribeRecoveryPoint, DisassociateBackupVaultMpaApprovalTeam) are named by NO filed issue at all; for the other four the replay report auto-filed a bare "operation X is a stub" issue from a DIFFERENT check (DeleteRecoveryPoint via backup.e2e-plan-selection-backup-job-to-recovery-point, StartRestoreJob via backup__events.backup-restore-job-event-drives-sqs-queue-message, PutBackupVaultAccessPolicy via backup.backup-vault-access-policy-json-round-trip, CreateTieringConfiguration via backup.tiering-configuration-lifecycle) — those issues record only that the handler is a stub; this issue records the BEHAVIOR these nine checks require of it once written, which is what makes the checks pass. All six live in services/service-com-amazonaws-backup-cryocontrollerusermanager and are bundled as one work item for this service; each must be closed separately.

=== Cause 1 of 6: DeleteRecoveryPoint is a 501 stub, so Vault Lock retention enforcement and the recovery-point not-found boundary cannot be observed at all ===
AWS: Smithy: DeleteRecoveryPoint, DELETE /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}, output Unit (HTTP 200, empty body), modeled errors InvalidParameterValueException, InvalidRequestException, InvalidResourceStateException, MissingParameterValueException, ResourceNotFoundException (all 400) and ServiceUnavailableException (500). Recorded behavior: (a) in a vault with an active Vault Lock, deleting a recovery point still inside the lock's retention window is refused InvalidRequestException with the fixed sentence "RecoveryPoint cannot be deleted or updated (Backup vault configured with Lock)" and the point survives (DescribeRecoveryPoint still reports it, Status COMPLETED); this holds for a lock applied through PutBackupVaultLockConfiguration AND for one applied through a CloudFormation AWS::Backup::BackupVault LockConfiguration; (b) an ARN that is not an indexed recovery point — a fabricated one, one whose backup job has not yet reached a terminal state, or one already deleted — is refused ResourceNotFoundException with the fixed sentence "Cannot find recovery point"; (c) in an unlocked vault the call succeeds with 200 and the point becomes not-found.
Emulator: every call answers HTTP 501 with error code `NotImplemented` and the message "operation DeleteRecoveryPoint is scaffolded but not implemented in this emulator" — services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/delete_recovery_point.rs still returns `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteRecoveryPoint"))`. 501/NotImplemented is not in the modeled error set, so an SDK surfaces an unmodeled failure.
Why: Vault Lock's entire point is that the SERVICE, not IAM, refuses premature deletion. A client cannot verify its compliance posture, cannot clean up recovery points, and cannot distinguish "protected by lock" from "does not exist" — all three outcomes collapse into one unmodeled 501.
Fix: implement the handler in .../src/ops/delete_recovery_point.rs: resolve the vault and the recovery point, refuse ResourceNotFoundException ("Cannot find recovery point") when the point is not indexed, refuse InvalidRequestException with the lock sentence when the vault's stored lock (crate::state::VaultLock) still protects it, otherwise remove it and answer the empty 200; delete the SCAFFOLDED marker.
Traces: backup.vault-lock-min-retention-enforced, backup.backup-vault-lock-compliance-mode-server-enforced, backup.deploy-backup-vault-cfn-lock-enforced, backup.delete-recovery-point-wrong-state-rejected.

=== Cause 2 of 6: DescribeRecoveryPoint is a 501 stub, so the recovery-point not-found boundary cannot be observed ===
AWS: Smithy: DescribeRecoveryPoint input BackupVaultName (required, pattern ^[a-zA-Z0-9\\-\\_]{2,50}$) and RecoveryPointArn (ARN, required); output carries RecoveryPointArn, Status (CREATING|AVAILABLE|COMPLETED|PARTIAL|EXPIRED|DELETING|STOPPED), BackupSizeInBytes, CreationDate, Lifecycle, VaultType and more; modeled errors InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException (400) and ServiceUnavailableException (500). Recorded behavior: an ARN naming no indexed recovery point in the named vault is refused ResourceNotFoundException with the fixed sentence "Cannot find recovery point"; an existing one answers 200 with RecoveryPointArn echoed and Status=COMPLETED for a finished job.
Emulator: HTTP 501, error code `NotImplemented`, message "operation DescribeRecoveryPoint is scaffolded but not implemented in this emulator" — .../src/ops/describe_recovery_point.rs carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeRecoveryPoint"))`. No filed issue names this operation.
Why: a client cannot read back ANY recovery point — not its status, not its size, not its lifecycle — and cannot test the "not yet / no longer there" boundary that every backup-then-verify workflow depends on; the refusal it does get is unmodeled.
Fix: implement .../src/ops/describe_recovery_point.rs against the stored recovery points in crate::state (ResourceNotFoundException "Cannot find recovery point" for an unknown ARN) and delete the SCAFFOLDED marker.
Traces: backup.e2e-legal-hold-blocks-deletion-then-release.

=== Cause 3 of 6: DisassociateBackupVaultMpaApprovalTeam is a 501 stub, so the unsupported-vault-type refusal cannot be observed ===
AWS: Smithy: DisassociateBackupVaultMpaApprovalTeam, POST /backup-vaults/{BackupVaultName}/mpaApprovalTeam?delete, output Unit (modeled HTTP 204), modeled errors InvalidParameterValueException, InvalidRequestException, MissingParameterValueException, ResourceNotFoundException (400) and ServiceUnavailableException (500). Recorded behavior: called on a plain (non-logically-air-gapped) vault, which does not support Multi-party Approval at all, it is refused InvalidParameterValueException with the fixed sentence "Operation not supported for the provided vault type", and the vault's MPA state is left unset (DescribeBackupVault reports no MpaApprovalTeamArn).
Emulator: HTTP 501, error code `NotImplemented`, message "operation DisassociateBackupVaultMpaApprovalTeam is scaffolded but not implemented in this emulator" — .../src/ops/disassociate_backup_vault_mpa_approval_team.rs carries the SCAFFOLDED marker. No filed issue names this operation.
Why: the vault-type precondition is invisible: a client cannot tell that MPA is unsupported for the vault it named, and gets an unmodeled 501 instead of the modeled parameter refusal.
Fix: implement .../src/ops/disassociate_backup_vault_mpa_approval_team.rs — resolve the vault, refuse InvalidParameterValueException with the fixed sentence for a vault type that does not support MPA, clear the association otherwise, answer the modeled 204; delete the SCAFFOLDED marker.
Traces: backup.mpa-approval-team-association-enforced.

=== Cause 4 of 6: StartRestoreJob is a 501 stub, so the restore-job validation boundary cannot be observed ===
AWS: Smithy: StartRestoreJob input RecoveryPointArn (ARN, required) and Metadata (required, sensitive map), modeled errors InvalidParameterValueException, InvalidRequestException, MissingParameterValueException, ResourceNotFoundException (400), ServiceUnavailableException (500); output RestoreJobId. Recorded behavior: a RecoveryPointArn naming no recovery point is refused InvalidParameterValueException — NOT ResourceNotFoundException — with the fixed sentence "Cannot find recovery point" (the sibling restore reads DescribeRestoreJob, PutRestoreValidationResult and GetRestoreJobMetadata instead answer ResourceNotFoundException "Cannot find restore job" for an unknown RestoreJobId).
Emulator: HTTP 501, error code `NotImplemented` ("operation StartRestoreJob is scaffolded but not implemented in this emulator") from .../src/ops/start_restore_job.rs, so the check never even reaches the sibling reads. The auto-filed stub issue for this operation records only that the handler is unwritten; the class/message split above is the behavior required of it.
Why: a client cannot start a restore at all, and cannot distinguish a bad recovery-point reference (parameter class) from a bad restore-job reference (not-found class) — AWS deliberately uses two different classes here.
Fix: implement .../src/ops/start_restore_job.rs with that refusal classification, and keep the restore-job reads' ResourceNotFoundException "Cannot find restore job" consistent with it.
Traces: backup.e2e-restore-job-validation-flow.

=== Cause 5 of 6: PutBackupVaultAccessPolicy is a 501 stub, so a malformed policy document is not refused ===
AWS: Smithy: PutBackupVaultAccessPolicy, PUT /backup-vaults/{BackupVaultName}/access-policy, input Policy (IAMPolicy), output Unit (200), modeled errors InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException (400) and ServiceUnavailableException (500). Recorded behavior: a Policy that is not a well-formed IAM policy document is refused InvalidParameterValueException and no policy is attached (GetBackupVaultAccessPolicy keeps answering as before), while a well-formed document is accepted with 200 and round-trips.
Emulator: HTTP 501, error code `NotImplemented` from .../src/ops/put_backup_vault_access_policy.rs, so neither the acceptance nor the refusal exists.
Why: a resource-policy guardrail is untestable, and a client gets an unmodeled 501 where AWS gives a modeled, actionable parameter error.
Fix: implement .../src/ops/put_backup_vault_access_policy.rs: validate the policy document and refuse InvalidParameterValueException for a malformed one, store it on the vault otherwise.
Traces: backup.put-backup-vault-access-policy-malformed-policy-rejected.

=== Cause 6 of 6: CreateTieringConfiguration is a 501 stub, so the duplicate-name race has no winner ===
AWS: Smithy: CreateTieringConfiguration, PUT /tiering-configurations, output TieringConfigurationArn/TieringConfigurationName/CreationTime, modeled errors AlreadyExistsException (400), ConflictException (409), InvalidParameterValueException, LimitExceededException, MissingParameterValueException (400) and ServiceUnavailableException (500). Recorded behavior: with N concurrent creates of the same TieringConfigurationName, exactly ONE succeeds and every loser is refused ConflictException or AlreadyExistsException; GetTieringConfiguration then reports the single survivor, and after DeleteTieringConfiguration it answers ResourceNotFoundException "Cannot find tiering configuration".
Emulator: all six concurrent creates were refused with HTTP 501 `NotImplemented` from .../src/ops/create_tiering_configuration.rs, so the replay observed ZERO winners (the modeled @range validation does run first, which is why the sibling check in the other backup issue gets a 400 instead).
Why: no tiering configuration can ever exist, so uniqueness, conflict classification and the delete-then-gone boundary are all unobservable, and the refusal is unmodeled.
Fix: implement .../src/ops/create_tiering_configuration.rs with a single-winner uniqueness check on TieringConfigurationName answering ConflictException/AlreadyExistsException to the losers, plus the Get/List/Delete round trip over the stored configurations.
Traces: backup.concurrent-create-tiering-configuration-duplicate.

## Evidence
Suite: backup 1/46 (2%), 45 failed. Every failure below is a 501 `NotImplemented` answered where a modeled 400 was required; the suite classified them as mismatches, so none carries an ALREADY FILED or OWNED marker.

Cause 1 — [mismatch] backup.vault-lock-min-retention-enforced: AssertionError: expected InvalidRequestException, got NotImplemented (DeleteRecoveryPoint on a point in a vault locked with MinRetentionDays=1/ChangeableForDays=3). Expected InvalidRequestException 'RecoveryPoint cannot be deleted or updated (Backup vault configured with Lock)'.
  [mismatch] backup.backup-vault-lock-compliance-mode-server-enforced: AssertionError: expected InvalidRequestException, got NotImplemented (same call, compliance-mode lock).
  [mismatch] backup.deploy-backup-vault-cfn-lock-enforced: AssertionError: expected the CFN-templated MinRetentionDays lock to reject the delete with InvalidRequestException, got 'NotImplemented' (lock applied via AWS::Backup::BackupVault LockConfiguration; the stack reached CREATE_COMPLETE and DescribeBackupVault correctly reported Locked=true, MinRetentionDays=1).
  [mismatch] backup.delete-recovery-point-wrong-state-rejected: AssertionError: expected ResourceNotFoundException (job_state='CREATED'), got NotImplemented. Expected ResourceNotFoundException 'Cannot find recovery point' for a reserved-but-not-yet-indexed EFS recovery point.

Cause 2 — [mismatch] backup.e2e-legal-hold-blocks-deletion-then-release: AssertionError: unexpected code: NotImplemented, raised on DescribeRecoveryPoint(BackupVaultName=v, RecoveryPointArn=<fabricated arn>). Expected ResourceNotFoundException 'Cannot find recovery point'.

Cause 3 — [mismatch] backup.mpa-approval-team-association-enforced: AssertionError: expected disassociating a non-associated MPA team on a vault type that does not support MPA to be rejected with InvalidParameterValueException, got NotImplemented. Expected InvalidParameterValueException 'Operation not supported for the provided vault type'.

Cause 4 — [mismatch] backup.e2e-restore-job-validation-flow: AssertionError: unexpected code: NotImplemented, raised on StartRestoreJob(RecoveryPointArn=<fabricated arn>, ResourceType='DynamoDB', Metadata={targetTableName:...}). Expected InvalidParameterValueException 'Cannot find recovery point'.

Cause 5 — [mismatch] backup.put-backup-vault-access-policy-malformed-policy-rejected: AssertionError: expected InvalidParameterValueException, got 'NotImplemented'.

Cause 6 — [mismatch] backup.concurrent-create-tiering-configuration-duplicate: AssertionError: expected exactly one winning create, got 0 (6 threads released from a barrier onto CreateTieringConfiguration with one shared name; all six got 501 NotImplemented). Expected exactly one 200 and five ConflictException/AlreadyExistsException.

## Steps to Reproduce
Cause 1:
1. backup.CreateBackupVault(v); backup.PutBackupVaultLockConfiguration(BackupVaultName=v, MinRetentionDays=1, MaxRetentionDays=36500, ChangeableForDays=3); DescribeBackupVault reports Locked=true.
2. dynamodb.CreateTable -> ACTIVE; backup.StartBackupJob(BackupVaultName=v, ResourceArn=<table arn>, IamRoleArn=<role>, Lifecycle={DeleteAfterDays:7}); poll DescribeBackupJob to COMPLETED.
3. backup.DeleteRecoveryPoint(BackupVaultName=v, RecoveryPointArn=<job's rp>) -> expect InvalidRequestException 'RecoveryPoint cannot be deleted or updated (Backup vault configured with Lock)'; emulator answers HTTP 501 NotImplemented.
4. Repeat with a fabricated RecoveryPointArn -> expect ResourceNotFoundException 'Cannot find recovery point'. Repeat with the lock applied by a CloudFormation AWS::Backup::BackupVault LockConfiguration for the same result.

Cause 2:
1. backup.CreateBackupVault(v).
2. backup.DescribeRecoveryPoint(BackupVaultName=v, RecoveryPointArn='arn:aws:backup:<region>:<account>:recovery-point:does-not-exist') -> expect ResourceNotFoundException 'Cannot find recovery point'; emulator answers HTTP 501 NotImplemented.

Cause 3:
1. backup.CreateBackupVault(v) (a plain vault).
2. backup.DisassociateBackupVaultMpaApprovalTeam(BackupVaultName=v) -> expect InvalidParameterValueException 'Operation not supported for the provided vault type'; emulator answers HTTP 501 NotImplemented.

Cause 4:
1. backup.StartRestoreJob(RecoveryPointArn='arn:aws:backup:<region>:<account>:recovery-point:<uuid>', Metadata={'targetTableName': t}, IamRoleArn=role, ResourceType='DynamoDB', IdempotencyToken=tok) -> expect InvalidParameterValueException 'Cannot find recovery point'; emulator answers HTTP 501 NotImplemented.
2. Then backup.DescribeRestoreJob / PutRestoreValidationResult / GetRestoreJobMetadata with an unknown RestoreJobId -> expect ResourceNotFoundException 'Cannot find restore job'.

Cause 5:
1. backup.CreateBackupVault(v).
2. backup.PutBackupVaultAccessPolicy(BackupVaultName=v, Policy='<not a valid IAM policy document>') -> expect InvalidParameterValueException and no policy attached; emulator answers HTTP 501 NotImplemented.

Cause 6:
1. Release 6 threads simultaneously on backup.CreateTieringConfiguration(TieringConfiguration={TieringConfigurationName=n, BackupVaultName='*', ResourceSelection=[{Resources:['*'], ResourceType:'S3', TieringDownSettingsInDays:90}]}).
2. Expect exactly one 200 and five ConflictException/AlreadyExistsException, then GetTieringConfiguration(n) returning the survivor; emulator answers HTTP 501 NotImplemented to all six.

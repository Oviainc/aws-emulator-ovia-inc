Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.vault-lock-governance-mode-server-enforced backup.delete-vault-lock-config-during-active-lock-rejected backup.delete-vault-lock-in-compliance-mode-blocked backup.tiering-configuration-min-age-ttl-enforced backup.start-backup-job-invalid-resource-arn-rejected backup.start-backup-job-insufficient-role-permission-denied
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup.vault-lock-governance-mode-server-enforced|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: backup.delete-vault-lock-config-during-active-lock-rejected|run#11|AssertionError|assert message == "Backup vault Lock configuration is immutable and cannot be deleted", f"unexpected message {message!r}"
Issue-Key: backup.delete-vault-lock-in-compliance-mode-blocked|run#5|AssertionError|assert message == (
Issue-Key: backup.tiering-configuration-min-age-ttl-enforced|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: backup.start-backup-job-invalid-resource-arn-rejected|run#8|AssertionError|assert code == "AccessDeniedException", f"unexpected StartBackupJob error code: {code}"
Issue-Key: backup.start-backup-job-insufficient-role-permission-denied|run#7|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# AWS Backup vault-lock and tiering refusals use emulator-invented messages, and StartBackupJob performs no authorization at all (5 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBackupVaultLockConfiguration

## Description
Five distinct root causes behind six net-new failing checks in the `backup` group. All five live in the AWS Backup crate (services/service-com-amazonaws-backup-cryocontrollerusermanager), so they are bundled as one work item for this service; each must be closed separately. None of these checks is marked ALREADY FILED or OWNED in the replay report, and no open issue names any of these operations (the nearest open backup issues — 20261024T031359Z-backup-events-aws-backup-job-lifecycle-and-eventing-diverge-jobs.md, 20261027T205645Z-backup-a-backup-vault-s-default-encryptionkeyarn-names-a.md — are about job lifecycle/eventing and about the vault's default EncryptionKeyArn, not about lock refusals or StartBackupJob authorization).

=== Cause 1 of 5: the immutable-Vault-Lock refusal message is an emulator sentence, and the SAME sentence is used for the update path and the delete path ===
AWS: Smithy: PutBackupVaultLockConfiguration and DeleteBackupVaultLockConfiguration both model InvalidRequestException (HTTP 400, "Indicates that something is wrong with the input to the request"); the ChangeableForDays doc states "On and after the lock date, the Vault Lock becomes immutable and cannot be changed or deleted". Recorded behavior: against a vault whose compliance-mode lock is already immutable (a logically air-gapped vault is immutable from creation), the two paths answer DIFFERENT, path-specific messages, both fixed text with no variable parts: PutBackupVaultLockConfiguration -> InvalidRequestException "Backup vault Lock configuration is immutable and cannot be updated"; DeleteBackupVaultLockConfiguration -> InvalidRequestException "Backup vault Lock configuration is immutable and cannot be deleted". The stored retention bounds are left untouched either way.
Emulator: both handlers answer InvalidRequestException with one shared, invented sentence that also interpolates the vault name: "Backup vault {BackupVaultName} has an immutable Vault Lock configuration" — services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/put_backup_vault_lock_configuration.rs (the `is_immutable_at(now)` branch) and .../delete_backup_vault_lock_configuration.rs (same branch). The status and the error code are right; the message is wrong and is not path-specific. In AWS's message the whole sentence is fixed text — no request or state value appears in it, so interpolating the vault name is itself the divergence.
Why: a client that distinguishes "lock cannot be updated" from "lock cannot be deleted" (compliance-reporting tools quote these messages verbatim to an operator, and some retry logic greps them) sees one indistinguishable sentence, and any assertion on AWS's wording fails even though the refusal itself is correct.
Fix: the refusal messages in the immutable-lock branch of each handler — put_backup_vault_lock_configuration.rs and delete_backup_vault_lock_configuration.rs — which must be two distinct fixed sentences with no interpolated vault name.
Traces: backup.vault-lock-governance-mode-server-enforced (update path), backup.delete-vault-lock-config-during-active-lock-rejected (delete path).

=== Cause 2 of 5: the below-floor ChangeableForDays refusal message is an emulator sentence ===
AWS: Smithy: PutBackupVaultLockConfiguration input ChangeableForDays targets Long with NO @range, and its doc states "Backup enforces a 72-hour cooling-off period before Vault Lock takes effect and becomes immutable. Therefore, you must set ChangeableForDays to 3 or greater. The maximum value you can specify is 36,500 days"; the modeled refusal is InvalidParameterValueException (HTTP 400). Recorded behavior: ChangeableForDays=1 is refused with InvalidParameterValueException and the message "Backup vault Lock configuration changeable days must be between 3 and 36,500." — entirely fixed text (including the thousands separator and the trailing period); the offending value is NOT echoed. No lock is applied, and ChangeableForDays=3 (exactly at the floor) is accepted.
Emulator: the same call is refused InvalidParameterValueException with "ChangeableForDays must be between 3 and 36500 days, got 1" — the generic `out_of_range` closure in put_backup_vault_lock_configuration.rs formats "{member} must be between {min} and {max} days, got {value}" for ChangeableForDays, MinRetentionDays and MaxRetentionDays alike. Status and code match; the sentence, its punctuation and its echoing of the request value do not.
Why: a client or compliance test pinned to AWS's documented wording treats a correctly-refused call as an unknown failure, and the message shape differs structurally (AWS names no value from the request here, the emulator does).
Fix: the ChangeableForDays branch of the parameter validation in put_backup_vault_lock_configuration.rs — it needs its own message rather than the shared out-of-range template.
Traces: backup.delete-vault-lock-in-compliance-mode-blocked.

=== Cause 3 of 5: the below-floor TieringDownSettingsInDays refusal is rendered as a framework constraint-violation sentence instead of the service's own ===
AWS: Smithy: CreateTieringConfiguration models InvalidParameterValueException (400), and TieringDownSettingsInDays inside TieringConfiguration.ResourceSelection carries the documented 60..36500 floor. Recorded behavior: a value below the floor is refused InvalidParameterValueException with the service's own sentence "Invalid TieringDownSettingsInDays. Days must be between 60 and 36500" — fixed text naming the member and the two limits, with no request path and no echoed value — and nothing is persisted (GetTieringConfiguration then answers ResourceNotFoundException "Cannot find tiering configuration").
Emulator: the call is refused with the right code but with the generic constraint-violation rendering "Value '30' at 'tieringConfiguration.resourceSelection.1.member.tieringDownSettingsInDays' failed to satisfy constraint: Member must have value greater than or equal to 60" — i.e. the modeled @range is refused by the shared pre-handler validation walk and never reaches a service-worded refusal.
Why: the message quotes an internal member path and the submitted value, neither of which AWS exposes for this parameter; a client surfacing the message to a user, or matching on it, gets text no AWS account ever returns.
Fix: this operation's validation in the AWS Backup crate — the TieringDownSettingsInDays range must be refused by the CreateTieringConfiguration handler with the service's own InvalidParameterValueException sentence rather than by the generic modeled-constraint walk.
Traces: backup.tiering-configuration-min-age-ttl-enforced.

=== Cause 4 of 5: StartBackupJob with a syntactically invalid ResourceArn is classified as a parameter error, where AWS refuses it as an authorization failure ===
AWS: Smithy: StartBackupJob input ResourceArn targets ARN with NO pattern, and the operation models InvalidParameterValueException/InvalidRequestException/ResourceNotFoundException/LimitExceededException/MissingParameterValueException/ServiceUnavailableException — AccessDeniedException is NOT in that set, it is the service-common authorization refusal. Recorded behavior: StartBackupJob authorizes against the (malformed) resource ARN BEFORE parameter validation runs, so a ResourceArn such as "not-a-real-arn-at-all" is refused AccessDeniedException with the fixed message "Insufficient privileges to perform this action." and no backup job is created (ListBackupJobs for the vault stays empty).
Emulator: the call is refused InvalidParameterValueException by the ARN syntax check in services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/start_backup_job.rs (the `crate::state::rejected_arn("ResourceArn", ..)` loop), which runs before any authorization decision.
Why: a client that branches on the refusal class — retry/ask-for-permissions on AccessDenied vs. fix-the-request on a parameter error — takes the wrong branch, and an SDK's error-type matching on AccessDeniedException never fires.
Fix: the ordering and classification in the StartBackupJob handler: the authorization decision on the named resource must precede ARN-syntax parameter validation, and the refusal must be the AccessDeniedException class with AWS's fixed sentence.
Traces: backup.start-backup-job-invalid-resource-arn-rejected.

=== Cause 5 of 5: StartBackupJob performs no caller authorization — an identity with an explicit deny on backup:StartBackupJob succeeds ===
AWS: recorded behavior (authorization is not expressed in the Smithy model; AccessDeniedException is the service-common refusal, HTTP 400 for this service's REST-JSON error channel). A caller whose identity-based policy explicitly denies backup:StartBackupJob is refused AccessDeniedException with the message "User: <caller user ARN> is not authorized to perform: backup:StartBackupJob on resource: <the backup vault ARN> with an explicit deny in an identity-based policy" — the fixed text is "User: ", " is not authorized to perform: ", " on resource: " and " with an explicit deny in an identity-based policy"; the variable parts are the caller's own IAM user ARN and the vault ARN built from the request's BackupVaultName plus the request's region and account. No job is created (ListBackupJobs ByResourceArn stays empty), while the identical call from a permissioned caller is accepted and the job progresses.
Emulator: the call SUCCEEDS for the denied identity — the replay recorded no error at all (expected AccessDeniedException, got None) and a BackupJobId was handed back. start_backup_job.rs validates ARN syntax, windows, vault existence and vault state, and consults no caller identity or policy anywhere.
Why: this is an overly-permissive divergence: a client (or a security test) verifying that a deny boundary holds observes a successful backup and a created recovery point where AWS creates nothing, so least-privilege regressions pass silently against the emulator.
Fix: caller authorization for this operation in the AWS Backup crate — the StartBackupJob handler must evaluate the caller's identity policy against backup:StartBackupJob on the vault ARN and refuse with the AccessDeniedException class and AWS's sentence before creating any job state.
Traces: backup.start-backup-job-insufficient-role-permission-denied.

## Evidence
Suite: backup 1/46 (2%), 45 failed; these six checks are the net-new mismatches in this bundle.

Cause 1 — [mismatch] backup.vault-lock-governance-mode-server-enforced:
  AssertionError: expected message 'Backup vault Lock configuration is immutable and cannot be updated', got 'Backup vault sim2real-lag-c9355801cd7a has an immutable Vault Lock configuration'
  (CreateLogicallyAirGappedBackupVault(MinRetentionDays=10, MaxRetentionDays=100) -> wait AVAILABLE -> PutBackupVaultLockConfiguration(MinRetentionDays=1, MaxRetentionDays=36500)). Code InvalidRequestException matched; only the message diverged. Expected: the fixed sentence with no vault name. Actual: a sentence interpolating the vault name.
Cause 1 — [mismatch] backup.delete-vault-lock-config-during-active-lock-rejected:
  AssertionError: unexpected message 'Backup vault sim2real-lag-c5c492f2cf49 has an immutable Vault Lock configuration'
  Expected: 'Backup vault Lock configuration is immutable and cannot be deleted' (note: the DELETE wording, different from cause 1's UPDATE wording). Actual: the same shared sentence as the update path.

Cause 2 — [mismatch] backup.delete-vault-lock-in-compliance-mode-blocked:
  AssertionError: actual_message='ChangeableForDays must be between 3 and 36500 days, got 1'
  Expected: InvalidParameterValueException 'Backup vault Lock configuration changeable days must be between 3 and 36,500.' for PutBackupVaultLockConfiguration(ChangeableForDays=1, MinRetentionDays=7, MaxRetentionDays=30). Code matched; message diverged.

Cause 3 — [mismatch] backup.tiering-configuration-min-age-ttl-enforced:
  AssertionError: expected message 'Invalid TieringDownSettingsInDays. Days must be between 60 and 36500', got "Value '30' at 'tieringConfiguration.resourceSelection.1.member.tieringDownSettingsInDays' failed to satisfy constraint: Member must have value greater than or equal to 60"
  (the check deliberately disables botocore parameter validation so the below-floor value reaches the wire). Code InvalidParameterValueException matched; message diverged.

Cause 4 — [mismatch] backup.start-backup-job-invalid-resource-arn-rejected:
  AssertionError: unexpected StartBackupJob error code: InvalidParameterValueException
  Expected: AccessDeniedException, message 'Insufficient privileges to perform this action.' Actual: InvalidParameterValueException from the ARN-syntax check.

Cause 5 — [mismatch] backup.start-backup-job-insufficient-role-permission-denied:
  AssertionError: expected AccessDeniedException, got None
  Expected: AccessDeniedException 'User: <denied user ARN> is not authorized to perform: backup:StartBackupJob on resource: arn:aws:backup:<region>:<account>:backup-vault:<vault> with an explicit deny in an identity-based policy', and no job created. Actual: the call succeeded and returned a BackupJobId for the explicitly-denied caller.

## Steps to Reproduce
Cause 1:
1. backup.CreateLogicallyAirGappedBackupVault(BackupVaultName=v, MinRetentionDays=10, MaxRetentionDays=100); poll DescribeBackupVault until VaultState=AVAILABLE (Locked is true from creation).
2. backup.PutBackupVaultLockConfiguration(BackupVaultName=v, MinRetentionDays=1, MaxRetentionDays=36500) -> expect InvalidRequestException 'Backup vault Lock configuration is immutable and cannot be updated'.
3. backup.DeleteBackupVaultLockConfiguration(BackupVaultName=v) -> expect InvalidRequestException 'Backup vault Lock configuration is immutable and cannot be deleted'. The emulator answers one shared sentence naming the vault for both.

Cause 2:
1. backup.CreateBackupVault(BackupVaultName=v).
2. backup.PutBackupVaultLockConfiguration(BackupVaultName=v, ChangeableForDays=1, MinRetentionDays=7, MaxRetentionDays=30) -> expect InvalidParameterValueException 'Backup vault Lock configuration changeable days must be between 3 and 36,500.'; DescribeBackupVault must still report no lock.

Cause 3:
1. Use a backup client with botocore parameter_validation disabled.
2. backup.CreateTieringConfiguration(TieringConfiguration={TieringConfigurationName=n, BackupVaultName=v, ResourceSelection=[{Resources:['*'], ResourceType:'S3', TieringDownSettingsInDays:30}]}) -> expect InvalidParameterValueException 'Invalid TieringDownSettingsInDays. Days must be between 60 and 36500'.

Cause 4:
1. backup.CreateBackupVault(BackupVaultName=v); create a backup service role.
2. backup.StartBackupJob(BackupVaultName=v, ResourceArn='not-a-real-arn-at-all', IamRoleArn=role) -> expect AccessDeniedException 'Insufficient privileges to perform this action.' and ListBackupJobs(ByBackupVaultName=v) empty.

Cause 5:
1. iam.CreateUser + access key, attach an identity policy with an explicit Deny on backup:StartBackupJob; create a vault, a DynamoDB table and a working backup service role.
2. Call backup.StartBackupJob with the denied user's credentials -> expect AccessDeniedException with the explicit-deny sentence and NO job; the emulator returns 200 with a BackupJobId.

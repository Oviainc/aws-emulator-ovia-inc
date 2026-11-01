Issue-Source: trace-sweep-divergence
Issue-Trace: backup-mpa-approval-team-association
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-mpa-approval-team-association backup-mpa-approval-team-association-errors backup-restore-access-vault-creation-error backup-restore-access-vault-source-validation
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|AssociateBackupVaultMpaApprovalTeam|4xx->4xx|$.Code
Issue-Key: backup|AssociateBackupVaultMpaApprovalTeam|4xx->4xx|$.Context
Issue-Key: backup|AssociateBackupVaultMpaApprovalTeam|4xx->4xx|$.Message
Issue-Key: backup|CreateRestoreAccessBackupVault|4xx->4xx|$.Code
Issue-Key: backup|CreateRestoreAccessBackupVault|4xx->4xx|$.Message
Issue-Key: backup|ListRestoreAccessBackupVaults|4xx->2xx|$.Code
Issue-Key: backup|ListRestoreAccessBackupVaults|4xx->2xx|$.Message
Issue-Key: backup|ListRestoreAccessBackupVaults|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|ListRestoreAccessBackupVaults|4xx->2xx|status
Issue-Commit: 32f27f5adb5ef4f1260f24418d9e3155f5d658ad

# AWS Backup restore-access-vault and MPA-association refusals omit the error Code member and answer an emulator-authored vault-type sentence (3 causes)

> _Note: this issue was identified against the code base at commit `32f27f5adb5ef4f1260f24418d9e3155f5d658ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateBackupVaultMpaApprovalTeam

## Description
Three distinct root causes in the `backup` crate's logically-air-gapped / restore-access-vault and MPA-approval-team validation path, bundled as one work item for this service; each must be closed separately. They share one fault: the crate answers every vault-type-or-parameter precondition on these operations with a single emulator-authored sentence ('Operation not supported for the provided vault type'), never populates the modeled `Code` and `Context` members of InvalidParameterValueException, and in one case does not refuse the request at all. Smithy: InvalidParameterValueException (com.amazonaws.backup#InvalidParameterValueException, client error, HTTP 400) models four members — Code, Message, Type and Context — so an error body carrying only `__type`/`Message` is structurally incomplete for this service, which is what all three causes have in common. The already-open behavioral issue 20261029T175130Z-backup-aws-backup-recovery-point-restore-job-mpa-and.md covers the *sibling* DisassociateBackupVaultMpaApprovalTeam while it is a 501 stub and does not cover any of these three operations or the Code/Context omission; 20261029T193050Z-backup-aws-backup-plan-and-selection-refusals-diverge-validation.md records the same Code-member omission for the plan/selection operations, not for these.

=== Cause 1 of 3: AssociateBackupVaultMpaApprovalTeam does not validate MpaApprovalTeamArn, and its refusal carries neither Code nor Context ===
AWS: Smithy: AssociateBackupVaultMpaApprovalTeam, PUT /backup-vaults/{BackupVaultName}/mpaApprovalTeam, input BackupVaultName (httpLabel, required, pattern ^[a-zA-Z0-9\-\_]{2,50}$), MpaApprovalTeamArn (ARN, required) and RequesterComment (sensitive); output Unit (modeled HTTP 204); modeled errors InvalidParameterValueException, InvalidRequestException, MissingParameterValueException, ResourceNotFoundException (all 400) and ServiceUnavailableException (500). Recorded behavior: when MpaApprovalTeamArn is not a well-formed MPA approval-team ARN, the call is refused HTTP 400 with x-amzn-errortype `InvalidParameterValueException` and a body whose `Code` is the fixed token `INVALID_PARAMETER_VALUE`, whose `Context` is the offending input member's name (here `MpaApprovalTeamArn`), and whose `Message` is the one-sentence template 'Parameter <MemberName> is invalid.' — fixed text 'Parameter ' and ' is invalid.', with the member name taken from the request's rejected input member. Hypothesis: the ARN's shape is validated before the vault-type precondition, which is why a plain vault yields the parameter error rather than the unsupported-vault-type error.
Emulator: answers HTTP 400 (correct status) but with `Message` = 'Operation not supported for the provided vault type' and NO `Code` and NO `Context` member at all — it applies its vault-type precondition and never inspects MpaApprovalTeamArn.
Why: a client that branches on `Code`/`Context` (the documented way to find which parameter the service rejected for this service's error shape) sees both fields absent, so it cannot tell the caller which input to fix; and because the message blames the vault type, a caller with a malformed team ARN is sent to change its vault instead of its ARN — it will keep retrying with the same bad ARN on a vault type that can never make the call succeed.
Fix: the operation's input validation and error serializer: validate MpaApprovalTeamArn's form before the vault-type precondition and emit InvalidParameterValueException with Code=INVALID_PARAMETER_VALUE, Context=<rejected member name> and the 'Parameter <MemberName> is invalid.' message; the serializer must carry Code and Context, not just Message.
Traces: backup-mpa-approval-team-association#task#2 (representative) and backup-mpa-approval-team-association-errors#task#2 — same cluster. Handler: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/associate_backup_vault_mpa_approval_team.rs.

=== Cause 2 of 3: CreateRestoreAccessBackupVault's unsupported-source-vault refusal omits Code and rewords the message ===
AWS: Smithy: CreateRestoreAccessBackupVault, PUT /restore-access-backup-vaults, input SourceBackupVaultArn (ARN, required), BackupVaultName, BackupVaultTags, CreatorRequestId (idempotencyToken), RequesterComment; output RestoreAccessBackupVaultArn/VaultState/RestoreAccessBackupVaultName/CreationDate; modeled errors AlreadyExistsException, InvalidParameterValueException, InvalidRequestException, LimitExceededException, MissingParameterValueException, ResourceNotFoundException (400) and ServiceUnavailableException (500). Recorded behavior: when SourceBackupVaultArn names a vault that is not a logically-air-gapped vault, the call is refused HTTP 400 with body `Code` = the fixed token `ERROR_4134` and `Message` = the entirely fixed sentence 'Provided backup vault does not support this operation.' (no variable parts — no vault name, arn or type is interpolated).
Emulator: answers HTTP 400 with `Message` = 'Operation not supported for the provided vault type' and no `Code` member.
Why: the status is right but neither machine-readable nor human-readable payload matches: a client keying off `Code` == 'ERROR_4134' to distinguish 'wrong source vault type' from the operation's other 400s (AlreadyExists, LimitExceeded, ResourceNotFound) gets None and cannot classify the failure, and any log/assertion matching AWS's sentence fails.
Fix: the operation's validation and error serializer — refuse the non-air-gapped source vault with InvalidParameterValueException carrying Code='ERROR_4134' and AWS's fixed sentence, instead of the crate's generic vault-type string.
Traces: backup-restore-access-vault-creation-error#task#2 (cluster representative) and backup-restore-access-vault-source-validation#task#2. Handler: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/create_restore_access_backup_vault.rs.

=== Cause 3 of 3: ListRestoreAccessBackupVaults does not refuse a non-air-gapped vault at all — it answers 200 ===
AWS: Smithy: ListRestoreAccessBackupVaults, GET /logically-air-gapped-backup-vaults/{BackupVaultName}/restore-access-backup-vaults, input BackupVaultName (httpLabel, required), NextToken, MaxResults (@range 1..1000); output RestoreAccessBackupVaults + NextToken; modeled errors InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException (400) and ServiceUnavailableException (500). Recorded behavior: the URI is scoped to logically-air-gapped vaults, so naming a vault of any other type is refused HTTP 400 with x-amzn-errortype `InvalidParameterValueException`, `Code` = the fixed token `ERROR_3936` and `Message` = the fixed two-sentence text 'The vault type is invalid. Accepted type is LOGICALLY_AIR_GAPPED_BACKUP_VAULT.' (fully fixed text; the accepted type name is the literal enum token, not a per-request value).
Emulator: answers HTTP 200 — no refusal is produced at all, so the caller receives a success body where AWS produces a modeled client error.
Why: this is the most damaging of the three: a client enumerating restore-access vaults against a vault that cannot have any gets a successful (empty) listing instead of being told the vault type is wrong. 'No restore-access vaults exist' and 'this vault type can never have restore-access vaults' become indistinguishable, so a caller silently proceeds down a path AWS would have stopped, and any error-handling branch for this precondition is dead code against the emulator.
Fix: the operation's handler/validation — resolve the named vault, and before listing, refuse a vault whose type is not LOGICALLY_AIR_GAPPED_BACKUP_VAULT with InvalidParameterValueException (HTTP 400, Code='ERROR_3936', AWS's fixed message); only an air-gapped vault proceeds to the listing.
Traces: backup-restore-access-vault-source-validation#task#3. Handler: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/list_restore_access_backup_vaults.rs.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 84 fixtures replayed, 481 steps, 177 passed, score 36.8%). The three clusters below carried neither a stub_path nor an owned_by, i.e. no filed issue holds their replay key.

Cause 1 — [mismatch] backup / AssociateBackupVaultMpaApprovalTeam, fixture backup-mpa-approval-team-association step task#2 (cluster also covers backup-mpa-approval-team-association-errors#task#2); expected status 400, actual 400:
  $.Code: missing (expected 'INVALID_PARAMETER_VALUE', got None)
  $.Context: missing (expected 'MpaApprovalTeamArn', got None)
  $.Message: value_mismatch (expected 'Parameter MpaApprovalTeamArn is invalid.', got 'Operation not supported for the provided vault type')

Cause 2 — [mismatch] backup / CreateRestoreAccessBackupVault, fixture backup-restore-access-vault-creation-error step task#2 (cluster also covers backup-restore-access-vault-source-validation#task#2); expected status 400, actual 400:
  $.Code: missing (expected 'ERROR_4134', got None)
  $.Message: value_mismatch (expected 'Provided backup vault does not support this operation.', got 'Operation not supported for the provided vault type')

Cause 3 — [mismatch] backup / ListRestoreAccessBackupVaults, fixture backup-restore-access-vault-source-validation step task#3; expected status 400, actual status 200:
  status: expected 400, got 200
  $.Code: missing (expected 'ERROR_3936', got None)
  $.Message: missing (expected 'The vault type is invalid. Accepted type is LOGICALLY_AIR_GAPPED_BACKUP_VAULT.', got None)
  $headers.x-amzn-errortype: missing (expected 'InvalidParameterValueException', got None)

Model evidence (services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json): InvalidParameterValueException is a client error defaulting to HTTP 400 with members Code, Message, Type, Context — the Code and Context the emulator never emits are modeled members, not invented fields.

## Steps to Reproduce
Cause 1 (fixture backup-mpa-approval-team-association, step task#2):
1. backup.CreateBackupVault(BackupVaultName=<plain vault>).
2. backup.AssociateBackupVaultMpaApprovalTeam(BackupVaultName=<that vault>, MpaApprovalTeamArn=<a malformed / non-MPA ARN>).
3. Expect HTTP 400, x-amzn-errortype InvalidParameterValueException, body Code='INVALID_PARAMETER_VALUE', Context='MpaApprovalTeamArn', Message='Parameter MpaApprovalTeamArn is invalid.'. The emulator answers 400 with Message='Operation not supported for the provided vault type' and no Code/Context.

Cause 2 (fixture backup-restore-access-vault-creation-error, step task#2):
1. backup.CreateBackupVault(BackupVaultName=<plain, non-air-gapped vault>).
2. backup.CreateRestoreAccessBackupVault(SourceBackupVaultArn=<that vault's arn>, CreatorRequestId=<token>).
3. Expect HTTP 400 with Code='ERROR_4134' and Message='Provided backup vault does not support this operation.'. The emulator answers 400 with Message='Operation not supported for the provided vault type' and no Code.

Cause 3 (fixture backup-restore-access-vault-source-validation, step task#3):
1. backup.CreateBackupVault(BackupVaultName=<plain, non-air-gapped vault>).
2. backup.ListRestoreAccessBackupVaults(BackupVaultName=<that vault>).
3. Expect HTTP 400, x-amzn-errortype InvalidParameterValueException, Code='ERROR_3936', Message='The vault type is invalid. Accepted type is LOGICALLY_AIR_GAPPED_BACKUP_VAULT.'. The emulator answers HTTP 200 with a listing body and no error at all.

Issue-Source: trace-sweep-divergence
Issue-Trace: backup-tiering-configuration-boundary-values
Issue-Step: teardown#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-tiering-configuration-boundary-values backup-tiering-configuration-errors backup-tiering-configuration-get-delete-nonexistent backup-tiering-configuration-lifecycle backup-tiering-configuration-duplicate-create-conflict backup-restore-job-error-paths backup-restore-job-listing-describe-and-validation-error backup-restore-inferred-metadata-and-validation-errors backup-mpa-approval-team-association backup-mpa-approval-team-association-errors
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|GetTieringConfiguration|4xx->4xx|$.Code
Issue-Key: backup|DeleteTieringConfiguration|4xx->4xx|$.Code
Issue-Key: backup|CreateTieringConfiguration|4xx->4xx|$.Code
Issue-Key: backup|DescribeRestoreJob|4xx->4xx|$.Code
Issue-Key: backup|GetRestoreJobMetadata|4xx->4xx|$.Code
Issue-Key: backup|PutRestoreValidationResult|4xx->4xx|$.Code
Issue-Key: backup|DisassociateBackupVaultMpaApprovalTeam|4xx->4xx|$.Code
Issue-Commit: f6167f4eb06772a3465b684848e681d85c2982a9

# AWS Backup refusal bodies omit the modeled Code field

> _Note: this issue was identified against the code base at commit `f6167f4eb06772a3465b684848e681d85c2982a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTieringConfiguration

## Description
7 net-new causes, all the same fault: AWS Backup's modeled exception structures carry a `Code` member alongside `Message`, and the real service always populates it with the service's internal error code for that refusal class. The emulator serves the right HTTP status and (mostly) the right message but never emits `Code` at all, so every one of these refusal bodies is missing a modeled member. Each operation below is a separate handler/serializer that must be fixed, hence a separate cause.

Cause 1 of 7 — GetTieringConfiguration on a name that does not exist.
AWS: Smithy: GetTieringConfiguration errors include ResourceNotFoundException (httpError 400); shape ResourceNotFoundException has members Code, Message, Type, Context. Recorded behavior: status 400 with Code = 'ERROR_2C01' for a missing tiering configuration. Fixed text is the code constant itself; nothing in it is request-derived.
Emulator: status 400 is correct, but `$.Code` is absent from the body (expected 'ERROR_2C01', got None).
Why: a client that branches on the Backup error code (the SDKs surface it, and runbooks/retry logic key off 'ERROR_2C01') sees no code at all and cannot distinguish a missing tiering configuration from any other 400.
Fix: the GetTieringConfiguration error path / the Backup error serializer — the ResourceNotFoundException it constructs must set Code.
Traces: backup-tiering-configuration-boundary-values#teardown#7 (also teardown#8, backup-tiering-configuration-errors#task#1, backup-tiering-configuration-get-delete-nonexistent#task#1, backup-tiering-configuration-lifecycle#teardown#5).

Cause 2 of 7 — DeleteTieringConfiguration on a name that does not exist.
AWS: Smithy: DeleteTieringConfiguration errors include ResourceNotFoundException (httpError 400), same Code/Message/Type/Context members. Recorded behavior: status 400 with Code = 'ERROR_2C01'.
Emulator: status 400, `$.Code` missing (expected 'ERROR_2C01', got None).
Why: same loss of the machine-readable code on the delete path, so idempotent-delete logic that tolerates only the not-found code cannot recognise it.
Fix: the DeleteTieringConfiguration error path / Backup error serializer.
Traces: backup-tiering-configuration-errors#task#2 (also backup-tiering-configuration-get-delete-nonexistent#task#2, backup-tiering-configuration-lifecycle#teardown#6).

Cause 3 of 7 — CreateTieringConfiguration for a name that already exists.
AWS: Smithy: CreateTieringConfiguration errors include AlreadyExistsException (httpError 400) whose members are Code, Message, CreatorRequestId, Arn, Type, Context. Recorded behavior: status 400 with Code = 'ERROR_1C01' for the duplicate-create conflict.
Emulator: status 400, `$.Code` missing (expected 'ERROR_1C01', got None).
Why: a create-if-absent client cannot tell the already-exists refusal from a validation refusal, so it retries or fails instead of treating the configuration as present.
Fix: the CreateTieringConfiguration duplicate-name branch / Backup error serializer.
Traces: backup-tiering-configuration-duplicate-create-conflict#task#2 (also backup-tiering-configuration-errors#task#4).

Cause 4 of 7 — DescribeRestoreJob on a restore-job id that does not exist.
AWS: Smithy: DescribeRestoreJob errors include ResourceNotFoundException (httpError 400). Recorded behavior: status 400 with Code = 'ERROR_2801' for an unknown restore job.
Emulator: status 400, `$.Code` missing (expected 'ERROR_2801', got None).
Why: restore-job pollers that key on the not-found code to decide 'job id never existed' vs 'transient' get no code.
Fix: the DescribeRestoreJob lookup-failure path / Backup error serializer.
Traces: backup-restore-job-error-paths#task#1 (also backup-restore-job-listing-describe-and-validation-error#task#4).

Cause 5 of 7 — GetRestoreJobMetadata on a restore-job id that does not exist.
AWS: Smithy: GetRestoreJobMetadata errors include ResourceNotFoundException (httpError 400). Recorded behavior: status 400 with Code = 'ERROR_2801'.
Emulator: status 400, `$.Code` missing (expected 'ERROR_2801', got None).
Why: same as cause 4, on the metadata read.
Fix: the GetRestoreJobMetadata lookup-failure path / Backup error serializer.
Traces: backup-restore-job-error-paths#task#2 (also backup-restore-job-listing-describe-and-validation-error#task#5).

Cause 6 of 7 — PutRestoreValidationResult on a restore-job id that does not exist.
AWS: Smithy: PutRestoreValidationResult (PUT /restore-jobs/{RestoreJobId}/validations, success 204) errors include ResourceNotFoundException (httpError 400). Recorded behavior: status 400 with Code = 'ERROR_2801'.
Emulator: status 400, `$.Code` missing (expected 'ERROR_2801', got None).
Why: a validation reporter cannot distinguish an unknown job from a rejected status value.
Fix: the PutRestoreValidationResult validation/lookup path / Backup error serializer.
Traces: backup-restore-inferred-metadata-and-validation-errors#task#3 (also #task#4 and backup-restore-job-listing-describe-and-validation-error#task#6).

Cause 7 of 7 — DisassociateBackupVaultMpaApprovalTeam when the vault has no MPA approval team association.
AWS: Smithy: DisassociateBackupVaultMpaApprovalTeam (POST /backup-vaults/{BackupVaultName}/mpaApprovalTeam?delete, success 204) errors include InvalidRequestException / ResourceNotFoundException (httpError 400), all carrying Code. Recorded behavior: status 400 with Code = 'ERROR_3935' for disassociating a team that is not associated.
Emulator: status 400, `$.Code` missing (expected 'ERROR_3935', got None).
Why: an MPA teardown script that treats 'already disassociated' as success (by code) cannot do so and aborts.
Fix: the DisassociateBackupVaultMpaApprovalTeam refusal path / Backup error serializer.
Traces: backup-mpa-approval-team-association#task#3 (also backup-mpa-approval-team-association-errors#task#4).

Note: the open backup refusal issues (e.g. 20261103T061309Z-backup-aws-backup-plan-selection-and-vault-refusals-diverge.md, 20261108T023447Z-backup-aws-backup-plan-and-selection-refusals-diverge-missing.md) cover plan/selection refusals and do not carry the replay keys for any of the seven causes above; the replay report reported all seven as unowned.

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout.

1) GetTieringConfiguration, backup-tiering-configuration-boundary-values#teardown#7 (5 steps in cluster): expected 400 with $.Code='ERROR_2C01'; actual 400 with $.Code missing (None).
2) DeleteTieringConfiguration, backup-tiering-configuration-errors#task#2 (3 steps): expected $.Code='ERROR_2C01'; actual missing.
3) CreateTieringConfiguration, backup-tiering-configuration-duplicate-create-conflict#task#2 (2 steps): expected $.Code='ERROR_1C01'; actual missing.
4) DescribeRestoreJob, backup-restore-job-error-paths#task#1 (2 steps): expected $.Code='ERROR_2801'; actual missing.
5) GetRestoreJobMetadata, backup-restore-job-error-paths#task#2 (2 steps): expected $.Code='ERROR_2801'; actual missing.
6) PutRestoreValidationResult, backup-restore-inferred-metadata-and-validation-errors#task#3 (3 steps): expected $.Code='ERROR_2801'; actual missing.
7) DisassociateBackupVaultMpaApprovalTeam, backup-mpa-approval-team-association#task#3 (2 steps): expected $.Code='ERROR_3935'; actual missing.

In every case the HTTP status matched and only the modeled `Code` member was absent from the JSON error body. Smithy shapes ResourceNotFoundException / InvalidParameterValueException / AlreadyExistsException / InvalidRequestException in com.amazonaws.backup all declare a `Code` member.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) and inspect the listed fixture steps, or reproduce by hand:
   a. GET /tiering-configurations/<name that was never created> -> 400; body has Message but no Code (AWS: Code='ERROR_2C01').
   b. DELETE /tiering-configurations/<name that was never created> -> 400; no Code (AWS: 'ERROR_2C01').
   c. PUT /tiering-configurations twice with the same TieringConfigurationName -> second call 400; no Code (AWS: 'ERROR_1C01').
   d. GET /restore-jobs/<unknown id> -> 400; no Code (AWS: 'ERROR_2801').
   e. GET /restore-jobs/<unknown id>/metadata -> 400; no Code (AWS: 'ERROR_2801').
   f. PUT /restore-jobs/<unknown id>/validations with ValidationStatus -> 400; no Code (AWS: 'ERROR_2801').
   g. POST /backup-vaults/<vault with no MPA team>/mpaApprovalTeam?delete -> 400; no Code (AWS: 'ERROR_3935').
3. Each response body should contain the modeled `Code` member for its refusal class.

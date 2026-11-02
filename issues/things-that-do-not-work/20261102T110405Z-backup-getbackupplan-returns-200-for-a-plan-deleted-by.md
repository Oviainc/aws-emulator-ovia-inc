Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.e2e-plan-selection-backup-job-to-recovery-point
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup.e2e-plan-selection-backup-job-to-recovery-point|_error#21|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Commit: fb0a9c1858b32d12ecab08653c5d3b6198801bbb

# GetBackupPlan returns 200 for a plan deleted by DeleteBackupPlan instead of refusing ResourceNotFoundException 'Failed reading Backup plan with provided version'

> _Note: this issue was identified against the code base at commit `fb0a9c1858b32d12ecab08653c5d3b6198801bbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBackupPlan

## Description
Cause 1 of 1: a backup plan that DeleteBackupPlan has removed stays readable through GetBackupPlan, so the deletion has no observable refusal.

Failing check ids: backup.e2e-plan-selection-backup-job-to-recovery-point

AWS: DeleteBackupPlan removes the plan (Smithy: backup#DeleteBackupPlan, DELETE /backup/plans/{BackupPlanId}, output BackupPlanId/BackupPlanArn/DeletionDate/VersionId). A subsequent GetBackupPlan for that same BackupPlanId with no VersionId is refused with the modeled client error ResourceNotFoundException (Smithy: backup#GetBackupPlan errors include ResourceNotFoundException, errorKind client, httpError 400), message 'Failed reading Backup plan with provided version'. The message is entirely fixed text for this error class — it carries no value from the request or from state. The sibling refusal in the same flow is already correct in shape: GetBackupSelection on a deleted selection answers ResourceNotFoundException 'Cannot find Backup selection'.

Emulator: GetBackupPlan on the deleted plan answers HTTP 200 with the full plan body — BackupPlan (rules intact), BackupPlanId, BackupPlanArn, VersionId, CreatorRequestId, CreationDate — and merely adds a DeletionDate member. No error is raised at all. The plan id, version id and timestamps in the replay are per-run generated values and are not part of the requirement; what is required is that the read be refused once the plan is deleted.

Why: a client cannot tell a deleted plan from a live one. Deletion is confirmed by a 404/400-class refusal on the follow-up read, and teardown/idempotency logic ("delete, then poll until the read is refused") never terminates or wrongly concludes the plan still exists; an inventory walk keeps listing a plan that AWS no longer serves.

Fix: the GetBackupPlan handler / plan state in the backup crate (services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/get_backup_plan.rs and the plan store in src/state.rs): once a plan carries a deletion, an unversioned read must raise ResourceNotFoundException with the fixed message above rather than serializing the stored plan with DeletionDate set. Note: open issues 20261031T042341Z-backup-aws-backup-a-deleted-backup-plan-stays-readable.md and 20261029T193050Z-backup-aws-backup-plan-state-and-response-bodies-diverge.md describe a related 'deleted plan stays readable' class; the replay key for this check is not carried by either, so it is filed here and should be closed together with them if one fix covers both.

Traces: backup.e2e-plan-selection-backup-job-to-recovery-point — the plan+selection+job flow runs to completion, the recovery point and selection delete and refuse correctly, then DeleteBackupPlan succeeds and the following GetBackupPlan returns 200 instead of being refused.

## Evidence
Expected (AWS): after backup:DeleteBackupPlan(BackupPlanId=<id>), backup:GetBackupPlan(BackupPlanId=<id>) is refused:
  Error.Code = ResourceNotFoundException (HTTP 400, per the Smithy-defaulted httpError)
  Error.Message = "Failed reading Backup plan with provided version" (wholly fixed text)

Actual (emulator): HTTP 200 with a full GetBackupPlan body. From the replay:
  AssertionError: expected the call to be refused, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'BackupPlan': {'BackupPlanName': '<generated>', 'Rules': [...]}, 'BackupPlanId': '<generated>', 'BackupPlanArn': 'arn:aws:backup:<region>:<account>:backup-plan:<generated>', 'VersionId': '<generated>', 'CreatorRequestId': '<generated>', 'CreationDate': ..., 'DeletionDate': ...}
(the names, ids and timestamps shown are per-run values, not requirements; the requirement is the refusal)

The immediately preceding assertions in the same check pass, so the divergence is specific to GetBackupPlan after delete: DeleteBackupSelection + GetBackupSelection correctly answer ResourceNotFoundException 'Cannot find Backup selection'.

## Steps to Reproduce
1. backup:CreateBackupVault with any vault name.
2. backup:CreateBackupPlan with one rule targeting that vault.
3. backup:CreateBackupSelection on the plan with a backup-trusting IAM role, then backup:DeleteBackupSelection; confirm backup:GetBackupSelection is refused ResourceNotFoundException.
4. backup:DeleteBackupPlan(BackupPlanId=<the plan id>).
5. backup:GetBackupPlan(BackupPlanId=<the same plan id>), no VersionId.
   Expected: refusal, Code ResourceNotFoundException, Message "Failed reading Backup plan with provided version".
   Actual: HTTP 200 with the plan body plus a DeletionDate member.

Or replay the behavioral check directly: backup.e2e-plan-selection-backup-job-to-recovery-point.

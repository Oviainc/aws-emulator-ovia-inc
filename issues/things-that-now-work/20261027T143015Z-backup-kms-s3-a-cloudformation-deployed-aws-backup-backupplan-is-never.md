Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle
Issue-Key: backup|GetBackupPlan|ResourceNotFoundException
Issue-Commit: 4507d6d6dd116d6bbaed085269ed053454966346

# A CloudFormation-deployed AWS::Backup::BackupPlan is never registered in AWS Backup: the stack's BackupPlanId output is refused 400 ResourceNotFoundException by GetBackupPlan

> _Note: this issue was identified against the code base at commit `4507d6d6dd116d6bbaed085269ed053454966346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBackupPlan

## Description
One root cause.

Cause 1 of 1: a backup estate deployed through CloudFormation cannot be read back through the AWS Backup API, because the plan the stack reports creating does not exist in the backup service's state.

AWS: CloudFormation provisions AWS::Backup::BackupPlan through the backup control plane (CreateBackupPlan), so the resource's physical id / Ref — and any stack Output built from it — is the BackupPlanId AWS Backup minted. Reading that id back is a plain 200: Smithy, GetBackupPlan = GET /backup/plans/{BackupPlanId} (BackupPlanId is an httpLabel, required), success code 200, returning BackupPlan (Rules with RuleName/ScheduleExpression/Lifecycle/StartWindowMinutes/CompletionWindowMinutes), BackupPlanId, BackupPlanArn, VersionId, CreationDate. The same id keys ListBackupSelections (GET /backup/plans/{BackupPlanId}/selections) and GetBackupSelection. GetBackupPlan's modeled errors are InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException (all HTTP 400) and ServiceUnavailableException (500); ResourceNotFoundException is documented "A resource that is required for the action doesn't exist" — i.e. it is the answer for a plan id that was never created, not for one CloudFormation just created. Rules a template leaves unset inherit the service defaults a direct CreateBackupPlan would apply (StartWindowMinutes 480, CompletionWindowMinutes 10080).

Emulator: the stack reaches CREATE_COMPLETE and DescribeStacks returns a non-empty BackupPlanId output (the check's deploy phase asserts the outputs are present and passes), but the first read of that id in the next phase is refused: GetBackupPlan answers 400 ResourceNotFoundException with message "Cannot find Backup plan". So CREATE_COMPLETE is reported for a resource no backup state record exists for — the provisioner never created the plan through the backup service (or published a physical id that is not a backup plan id). GetBackupPlan itself is implemented and simply reads the stored record (services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/get_backup_plan.rs lines 198-200 look the id up in crate::state and map a miss to ResourceNotFoundException), which is why the refusal is the correct answer for an absent plan and the absence is the defect. Secondary, in the same refusal: the message's text is the fixed string "Cannot find Backup plan" with no variable part — it never names the BackupPlanId from the request, while AWS's not-found message for this operation identifies the plan it could not find. Hypothesis: AWS renders the requested id into that message; the model does not state the template and the recording only shows the emulator's string, so the 200 is the requirement being filed and the message wording is reported as observed.

Why: the standard review/inspection workflow — deploy the vault, plan and selection by template, then answer questions off the deployed estate with get-backup-plan / list-backup-selections / get-backup-selection / describe-backup-vault — is completely unreachable. A client gets a stack that claims success and an API that denies the resource exists, so every downstream read, and every assertion about rule count, schedule, retention and the inherited 480/10080 windows, fails on a 400 instead of returning data.

Fix: the CloudFormation resource provisioner for AWS::Backup::BackupPlan (and the companion AWS::Backup::BackupSelection) must materialize the resource in the backup service's state the way CreateBackupPlan/CreateBackupSelection do, and must use the BackupPlanId the backup service mints as the resource's physical id so Ref and stack Outputs carry it; rules the template leaves unset must take the same service defaults the direct create applies. No change belongs in get_backup_plan.rs beyond the message template noted above.

Related but NOT covering this: open issue 20261024T031359Z-backup-getbackupplan-stub.md lists this same check id but is scoped to GetBackupPlan's unimplemented ScheduledRunsPreview member; a preview member does not make a CloudFormation-created plan findable, so this cause is filed separately.

Traces: backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle, phase 2/4 "solve", first call backup.GetBackupPlan.

## Evidence
Check backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle, verdict [error].

Expected (AWS): phase 1 deploy reaches CREATE_COMPLETE and outputs BucketName/BackupVaultName/BackupPlanId/RuleName; phase 2 solve then calls backup.GetBackupPlan(BackupPlanId=<the stack's BackupPlanId output>) and gets HTTP 200 with BackupPlan.Rules carrying one rule (RuleName as the stack output names it, ScheduleExpression cron(0 0 * * ? *), Lifecycle.DeleteAfterDays 35, StartWindowMinutes 480, CompletionWindowMinutes 10080, no CopyActions), followed by ListBackupSelections/GetBackupSelection/DescribeBackupVault on the same estate.

Actual (emulator): phase 1 passed (outputs present and named from the check's prefix), phase 2 raised on the very first trajectory op:

  _PhaseError: [phase 2/4 solve] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetBackupPlan operation: Cannot find Backup plan

HTTP 400, error code ResourceNotFoundException, message "Cannot find Backup plan" — all fixed text, no value from the request (the BackupPlanId asked for is absent from it).

## Steps to Reproduce
1. Boot the emulator (backup, cloudformation, kms, s3).
2. CreateStack with a template declaring an AWS::S3::Bucket (versioned, AES256 default encryption), an AWS::Backup::BackupVault, an AWS::Backup::BackupPlan with one rule (ScheduleExpression cron(0 0 * * ? *), Lifecycle.DeleteAfterDays 35, no windows and no CopyActions) and an AWS::Backup::BackupSelection selecting by tag, with Outputs exporting BucketName, BackupVaultName, BackupPlanId and RuleName; Capabilities=[CAPABILITY_NAMED_IAM].
3. Poll DescribeStacks until CREATE_COMPLETE and read the Outputs — BackupPlanId is non-empty.
4. Call backup GetBackupPlan with that BackupPlanId.
5. Observe HTTP 400 ResourceNotFoundException "Cannot find Backup plan" instead of HTTP 200 with the deployed plan; ListBackupSelections and GetBackupSelection on the same id are unreachable for the same reason.

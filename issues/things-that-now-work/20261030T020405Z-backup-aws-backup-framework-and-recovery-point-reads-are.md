Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.update-framework-concurrent-conflict-detected backup.e2e-plan-selection-backup-job-to-recovery-point
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup.update-framework-concurrent-conflict-detected|run#10|AssertionError|assert successes, "neither concurrent UpdateFramework call succeeded"
Issue-Key: backup.e2e-plan-selection-backup-job-to-recovery-point|_poll_recovery_point_gone#15|AssertionError|assert code == "ResourceNotFoundException", f"recovery point {rp_arn}: expected ResourceNotFoundException, got {code}"
Issue-Commit: 80128cf5220e52d7fa07858b1a17d4e0aeba15d4

# AWS Backup framework and recovery-point reads are unreachable: UpdateFramework and DescribeRecoveryPoint answer 501 NotImplemented, so update serialization and the post-delete not-found boundary are unobservable (2 causes)

> _Note: this issue was identified against the code base at commit `80128cf5220e52d7fa07858b1a17d4e0aeba15d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFramework

## Description
Two distinct root causes behind the two net-new failing checks in the `backup` group that the replay report left unmarked (neither carries an ALREADY FILED nor an OWNED marker, so neither check's replay key is registered against any filed issue). Both checks reached a 501 `NotImplemented` scaffold stub on a call they expected to succeed or to be refused with a specific modeled error, so the suite reported them as MISMATCHES rather than stub errors. Both handlers live in services/service-com-amazonaws-backup-cryocontrollerusermanager and are bundled as one work item for this service; each cause must be closed separately.

Related already-filed issues, named here so this is not re-filed as a third work item for the same handlers: 20261030T001950Z-backup-updateframework-stub.md records only that UpdateFramework's handler is an unwritten stub (it names a single blocked unit, backup__events.backup-framework-compliance-change-event), and 20261029T175130Z-backup-aws-backup-recovery-point-restore-job-mpa-and.md records (as its Cause 2) that DescribeRecoveryPoint is a stub, grounded on a different check (backup.e2e-legal-hold-blocks-deletion-then-release). Neither carries the two check ids in `trace_ids` above, so neither will be re-replayed for them. This issue records the BEHAVIOR these two checks require of the two handlers once written, which is what makes the checks pass.

=== Cause 1 of 2: UpdateFramework is a 501 stub, so a concurrent framework update has no winner and the serialized ConflictException never happens ===
AWS: Smithy: UpdateFramework, PUT /audit/frameworks/{FrameworkName}, input FrameworkName (httpLabel, required, @length 1..256, @pattern ^[a-zA-Z][_a-zA-Z0-9]*$), FrameworkDescription (@length 0..1024, @pattern \S), FrameworkControls, IdempotencyToken (@idempotencyToken); output FrameworkName/FrameworkArn/CreationTime at HTTP 200; modeled errors AlreadyExistsException, InvalidParameterValueException, LimitExceededException, MissingParameterValueException, ResourceNotFoundException (400), ConflictException (409, documented "Backup can't perform the action that you requested until it finishes performing a previous action. Try again later.") and ServiceUnavailableException (500). Recorded behavior: AWS Backup serializes framework deployment updates, so with two UpdateFramework calls fired concurrently at the SAME FrameworkName exactly ONE answers 200 and the other is refused ConflictException (409); the framework then settles to a terminal DeploymentStatus (COMPLETED or FAILED, never a transient *_IN_PROGRESS), and DescribeFramework reports the winner's FrameworkDescription verbatim — one of the two submitted values, never a blend, a blank or a third value.
Emulator: BOTH concurrent calls are refused HTTP 501 with error code `NotImplemented` and the message "operation UpdateFramework is scaffolded but not implemented in this emulator" — services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/update_framework.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateFramework"))`. The replay therefore observed ZERO winners ("neither concurrent UpdateFramework call succeeded"). 501/NotImplemented is not in the modeled error set, so an SDK surfaces an unmodeled failure.
Why: a client can never modify an existing audit framework at all — its description and its control set are write-once at CreateFramework. Worse, the write-serialization guarantee is invisible: a client that relies on exactly one of two racing updates winning (and on retrying the ConflictException loser) cannot distinguish "my update lost the race" from "the operation does not exist", and both racers failing identically is the one outcome AWS never produces.
Fix: implement the handler in .../src/ops/update_framework.rs: resolve the stored framework by FrameworkName (ResourceNotFoundException when absent), take the state lock for the whole read-modify-write so concurrent updates on one name serialize, refuse the loser with the modeled ConflictException (409) while a previous action on that framework is still in flight, apply FrameworkDescription/FrameworkControls for the winner, answer 200 with FrameworkName/FrameworkArn/CreationTime, and leave the framework in a terminal DeploymentStatus so DescribeFramework reports the winner's description; then delete the SCAFFOLDED marker line.
Traces: backup.update-framework-concurrent-conflict-detected.

=== Cause 2 of 2: DescribeRecoveryPoint is a 501 stub, so the full plan+selection+job flow cannot observe that a deleted recovery point is gone ===
AWS: Smithy: DescribeRecoveryPoint, GET /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}, input BackupVaultName (httpLabel, required, @pattern ^[a-zA-Z0-9\-\_]{2,50}$), RecoveryPointArn (httpLabel, required, ARN), BackupVaultAccountId (httpQuery backupVaultAccountId, @pattern ^[0-9]{12}$); output at HTTP 200 carries RecoveryPointArn, BackupVaultName, BackupVaultArn, ResourceArn, ResourceType, CreatedBy, Status (RecoveryPointStatus: AVAILABLE|COMPLETED|CREATING|DELETING|EXPIRED|PARTIAL|STOPPED), BackupSizeInBytes, CreationDate, Lifecycle, CalculatedLifecycle, StorageClass, VaultType, IsEncrypted/IsParent (both @default false, required); modeled errors InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException (400, documented "A resource that is required for the action doesn't exist.") and ServiceUnavailableException (500). Recorded behavior: after DeleteRecoveryPoint succeeds, DescribeRecoveryPoint on that same (BackupVaultName, RecoveryPointArn) is refused ResourceNotFoundException with the fixed sentence "Cannot find recovery point" — fixed text, no variable part; while the point still exists the same call answers 200 with the ARN echoed and Status=COMPLETED for a finished job.
Emulator: the call answers HTTP 501 with error code `NotImplemented` and the message "operation DescribeRecoveryPoint is scaffolded but not implemented in this emulator" — .../src/ops/describe_recovery_point.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeRecoveryPoint"))`. Everything before it in the flow now works (CreateBackupVault, CreateBackupPlan, CreateBackupSelection, StartBackupJob polled to COMPLETED, ListRecoveryPointsByBackupVault showing the point, and DeleteRecoveryPoint itself answering success), so the ONLY divergence left in this end-to-end check is this read.
Why: a client can never read back a recovery point — not its Status, not its BackupSizeInBytes, not its Lifecycle — and cannot confirm a deletion took effect: "deleted", "not yet indexed" and "never existed" all collapse into one unmodeled 501 instead of the modeled 400 ResourceNotFoundException the SDK knows how to classify. Any backup-then-verify or delete-then-verify workflow hangs or aborts here.
Fix: implement .../src/ops/describe_recovery_point.rs against the recovery points stored in crate::state: resolve the vault and the ARN, refuse ResourceNotFoundException with "Cannot find recovery point" when the ARN is not an indexed point of that vault, otherwise answer 200 populating the stored attributes (including the @default-false IsEncrypted/IsParent members); then delete the SCAFFOLDED marker line.
Traces: backup.e2e-plan-selection-backup-job-to-recovery-point.

## Evidence
Suite: 8/97 passed. backup 1/46 (2%), 45 failed. The two failures below are the only backup-group failures the report left unmarked (every other failure is marked ALREADY FILED or OWNED).

Cause 1 — [mismatch] backup.update-framework-concurrent-conflict-detected:
  File "check_backup_update_framework_concurrent_conflict_detected.py", line 58, in run
    assert successes, "neither concurrent UpdateFramework call succeeded"
  AssertionError: neither concurrent UpdateFramework call succeeded
The check created a framework (CreateFramework with FrameworkControls=[{ControlName: BACKUP_RECOVERY_POINT_MINIMUM_RETENTION_CHECK}]) and waited for DescribeFramework's DeploymentStatus to leave CREATE_IN_PROGRESS — both of which the emulator served — then fired two UpdateFramework calls concurrently from a 2-worker pool with descriptions 'sim2real-racer-a' and 'sim2real-racer-b'. Expected: exactly one 200 and exactly one ConflictException (409), then DescribeFramework reporting the winner's description. Actual: both calls refused HTTP 501 `NotImplemented` ("operation UpdateFramework is scaffolded but not implemented in this emulator"), so zero successes and zero ConflictExceptions. Handler confirmed in the checkout at services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/update_framework.rs: SCAFFOLDED marker on line 4, `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateFramework"))` on line 186.

Cause 2 — [mismatch] backup.e2e-plan-selection-backup-job-to-recovery-point:
  File "check_backup_e2e_plan_selection_backup_job_to_recovery_point.py", line 48, in run
    _poll_recovery_point_gone(ctx, vault_name, rp_arn)
  File "...", line 262, in _poll_recovery_point_gone
    assert code == "ResourceNotFoundException", f"recovery point {rp_arn}: expected ResourceNotFoundException, got {code}"
  AssertionError: recovery point arn:aws:backup:<region>:<account>:recovery-point:<uuid>: expected ResourceNotFoundException, got NotImplemented
The flow succeeded all the way through DeleteRecoveryPoint; the following DescribeRecoveryPoint(BackupVaultName=<vault>, RecoveryPointArn=<the deleted point>) answered HTTP 501 `NotImplemented` ("operation DescribeRecoveryPoint is scaffolded but not implemented in this emulator") where AWS answers 400 ResourceNotFoundException with the fixed sentence "Cannot find recovery point". Handler confirmed in the checkout at services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/describe_recovery_point.rs: SCAFFOLDED marker on line 4, `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeRecoveryPoint"))` on line 260.

## Steps to Reproduce
Cause 1 (UpdateFramework):
1. backup.CreateFramework(FrameworkName=f, FrameworkControls=[{ControlName: 'BACKUP_RECOVERY_POINT_MINIMUM_RETENTION_CHECK'}]).
2. Poll backup.DescribeFramework(FrameworkName=f) until DeploymentStatus is terminal (COMPLETED or FAILED).
3. From two threads simultaneously call backup.UpdateFramework(FrameworkName=f, FrameworkDescription='sim2real-racer-a' / 'sim2real-racer-b', FrameworkControls=[{ControlName: 'BACKUP_RECOVERY_POINT_MINIMUM_RETENTION_CHECK'}]).
4. Expect exactly one HTTP 200 and exactly one ConflictException (409); emulator answers HTTP 501 NotImplemented to BOTH.
5. Expect backup.DescribeFramework(FrameworkName=f) to then report a terminal DeploymentStatus and FrameworkDescription equal to the winning racer's value.

Cause 2 (DescribeRecoveryPoint):
1. dynamodb.CreateTable(t) -> ACTIVE; iam.CreateRole trusting backup.amazonaws.com with AWSBackupServiceRolePolicyForBackup attached; backup.CreateBackupVault(v).
2. backup.CreateBackupPlan(plan with a rule targeting v); backup.CreateBackupSelection(BackupPlanId=plan, BackupSelection={IamRoleArn=role, Resources=[<table arn>]}).
3. backup.StartBackupJob(BackupVaultName=v, ResourceArn=<table arn>, IamRoleArn=role); poll backup.DescribeBackupJob to COMPLETED and take RecoveryPointArn; confirm it appears in backup.ListRecoveryPointsByBackupVault(BackupVaultName=v).
4. backup.DeleteRecoveryPoint(BackupVaultName=v, RecoveryPointArn=rp) -> succeeds.
5. backup.DescribeRecoveryPoint(BackupVaultName=v, RecoveryPointArn=rp) -> expect 400 ResourceNotFoundException 'Cannot find recovery point'; emulator answers HTTP 501 NotImplemented.
6. Also call backup.DescribeRecoveryPoint for a point that still exists -> expect 200 with RecoveryPointArn echoed and Status=COMPLETED.

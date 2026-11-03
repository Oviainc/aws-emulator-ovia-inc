Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.e2e-plan-selection-backup-job-to-recovery-point backup.e2e-legal-hold-blocks-deletion-then-release
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup.e2e-plan-selection-backup-job-to-recovery-point|_error#22|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: backup.e2e-legal-hold-blocks-deletion-then-release|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: d7e04dcd1593e6794e95dfd8903b7eb44f6eb886

# AWS Backup: a deleted backup plan stays readable through GetBackupPlan, and ListRecoveryPointsByLegalHold answers 501 so a legal hold's coverage is unobservable (2 causes)

> _Note: this issue was identified against the code base at commit `d7e04dcd1593e6794e95dfd8903b7eb44f6eb886`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBackupPlan

## Description
This issue bundles 2 distinct root causes in the AWS Backup emulator, both grounded in failing behavioral checks of the `backup` service. Each must be closed before submitting.

Cause 1 of 2: GetBackupPlan keeps serving a plan that DeleteBackupPlan already removed.
AWS: DeleteBackupPlan removes the plan; a subsequent GetBackupPlan for it is refused with HTTP 400 ResourceNotFoundException (Smithy: GetBackupPlan GET /backup/plans/{BackupPlanId}, errors InvalidParameterValueException/MissingParameterValueException/ResourceNotFoundException, all httpError 400; success code 200). The recorded message for this error class is the fixed sentence `Failed reading Backup plan with provided version` — it has no variable part, and the error carries the service's `Code` for the condition plus the `x-amzn-errortype: ResourceNotFoundException` header.
Emulator: GetBackupPlan answers HTTP 200 after DeleteBackupPlan succeeded, returning the whole plan body (BackupPlanName, Rules with RuleName/TargetBackupVaultName/ScheduleExpression/StartWindowMinutes/CompletionWindowMinutes/RuleId, BackupPlanId, BackupPlanArn, VersionId, CreatorRequestId, CreationDate) and, tellingly, a populated DeletionDate — so the deletion IS recorded in state but is not honored on the read path. `state.rs` comments the intent explicitly ("A deleted plan is still READABLE (it reports its `DeletionDate`)"), which is the divergence: only `live_backup_plan` filters on `deletion_date`, and the read path does not use it.
Why: deletion is not observable. A client (or a teardown/convergence loop) that polls GetBackupPlan until it refuses never terminates, and callers keep reading and acting on a plan AWS has deleted.
Fix: GetBackupPlan's handler / the plan read projection in the backup crate — once a plan version carries a DeletionDate, every read of it must refuse with the 400 ResourceNotFoundException and that exact sentence instead of returning the body.
Traces: backup.e2e-plan-selection-backup-job-to-recovery-point (the check's `_error(... get_backup_plan ...)` raised `expected the call to be refused, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200 ...}}`).
Related already-open issue: 20261029T193050Z-backup-aws-backup-plan-state-and-response-bodies-diverge.md describes the same read-after-delete behavior as its Cause 2, but it carries only wire replay keys; this behavioral check is not covered by it, so it is filed here with its own replay key. Closing one should close the other — re-replay both.

Cause 2 of 2: ListRecoveryPointsByLegalHold is an unimplemented scaffold stub, so nothing can observe which recovery points a legal hold covers.
AWS: ListRecoveryPointsByLegalHold answers HTTP 200 with `RecoveryPoints` (and `NextToken` when paginating) listing the recovery points the hold's RecoveryPointSelection matched (Smithy: GET /legal-holds/{LegalHoldId}/recovery-points, output RecoveryPoints: RecoveryPointsList, NextToken; modeled errors are only InvalidParameterValueException/MissingParameterValueException (400) and ServiceUnavailableException (500) — there is no not-implemented outcome in the contract).
Emulator: the handler at services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/list_recovery_points_by_legal_hold.rs returns `crate::scaffolded::Scaffolded::not_implemented("ListRecoveryPointsByLegalHold")` — HTTP 501 with error code `NotImplemented` and message `operation ListRecoveryPointsByLegalHold is scaffolded but not implemented in this emulator`. Every other step of the legal-hold path around it IS implemented (CreateLegalHold stores the record and GetLegalHold settles CREATING -> ACTIVE after LEGAL_HOLD_SETTLE_AFTER = 3s, StartBackupJob/DescribeBackupJob settle a job in 500ms), so the hold exists and is ACTIVE while its coverage is unreadable. The behavioral check's convergence loop therefore never sees the hold attach to the recovery point and the check is killed by the harness: `Check timed out after 60.0s`.
Why: a client cannot enumerate what a legal hold protects, which is the only API answer to "is this recovery point held?". Any workflow that waits for a hold to take effect before asserting its protection hangs instead of converging, and 501 is not an outcome the operation's contract admits, so SDK retry/error handling cannot classify it.
Fix: implement the handler in services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/list_recovery_points_by_legal_hold.rs against the stored LegalHoldRecord.recovery_point_selection and the recovery-point store in src/state.rs (match stored recovery points by the selection's ResourceIdentifiers/DateRange/VaultNames and page them with MaxResults/NextToken), and delete the SCAFFOLDED marker.
Traces: backup.e2e-legal-hold-blocks-deletion-then-release.
Note: no open issue covers this operation — the filed legal-hold stub issues name CreateLegalHold, ListLegalHolds and CancelLegalHold only.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1) against this checkout: 16/97 passed; these two `backup` checks failed and were not marked ALREADY FILED or OWNED.

Cause 1 — backup.e2e-plan-selection-backup-job-to-recovery-point [mismatch]:
  File "check_backup_e2e_plan_selection_backup_job_to_recovery_point.py", line 57, in run
    code_plan, message_plan = _error(lambda: backup.get_backup_plan(BackupPlanId=plan_id))
  AssertionError: expected the call to be refused, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'BackupPlan': {'BackupPlanName': ..., 'Rules': [{'RuleName': 'daily', 'TargetBackupVaultName': ..., 'ScheduleExpression': 'cron(0 5 * * ? *)', 'StartWindowMinutes': 480, 'CompletionWindowMinutes': 10080, 'RuleId': ...}]}, 'BackupPlanId': ..., 'BackupPlanArn': 'arn:aws:backup:<region>:<account>:backup-plan:<plan-id>', 'VersionId': ..., 'CreatorRequestId': ..., 'CreationDate': ..., 'DeletionDate': ...}
  Expected (the check's assertion, and AWS's behavior): ClientError with Code 'ResourceNotFoundException' and Message 'Failed reading Backup plan with provided version' on HTTP 400. (The ids, ARNs, names and timestamps above are per-run values, not requirements; the requirement is the 400 + code + that fixed sentence.)
  Code: src/state.rs — `live_backup_plan` filters on `deletion_date.is_none()`, but the plan READ path does not, and its doc comment states a deleted plan is still readable.

Cause 2 — backup.e2e-legal-hold-blocks-deletion-then-release [mismatch]: 'Check timed out after 60.0s'. The check creates a backup job, waits for it to settle, creates a legal hold, polls GetLegalHold until ACTIVE, then polls `list_recovery_points_by_legal_hold(LegalHoldId=...)` for up to 300s for the hold to cover the recovery point. That probe is wrapped in try/except, so the 501 refusal is swallowed and the loop can never satisfy its predicate — the harness kills the check at its 60s budget.
  Emulator source: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/list_recovery_points_by_legal_hold.rs line 164 — `Err(crate::scaffolded::Scaffolded::not_implemented("ListRecoveryPointsByLegalHold"))` => HTTP 501, code 'NotImplemented', message 'operation ListRecoveryPointsByLegalHold is scaffolded but not implemented in this emulator'.
  Expected: HTTP 200 with RecoveryPoints containing the held recovery point's RecoveryPointArn.
  Model ground: smithy_query operation GetBackupPlan and ListRecoveryPointsByLegalHold (model services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json) — GetBackupPlan ResourceNotFoundException httpError 400; ListRecoveryPointsByLegalHold output {RecoveryPoints, NextToken}, http 200, errors 400/400/500 only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite simparity-behavioral-aws-v1.
2. Cause 1 — replay backup.e2e-plan-selection-backup-job-to-recovery-point, or by hand: CreateBackupVault, CreateBackupPlan (one rule targeting the vault), CreateBackupSelection, StartBackupJob, poll DescribeBackupJob to COMPLETED, DeleteRecoveryPoint, DeleteBackupSelection, DeleteBackupPlan, then GET /backup/plans/{BackupPlanId}. Expect 400 ResourceNotFoundException with Message 'Failed reading Backup plan with provided version'; observe 200 with the full plan body carrying DeletionDate.
3. Cause 2 — replay backup.e2e-legal-hold-blocks-deletion-then-release, or by hand: StartBackupJob into a vault, wait for the job to reach COMPLETED, CreateLegalHold with RecoveryPointSelection {ResourceIdentifiers: [<the protected resource ARN>], DateRange: {FromDate: <past>, ToDate: <now>}}, poll GetLegalHold until Status=ACTIVE, then GET /legal-holds/{LegalHoldId}/recovery-points. Expect 200 with the held RecoveryPointArn in RecoveryPoints; observe 501 NotImplemented.
4. Compare status, error code and error message against each expectation above.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.cli-create-backup-plan-and-verify-eventbridge-rule-target
Issue-Key: backup__events.cli-create-backup-plan-and-verify-eventbridge-rule-target|_run_aws#11|AssertionError|raise AssertionError(f"aws CLI command failed: {' '.join(cmd)}\nstderr:\n{result.stderr}")
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# Implement backup CreateBackupPlan: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackupPlan

## Description
`CreateBackupPlan` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/create_backup_plan.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateBackupPlan` call, so each one unblocks only when `CreateBackupPlan` works: `backup__events.cli-create-backup-plan-and-verify-eventbridge-rule-target`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/create_backup_plan.rs`. Replaying the corpus, the emulator refused the `CreateBackupPlan` call: File "check_backup__events_cli_create_backup_plan_and_verify_eventbridge_rule_target.py", line 51, in run
    _run_aws(ctx, ["backup", "create-backup-plan", "--backup-plan", json.dumps(plan_body)])
    ~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "check_backup__events_cli_create_backup_plan_and_verify_eventbridge_rule_target.py", line 104, in _run_aws
    raise AssertionError(f"aws CLI command failed: {' '.join(cmd)}\nstderr:\n{result.stderr}")
AssertionError: aws CLI command failed: aws --region us-west-2 --output json backup create-backup-plan --backup-plan {"BackupPlanName": "sim2real-beh-plan-e938edd87d96", "Rules": [{"RuleName": "daily", "TargetBackupVaultName": "sim2real-beh-vault-a649b75f4483", "ScheduleExpression": "cron(0 5 * * ? *)", "StartWindowMinutes": 60, "CompletionWindowMinutes": 120}]}
stderr:

aws: [ERROR]: An error occurred (NotImplemented) when calling the CreateBackupPlan operation: operation CreateBackupPlan is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the backup emulator
Call CreateBackupPlan and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

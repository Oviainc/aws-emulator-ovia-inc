Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.eventbridge-target-missing-backup-permission-not-invoked
Issue-Key: backup__events.eventbridge-target-missing-backup-permission-not-invoked|run#33|AssertionError|assert bad_final["status"] == "FAILED", f"expected the backup task to fail on AccessDenied: {bad_final}"
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# backup__events: a Step Functions aws-sdk:backup:startBackupJob task succeeds under an execution role with no backup:StartBackupJob permission, so the authorization-denied path is unreachable

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
Cause 1 of 1: AWS Backup StartBackupJob is not authorized against the caller's identity when it is invoked through a Step Functions AWS SDK service integration, so a state machine whose execution role deliberately holds NO backup permission still creates a real backup job and its execution settles SUCCEEDED.

AWS: Smithy (service com.amazonaws.backup, operation StartBackupJob -> StartBackupJobOutput{BackupJobId, RecoveryPointArn, CreationDate, IsParent}) models no authorization error of its own; authorization is the IAM contract every AWS API call is subject to. A Step Functions Task whose Resource is `arn:aws:states:::aws-sdk:backup:startBackupJob` issues that call with the state machine's execution-role credentials. When that role has no `backup:StartBackupJob` statement, IAM refuses the call with HTTP 403 and error code `AccessDeniedException`, whose message is AWS's fixed IAM template `User: <role-session-arn> is not authorized to perform: backup:StartBackupJob on resource: <resource-arn> because no identity-based policy allows the backup:StartBackupJob action` — the fixed text is the template, and the principal ARN, action and resource ARN are taken from the request and the caller's identity, never from one recording. Step Functions then fails the task: the execution's `status` is `FAILED`, the task failure carries `error` `Backup.AccessDeniedException` (the SDK-integration error name, service prefix + error code) with the IAM message as its `cause`, and NO backup job and NO recovery point are created. The positive control — the identical pair whose role does hold `backup:StartBackupJob` and `iam:PassRole` on the backup service role — succeeds end to end.

Emulator: both halves succeed. The execution started on the permission-less state machine returns from DescribeExecution with `status: 'SUCCEEDED'`, an `output` carrying a fresh `BackupJobId` and `RecoveryPointArn`, and `stopDate` ~0.5s after `startDate`; a real backup job is created for the target table. The emulator never evaluates the execution role's identity-based policy for the integration call, so the denial is unobservable and the only difference between the permitted and un-permitted state machines disappears.

Why: a client cannot test or rely on least-privilege for Step Functions AWS SDK integrations. Code that asserts a denied task fails (catch States.TaskFailed / Backup.AccessDeniedException, route to a Catch branch, alarm on FAILED executions) sees SUCCEEDED instead, and a workflow that should have been blocked silently mutates state — it creates backup jobs and recovery points. Negative-path tests pass against the emulator and fail against AWS, and an over-broad role is indistinguishable from a correctly scoped one.

Fix: authorization of the Step Functions AWS SDK integration call path — the integration must sign/attribute the call as the state machine's execution role and the backup StartBackupJob handler must gate on the caller's identity-based policy before mutating state, refusing 403 AccessDeniedException with AWS's IAM message template; Step Functions must then surface that refusal as a task failure (error `Backup.AccessDeniedException`) and settle the execution FAILED. Related already-open issues describe the backup-side gap generally (20261029T175130Z-backup-aws-backup-vault-lock-and-tiering-refusals-use.md says StartBackupJob performs no authorization at all) and the event-propagation side (20261102T091831Z-backup-events-no-aws-backup-state-change-event.md); neither carries this check's replay key, so it is filed here.

Traces: backup__events.eventbridge-target-missing-backup-permission-not-invoked — the assertion is `expected the backup task to fail on AccessDenied`, and the DescribeExecution body shows `'status': 'SUCCEEDED'` with a populated BackupJobId/RecoveryPointArn output.

## Evidence
Check: backup__events.eventbridge-target-missing-backup-permission-not-invoked (verdict: mismatch).

Setup (from the check): two identical state machines, each with one Task state `StartBackupJob` whose Resource is `arn:aws:states:::aws-sdk:backup:startBackupJob` and Parameters {BackupVaultName, ResourceArn (a DynamoDB table), IamRoleArn (a backup service role)}. The 'good' role trusts events.amazonaws.com + states.amazonaws.com and allows states:StartExecution, backup:StartBackupJob and iam:PassRole on the backup service role. The 'noperm' role trusts the same two principals but allows ONLY states:StartExecution — no backup action at all. Each is wired as the RoleArn of an EventBridge rule target, and the rule is fired with PutEvents.

Expected (AWS): the correctly-trusted target IS invoked, but the SDK task is refused by IAM with 403 AccessDeniedException ('User: <assumed-role-arn> is not authorized to perform: backup:StartBackupJob on resource: <arn> because no identity-based policy allows the backup:StartBackupJob action' — fixed template text, variable principal/action/resource), Step Functions fails the task with error `Backup.AccessDeniedException`, DescribeExecution reports status FAILED, and no backup job exists for the table after the execution.

Actual (emulator): DescribeExecution on the no-permission execution reports
  'status': 'SUCCEEDED'
  'output': '{"BackupJobId":"...","RecoveryPointArn":"arn:aws:backup:<region>:<account>:recovery-point:...","CreationDate":...,"IsParent":false}'
  'redriveStatusReason': 'Execution is SUCCEEDED and cannot be redriven'
AssertionError raised by the check: `expected the backup task to fail on AccessDenied: {... 'status': 'SUCCEEDED' ...}`. The generated ids, ARNs and timestamps in that body are per-run values, not part of the requirement; the requirement is the terminal status, the error name and the absence of a created job.

## Steps to Reproduce
1. Create a DynamoDB table (backup resource), a backup vault, and a backup service role.
2. Create role A trusting events.amazonaws.com and states.amazonaws.com with Allow states:StartExecution, backup:StartBackupJob, iam:PassRole(<backup service role>).
3. Create role B trusting the same principals with Allow states:StartExecution ONLY (no backup action).
4. Create two state machines with the same definition — a single Task, Resource `arn:aws:states:::aws-sdk:backup:startBackupJob`, Parameters {BackupVaultName, ResourceArn=<table arn>, IamRoleArn=<backup service role>} — one with role A, one with role B.
5. Create two EventBridge rules (patterns on source sim2real.test, detail-type trigger-good / trigger-bad) and PutTargets each state machine with its own role as the target RoleArn.
6. PutEvents detail-type trigger-good; wait for the execution: it SUCCEEDS and a backup job exists (positive control).
7. PutEvents detail-type trigger-bad; wait for the execution to reach a terminal state and call DescribeExecution.
   Expected: status FAILED, task error `Backup.AccessDeniedException` with the IAM AccessDeniedException message, no backup job created for the table.
   Observed: status SUCCEEDED with a BackupJobId/RecoveryPointArn output and a real backup job created.

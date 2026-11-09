Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.cfn-deploy-backup-plan-with-eventbridge-rule backup__events.cfn-deploy-eventbridge-role-trust-policy-for-backup-target backup__events.cfn-deploy-backup-plan-missing-eventbridge-permission-rejected backup__events.cfn-deploy-backup-eventbridge-target-missing-permission-negative
Issue-Key: backup__events.cfn-deploy-backup-plan-with-eventbridge-rule|_poll_until#23|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: backup__events.cfn-deploy-eventbridge-role-trust-policy-for-backup-target|_poll_until#19|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: backup__events.cfn-deploy-backup-plan-missing-eventbridge-permission-rejected|_client_error#21|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: backup__events.cfn-deploy-backup-eventbridge-target-missing-permission-negative|run#41|AssertionError|assert bad_execution["status"] == "FAILED", (
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# AWS Backup: 2 net-new causes — a stack-deployed EventBridge schedule never starts a backup job, and backup:StartBackupJob is not authorized against the caller's role

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
Two independent defects grounded in four failing checks under the `backup__events` heading.

Cause 1 of 2: a CloudFormation-deployed EventBridge scheduled rule with an AWS Backup target never starts a backup job.
AWS: when a stack deploys an `AWS::Events::Rule` with a schedule and a target that calls AWS Backup (through the deployed target role whose trust policy names events.amazonaws.com), the schedule fires and `ListBackupJobs` shows a job created for the plan/vault; the job's state advances from CREATED/RUNNING to COMPLETED.
Emulator: no backup job ever appears. Both the plain deployment and the variant that exercises the deployed trust policy poll `ListBackupJobs` to their deadlines and see nothing (`Last result: None`). No error is returned to anybody — the delivery simply does not happen.
Why: a client that wires scheduled backups through EventBridge sees a healthy stack, an ENABLED rule and a trusted role, yet no recovery points are ever produced — a silent data-protection failure.
Fix: the EventBridge scheduled-rule firing path and its AWS Backup target invocation (target dispatch for a backup target), plus the AWS::Events::Rule provider's wiring of the deployed role.
Traces: backup__events.cfn-deploy-backup-plan-with-eventbridge-rule; backup__events.cfn-deploy-eventbridge-role-trust-policy-for-backup-target.

Cause 2 of 2: StartBackupJob is not authorized — a role without `backup:StartBackupJob` succeeds.
AWS: a caller whose role has no `backup:StartBackupJob` grant is refused with HTTP 400 `AccessDeniedException`; the message follows AWS's identity-based-denial template `User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: backup:StartBackupJob on resource: <resource> because no identity-based policy allows the backup:StartBackupJob action` — fixed text is the `User: ... is not authorized to perform: <action> on resource: <resource> because no identity-based policy allows the <action> action` frame, with the assumed-role ARN, session name, action and resource taken from the request and the caller's credentials. The same denial governs an EventBridge target invoking Backup under an under-permissioned role: the scheduled execution FAILS rather than succeeding.
Emulator: the call is accepted. StartBackupJob returns HTTP 200 with a `BackupJobId` and `RecoveryPointArn` for a role carrying no backup grant, and the under-permissioned target's scheduled execution ends `SUCCEEDED` instead of `FAILED`.
Why: every negative authorization test a client writes passes vacuously, and least-privilege roles appear to work, so a real deployment that is missing the grant is not caught until production.
Fix: authorization evaluation on the StartBackupJob handler (identity-based policy check for the assumed role), and the EventBridge target invocation path's propagation of that denial into the execution's failure.
Traces: backup__events.cfn-deploy-backup-plan-missing-eventbridge-permission-rejected; backup__events.cfn-deploy-backup-eventbridge-target-missing-permission-negative.

Note: open issue 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md and 20261107T205257Z-backup-events-4-net-new-causes-backup-emits.md carry other backup__events checks; neither carries the four replay keys above.

## Evidence
Cause 1: `AssertionError: backup job created by the deployed schedule not met within 239.915321s. Last result: None`; `AssertionError: backup job created by the deployed schedule using the deployed role not met within 239.283216s. Last result: None`.
Cause 2: `AssertionError: expected the call to be refused, but it succeeded` (the check expected `AccessDeniedException` from a role missing backup:StartBackupJob); and `AssertionError: expected the under-permissioned target's scheduled execution to FAIL, got SUCCEEDED`. A sibling recorded response shows the accepted call: HTTP 200 with `BackupJobId` and `RecoveryPointArn`.

## Steps to Reproduce
1. Cause 1: deploy a stack with a backup vault + plan, an `AWS::Events::Rule` on a short schedule whose target starts a backup job via a deployed role trusting events.amazonaws.com; poll `backup.list_backup_jobs()` — no job ever appears.
2. Cause 2: create an IAM role with NO `backup:StartBackupJob` statement, assume it, and call `backup.start_backup_job(...)` — expect HTTP 400 AccessDeniedException, observe HTTP 200 with a BackupJobId. Then wire that role as an EventBridge target role and observe the scheduled execution end SUCCEEDED instead of FAILED.
3. Replay the four checks in trace_ids.

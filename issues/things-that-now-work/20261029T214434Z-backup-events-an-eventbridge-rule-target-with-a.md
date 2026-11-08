Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.eventbridge-rule-target-role-missing-trust-policy-rejected
Issue-Key: backup__events.eventbridge-rule-target-role-missing-trust-policy-rejected|run#36|AssertionError|assert executions, (
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# backup__events: an EventBridge rule target with a correctly-trusted role starts the state machine but the aws-sdk:backup:startBackupJob Task ends FAILED, so the trust-policy boundary has no working positive control (1 cause)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
One net-new root cause behind the single remaining un-filed failure in the `backup__events` group (every other failure in this group is marked ALREADY FILED for a scaffold stub, or OWNED by 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md / 20261024T031359Z-backup-events-aws-backup-job-lifecycle-and-eventing-diverge-jobs.md / 20261027T143015Z-backup-events-the-cli-driven-backup-plan-eventbridge-rule-target.md / 20261029T175130Z-backup-listbackupjobs-stub.md, and is not re-filed here).

Related open issue, named for the fixer: 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md describes the same inability to make an `arn:aws:states:::aws-sdk:backup:*` integration call as its cause 2, but that issue does NOT carry this check's replay key (its Issue-Trace-Ids list this check nowhere), so it does not gate this failure. Fixing cause 2 there is expected to fix this one; this issue is what makes that provable by re-replaying the check id below.

=== Cause 1 of 1: a Step Functions Task state whose Resource is arn:aws:states:::aws-sdk:backup:startBackupJob is dispatched (the execution starts and runs) but terminates FAILED instead of SUCCEEDED, and no backup job is created ===
AWS: recorded behavior only — the Step Functions AWS SDK integration surface is not in either service's Smithy model. On AWS, an EventBridge rule whose target is a state machine ARN with a RoleArn whose trust policy allows sts:AssumeRole for Principal {"Service": "events.amazonaws.com"} and whose inline policy allows states:StartExecution is assumed at rule-trigger time, the execution starts, and a Task state with Resource "arn:aws:states:::aws-sdk:backup:startBackupJob" calls Backup StartBackupJob with the state machine's role, parameters BackupVaultName / ResourceArn / IamRoleArn taken from the input. That call is the modeled operation (Smithy: backup StartBackupJob, PUT /backup-jobs, 200; required input BackupVaultName (@pattern ^[a-zA-Z0-9\-\_]{2,50}$), ResourceArn, IamRoleArn; output BackupJobId, RecoveryPointArn, CreationDate, IsParent; modeled errors InvalidParameterValueException, InvalidRequestException, LimitExceededException, MissingParameterValueException, ResourceNotFoundException, ServiceUnavailableException). The execution therefore reaches ListExecutions status SUCCEEDED, carrying the StartBackupJob response as the Task result, and ListBackupJobs(ByResourceArn=<the resource>) reports a job in a non-terminal or COMPLETED state — never FAILED/ABORTED/EXPIRED. This positive control is what gives the check's negative half its meaning: the SAME state machine targeted with a role that grants states:StartExecution but trusts lambda.amazonaws.com instead of events.amazonaws.com must never be assumable, so no execution and no backup job appear.
Emulator: the negative half holds (no execution, no job for the wrongly-trusted role), and the rule DOES start executions for the correctly-trusted role — the replay observes statuses ['RUNNING', 'RUNNING', 'FAILED'] — but every such execution terminates FAILED rather than SUCCEEDED, so the check fails on `AssertionError: positive control with a correctly-trusted role should have succeeded, executions seen: ['RUNNING', 'RUNNING', 'FAILED']`. The failure is the Task state's integration call, not the EventBridge delivery: the sibling checks in this group record the terminal cause verbatim as error='States.Runtime', cause="state 'StartBackupJob': 'arn:aws:states:::aws-sdk:backup:startBackupJob' names an integration with 'backup', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it". No backup job is created for the resource, so the subsequent ListBackupJobs assertion is unreachable.
Why: the trust-policy boundary this check exists to prove is untestable — the denied case and the allowed case are indistinguishable, because both end with no execution result and no backup job. A least-privilege regression (EventBridge assuming a role that does not trust events.amazonaws.com) would pass silently against this emulator. More broadly, any client using the documented EventBridge-rule -> Step Functions -> AWS Backup orchestration cannot run its workflow at all: the execution fails for a reason internal to the emulator rather than anything in the request.
Fix: the Step Functions Task-state AWS SDK integration dispatch (the `aws-sdk:<service>:<operation>` resolver in the sfn crate) must recognise the `backup` service and route `startBackupJob` to the Backup emulator's StartBackupJob handler under the state machine's role, returning its modeled output as the Task result so the execution reaches SUCCEEDED; a service/operation it genuinely cannot route must still fail as AWS does rather than with an emulator-internal cause sentence. Nothing in the backup crate's StartBackupJob handler is implicated by this cause on its own.
Traces: backup__events.eventbridge-rule-target-role-missing-trust-policy-rejected.

## Evidence
Suite: backup__events 7/50 (14%), 43 failed. 42 of those are ALREADY FILED or OWNED; this is the one net-new failure.

[mismatch] backup__events.eventbridge-rule-target-role-missing-trust-policy-rejected
  File "check_backup__events_eventbridge_rule_target_role_missing_trust_policy_rejected.py", line 111, in run
  AssertionError: positive control with a correctly-trusted role should have succeeded, executions seen: ['RUNNING', 'RUNNING', 'FAILED']

Expected (AWS): sfn.ListExecutions(stateMachineArn=<sm>) contains an execution with status 'SUCCEEDED' within the poll window, and backup.ListBackupJobs(ByResourceArn=<table arn>) then reports at least one job whose State is not in (FAILED, ABORTED, EXPIRED).
Actual (emulator): executions are created and observed RUNNING, then settle to 'FAILED'; no SUCCEEDED execution ever appears and no backup job is created for the resource. The sibling checks in this group record the terminal detail as error='States.Runtime' with cause="state 'StartBackupJob': 'arn:aws:states:::aws-sdk:backup:startBackupJob' names an integration with 'backup', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it" — fixed text, with the state name ('StartBackupJob') and the integration ARN taken from the state machine definition. AWS emits no such cause for a routable integration.

The negative half of the same check passes against the emulator: PutTargets with the lambda-trusted role returns FailedEntryCount 0, the 30s window sees no new execution, and ListBackupJobs is empty — so the divergence is isolated to the integration call made by the state machine the correctly-trusted role starts.

## Steps to Reproduce
1. backup.UpdateRegionSettings to opt DynamoDB in (if not already); dynamodb.CreateTable -> wait table_exists; backup.CreateBackupVault.
2. iam.CreateRole trusting backup.amazonaws.com with AWSBackupServiceRolePolicyForBackup (the IamRoleArn passed to the job), and iam.CreateRole trusting states.amazonaws.com with backup:StartBackupJob + iam:PassRole (the state machine's role).
3. sfn.CreateStateMachine(type=STANDARD) with a single Task state: Resource 'arn:aws:states:::aws-sdk:backup:startBackupJob', Parameters {"BackupVaultName.$": "$.vaultName", "ResourceArn.$": "$.resourceArn", "IamRoleArn.$": "$.iamRoleArn"}, End true.
4. iam.CreateRole trusting events.amazonaws.com with an inline policy allowing states:StartExecution on that state machine ARN (the correctly-trusted invoke role).
5. events.PutRule(State=ENABLED, EventPattern={"source":["sim2real.test.<rule>"],"detail-type":["TriggerBackup"]}) and events.PutTargets(Targets=[{Id:'1', Arn:<state machine arn>, RoleArn:<invoke role arn>, Input: {"vaultName":...,"resourceArn":<table arn>,"iamRoleArn":<backup role arn>}}]).
6. events.PutEvents with that Source/DetailType, then poll sfn.ListExecutions(stateMachineArn=<sm>) -> AWS yields a SUCCEEDED execution; the emulator yields executions that go RUNNING then FAILED (States.Runtime, 'names an integration with backup, which is not one of the services this emulator makes a Task state's integration call to').
7. backup.ListBackupJobs(ByResourceArn=<table arn>) -> AWS reports a non-failed job; the emulator reports none.

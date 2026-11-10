Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.backup-restore-job-event-drives-sqs-queue-message backup__events.restore-job-completed-event-triggers-rule
Issue-Key: backup__events.backup-restore-job-event-drives-sqs-queue-message|_poll_sqs_for_event#19|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Key: backup__events.restore-job-completed-event-triggers-rule|_wait_for_message#20|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Commit: eebab1fc03169ac9ede9275cc690165ff01e833c

# backup__events: no "Restore Job State Change" event is ever published for a StartRestoreJob, so every restore-job EventBridge-to-SQS delivery check hangs to the harness cap

> _Note: this issue was identified against the code base at commit `eebab1fc03169ac9ede9275cc690165ff01e833c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartRestoreJob

## Description
One root cause behind the two net-new failing checks of the `backup__events` group (the group's other 14 failures are marked ALREADY FILED for the ListCopyJobs stub, or OWNED by 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md / 20261107T205257Z-... / 20261024T031359Z-... / 20261109T171600Z-... / 20261108T153446Z-..., and are not re-filed here).

=== Cause 1 of 1: the restore-job lifecycle publishes no "Restore Job State Change" event onto the EventBridge default bus, so a rule matching it delivers nothing to its SQS target ===
AWS: recorded behavior only — the event envelope is not in either service's Smithy model. Smithy does model the surface the event mirrors: StartRestoreJob (PUT /restore-jobs, 200) returns RestoreJobId, and DescribeRestoreJob (GET /restore-jobs/{RestoreJobId}) reports Status (enum RestoreJobStatus), RecoveryPointArn, SourceResourceArn, CreatedResourceArn and ResourceType. AWS Backup natively emits an event onto the account's default event bus at EACH restore-job lifecycle transition — not only the terminal one — with the standard envelope (version, id, detail-type, source, account, time, region, resources) where source == "aws.backup" and detail-type == "Restore Job State Change", and a `detail` carrying at least restoreJobId, status, sourceResourceArn and createdResourceArn. The detail's values agree with DescribeRestoreJob for the same job: detail.restoreJobId == RestoreJobId, detail.status == Status (RUNNING on the first delivered event, then the terminal COMPLETED), detail.sourceResourceArn == the ARN of the resource the recovery point was taken from, detail.createdResourceArn == CreatedResourceArn. An ENABLED rule whose EventPattern is {"source":["aws.backup"],"detail-type":["Restore Job State Change"]} delivers that envelope UNCHANGED to each configured target, including an SQS queue whose resource policy allows events.amazonaws.com with an aws:SourceArn condition on the rule ARN, so ReceiveMessage on that queue yields a body that parses as the envelope.
Emulator: nothing is ever published for a restore job. The restore-job lifecycle writes job state only; there is no emission onto the EventBridge default bus anywhere in it. Both checks therefore sit in their SQS receive loop with no message ever arriving and are reported as `Error: Check timed out after 180.0s` — the wait is on the emulator's own missing emission, not on a runner fault: the queue, the rule, the target binding and the queue policy were all created successfully in the same run, and the checks' own StartRestoreJob call returned a RestoreJobId.
Why: a client built on Backup restore eventing observes absolutely nothing and cannot distinguish "not delivered" from "not yet". Notify-on-restore-complete, a queue or Lambda consumer that reacts to restore status, correlation of the event's restoreJobId/createdResourceArn against DescribeRestoreJob, and any fan-out keyed on detail-type "Restore Job State Change" all hang with no error. Any suppression/negative test built on the same bus also passes vacuously against an emulator that delivers nothing at all.
Fix: the restore-job lifecycle's event-emission path in the backup crate (the StartRestoreJob handler and the restore-job state progression in crate::state) must publish a state-change event onto the EventBridge default bus at every restore-job transition, with AWS's source, detail-type and detail members, whose values are the same ones DescribeRestoreJob reports for that job; the EventBridge rule-matching and target-dispatch path must then deliver one matched event to all of the rule's targets. Note for the fixer: open issue 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md names "Restore Job State Change" in the detail-type set of its cause 1, but it does NOT carry these two checks' replay keys (its id list covers Backup Job / Copy Job / Recovery Point / restore-from-nonexistent-recovery-point deliveries), so neither check was marked OWNED by it and this issue is what gates them; fixing that issue's cause 1 for the restore-job lifecycle is expected to close this one, and the two check ids above are the ones to re-replay.
Traces: backup__events.backup-restore-job-event-drives-sqs-queue-message, backup__events.restore-job-completed-event-triggers-rule.

## Evidence
Suite: backup__events 40/56 (71%), 16 failed; 14 of those are ALREADY FILED or OWNED, leaving these 2 net-new.

[mismatch] backup__events.backup-restore-job-event-drives-sqs-queue-message: Error: Check timed out after 180.0s.
  Setup that succeeded in the same run: dynamodb.CreateTable -> ACTIVE; backup.CreateBackupVault; iam.CreateRole trusted by backup.amazonaws.com with AWSBackupServiceRolePolicyForBackup + ...ForRestores; events.PutRule(State=ENABLED, EventPattern={"source":["aws.backup"],"detail-type":["Restore Job State Change"]}); sqs.CreateQueue + SetQueueAttributes with a policy allowing events.amazonaws.com under ArnEquals aws:SourceArn=<rule arn>; events.PutTargets binding the queue; StartBackupJob -> DescribeBackupJob COMPLETED; StartRestoreJob -> a RestoreJobId.
  Expected (AWS): sqs.ReceiveMessage yields an envelope with source=='aws.backup', detail-type=='Restore Job State Change', detail.restoreJobId == the returned RestoreJobId, detail.status reaching 'COMPLETED', detail.sourceResourceArn == the source table ARN, and detail.createdResourceArn == DescribeRestoreJob's CreatedResourceArn.
  Actual (emulator): no message ever arrives; the check is killed in its receive loop.

[mismatch] backup__events.restore-job-completed-event-triggers-rule: Error: Check timed out after 180.0s.
  Same shape (rule pattern {"source":["aws.backup"],"detail-type":["Restore Job State Change"]} -> SQS target; StartBackupJob to COMPLETED, then StartRestoreJob with Metadata={'TargetTableName': <name>} and ResourceType='DynamoDB').
  Expected (AWS): a delivered envelope whose detail.restoreJobId matches and whose detail.status is terminal, with source=='aws.backup', detail-type=='Restore Job State Change' and detail.status=='COMPLETED'; AWS delivers a status='RUNNING' event first, so the consumer must select for the terminal one.
  Actual (emulator): nothing is delivered for either transition.

## Steps to Reproduce
1. dynamodb.CreateTable (single HASH key, PAY_PER_REQUEST) and wait for ACTIVE; take the TableArn.
2. backup.CreateBackupVault(<fresh name>); iam.CreateRole trusted by backup.amazonaws.com with AWSBackupServiceRolePolicyForBackup and AWSBackupServiceRolePolicyForRestores attached.
3. events.PutRule(Name=<rule>, State='ENABLED', EventPattern='{"source":["aws.backup"],"detail-type":["Restore Job State Change"]}').
4. sqs.CreateQueue; read its QueueArn; sqs.SetQueueAttributes with a Policy allowing Principal {"Service":"events.amazonaws.com"} sqs:SendMessage on that queue under Condition ArnEquals aws:SourceArn == <the rule ARN>; events.PutTargets(Rule=<rule>, Targets=[{Id:'target-1', Arn:<queue arn>}]).
5. backup.StartBackupJob(BackupVaultName=<vault>, ResourceArn=<table arn>, IamRoleArn=<role>) and poll DescribeBackupJob to COMPLETED; take its RecoveryPointArn.
6. backup.StartRestoreJob(RecoveryPointArn=<rp>, Metadata={'TargetTableName': <new name>}, IamRoleArn=<role>, ResourceType='DynamoDB') and note the RestoreJobId.
7. sqs.ReceiveMessage(QueueUrl, MaxNumberOfMessages=10, WaitTimeSeconds=10) in a loop. AWS delivers at least two envelopes for that restoreJobId (detail.status 'RUNNING' then 'COMPLETED') with source 'aws.backup' and detail-type 'Restore Job State Change'; the emulator delivers nothing and the loop runs to its deadline.
8. backup.DescribeRestoreJob(RestoreJobId) and compare RestoreJobId / Status / CreatedResourceArn with the delivered event's detail.restoreJobId / detail.status / detail.createdResourceArn — they must agree.

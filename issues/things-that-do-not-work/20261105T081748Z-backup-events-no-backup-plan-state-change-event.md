Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.backup-framework-compliance-change-event
Issue-Key: backup__events.backup-framework-compliance-change-event|_wait_for_event_matching#26|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Commit: c2adee7ecabe7e23afd8d863b717702657c4c27a

# backup__events: no 'Backup Plan State Change' event is ever published for CreateBackupPlan/UpdateBackupPlan, so a rule scoped to that detail-type never fires

> _Note: this issue was identified against the code base at commit `c2adee7ecabe7e23afd8d863b717702657c4c27a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateBackupPlan

## Description
Cause 1 of 1: AWS Backup plan mutations publish no EventBridge event in this emulator, so an EventBridge rule matching the documented 'Backup Plan State Change' detail-type never receives anything and any check that waits on it burns its whole budget. Failing check id: backup__events.backup-framework-compliance-change-event.

AWS: recorded behavior only — the Smithy model for com.amazonaws.backup#UpdateBackupPlan models only the synchronous response (BackupPlanId / BackupPlanArn / CreationDate / VersionId / AdvancedBackupSettings) and says nothing about eventing; eventing is AWS Backup's documented EventBridge event catalog. Per that catalog AWS Backup publishes, on the default event bus, an event with "source": "aws.backup" and "detail-type": "Backup Plan State Change" whenever a backup plan is created, modified or deleted. The envelope's "detail" names the plan the mutation was performed on ("backupPlanId" = the BackupPlanId of the plan the request carried) and the transition ("state" = CREATED / MODIFIED / DELETED — MODIFIED for an UpdateBackupPlan). Those detail values are taken from the request and the emulator's stored plan, never from any single recording; the fixed parts of the contract are the source token, the detail-type string and the detail member names/state vocabulary. The same catalog has NO framework or compliance detail-type at all, so creating and updating a Backup Audit Manager framework must publish nothing — that negative half of the check cannot even be reached today because the positive half never converges.

Emulator: the plan is written and the 200 response is correct, but nothing is ever delivered to the rule's target. A rule put with EventPattern {"source":["aws.backup"],"detail-type":["Backup Plan State Change"]} and an SQS target (queue policy allowing events.amazonaws.com for that rule ARN) receives no message; polling the queue for 90s yields nothing and the harness aborts the check with 'Check timed out after 60.0s'. The handler returns straight from the state write with no publish step (see Traces).

Why: a client that drives plan governance off EventBridge — audit/notification pipelines, drift detection, 'plan was modified' alarms, Lambda/SQS/SNS fan-out on plan lifecycle — observes nothing at all against the emulator. Worse, the failure is silent and indistinguishable from 'no change happened': the mutation succeeds, so a test or workflow that waits for the plan event simply hangs until its own timeout instead of failing fast. It also makes the negative assertion (that AWS emits no framework/compliance event) unverifiable, so an emulator that later invented such an event would not be caught.

Fix: the backup crate's plan-mutation handlers and the state they write — CreateBackupPlan, UpdateBackupPlan and DeleteBackupPlan — must publish a 'Backup Plan State Change' event (source aws.backup, detail.backupPlanId from the stored plan, detail.state CREATED/MODIFIED/DELETED) onto the emulator's EventBridge default bus, through the same publication seam the other aws.backup events are meant to use; framework create/update must continue to publish nothing. Related open issues cover sibling event families but not this one and do not carry this check's replay key: 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md (backup/copy/restore/recovery-point state changes), 20261030T103029Z-backup-events-no-copy-job-state-change-event.md and 20261102T091831Z-backup-events-no-aws-backup-state-change-event.md.

Traces: backup__events.backup-framework-compliance-change-event. Handler with no publication step: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/update_backup_plan.rs:167-238 (state write then Ok(UpdateBackupPlanOutput{..}), no event emitted). Framework path, which correctly emits nothing and does settle, for contrast: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/create_framework.rs, update_framework.rs, and state.rs settle_frameworks / FRAMEWORK_SETTLE_AFTER (3s), so the check's DeploymentStatus wait is not what stalls.

## Evidence
Expected (AWS, documented AWS Backup EventBridge event catalog): after UpdateBackupPlan on plan <BackupPlanId>, a message arrives on the SQS target of a rule with EventPattern {"source":["aws.backup"],"detail-type":["Backup Plan State Change"]} whose body is an EventBridge envelope with "source":"aws.backup", "detail-type":"Backup Plan State Change", "detail":{"backupPlanId":"<the id returned by CreateBackupPlan>", "state":"MODIFIED", ...}. Separately, CreateFramework + UpdateFramework produce NO event for any framework/compliance detail-type.

Actual (emulator): PutRule, PutTargets, CreateBackupVault, CreateFramework, DescribeFramework (DeploymentStatus reaches COMPLETED), UpdateFramework, CreateBackupPlan and UpdateBackupPlan all answer 200, but the plan-rule queue never receives any message. ReceiveMessage (MaxNumberOfMessages=10, WaitTimeSeconds=10) returns no Messages for the full wait; the check is killed by the harness: 'Error: Check timed out after 60.0s'. Verdict recorded as mismatch for backup__events.backup-framework-compliance-change-event. The assertions on detail-type == 'Backup Plan State Change' and detail.state == 'MODIFIED' are therefore never reached.

## Steps to Reproduce
1. events.put_rule(Name=<rule>, EventPattern='{"source":["aws.backup"],"detail-type":["Backup Plan State Change"]}').
2. sqs.create_queue; read its QueueArn; set a queue Policy allowing Principal {"Service":"events.amazonaws.com"} sqs:SendMessage on that ARN with Condition ArnEquals aws:SourceArn = the rule ARN; events.put_targets(Rule=<rule>, Targets=[{Id:'t1', Arn:<queue arn>}]).
3. backup.create_backup_vault(BackupVaultName=<vault>).
4. backup.create_backup_plan(BackupPlan={BackupPlanName:<name>, Rules:[{RuleName:'rule1', TargetBackupVaultName:<vault>, ScheduleExpression:'cron(0 5 * * ? *)', StartWindowMinutes:60, CompletionWindowMinutes:120}]}) -> note BackupPlanId.
5. backup.update_backup_plan(BackupPlanId=<id>, BackupPlan={... ScheduleExpression:'cron(0 6 * * ? *)', StartWindowMinutes:90, CompletionWindowMinutes:180}).
6. Poll sqs.receive_message on the queue for up to 90s for an envelope with detail.backupPlanId == <id>. Observed: nothing ever arrives (check times out). Expected: one envelope with detail-type 'Backup Plan State Change' and detail.state 'MODIFIED'.
7. (Second half, currently unreachable) create a rule matching any framework/compliance detail-type on source aws.backup with its own queue, run CreateFramework + UpdateFramework, and confirm NO message arrives on it.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.cli-create-backup-plan-and-verify-eventbridge-rule-target
Issue-Key: backup__events.cli-create-backup-plan-and-verify-eventbridge-rule-target|_poll_sqs_for_job#25|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Commit: 4507d6d6dd116d6bbaed085269ed053454966346

# The CLI-driven Backup plan + EventBridge rule/target workflow never converges: no 'Backup Job State Change' event is ever delivered to the rule's SQS target

> _Note: this issue was identified against the code base at commit `4507d6d6dd116d6bbaed085269ed053454966346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
One root cause.

Cause 1 of 1: an on-demand backup job started against a vault created through the CLI never produces a native "Backup Job State Change" event on the event bus, so an EventBridge rule matching it with an SQS target receives nothing and any client waiting on the event waits forever.

AWS: StartBackupJob (Smithy: PUT /backup-jobs, success 200, required BackupVaultName/ResourceArn/IamRoleArn, returns BackupJobId and RecoveryPointArn) starts an asynchronous job, and AWS Backup publishes that job's transitions to the account's default event bus as notifications with source "aws.backup" and detail-type "Backup Job State Change", whose detail carries at least backupJobId, state and backupVaultName. An EventBridge rule whose pattern is {"source":["aws.backup"],"detail-type":["Backup Job State Change"]} with an SQS target therefore delivers the event envelope (source, detail-type, detail) into the queue within the job's lifetime. The Smithy model describes no eventing, so the envelope and detail members asserted here are recorded behavior only (source == "aws.backup", detail-type == "Backup Job State Change", detail.backupJobId == the BackupJobId StartBackupJob returned).

Emulator: the whole estate is accepted — CLI backup create-backup-vault, backup create-backup-plan, events put-rule and events put-targets all succeed, and StartBackupJob returns a BackupJobId — but no message matching that job id ever arrives on the SQS target. The check polls the queue and is killed at the harness's cap, verdict "Check timed out after 60.0s"; no event is produced for the job at any point. This is the same missing publication reported in open issue 20261024T031359Z-backup-events-aws-backup-job-lifecycle-and-eventing-diverge-jobs.md (jobs settle COMPLETED with no StatusMessage and no state-change event reaches EventBridge), which the replay marks as owning the other two eventing checks but not this one; this check reaches the gap through the CLI/plan route and must be re-replayed against whatever fix closes it.

Why: clients attach operational automation (SQS queues, SNS fan-out, Lambda) to Backup Job State Change events to learn that a backup finished or failed. Against this emulator none of that automation ever fires: the rule and target are created and look healthy, no error is returned anywhere, and the consumer simply blocks until its own timeout instead of receiving a terminal state. Nothing in the API surface tells the client the notification path is inert.

Fix: the backup job lifecycle (the StartBackupJob handler and the job-state machine in the backup service's state) must emit a notification for each job transition onto the events service's default bus with source "aws.backup", detail-type "Backup Job State Change" and a detail naming backupJobId, backupVaultName, resourceArn and state; the events service's rule-matching and target-delivery path must then route it to the rule's configured targets (SQS here).

Traces: backup__events.cli-create-backup-plan-and-verify-eventbridge-rule-target (killed at the 60s cap while polling the SQS target for the job's event).

## Evidence
Check backup__events.cli-create-backup-plan-and-verify-eventbridge-rule-target, verdict [mismatch]: "Check timed out after 60.0s".

Expected (AWS): after `aws backup create-backup-vault`, `aws backup create-backup-plan`, `aws events put-rule --event-pattern '{"source":["aws.backup"],"detail-type":["Backup Job State Change"]}'` and `aws events put-targets` with an SQS target, StartBackupJob returns a BackupJobId and the target queue receives a message whose body satisfies body["source"] == "aws.backup", body["detail-type"] == "Backup Job State Change" and body["detail"]["backupJobId"] == that BackupJobId.

Actual (emulator): every setup call succeeds and StartBackupJob returns a BackupJobId, but ReceiveMessage on the target queue never yields a message correlating with that job id; the check is still polling when the harness kills it at 60s. The two sibling eventing checks of the same service show the same absence from the API side — backup__events.backup-job-failed-state-change-event-detail ends with "backup job ended in COMPLETED, expected FAILED" and backup__events.backup-job-state-change-multiple-targets-fanout also exhausts the cap waiting for the event (both already owned by 20261024T031359Z-backup-events-aws-backup-job-lifecycle-and-eventing-diverge-jobs.md).

## Steps to Reproduce
1. Boot the emulator (backup, events, sqs, dynamodb, iam).
2. Create a DynamoDB table and an IAM role AWS Backup can assume; create an SQS queue whose policy allows events.amazonaws.com to SendMessage.
3. `aws backup create-backup-vault --backup-vault-name <vault>` and `aws backup create-backup-plan --backup-plan '{"BackupPlanName":...,"Rules":[{"RuleName":"daily","TargetBackupVaultName":"<vault>","ScheduleExpression":"cron(0 5 * * ? *)","StartWindowMinutes":60,"CompletionWindowMinutes":120}]}'`.
4. `aws events put-rule --name <rule> --event-pattern '{"source":["aws.backup"],"detail-type":["Backup Job State Change"]}'` then `aws events put-targets --rule <rule> --targets '[{"Id":"sqs-target","Arn":"<queue arn>"}]'`.
5. Call backup StartBackupJob with the vault, the table ARN and the role ARN; note the returned BackupJobId.
6. Poll ReceiveMessage on the queue for a body with detail.backupJobId == that id. Observe that no such message ever arrives, so the workflow never converges.

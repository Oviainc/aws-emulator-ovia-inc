Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.backup-job-completed-emits-eventbridge-event backup__events.backup-job-eventbridge-detail-type-list-matches-multiple-states backup__events.restore-from-nonexistent-recovery-point-emits-no-state-change-event backup__events.eventbridge-rule-target-role-missing-trust-policy-rejected
Issue-Key: backup__events.backup-job-completed-emits-eventbridge-event|_wait_for_event_matching#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Key: backup__events.backup-job-eventbridge-detail-type-list-matches-multiple-states|_wait_for_terminal_events#14|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Key: backup__events.restore-from-nonexistent-recovery-point-emits-no-state-change-event|_wait_for_event_matching#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: backup__events.eventbridge-rule-target-role-missing-trust-policy-rejected|run#38|AssertionError|assert executions, (
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# backup__events: no aws.backup state-change event is ever delivered to a rule target, and a correctly-trusted rule target role still fails StartBackupJob (2 causes)

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
Two net-new root causes in this service group.

Cause 1 of 2: AWS Backup publishes no 'Backup Job State Change' / 'Copy Job State Change' / 'Restore Job State Change' event to EventBridge, so every rule subscribed to source aws.backup stays silent and both the positive and the negative-delivery contracts hang.
Failing checks: backup__events.backup-job-completed-emits-eventbridge-event, backup__events.backup-job-eventbridge-detail-type-list-matches-multiple-states, backup__events.restore-from-nonexistent-recovery-point-emits-no-state-change-event.
AWS: a backup job that reaches COMPLETED emits an event with source `aws.backup`, detail-type `Backup Job State Change`, a detail carrying the job's real backupJobId/state/resourceArn and a `resources` array holding the recovery-point ARN that DescribeBackupJob reports for the same job; a copy job started from that recovery point emits `Copy Job State Change`, so a rule listing BOTH detail-types receives both on one target. StartRestoreJob from a nonexistent recovery point is refused synchronously, creates no restore job and emits no `Restore Job State Change` event. Recorded behavior only for the event bodies (the Backup Smithy model describes the control plane, not the EventBridge envelopes); the Backup Job State Change contract is the documented AWS Backup EventBridge event.
Emulator: all three checks end `Check timed out after 60.0s` — the target queue never receives any aws.backup event, so the positive deliveries never land and the negative check has no positive control to distinguish "correctly withheld" from "nothing is ever published".
Why: a client that drives backup automation off EventBridge (notify on COMPLETED, fan a copy job out of a recovery point, alarm on FAILED) gets no events at all; worse, a negative-delivery assertion passes vacuously on the emulator and fails on AWS.
Fix: the AWS Backup state machine's event emission — the backup/copy/restore job state transitions in the backup crate must publish the aws.backup envelope onto the default bus, with the detail fields taken from the same state DescribeBackupJob/DescribeCopyJob/DescribeRestoreJob serve. Open issues 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md, 20261024T031359Z-backup-events-aws-backup-job-lifecycle-and-eventing-diverge-jobs.md and 20261030T103029Z-backup-events-no-copy-job-state-change-event.md own neighbouring checks in this group but not these three replay keys.

Cause 2 of 2: an EventBridge rule target whose RoleArn IS correctly trusted by events.amazonaws.com and permitted to call backup:StartBackupJob drives a backup job that reaches FAILED instead of succeeding, so the positive control of the trust-policy differential collapses.
Failing check: backup__events.eventbridge-rule-target-role-missing-trust-policy-rejected.
AWS: with a target role trusting events.amazonaws.com and granting backup:StartBackupJob (plus the pass-role the job needs), a matched event starts a backup job that progresses RUNNING -> COMPLETED; the sibling target whose role omits the events.amazonaws.com trust statement is never assumed and invokes nothing. Recorded behavior only.
Emulator: the correctly-trusted control's executions are observed as `['RUNNING', 'FAILED']` and the assertion `positive control with a correctly-trusted role should have succeeded` fires — the rule-driven StartBackupJob is accepted and then fails, so the check cannot attribute the negative target's non-invocation to the missing trust statement.
Why: an operator cannot tell a genuine IAM trust failure from the emulator failing every rule-driven backup job; remediation automation built on "rule fires StartBackupJob" never completes.
Fix: the rule-target invocation path for backup:StartBackupJob (role assumption plus the StartBackupJob handler and its job state machine in the backup crate) — a job started through an assumed target role must settle the same way one started directly does.
Traces: the four failing check ids listed above.

## Evidence
Cause 1 — Expected: a COMPLETED DynamoDB on-demand backup job delivers an aws.backup 'Backup Job State Change' event to the subscribed SQS target carrying the job's true id/state/resourceArn; a rule listing both 'Backup Job State Change' and 'Copy Job State Change' receives both; StartRestoreJob on a nonexistent recovery point is refused and emits no Restore Job State Change while a real Backup Job State Change on the same rule/target proves the path live. Actual: `Check timed out after 60.0s` on all three — the queue stays empty.
Cause 2 — Expected: executions driven by the correctly-trusted target role reach a successful terminal state (assertion `executions` non-empty after filtering to success). Actual: `AssertionError: positive control with a correctly-trusted role should have succeeded, executions seen: ['RUNNING', 'FAILED']`.

## Steps to Reproduce
Cause 1: create a DynamoDB table, a backup vault and a backup service role; create a default-bus rule with pattern {"source":["aws.backup"],"detail-type":["Backup Job State Change"]} and an SQS target with a queue policy admitting events.amazonaws.com; StartBackupJob and wait for DescribeBackupJob to report COMPLETED; poll the queue — on AWS a message arrives whose detail.backupJobId equals the job id, on the emulator the queue stays empty. Repeat with a rule listing both Backup/Copy Job State Change detail-types, and with StartRestoreJob against a fabricated recovery-point ARN.
Cause 2: create two IAM roles, one trusting events.amazonaws.com and one omitting that trust, both allowing backup:StartBackupJob; put both as targets of one rule; PutEvents a matching event; list the resulting backup jobs — on AWS the trusted role's job reaches a successful terminal state, on the emulator it reaches FAILED.

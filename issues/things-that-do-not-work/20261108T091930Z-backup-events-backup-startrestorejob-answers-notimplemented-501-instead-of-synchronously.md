Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.restore-from-nonexistent-recovery-point-emits-no-state-change-event
Issue-Key: backup__events.restore-from-nonexistent-recovery-point-emits-no-state-change-event|run#25|AssertionError|assert error_code in {"ResourceNotFoundException", "InvalidParameterValueException", "InvalidRequestException"}, (
Issue-Commit: d259c56cebef1294bf0f05bb739042b6fdd3434e

# backup StartRestoreJob answers NotImplemented (501) instead of synchronously rejecting a nonexistent recovery point, so the negative Restore Job State Change non-delivery path is unverifiable

> _Note: this issue was identified against the code base at commit `d259c56cebef1294bf0f05bb739042b6fdd3434e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartRestoreJob

## Description
Cause 1 of 1: StartRestoreJob does not refuse a restore from a recovery point that does not exist; it refuses the call generically as an unimplemented scaffold, so no restore-job negative class is modelled at all.

AWS: Smithy (services/service-com-amazonaws-backup-cryocontrollerusermanager, operation StartRestoreJob, PUT /restore-jobs, code 200) models StartRestoreJob with required members RecoveryPointArn (target ARN) and Metadata (sensitive map), plus IamRoleArn, ResourceType (pattern ^[a-zA-Z0-9\-\_\.]{1,50}$), IdempotencyToken and CopySourceTagsToRestoredResource (default false). Its modelled client errors are InvalidParameterValueException, InvalidRequestException, MissingParameterValueException and ResourceNotFoundException, each HTTP 400. When the RecoveryPointArn names a recovery point that does not exist, AWS resolves the recovery point during the synchronous call and refuses it on the spot with one of those modelled 400 client errors — ResourceNotFoundException, InvalidParameterValueException or InvalidRequestException — and creates no restore job. Where AWS answers InvalidParameterValueException for this class, the message is the fixed text `Cannot find recovery point` (no variable parts). Because no restore job is created, AWS also publishes no `Restore Job State Change` event on the `aws.backup` source, so an EventBridge rule matching that detail-type receives nothing. Recorded behavior only for the exact code AWS selects per resource type: the check accepts any of the three modelled codes, and asserts the fixed message only in the InvalidParameterValueException case.

Emulator: the replay shows StartRestoreJob answering the error code `NotImplemented` with message `operation StartRestoreJob is scaffolded but not implemented in this emulator` (HTTP 501), for a request that is well formed and that AWS would have refused with a modelled 400. The check's assertion fails with `expected StartRestoreJob against a nonexistent recovery point to be rejected synchronously, got error_code='NotImplemented'`. Because the operation never runs, the emulator also models nothing of the surrounding contract this check exercises: that the bad call persists no restore job and that no `aws.backup` / `Restore Job State Change` event is delivered to the rule's SQS target.

NOTE — related open issue: `20261029T175130Z-backup-startrestorejob-stub.md` ("Implement backup StartRestoreJob: unimplemented scaffold stub") covers the same handler, but its Issue-Trace-Ids list only `backup__events.backup-restore-job-event-drives-sqs-queue-message` and `backup__events.restore-job-completed-event-triggers-rule`; it does not carry this check's replay key, and the suite did not mark this check as filed or owned. That issue asks for the happy path (a restore job that is created and emits its state-change events). This issue adds the refusal half of the contract: resolving RecoveryPointArn against state and answering a modelled 400 client error, with no restore job and no Restore Job State Change event, when it does not resolve. Fixing the stub without the refusal path leaves this check failing (it would then succeed where AWS refuses — an overly-permissive divergence).

Why: a client that handles a missing or already-expired recovery point by catching ResourceNotFoundException / InvalidParameterValueException from StartRestoreJob sees an unmodelled `NotImplemented` 501 instead. 501 is not in the operation's error set, so botocore surfaces it as an opaque ClientError that no retry/branch logic matches; restore orchestration that distinguishes "recovery point gone" from "service problem" takes the wrong branch, and a 501 reads as a server-side outage rather than a caller error. Once the stub is implemented without the resolution check, the opposite failure appears: the emulator accepts a restore from a nonexistent recovery point, creating a phantom restore job and (if events are wired) emitting Restore Job State Change events AWS would never send, so negative-path tests pass against the emulator and fail against AWS.

Fix: the StartRestoreJob handler and its validation — resolve RecoveryPointArn against the backup service state before creating any restore job, and return the modelled 400 client error (InvalidParameterValueException with the fixed message `Cannot find recovery point`, or ResourceNotFoundException) when it does not resolve, persisting no restore job and emitting no Restore Job State Change event. The handler file is named in the related stub issue above.

Traces: backup__events.restore-from-nonexistent-recovery-point-emits-no-state-change-event.

## Evidence
Failing check: backup__events.restore-from-nonexistent-recovery-point-emits-no-state-change-event (verdict: mismatch).

Expected (AWS): StartRestoreJob with RecoveryPointArn=arn:aws:backup:<region>:<account>:recovery-point:<uuid-that-does-not-exist>, Metadata={"TargetTableName": <name>}, IamRoleArn=<backup service role>, ResourceType="DynamoDB" is refused synchronously with HTTP 400 and one of the modelled client error codes ResourceNotFoundException / InvalidParameterValueException / InvalidRequestException; when the code is InvalidParameterValueException the message is exactly the fixed text `Cannot find recovery point`. No restore job is created and no event with detail-type `Restore Job State Change` from source `aws.backup` is delivered to a matching EventBridge rule's SQS target.

Actual (emulator): error code `NotImplemented`, HTTP 501, message `operation StartRestoreJob is scaffolded but not implemented in this emulator`. Harness assertion:
  File "check_backup__events_restore_from_nonexistent_recovery_point_emits_no_state_change_event.py", line 86, in run
    assert error_code in {"ResourceNotFoundException", "InvalidParameterValueException", "InvalidRequestException"}
  AssertionError: expected StartRestoreJob against a nonexistent recovery point to be rejected synchronously, got error_code='NotImplemented'

Message parts: in `Cannot find recovery point` the whole string is fixed text — it has no variable parts. In the emulator's message, `StartRestoreJob` is the operation name from the request and the rest is the emulator's own scaffold text; neither the code nor the message is modelled for this operation.

Model citation: Smithy StartRestoreJob — PUT /restore-jobs, success 200; required RecoveryPointArn (ARN), Metadata (sensitive map); errors InvalidParameterValueException(400), InvalidRequestException(400), MissingParameterValueException(400), ResourceNotFoundException(400), ServiceUnavailableException(500). `NotImplemented`/501 is not in that set.

Note: the check's positive control (a real StartBackupJob whose native `Backup Job State Change` event reaches the SQS target) runs before this assertion, so the rule/target/queue-policy wiring is exercised by the same check; the failure above is on the StartRestoreJob call itself.

## Steps to Reproduce
1. Boot the backup emulator (plus events, sqs, iam, dynamodb, sts).
2. Create a DynamoDB table, a backup vault, an IAM role trusted by backup.amazonaws.com, and an SQS queue.
3. PutRule with event pattern {"source":["aws.backup"],"detail-type":["Restore Job State Change","Backup Job State Change"]}, allow events.amazonaws.com to sqs:SendMessage on the queue, and PutTargets the queue as the rule target.
4. Call StartRestoreJob with RecoveryPointArn=arn:aws:backup:<region>:<account>:recovery-point:00000000-0000-0000-0000-000000000000, Metadata={"TargetTableName":"<new-name>"}, IamRoleArn=<role arn>, ResourceType="DynamoDB".
5. Observe error code `NotImplemented` (HTTP 501) instead of a modelled 400 client error (ResourceNotFoundException / InvalidParameterValueException `Cannot find recovery point` / InvalidRequestException).
6. After implementing the handler, re-check that the same call is still refused with a modelled 400, that no restore job is persisted, and that draining the queue for ~30 s yields no message with detail-type `Restore Job State Change`.
7. Re-replay backup__events.restore-from-nonexistent-recovery-point-emits-no-state-change-event to confirm.

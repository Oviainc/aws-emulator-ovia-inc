Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.cancel-completed-export-task-fails
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.cancel-completed-export-task-fails|run#11|AssertionError|assert code == "InvalidOperationException", (
Issue-Commit: 07569be71b553a95f2f3aaa6a23a9103793b0794

# logs CancelExportTask on a settled (COMPLETED/FAILED) export task answers NotImplemented instead of refusing with InvalidOperationException "The specified export task has already finished"

> _Note: this issue was identified against the code base at commit `07569be71b553a95f2f3aaa6a23a9103793b0794`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelExportTask

## Description
One net-new root cause.

Cause 1 of 1: CancelExportTask never reaches its terminal-state refusal path, so a cancel of an already-settled export task is answered with the scaffold's NotImplemented instead of AWS's InvalidOperationException.
Failing check ids: logs.cancel-completed-export-task-fails

AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) models CancelExportTask with input member `taskId` (target ExportTaskId, required, @length min 1 / max 512) and the error set {InvalidOperationException (client, HTTP 400), InvalidParameterException (client, HTTP 400), ResourceNotFoundException (client, HTTP 400), ServiceUnavailableException (server, HTTP 500)}. InvalidOperationException is the modeled error for "the operation is not valid on the specified resource" — i.e. the task exists but is no longer cancellable. Recorded behavior: for an export task that has reached a settled terminal state (COMPLETED, or FAILED), AWS refuses the cancel with HTTP 400, error code `InvalidOperationException` and the message `The specified export task has already finished`. That message is entirely FIXED text for this error class — it carries no value taken from the request or from state (no task id, no log group, no bucket name is interpolated). AWS also leaves the task untouched: a subsequent DescribeExportTasks(taskId=...) still reports the same status code it had before the rejected cancel.

Emulator: the CancelExportTask handler is still the unmodified scaffold, so every call — including this terminal-state one — is answered with error code `NotImplemented` and the message `operation CancelExportTask is scaffolded but not implemented in this emulator` (HTTP 501). The check got as far as the cancel: CreateExportTask and DescribeExportTasks answered, the task settled to a terminal state, and only the cancel diverged. Replay: `AssertionError: expected InvalidOperationException cancelling a COMPLETED task, got 'NotImplemented'`.

Why: a client that cancels an export task it has not yet observed as finished cannot distinguish "already finished, nothing to do" from "this API does not exist". Code that catches InvalidOperationException to treat the race as benign instead sees an unmodeled NotImplemented, which botocore surfaces as an unexpected 501 — so retry/backoff and idempotent-cleanup paths abort rather than proceeding, and the "status must not change" invariant of a rejected cancel is never exercised at all.

Fix: the CancelExportTask operation handler and its state lookup — resolve `taskId` against the export-task store, refuse an absent id with ResourceNotFoundException, refuse a task whose status code is already terminal with InvalidOperationException carrying the fixed message above, cancel only a task still in a non-terminal state, and in every refusal leave the stored task status unmodified.

Related open issue (does NOT carry this check): 20261025T192833Z-logs-cancelexporttask-stub.md asks for CancelExportTask to be implemented and lists five other units (logs.cancel-export-task-then-describe-shows-cancelled, logs.create-export-task-lifecycle, logs.export-task-cancel-ordering, logs.export-task-cancel-transitions-to-cancelled, logs.export-task-unknown-id-semantics). logs.cancel-completed-export-task-fails is not among them and was not marked owned or already-filed by the replay, so this issue carries its replay key and states the terminal-state refusal contract that must hold once the handler exists.

Traces: logs.cancel-completed-export-task-fails (verdict: mismatch).

## Evidence
Expected (real AWS, per the recorded check oracle and the Smithy error set): CancelExportTask(taskId=<id of a task whose DescribeExportTasks status.code is COMPLETED or FAILED>) -> HTTP 400, Error.Code = "InvalidOperationException", Error.Message = "The specified export task has already finished" (fixed text, no interpolated values); DescribeExportTasks(taskId=<id>) afterwards still reports the same status.code as before the cancel.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation CancelExportTask is scaffolded but not implemented in this emulator".

Replay detail for logs.cancel-completed-export-task-fails:
  File "check_logs_cancel_completed_export_task_fails.py", line 42, in run
    assert code == "InvalidOperationException", (
  AssertionError: expected InvalidOperationException cancelling a COMPLETED task, got 'NotImplemented'

Smithy citation: CancelExportTask input {taskId: ExportTaskId, required, @length 1..512}; errors InvalidOperationException/InvalidParameterException/ResourceNotFoundException (client, 400) and ServiceUnavailableException (server, 500).

## Steps to Reproduce
1. Boot the emulator and point a botocore `logs` client at it.
2. CreateLogGroup(logGroupName=<unique name>).
3. Create an S3 destination bucket and grant logs.<region>.amazonaws.com s3:GetBucketAcl and s3:PutObject on it (standard export bucket policy).
4. CreateExportTask(logGroupName=<group>, destination=<bucket>, destinationPrefix=<prefix>, fromTime=now-60000, to=now+60000) and keep the returned taskId.
5. Poll DescribeExportTasks(taskId=<taskId>) until status.code is one of COMPLETED / FAILED / CANCELLED; confirm it settled to COMPLETED or FAILED.
6. Call CancelExportTask(taskId=<taskId>).
   Expected: HTTP 400 with Error.Code "InvalidOperationException" and Error.Message "The specified export task has already finished".
   Observed: HTTP 501 with Error.Code "NotImplemented" and message "operation CancelExportTask is scaffolded but not implemented in this emulator".
7. Call DescribeExportTasks(taskId=<taskId>) again and confirm status.code is unchanged by the rejected cancel.

Or replay the check directly: logs.cancel-completed-export-task-fails.

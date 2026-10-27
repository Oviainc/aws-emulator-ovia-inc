Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.create-export-task-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.create-export-task-lifecycle|run#26|AssertionError|assert final["status"] == {"code": "CANCELLED", "message": "Export task cancelled by user"}, (
Issue-Commit: 6450b28f30d7a17eb91d6f9fe7c5797a4be7443c

# DescribeExportTasks hardcodes ExportTaskStatus.message to absent, so a cancelled export task reports only a code and never AWS's "Export task cancelled by user" status message

> _Note: this issue was identified against the code base at commit `6450b28f30d7a17eb91d6f9fe7c5797a4be7443c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExportTasks

## Description
One net-new root cause.

Cause 1 of 1: every export task DescribeExportTasks reports carries `status.code` but never `status.message`, because the stored export-task record has no status-message field at all and the projection into the modelled shape hardcodes the member absent. A task that a caller successfully cancelled therefore reports `{'code': 'CANCELLED'}` where AWS reports a code AND the message for that terminal class.
Failing check ids: logs.create-export-task-lifecycle

AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) models `ExportTask$status` -> `ExportTaskStatus`, a structure with TWO members: `code` (target `ExportTaskStatusCode`, enum values CANCELLED | COMPLETED | FAILED | PENDING | PENDING_CANCEL | RUNNING, doc "The status code of the export task") and `message` (target `ExportTaskStatusMessage`, a string, doc "The status message related to the status code"). The model states that a status carries a message explaining its code but does not enumerate the per-code texts, so for the texts themselves: recorded behavior only. The recording shows that after `CancelExportTask` is ACCEPTED (HTTP 200) for a task still PENDING/RUNNING, the task settles and `DescribeExportTasks(taskId=...)` reports `status` exactly equal to `{"code": "CANCELLED", "message": "Export task cancelled by user"}`; the same oracle records the settled-successfully class as `{"code": "COMPLETED", "message": "Completed successfully"}`. Both messages are entirely FIXED text for their status class — neither interpolates any value from the request or from state (no task id, no task name, no log group name, no destination bucket, no prefix, no timestamp appears in them). The message is paired with the code, so the requirement is: whichever terminal code the task settles at, the message AWS words for that class is reported alongside it.

Emulator: `ExportTaskStatus.message` is never populated for any export task in any status. `ExportTaskRecord` stores a `status_code` string and no status message, and the projection of a stored task into the modelled `ExportTask` builds `status: Some(ExportTaskStatus { code: Some(self.status_code.clone()), message: None })` — the member is hardcoded absent rather than derived from the status the task is in. In the replay the whole lifecycle otherwise worked: CreateExportTask answered with a taskId, DescribeExportTasks(taskId) returned exactly that one task, CancelExportTask was ACCEPTED (no error raised), and the task reached a terminal CANCELLED status. Only the status payload diverged: `AssertionError: CancelExportTask succeeded but task ended as {'code': 'CANCELLED'}` — i.e. the response omitted `message` entirely where AWS carries `"Export task cancelled by user"`.

Why: `status.message` is the only field that tells a client WHY an export task reached the code it did, and clients compare or display the whole status object. Code that asserts the status equals AWS's `{code, message}` pair, that renders "cancelled by user" versus "cancelled by the service" to an operator, or that reads `status['message']` to distinguish a user cancel from a failure reason, gets a KeyError or a false mismatch against every export task this emulator reports — even though the cancel itself succeeded and the code is right. Because the member is unconditionally absent, the same gap hides the FAILED reason too: a failed export reports a bare `{'code': 'FAILED'}` with no indication of the cause, so failure-diagnosis paths have nothing to read.

Fix: the export-task state record and its projection into the modelled `ExportTask` — give `ExportTaskRecord` the status message that belongs to its status (or derive it from `status_code` at projection time) so that the cancel transition stamps the user-cancel message when `CancelExportTask` moves an active task to CANCELLED, the clock-driven completion stamps the success message when a running task reaches COMPLETED, and a failed export carries its failure reason; then report that stored message as `ExportTaskStatus.message` instead of the hardcoded absent member. DescribeExportTasks itself needs no new logic once the record carries the message.

Related open issues (neither carries this check's replay key, and neither covers this status-message omission): 20261025T192833Z-logs-cancelexporttask-stub.md lists logs.create-export-task-lifecycle among the units it blocks, but it asks only for CancelExportTask to stop answering 501 — the cancel now succeeds and the handler is implemented, and this failure is downstream of that, in what DescribeExportTasks reports afterwards. 20261026T102156Z-logs-cloudwatch-logs-export-tasks-complete-but-write-no.md is about export tasks writing no in-window events to S3, a different concern from the reported status payload. 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md likewise concerns the export actually running, not the status message.

Traces: logs.create-export-task-lifecycle (verdict: mismatch).

## Evidence
Expected (real AWS, per the recorded check oracle and the Smithy ExportTaskStatus shape): after CancelExportTask(taskId=<id>) is accepted for a task in PENDING/RUNNING, DescribeExportTasks(taskId=<id>)["exportTasks"][0]["status"] == {"code": "CANCELLED", "message": "Export task cancelled by user"}. Both key/value pairs are present; "Export task cancelled by user" is fixed text for the user-cancel class with no interpolated request or state values. The sibling recorded class is {"code": "COMPLETED", "message": "Completed successfully"} (also wholly fixed text).

Actual (emulator): DescribeExportTasks reports status == {"code": "CANCELLED"} — the modelled member `message` is absent. The HTTP status of every call in the lifecycle was 2xx; no error code was returned. This is a response-shape divergence, not a refusal.

Replay detail for logs.create-export-task-lifecycle:
  File "check_logs_create_export_task_lifecycle.py", line 142, in run
    assert final["status"] == {"code": "CANCELLED", "message": "Export task cancelled by user"}, (
  AssertionError: CancelExportTask succeeded but task ended as {'code': 'CANCELLED'}

Smithy citation: com.amazonaws.cloudwatchlogs#ExportTaskStatus is a structure with members code (-> ExportTaskStatusCode, enum [CANCELLED, COMPLETED, FAILED, PENDING, PENDING_CANCEL, RUNNING], doc "The status code of the export task.") and message (-> ExportTaskStatusMessage, string, doc "The status message related to the status code."); com.amazonaws.cloudwatchlogs#ExportTask$status targets ExportTaskStatus.

Code citation (where the member is hardcoded absent):
  services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/state.rs:1852-1855 — status: Some(crate::ops::types::ExportTaskStatus { code: Some(self.status_code.clone()), message: None })
  services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/state.rs:1719 — pub struct ExportTaskRecord { .. } holds status_code but no status message field
  services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/state.rs:1811-1816 — ExportTaskRecord::cancel(now_ms) sets status_code = CANCELLED and the completion time, and stamps no message
  services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/cancel_export_task.rs:205 — task.cancel(now) is the accepted-cancel transition this status is read back from

## Steps to Reproduce
1. Boot the emulator and point botocore `logs`, `s3` and `sts` clients at it.
2. CreateLogGroup(logGroupName=<unique name>); CreateLogStream(logGroupName=<group>, logStreamName=<unique name>); PutLogEvents one event into that stream.
3. DescribeLogGroups(logGroupNamePrefix=<group>) and keep logGroups[0].arn.
4. Create an S3 destination bucket and PutBucketPolicy granting logs.<region>.amazonaws.com s3:GetBucketAcl on arn:aws:s3:::<bucket> and s3:PutObject on arn:aws:s3:::<bucket>/* (standard export bucket policy, conditioned on aws:SourceAccount and the log group ARN).
5. CreateExportTask(logGroupName=<group>, **{"from": 0}, to=<now_ms>, destination=<bucket>) and keep the returned taskId.
6. DescribeExportTasks(taskId=<taskId>) and confirm exactly that one task is returned.
7. Call CancelExportTask(taskId=<taskId>) promptly, while the task is still PENDING/RUNNING, and confirm it is ACCEPTED (no ClientError).
8. Poll DescribeExportTasks(taskId=<taskId>) until exportTasks[0].status.code is one of CANCELLED / COMPLETED / FAILED, then inspect exportTasks[0].status.
   Expected: status == {"code": "CANCELLED", "message": "Export task cancelled by user"}.
   Observed: status == {"code": "CANCELLED"} — the `message` member is absent.
9. (Same cause, other branch) Let an export task settle without cancelling it and inspect its status: expected {"code": "COMPLETED", "message": "Completed successfully"}, observed {"code": "COMPLETED"} with no message.

Or replay the check directly: logs.create-export-task-lifecycle.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected
Issue-Key: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected|run#28|AssertionError|assert any(k.startswith(f"{first_prefix}/") for k in keys), (
Issue-Commit: 51acfe3f2ac52bf94dbabe00a764942f56e02eae

# logs__s3: an export task reaches COMPLETED while nothing at all is written to the destination bucket, so the one-active-export positive control sees an empty bucket

> _Note: this issue was identified against the code base at commit `51acfe3f2ac52bf94dbabe00a764942f56e02eae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
One net-new root cause in the CloudWatch-Logs-to-S3 export surface, grounded in the single failing behavioral check whose id leads with `logs__s3` and which the replay report marks neither ALREADY FILED nor OWNED. Cause count: 1 — it is numbered below with its own failing check id, and it must be closed before this issue is submitted.

----------------------------------------------------------------
Cause 1 of 1: CreateExportTask records a task and lets it advance all the way to COMPLETED, but the export is never executed — no object is ever written under the destinationPrefix (the destination bucket's whole key listing stays empty).
Failing check: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected

AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) models CreateExportTask with logGroupName (@required, @length 1..512, @pattern ^[\.\-_/#A-Za-z0-9]+$), from and to (both @required long, @range min 0), destination (@required ExportDestinationBucket, @length 1..512) and destinationPrefix, returning CreateExportTaskResponse{taskId}; DescribeExportTasks returns ExportTasks whose status.code comes from the ExportTaskStatusCode enum [CANCELLED, COMPLETED, FAILED, PENDING, PENDING_CANCEL, RUNNING]. The operation's own modeled documentation states "Creates an export task so that you can efficiently export data from a log group to an Amazon S3 bucket", that the call is asynchronous, and that destinationPrefix "is the prefix used as the start of the key for every object exported" (default `exportedlogs`). The object payload itself is not described by the model, so for the content this is recorded behavior only: once DescribeExportTasks reports status.code COMPLETED for the task, the destination bucket holds at least one gzip object whose key begins with `<destinationPrefix>/`, carrying the named log group's events whose timestamp falls in the requested window. COMPLETED is a statement that the data landed; a task that would write nothing ends FAILED, not COMPLETED.
Emulator: the task is accepted, DescribeExportTasks does advance it to COMPLETED (the check's `assert first["status"]["code"] == "COMPLETED"` passes), and then a paginated s3:ListObjectsV2 over the whole destination bucket returns ZERO keys — `AssertionError: positive control: the accepted export wrote nothing under exp-first-<n>-<id>/: []`. The empty list is the entire bucket, not just the prefix, so no object was written anywhere. The handler at services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs confirms this by construction: it resolves the log group, validates the window, claims the account's single active-export slot, mints a task id and inserts a `crate::state::ExportTaskRecord` into `store.export_tasks` (lines ~211-276), and there is no path in it — nor in the `materialize_due_export_transitions(now)` clock it relies on to advance status — that reads log events or writes through the S3 data plane. The task's status is simulated; the export is not.
Why: COMPLETED is the only signal a client has that its log data is in S3. A client that polls DescribeExportTasks to a terminal COMPLETED and then reads `<destinationPrefix>/` gets a successful-looking export with no data at all, which is strictly worse than a FAILED task: a retention/archival job, an audit pipeline or an Athena table over the prefix silently archives and reports nothing, and a consumer cannot distinguish an empty export from a correct one. It also makes every downstream export contract unobservable — window filtering, per-stream-prefix selection, and the destination bucket's own encryption applying to the written objects all become ungradeable, so a later regression in any of them is undetectable. In this check specifically it defeats the POSITIVE CONTROL: the check's negative leg (a second concurrent CreateExportTask is refused 400 LimitExceededException with AWS's fixed message `Resource limit exceeded.`) now passes, and the proof that the refusal was a real quota refusal rather than a blanket failure is exactly the accepted task having written something — which it does not.
Fix: the export-task execution path in the cloudwatch-logs crate. The task minted by services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs (and the state/clock in that crate's state.rs that `materialize_due_export_transitions` drives, as surfaced by services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/describe_export_tasks.rs) must actually run the export before it reports COMPLETED: select the named log group's events whose timestamp falls in the requested window (honouring logStreamNamePrefix when present), gzip them, and write them through the S3 data plane under `<destinationPrefix>/` in the named destination bucket so the destination bucket's own default encryption and ownership controls apply. A task that writes nothing must not reach COMPLETED.
Related open issues, NEITHER of which carries this check's current replay key: 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md lists this check id among its trace ids but keys it on the now-passing message assertion (`assert message == "Resource limit exceeded."`) and keys its own Cause 1 on two different checks that timed out before the task ever settled — the task now settles, so the surviving defect has moved from status progression to the write; and 20261026T102156Z-logs-cloudwatch-logs-export-tasks-complete-but-write-no.md describes the same symptom class but under the `logs` service token, keyed on logs.export-task-time-range-window-respected / logs.export-task-honors-time-window-boundaries, i.e. on in-window CONTENT being wrong rather than on the bucket being entirely empty for a simple one-event export. This is not a reopen of a refusal: the call succeeds, the status is right, and the data is missing.
Traces: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected.

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 163/424 passed; service `logs__s3` 0/6, 6 failed — five of the six are marked OWNED by 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md, this one is not).

Cause 1 — accepted export writes nothing (1 check, verdict [mismatch]):
  [mismatch] logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected
    File "check_logs__s3_log_cwl_plumbing_second_concurrent_export_task_rejected.py", line 117, in run
      assert any(k.startswith(f"{first_prefix}/") for k in keys), (
    AssertionError: positive control: the accepted export wrote nothing under exp-first-4-<id>/: []

  Setup reached before the failure: s3.CreateBucket(<b>) plus a bucket policy allowing logs.<region>.amazonaws.com s3:GetBucketAcl on the bucket and s3:PutObject on <b>/* (conditioned on s3:x-amz-acl=bucket-owner-full-control and aws:SourceAccount); logs.CreateLogGroup(<g>) + CreateLogStream(<s>) + PutLogEvents of ONE event at now-60s, confirmed readable through GetLogEvents; CreateExportTask(logGroupName=<g>, from=now-60001, to=now+1000, destination=<b>, destinationPrefix=<first_prefix>).

  Assertions the check PASSED before line 117, which bound the finding:
    - the second, concurrent CreateExportTask was refused with code LimitExceededException;
    - its message was exactly AWS's fixed text `Resource limit exceeded.` (entirely fixed text, no variable parts — it names neither the task id nor its status);
    - the first task reached a terminal status and that status was COMPLETED;
    - the refused call left no task behind (DescribeExportTasks lists nothing with destinationPrefix == <second_prefix>);
    - the refused call wrote nothing under <second_prefix>/.

  Expected at line 117: after the first task reports COMPLETED, s3.ListObjectsV2 over the destination bucket lists at least one key beginning with `<first_prefix>/` (the exported gzip object holding the single in-window event).
  Actual:   the paginated listing of the ENTIRE bucket is `[]` — no object was written under <first_prefix>/ or anywhere else. (The prefix string quoted above is this run's generated value; the requirement is that the accepted export writes under whatever destinationPrefix the request named.)

  Corroboration from the checkout: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs lines 211-276 resolve the log group, refuse `to < from`, claim the account's single active-export slot, mint a task id and insert an ExportTaskRecord — and never read log-event state nor write through the S3 data plane. Status is advanced by `store.materialize_due_export_transitions(now)` alone (line 243), so COMPLETED is reported on a clock, independent of any write.

## Steps to Reproduce
Boot the emulator from this checkout and replay the `logs__s3` check listed in trace_ids.

Cause 1 (an accepted export task writes nothing):
  1. s3.CreateBucket(<b>) (with CreateBucketConfiguration.LocationConstraint for every region but us-east-1), then s3.PutBucketPolicy(<b>) allowing Principal {"Service": "logs.<region>.amazonaws.com"} s3:GetBucketAcl on arn:aws:s3:::<b> and s3:PutObject on arn:aws:s3:::<b>/* with Condition s3:x-amz-acl=bucket-owner-full-control and aws:SourceAccount=<account>.
  2. logs.CreateLogGroup(<g>) + logs.CreateLogStream(<g>, <s>).
  3. logs.PutLogEvents(<g>, <s>, [{timestamp: now_ms-60000, message: "<g> one line"}]); poll logs.GetLogEvents(startFromHead=true) until the event is readable.
  4. logs.CreateExportTask(logGroupName=<g>, from=now_ms-60001, to=now_ms+1000, destination=<b>, destinationPrefix=<p>) -> note taskId.
  5. Poll logs.DescribeExportTasks(taskId=<taskId>) until status.code is terminal.
     -> observe it reports COMPLETED.
  6. s3.ListObjectsV2(Bucket=<b>) (paginate; no Prefix, to prove nothing was written anywhere).
     -> expect at least one key beginning with `<p>/`, holding the event from step 3.
     -> observe `[]` — the bucket is empty.
  7. Negative-leg control that must keep passing while 6 is fixed: with the task from step 4 still non-terminal (verified via DescribeExportTasks), issue a second logs.CreateExportTask for the same account on a client configured with retries max_attempts=1 -> must be refused 400 LimitExceededException with message exactly `Resource limit exceeded.`, must leave no task in DescribeExportTasks, and must write nothing under its own prefix.

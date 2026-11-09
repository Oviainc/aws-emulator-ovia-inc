Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected
Issue-Key: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected|run#28|AssertionError|assert any(k.startswith(f"{first_prefix}/") for k in keys), (
Issue-Commit: 193a7c49386ca4d495ab2a056245bf053bc3ac07

# logs__s3: an export task reaches COMPLETED while writing no object at all under its destinationPrefix, so the one-active-export quota contract cannot be verified (1 cause)

> _Note: this issue was identified against the code base at commit `193a7c49386ca4d495ab2a056245bf053bc3ac07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
One net-new root cause in the CloudWatch Logs (`logs`) export-to-S3 surface, grounded in the failing behavioral check `logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected`. That check is marked neither ALREADY FILED nor OWNED in the replay report: its replay key has moved (it used to fail on the LimitExceededException message assertion and now gets past it), so no open issue carries it. Cause count: 1 — the single cause below must be closed before this issue is submitted.

----------------------------------------------------------------
Cause 1 of 1: an accepted CreateExportTask is advanced all the way to COMPLETED without ever writing an object to the destination bucket, so a successful-looking export lands nothing in S3.
Failing check: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected

AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) models CreateExportTask(logGroupName @required @length 1..512 @pattern ^[\.\-_/#A-Za-z0-9]+$, from @required long @range min 0, to @required long @range min 0, destination @required @length 1..512, destinationPrefix, logStreamNamePrefix, taskName) -> CreateExportTaskResponse{taskId}, with errors InvalidParameterException (400), LimitExceededException (400), OperationAbortedException (400), ResourceAlreadyExistsException (400), ResourceNotFoundException (400) and ServiceUnavailableException (500); DescribeExportTasks -> ExportTasks whose status.code comes from enum ExportTaskStatusCode [CANCELLED, COMPLETED, FAILED, PENDING, PENDING_CANCEL, RUNNING]. The model says nothing about the objects written, so the payload contract is recorded behavior only: once DescribeExportTasks reports status.code == COMPLETED for the task, the destination bucket holds at least one gzip object under `<destinationPrefix>/` carrying the named log group's events whose timestamp falls in [from, to). COMPLETED is the service's statement that the data has been delivered; a COMPLETED export over a log group that has in-window events never leaves the prefix empty.
Emulator: the export task IS accepted, and it now DOES settle — the check's `assert first["status"]["code"] == "COMPLETED"` passes — but nothing is written. s3:ListObjectsV2 over the whole destination bucket returns an EMPTY key list: `AssertionError: positive control: the accepted export wrote nothing under exp-first-2-<id>/: []` (the prefix name and id are this run's generated values, not part of the requirement; the requirement is that the listing is non-empty). The same bucket was created, tagged and given the standard logs.<region>.amazonaws.com GetBucketAcl/PutObject bucket policy by the check, and the single seeded event was confirmed readable through GetLogEvents before the export, so the group was non-empty and the window covered it.
Why: a client that waits for COMPLETED and then reads `<destinationPrefix>/` gets a successful-looking export with its log data missing — strictly worse than a FAILED task, because there is no signal to retry on. A retention/archival or audit pipeline silently archives nothing, Athena-over-the-prefix consumers see zero rows, and an empty export is indistinguishable from a correct one. It also makes the surrounding contract ungradable: this check's whole purpose is the one-active-export-per-account quota (a second CreateExportTask while the first is non-terminal must be refused 400 LimitExceededException with the fixed text `Resource limit exceeded.`, and the refused call must leave no task and no objects behind). The emulator now gets the refusal code AND that fixed message right — both assertions pass, so the invented-message defect recorded as Cause 2 of the open 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md is no longer observable — but the check still cannot certify the quota, because its positive control (the accepted export actually wrote something) fails.
Fix: the export-task execution path in the cloudwatch-logs crate. The task created by the CreateExportTask handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs) must select the named group's events with timestamp in [from, to) out of log-event state, gzip them and write them through the S3 data plane under destinationPrefix BEFORE the export-task state that services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/describe_export_tasks.rs reports is advanced to COMPLETED; a task that wrote nothing must not report COMPLETED. Related issues, NEITHER of which carries this check's replay key: the open 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md lists this check id but keys it on the LimitExceededException message assertion (which now passes) and its Cause 1 describes a task that never left PENDING — the task now settles, so the surviving defect is the missing objects, not the status progression; and the open 20261026T102156Z-logs-cloudwatch-logs-export-tasks-complete-but-write-no.md describes the same symptom class under the `logs` token (its Cause 1, keyed on logs.export-task-time-range-window-respected / logs.export-task-honors-time-window-boundaries, where objects exist but omit the in-window events) — here NO object is written at all. This is not a reopen of a refusal: every call in the failing leg succeeds with HTTP 200.
Traces: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected.

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 317/727 passed), service logs__s3 0/6 (6 failed). Of those six, five are marked OWNED by 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md and skipped; this one is not marked and is the subject of this issue.

  [mismatch] logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected
    File "check_logs__s3_log_cwl_plumbing_second_concurrent_export_task_rejected.py", line 117, in run
      assert any(k.startswith(f"{first_prefix}/") for k in keys), (
    AssertionError: positive control: the accepted export wrote nothing under exp-first-2-c56835d714cc/: []

What passed BEFORE that line, and is therefore established by this same replay:
  - the second CreateExportTask issued while the first task was provably non-terminal was refused with code LimitExceededException (assert at check line ~101 passed);
  - its message was exactly AWS's fixed text `Resource limit exceeded.` (the following assert passed) — entirely fixed text, no variable parts;
  - the refused call left no task behind (DescribeExportTasks for the group lists nothing with that destinationPrefix) and wrote nothing under `<second_prefix>/`;
  - the FIRST export task reached a terminal status and that status was COMPLETED (`assert first["status"]["code"] == "COMPLETED"` passed).

Setup performed by the check before the export: s3.CreateBucket(<dest>) (+ LocationConstraint outside us-east-1), PutBucketTagging, and a bucket policy allowing logs.<region>.amazonaws.com s3:GetBucketAcl on the bucket and s3:PutObject on <dest>/* conditioned on s3:x-amz-acl=bucket-owner-full-control and aws:SourceAccount; logs.CreateLogGroup + CreateLogStream; PutLogEvents of one event at now-60s, polled through GetLogEvents until readable; CreateExportTask(logGroupName=<g>, from=t0-1, to=now+1000, destination=<dest>, destinationPrefix=<first_prefix>).

Expected: after the task reports COMPLETED, s3:ListObjectsV2(Bucket=<dest>) contains at least one key beginning `<first_prefix>/` (a gzip object holding the in-window event).
Actual:   the whole-bucket key listing is `[]` — no object was written anywhere in the destination bucket, while DescribeExportTasks reports status.code COMPLETED.

## Steps to Reproduce
Boot the emulator from this checkout and replay the check in trace_ids (logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected).

Minimal reproduction of the cause on its own:
  1. s3.CreateBucket(<dest>) (with CreateBucketConfiguration.LocationConstraint=<region> outside us-east-1); s3.PutBucketPolicy(<dest>) allowing Principal {"Service": "logs.<region>.amazonaws.com"} s3:GetBucketAcl on arn:aws:s3:::<dest> and s3:PutObject on arn:aws:s3:::<dest>/* with Condition StringEquals {s3:x-amz-acl: bucket-owner-full-control, aws:SourceAccount: <account>}.
  2. logs.CreateLogGroup(logGroupName=<g>); logs.CreateLogStream(logGroupName=<g>, logStreamName=<s>).
  3. logs.PutLogEvents(<g>, <s>, [{timestamp: now_ms-60000, message: "<g> one line"}]); poll logs.GetLogEvents(<g>, <s>, startFromHead=true) until that message is readable.
  4. logs.CreateExportTask(logGroupName=<g>, from=now_ms-60001, to=now_ms+1000, destination=<dest>, destinationPrefix=<p>) -> note taskId.
  5. Poll logs.DescribeExportTasks(taskId=<taskId>) until status.code is terminal -> observe COMPLETED.
  6. s3.ListObjectsV2(Bucket=<dest>) (no Prefix, to see the whole bucket)
     -> expect at least one key starting with "<p>/", whose gunzipped body contains the seeded message.
     -> observe an empty Contents: nothing was written, even though the task reports COMPLETED.

Quota leg that must keep passing once the cause is fixed (it passes today):
  7. Create a first export task as in step 4 and confirm via DescribeExportTasks that it is still non-terminal.
  8. From a client configured with retries max_attempts=1 (standard retry mode classes LimitExceededException as throttling and hides it), call logs.CreateExportTask again for the same account with a different destinationPrefix
     -> expect 400 LimitExceededException with message exactly "Resource limit exceeded." (fixed text, no interpolated task id or status), no task recorded for that call, and nothing written under its prefix.

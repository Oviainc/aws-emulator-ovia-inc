Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption
Issue-Key: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption|_run_export_until_content#12|AssertionError|raise AssertionError(
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# A CloudWatch Logs export task reports COMPLETED but writes no .gz objects to the destination bucket

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issues 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md and 20261028T080827Z-logs-s3-an-export-task-reaches-completed-while.md own other checks of this service and do not carry this replay key.

Cause 1 of 1: the export task's status reaches COMPLETED while the destination prefix stays empty.
AWS: CreateExportTask moves the task through PENDING -> RUNNING -> COMPLETED and, by the time it reports COMPLETED, has written the exported events to the destination bucket as gzip objects under <destinationPrefix>/<export-task-id>/<log-stream-name>/000000.gz, plus the aws-logs-write-test marker. Those objects are encrypted with the destination bucket's own default encryption (so HeadObject reports the bucket's SSE algorithm and, for SSE-KMS, its key ARN) — which is exactly what cannot be checked when no object exists.
Emulator: the task reports COMPLETED in every one of 11 polling attempts while ListObjectsV2 under its prefix returns an empty list: "export task(s) to s3://<bucket>/exp-kms-* reported COMPLETED in every one of 11 attempts but never produced a .gz object -- last attempt's keys under its prefix: []". The status is therefore a lie about work that did not happen; the bucket name and prefix in the message are this run's values.
Why: a client that waits for COMPLETED and then reads the export gets nothing, with no error to act on — log archival silently loses all data, and the destination's encryption contract on exported objects is unverifiable.
Fix: the CloudWatch Logs export-task executor — it must write the gzip objects (and the write-test marker) under the task's destination prefix before reporting COMPLETED, and report FAILED when it cannot.
Traces: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption.

## Evidence
Expected: on COMPLETED, gzip objects under <destinationPrefix>/<task-id>/<stream>/000000.gz carrying the destination bucket's default encryption. Actual: status COMPLETED with an empty prefix across 11 attempts — 'never produced a .gz object -- last attempt's keys under its prefix: []'.

## Steps to Reproduce
1) Create a log group/stream and PutLogEvents some events; create a destination bucket with a default encryption configuration and a log-delivery bucket policy.
2) CreateExportTask into that bucket with a destinationPrefix; poll DescribeExportTasks until COMPLETED.
3) ListObjectsV2 under the prefix: no .gz object exists.

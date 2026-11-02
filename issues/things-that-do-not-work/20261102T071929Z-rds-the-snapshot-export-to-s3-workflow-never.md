Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cli-rds-create-snapshot-export-to-s3-workflow rds__s3.cli-rds-export-invalid-bucket-name-negative rds__s3.rds-export-task-kms-key-missing-permission-negative
Issue-Key: rds__s3.cli-rds-create-snapshot-export-to-s3-workflow|_poll_until#11|Timeout|last = probe()
Issue-Key: rds__s3.cli-rds-export-invalid-bucket-name-negative|_poll_until#9|Timeout|last = probe()
Issue-Key: rds__s3.rds-export-task-kms-key-missing-permission-negative|_wait_for#6|Timeout|last = probe()
Issue-Commit: bf05938345e3744e1c8e3343a2f8d43b66feefe9

# rds: the snapshot-export-to-S3 workflow never converges — export task, source snapshot and source instance never reach the states DescribeExportTasks / DescribeDBSnapshots / DescribeDBInstances must report, so three export checks exhaust their budget without ever reaching an assertion

> _Note: this issue was identified against the code base at commit `bf05938345e3744e1c8e3343a2f8d43b66feefe9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
ONE net-new root cause in the rds emulator, grounded in three failing behavioral checks the replay report did not mark ALREADY FILED or OWNED. Service attribution: the check ids lead with the cross-service group token rds__s3; every call that stalls is an RDS control-plane call, so the issue belongs to rds (bootstrapped).

Cause 1 of 1: an RDS snapshot export to S3 never advances to a state a client can observe, so every workflow built on it hangs.

AWS: Smithy: StartExportTask takes ExportTaskIdentifier, SourceArn, S3BucketName, IamRoleArn and KmsKeyId (all required) plus optional S3Prefix/ExportOnly, and returns an ExportTask carrying Status, PercentProgress, TaskStartTime/TaskEndTime and FailureCause; DescribeExportTasks returns ExportTasks (paginated on Marker/MaxRecords, MaxRecords range 20..100) and refuses an unknown identifier with ExportTaskNotFoundFault (httpError 404, awsQueryError code ExportTaskNotFound). The model does not enumerate the Status values, so for the progression: recorded behavior only — a started export task leaves a non-terminal status and settles on COMPLETE (or FAILED / CANCELED), at which point objects exist under s3://<bucket>/<export task identifier>/; CreateDBSnapshot's snapshot reaches Status "available"; CreateDBInstance's instance reaches DBInstanceStatus "available". The two negative checks additionally grade modeled refusals of StartExportTask itself: KMSKeyNotAccessibleFault (httpError 400, awsQueryError code KMSKeyNotAccessibleFault) when the KMS key's policy denies the caller kms:CreateGrant/kms:DescribeKey, with Message "The KMS key <key reference> doesn't exist or is disabled." — fixed text, with <key reference> echoed back exactly as the request passed it (key id when a key id was passed) — and InvalidS3BucketFault (httpError 400, awsQueryError code InvalidS3BucketFault) for a bucket name RDS will not write to; in both cases no export task is created and no object is written under the attempted prefix.

Emulator: none of those states ever arrives. All three checks terminate with "Check timed out after 60.0s" — the per-check budget is exhausted inside one of the state polls (the source DB instance waiting for DBInstanceStatus "available", the snapshot waiting for Status "available", or DescribeExportTasks waiting for a terminal Status), so not one of them reaches its first assertion. Nothing is therefore observed about the COMPLETE export and its S3 objects, about the KMSKeyNotAccessibleFault refusal and its message, or about the invalid-bucket-name refusal. Hypothesis: the export-task record (and/or the snapshot/instance record it is built on) is created in a non-terminal status and never advanced by any state transition, so a poller can never make progress.

Why: a client relying on AWS's behavior hangs. The documented way to use RDS export is start-export-task followed by describe-export-tasks until Status is terminal; against the emulator that loop never ends, so every script, waiter or IaC step that exports a snapshot to S3 stalls instead of failing fast, and the negative paths that AWS refuses synchronously (an inaccessible KMS key, an invalid bucket name) are never refused — a caller is given no error at all where AWS gives 400 with a named fault.

Fix: the rds crate's export-task state machine — the handlers behind StartExportTask / DescribeExportTasks (and CancelExportTask) plus the status progression of the DB snapshot and DB instance records they depend on. StartExportTask must also perform its up-front validation (KMS key accessibility, S3 bucket name) and refuse with the modeled faults and messages above instead of creating a task that never settles.

Related open issues (neither carries these three check ids, so neither covers them): 20261029T072224Z-rds-s3-startexporttask-is-an-unimplemented-scaffold-stub.md and 20261029T195009Z-rds-s3-5-net-new-root-causes-startexporttask.md own other rds__s3 export checks whose StartExportTask call returned a refusal; these three never get that far, they time out in a state poll, so closing those issues leaves them failing.

Traces: rds__s3.cli-rds-create-snapshot-export-to-s3-workflow, rds__s3.cli-rds-export-invalid-bucket-name-negative, rds__s3.rds-export-task-kms-key-missing-permission-negative.

## Evidence
Expected (AWS): 
- cli-rds-create-snapshot-export-to-s3-workflow: `aws rds create-db-snapshot` then poll DescribeDBSnapshots until Status == "available"; `aws rds start-export-task --export-task-identifier <id> --source-arn <snapshot arn> --s3-bucket-name <bucket> --iam-role-arn <role> --kms-key-id <key>` succeeds; poll DescribeExportTasks until Status is in (COMPLETE, FAILED, CANCELED); the status is COMPLETE and ListObjectsV2(Bucket=<bucket>, Prefix="<export task identifier>/") reports KeyCount > 0.
- rds-export-task-kms-key-missing-permission-negative: StartExportTask against a KMS key whose policy denies kms:CreateGrant/kms:DescribeKey is refused synchronously with Error.Code == "KMSKeyNotAccessibleFault" and Error.Message == "The KMS key <key id as passed> doesn't exist or is disabled.", no objects exist under the attempted prefix, and the positive control with an accessible key reaches Status COMPLETE and writes objects under its own prefix.
- cli-rds-export-invalid-bucket-name-negative: the export against an invalid S3 bucket name is refused, and nothing is created.

Actual (replay): all three checks report only `Error: Check timed out after 60.0s`. No RDS status, error code or error message is produced for any of the three outcomes, because each check is still inside a state poll (DescribeDBInstances / DescribeDBSnapshots / DescribeExportTasks) when the budget expires. The export task identifiers, bucket names, role ARNs and key ids in each run are per-run values and are not part of the requirement; the requirement is that a started export reaches a terminal Status and that the two refusals above are answered synchronously with the named fault codes and messages.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check ids in trace_ids.

Manually (positive workflow):
1. Create an S3 bucket, a KMS key, and an IAM role trusted by export.rds.amazonaws.com with kms:CreateGrant/DescribeKey/Decrypt/GenerateDataKey* on the key and s3:PutObject*/GetObject*/ListBucket/DeleteObject*/GetBucketLocation on the bucket.
2. Create a DB subnet group over two subnets and CreateDBInstance (mysql, db.t3.micro, 20 GiB gp2). Poll DescribeDBInstances until DBInstanceStatus == "available" — observe whether it ever does.
3. CreateDBSnapshot and poll DescribeDBSnapshots until Status == "available".
4. StartExportTask with that snapshot ARN, the bucket, the role and the key id. Poll DescribeExportTasks until Status is COMPLETE/FAILED/CANCELED — observe that no terminal status arrives.
5. On COMPLETE, ListObjectsV2 under the prefix "<export task identifier>/" and confirm KeyCount > 0.

Manually (negative refusals):
6. Repeat with a KMS key whose key policy explicitly denies every principal kms:CreateGrant and kms:DescribeKey: StartExportTask must be refused immediately with KMSKeyNotAccessibleFault and Message "The KMS key <key id as passed> doesn't exist or is disabled.", and no object may exist under the attempted S3 prefix.
7. Repeat with an invalid S3 bucket name: StartExportTask must be refused immediately (InvalidS3BucketFault) and create no export task.

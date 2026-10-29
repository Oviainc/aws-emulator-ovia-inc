Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cli-rds-export-invalid-bucket-name-negative rds__s3.rds-export-task-kms-key-missing-permission-negative rds__s3.cli-rds-create-snapshot-export-to-s3-workflow
Issue-Key: rds__s3.cli-rds-export-invalid-bucket-name-negative|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-kms-key-missing-permission-negative|_wait_for#6|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.cli-rds-create-snapshot-export-to-s3-workflow|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 6074294cbfccbc2257fa0c432afede02752270ab

# rds__s3: 3 net-new root causes — StartExportTask's documented refusals (non-existent S3 bucket, inaccessible KMS key) and its terminal COMPLETE/object-delivery contract are all unavailable

> _Note: this issue was identified against the code base at commit `6074294cbfccbc2257fa0c432afede02752270ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
Three unowned rds__s3 failures, all on snapshot export to S3. The 501 scaffold stub itself is already filed (20261029T072224Z-rds-s3-startexporttask-is-an-unimplemented-scaffold-stub.md, and as one cause of 20261029T195009Z-rds-s3-5-net-new-root-causes-startexporttask.md); what neither of those carries is these three checks' replay keys or the three specific contracts below, each of which a StartExportTask implementation has to satisfy and each of which is separately observable. Confirmed stub: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/start_export_task.rs — "SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented and asserts NOTHING about StartExportTask's real behaviour."

Cause 1 of 3: a StartExportTask naming an S3 bucket that does not exist is not refused with InvalidS3BucketFault.
AWS: Smithy StartExportTask models InvalidS3BucketFault (HTTP 400, awsQueryError code `InvalidS3BucketFault`). The request is refused SYNCHRONOUSLY, and the message is fixed text with the request's S3BucketName substituted: "The S3 bucket <S3BucketName> isn't valid. Either it isn't located in the current AWS Region or it doesn't exist." No export task is created — a following DescribeExportTasks never lists that ExportTaskIdentifier. The identical request against an existing bucket is accepted.
Emulator: HTTP 501 `NotImplemented` for every StartExportTask, so neither the refusal nor the accept exists.
Why: a client cannot distinguish a bad bucket from an unimplemented API; worse, once the operation is implemented without this check the failure surfaces only asynchronously (or not at all), and the "no task created for a refused request" invariant clients poll on is unestablished.
Fix: the StartExportTask handler's S3-bucket resolution (resolve S3BucketName in the request's region before anything is recorded; refuse with the modelled fault and the message above; record nothing).
Traces: rds__s3.cli-rds-export-invalid-bucket-name-negative.

Cause 2 of 3: a StartExportTask whose KMS key denies the caller kms:CreateGrant/kms:DescribeKey is not refused with KMSKeyNotAccessibleFault.
AWS: Smithy StartExportTask models KMSKeyNotAccessibleFault (HTTP 400, awsQueryError code `KMSKeyNotAccessibleFault`). AWS requires both kms:CreateGrant and kms:DescribeKey on the KmsKeyId for an export and refuses synchronously when the key policy denies them, wording the denial as absence: fixed text "The KMS key <KmsKeyId> doesn't exist or is disabled.", with the key reference echoed exactly as the request passed it (here the key id, not the ARN). Nothing is written: no object appears under the request's S3Prefix. A correctly-permissioned key on the same snapshot/bucket/role still exports.
Emulator: HTTP 501 `NotImplemented`, so the permission boundary does not exist.
Why: this is an over-permissive-vs-silent boundary on customer-managed encryption — a client gets no refusal where AWS refuses, and once implemented without the grant/describe check an export would be accepted against a key the exporter cannot use.
Fix: the StartExportTask handler's KMS-key resolution and grant/describe authorization step, worded as above, before any task record or S3 write.
Traces: rds__s3.rds-export-task-kms-key-missing-permission-negative.

Cause 3 of 3: the snapshot-export happy path never reaches a terminal COMPLETE task with objects in S3.
AWS: an accepted StartExportTask returns the ExportTask (Status `STARTING`, S3Bucket/S3Prefix/IamRoleArn/KmsKeyId/SourceArn echoed), DescribeExportTasks reports it until Status becomes `COMPLETE` with PercentProgress 100, and the bucket then holds the export's objects under the task's prefix (per-table files plus the export_info/ metadata objects). The same flow works through the AWS CLI (`aws rds create-db-snapshot` + `aws rds start-export-task`).
Emulator: StartExportTask is refused 501, so no task is ever created, DescribeExportTasks lists nothing, and no object is written.
Why: the entire RDS -> S3 export data path is unavailable, including everything a client builds on the terminal status (PercentProgress, FailureCause, the object layout it reads back).
Fix: the StartExportTask handler plus the export-task record and its terminal transition, and the S3 object delivery it performs through the named IAM role.
Traces: rds__s3.cli-rds-create-snapshot-export-to-s3-workflow.

## Evidence
Suite simparity-behavioral-aws-v1, service group rds__s3 2/134 passed. The three checks below are unmarked in the replay report (neither OWNED nor ALREADY FILED); all three fail with `Error: Check timed out after 60.0s`.

Cause 1 — rds__s3.cli-rds-export-invalid-bucket-name-negative [mismatch] asserts, verbatim: `bad_proc.returncode != 0` and stderr exactly "aws: [ERROR]: An error occurred (InvalidS3BucketFault) when calling the StartExportTask operation: The S3 bucket <nonexistent-bucket> isn't valid. Either it isn't located in the current AWS Region or it doesn't exist." (fixed text; the bucket name comes from the request), then `export_bad not in {t["ExportTaskIdentifier"] for t in rds.describe_export_tasks()["ExportTasks"]}`, then the identical request against the real bucket succeeding.

Cause 2 — rds__s3.rds-export-task-kms-key-missing-permission-negative [mismatch] asserts `error["Code"] == "KMSKeyNotAccessibleFault"` and `error["Message"] == f"The KMS key {denied_key_id} doesn't exist or is disabled."` (fixed text; the key reference echoed as passed), then `_list_all_keys(s3, bucket, prefix="kms-denied-attempt") == []`, then a positive control on an accessible key reaching `task["Status"] == "COMPLETE"` with objects under its prefix.

Cause 3 — rds__s3.cli-rds-create-snapshot-export-to-s3-workflow [mismatch] runs `aws rds create-db-snapshot` then `aws rds start-export-task`, polls `describe_export_tasks(ExportTaskIdentifier=...)` for a terminal status and asserts `task["Status"] == "COMPLETE"`, then lists the delivered objects in the bucket.

Emulator, for all three: StartExportTask answers HTTP 501 `NotImplemented` (src/ops/start_export_task.rs is still marked SCAFFOLDED, NOT IMPLEMENTED), so no refusal, no task and no object exists.

What the timeout does and does not establish: the harness cap is 60 s and each of these checks first stands up a two-AZ VPC (ec2.create_vpc + the botocore `vpc_available` waiter at 15 s per attempt + ec2.create_subnet), a KMS key, an IAM role, a DB instance and a snapshot, with multi-second polls throughout — so the cap does NOT localise the stall to the StartExportTask call, and these checks additionally sit behind the already-filed EC2 gaps (20261022T193451Z-ec2-createsubnet-stub.md; CreateInternetGateway, filed by the report as blocking 359 checks). What is established independently of the timeout is that none of the three contracts above can hold while StartExportTask is a 501 stub. Re-replay all three ids after the export path is implemented.

## Steps to Reproduce
Build and boot the emulator, then replay the checks in trace_ids.

Manually (one DB snapshot S, one existing bucket B, one export role R, one KMS key K):
1. `rds.start_export_task(ExportTaskIdentifier=T1, SourceArn=<S arn>, S3BucketName=<a bucket that does not exist>, IamRoleArn=R, KmsKeyId=K)` -> expect HTTP 400 `InvalidS3BucketFault`, message "The S3 bucket <name> isn't valid. Either it isn't located in the current AWS Region or it doesn't exist."; then `rds.describe_export_tasks()` must not list T1. The emulator answers 501 NotImplemented.
2. Create a KMS key whose policy explicitly denies kms:CreateGrant and kms:DescribeKey to every principal, then `rds.start_export_task(ExportTaskIdentifier=T2, SourceArn=<S arn>, S3BucketName=B, IamRoleArn=R, KmsKeyId=<denied key id>, S3Prefix="p")` -> expect HTTP 400 `KMSKeyNotAccessibleFault`, message "The KMS key <denied key id> doesn't exist or is disabled.", and zero objects under s3://B/p/. The emulator answers 501 NotImplemented.
3. `rds.start_export_task(ExportTaskIdentifier=T3, SourceArn=<S arn>, S3BucketName=B, IamRoleArn=R, KmsKeyId=K)` -> expect the ExportTask echoed with Status STARTING, then DescribeExportTasks reaching Status COMPLETE / PercentProgress 100, then the export's objects present in B under the task's prefix. The emulator answers 501 NotImplemented.
4. The same three steps through the AWS CLI (`aws rds start-export-task ...`) must produce the same codes and messages on stderr.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cli-rds-create-export-task-verify-s3-sdk rds__s3.cli-rds-create-snapshot-export-to-s3-workflow rds__s3.cli-rds-export-missing-permission-negative rds__s3.cli-rds-export-task-s3-object-listing-verify rds__s3.cli-rds-s3-bucket-policy-blocks-export-negative rds__s3.cli-rds-start-export-task-workflow rds__s3.deploy-cfn-rds-s3-bucket-policy-restricts-export-negative rds__s3.deploy-cfn-rds-s3-export-role-stack-positive rds__s3.deploy-rds-export-task-role-stack-positive rds__s3.deploy-rds-s3-export-role-stack-positive rds__s3.deploy-s3-bucket-policy-grants-rds-export-negative rds__s3.rds-export-manifest-json-format-preserved rds__s3.rds-export-snapshot-s3-key-prefix-positive rds__s3.rds-export-snapshot-to-s3-positive rds__s3.rds-export-task-bucket-policy-deny-negative rds__s3.rds-export-task-creates-s3-objects-positive rds__s3.rds-export-task-missing-s3-permission-negative rds__s3.rds-export-task-object-key-collision-negative rds__s3.rds-export-task-parquet-format-schema-preserved rds__s3.rds-export-task-partial-table-list-positive rds__s3.rds-export-task-respects-s3-prefix-positive rds__s3.rds-export-task-role-trusts-export-service-positive rds__s3.rds-export-task-s3-bucket-policy-denies-write-negative rds__s3.rds-export-task-s3-bucket-policy-deny-negative rds__s3.rds-export-task-status-transitions-visible-via-describe rds__s3.rds-export-task-writes-to-s3-positive rds__s3.rds-export-task-wrong-bucket-negative rds__s3.rds-iam-role-trust-s3-service-positive rds__s3.rds-s3-export-manifest-content-positive rds__s3.rds-s3-import-missing-role-association-negative rds__s3.rds-snapshot-export-missing-s3-permission-negative rds__s3.rds-snapshot-export-task-creates-s3-objects rds__s3.rds-snapshot-export-task-manifest-file-in-s3 rds__s3.rds-snapshot-export-task-respects-s3-prefix rds__s3.rds-snapshot-export-task-writes-to-s3-positive rds__s3.rds-start-export-task-writes-objects-to-s3-positive
Issue-Key: rds__s3.cli-rds-create-export-task-verify-s3-sdk|_wait_until#16|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.cli-rds-create-snapshot-export-to-s3-workflow|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.cli-rds-export-missing-permission-negative|_poll_until#9|Timeout|last = probe()
Issue-Key: rds__s3.cli-rds-export-task-s3-object-listing-verify|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.cli-rds-s3-bucket-policy-blocks-export-negative|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.cli-rds-start-export-task-workflow|_wait_for#13|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.deploy-cfn-rds-s3-bucket-policy-restricts-export-negative|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.deploy-cfn-rds-s3-export-role-stack-positive|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.deploy-rds-export-task-role-stack-positive|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.deploy-rds-s3-export-role-stack-positive|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.deploy-s3-bucket-policy-grants-rds-export-negative|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-manifest-json-format-preserved|_poll_until#13|Timeout|return last
Issue-Key: rds__s3.rds-export-snapshot-s3-key-prefix-positive|_poll_until#11|Timeout|return last
Issue-Key: rds__s3.rds-export-snapshot-to-s3-positive|_poll_until#10|Timeout|return last
Issue-Key: rds__s3.rds-export-task-bucket-policy-deny-negative|_poll_until#24|Timeout|return last
Issue-Key: rds__s3.rds-export-task-creates-s3-objects-positive|_poll_until#15|Timeout|return last
Issue-Key: rds__s3.rds-export-task-missing-s3-permission-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-object-key-collision-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-parquet-format-schema-preserved|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-partial-table-list-positive|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-respects-s3-prefix-positive|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-role-trusts-export-service-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-s3-bucket-policy-denies-write-negative|_poll_until#20|Timeout|return last
Issue-Key: rds__s3.rds-export-task-s3-bucket-policy-deny-negative|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-status-transitions-visible-via-describe|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-writes-to-s3-positive|_wait_export_task_terminal#15|Timeout|return task
Issue-Key: rds__s3.rds-export-task-wrong-bucket-negative|_start_export_task_expect_error#11|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-iam-role-trust-s3-service-positive|_poll_until#15|Timeout|return last
Issue-Key: rds__s3.rds-s3-export-manifest-content-positive|_wait_export_task_terminal#13|Timeout|return task
Issue-Key: rds__s3.rds-s3-import-missing-role-association-negative|run#9|Timeout|time.sleep(2)
Issue-Key: rds__s3.rds-snapshot-export-missing-s3-permission-negative|_poll_until#3|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-snapshot-export-task-creates-s3-objects|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-snapshot-export-task-manifest-file-in-s3|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-snapshot-export-task-respects-s3-prefix|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-snapshot-export-task-writes-to-s3-positive|_poll_export_status#11|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-start-export-task-writes-objects-to-s3-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# rds__s3: the snapshot-export-to-S3 path never reaches a terminal state — every seeded-snapshot export check is killed by the run budget with no verdict (36 checks)

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
One net-new root cause in the rds__s3 suite, reached by 36 checks. It is NOT covered by the open rds__s3 issues, which carry other checks' replay keys for refusal-shape causes reached at StartExportTask time (20261109T135215Z-rds-s3-9-net-new-causes-export-task.md, 20261107T185637Z-rds-s3-rds-snapshot-export-to-s3-7-causes-no.md, 20261107T061500Z-rds-s3-6-remaining-causes-in-engine-s3-statements-and-cloudformation.md, 20261029T195009Z-rds-s3-5-net-new-root-causes-startexporttask.md). Those checks get an answer and assert on its shape; the 36 here never get an answer at all, and none of them carries a replay key in any open issue. Name those issues when merging.

Cause 1 of 1: the RDS snapshot -> S3 export pipeline never progresses to a terminal state, so every check that provisions a seeded DB snapshot and then waits for the export task is killed by the suite's per-check budget with no verdict.
AWS: recorded behavior only for the timing; the states themselves are modelled. DescribeExportTasks reports an export task's Status moving STARTING -> IN_PROGRESS -> COMPLETE (or FAILED / CANCELED), and on COMPLETE the bucket holds non-empty Parquet data objects plus an export manifest under <S3Prefix>/<ExportTaskIdentifier>/, with PercentProgress reaching 100 and no FailureCause. The upstream legs these checks drive are equally modelled: CreateDBInstance -> DescribeDBInstances reports DBInstanceStatus "available" with an Endpoint.Address/Port that accepts an engine client connection, and CreateDBSnapshot -> DescribeDBSnapshots reports Status "available".
Emulator: none of these checks produces any assertion — all 36 are reported "Error: Check timed out after 60.0s", i.e. the harness killed them while a poll loop was still waiting. The loops being waited on are, in order of the shared helpers these checks use: _wait_db_instance_available (DescribeDBInstances until DBInstanceStatus == "available"), the engine seed against Endpoint.Address:Port, _wait_snapshot_available (DescribeDBSnapshots until Status == "available") and _wait_export_task_terminal / _poll_until (DescribeExportTasks until Status is COMPLETE/FAILED/CANCELED). Every one of these is a state the emulator should be able to report immediately; instead no check in this group ever reaches a terminal export status or a listed export object. Hypothesis: the export task is accepted and recorded but never advanced past its initial status, and/or the provisioned DB instance never reports "available" with a connectable endpoint, so each poll spins to the deadline.
Why: a client cannot complete, observe or verify a single RDS snapshot export to S3. Neither the happy path (COMPLETE plus Parquet objects, prefix layout, manifest content, partial table lists, status transitions visible via DescribeExportTasks) nor any of the authorization negatives (bucket-policy deny, missing s3:PutObject, wrong bucket, key collision) can be exercised, because the task never terminates either way — the pipeline is indistinguishable from a hang. Callers that poll DescribeExportTasks, as the AWS CLI and SDK waiters do, never return.
Fix: the RDS export-task state machine (the StartExportTask handler and whatever advances DescribeExportTasks' Status/PercentProgress) must drive a started task to COMPLETE — writing the Parquet data objects and the manifest under <S3Prefix>/<ExportTaskIdentifier>/ — or to FAILED with a FailureCause, promptly; and the DB-instance/snapshot lifecycle these exports are built on must report "available" with an endpoint that accepts an engine client connection.
Traces: all 36 check ids in trace_ids, each killed at the 60.0s budget with no verdict.

## Evidence
All 36 rds__s3 checks listed in trace_ids report exactly:
  Error: Check timed out after 60.0s
with no assertion reached, so no response shape was compared.

The awaited conditions, from the checks' own shared helpers:
  _wait_db_instance_available(rds, instance_id, timeout_s=2400.0, interval_s=20.0) — DescribeDBInstances until DBInstanceStatus == "available", raising on failed / incompatible-parameters / incompatible-network.
  _seed_table_data(...) — connects to instance["Endpoint"]["Address"]:instance["Endpoint"]["Port"] with the engine client and writes rows.
  _wait_snapshot_available(rds, snapshot_id, timeout_s=1800.0, interval_s=15.0) — DescribeDBSnapshots until Status == "available".
  _wait_export_task_terminal(rds, export_task_id, timeout_s=3600.0, interval_s=20.0) — DescribeExportTasks until Status in ("COMPLETE", "FAILED", "CANCELED").
and then, on COMPLETE, assertions of the form:
  assert task["Status"] == "COMPLETE", f"export task ended in {task['Status']!r}: {task.get('FailureCause')}"
  assert len(objects) > 0, "export task reported COMPLETE but no objects were written to S3"
  parquet_objects = [o for o in objects if o["Key"].endswith(".parquet")]; assert parquet_objects; assert all(o["Size"] > 0 ...)

Expected: each poll returns within the budget — "available", "available", then a terminal export Status of COMPLETE with non-empty .parquet objects under <prefix>/<export task id>/. Actual: no poll ever satisfies its predicate; the check is killed. By contrast the rds__s3 checks that fail with an assertion (the ones already owned by open issues) do get an answer from StartExportTask, which places this cause after acceptance, in the progression of the task and of the instance/snapshot lifecycle it rests on. Identifiers, endpoints and prefixes are per-run values.

## Steps to Reproduce
1. Create a bucket, a symmetric KMS key, and an IAM role trusting export.rds.amazonaws.com with s3:PutObject*/ListBucket/GetObject*/DeleteObject*/GetBucketLocation on that bucket and kms:CreateGrant/DescribeKey on the key.
2. CreateDBSubnetGroup over private subnets, then CreateDBInstance (mysql, db.t3.micro, DBName set) and poll DescribeDBInstances for DBInstanceStatus "available"; connect to the reported Endpoint.Address:Port and insert rows.
3. CreateDBSnapshot and poll DescribeDBSnapshots for Status "available".
4. StartExportTask(ExportTaskIdentifier=..., SourceArn=<snapshot arn>, S3BucketName=<bucket>, IamRoleArn=<role>, KmsKeyId=<key arn>) and poll DescribeExportTasks: AWS reaches COMPLETE and the bucket holds non-empty .parquet objects plus a manifest under <prefix>/<ExportTaskIdentifier>/; the emulator never leaves the non-terminal status.
5. Re-replay the 36 rds__s3 checks in trace_ids to confirm the fix.

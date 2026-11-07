Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cli-rds-export-task-s3-object-listing-verify rds__s3.rds-export-manifest-json-format-preserved rds__s3.rds-export-task-creates-s3-objects-positive rds__s3.rds-snapshot-export-task-manifest-file-in-s3
Issue-Key: rds__s3.cli-rds-export-task-s3-object-listing-verify|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-manifest-json-format-preserved|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-creates-s3-objects-positive|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-snapshot-export-task-manifest-file-in-s3|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds__s3: the snapshot-export-to-S3 flow never completes within the check budget — all four export checks are cut off at the harness cap with no response

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
One root cause behind the four unowned rds__s3 failures in this suite.

Cause 1 of 1: the Aurora-snapshot-export-to-S3 flow does not converge. All four checks run the same emulator-only sequence and all four are cut off by the harness after 60 s having produced no assertion at all — i.e. one of the calls in that sequence either never answers or never settles into the state the next step waits for.

AWS: recorded behavior only for the end-to-end timing; the per-step contract the checks assert is RDS's documented export behavior — CreateDBCluster/CreateDBInstance bring a serverless Aurora instance to DBInstanceStatus 'available'; rds-data ExecuteStatement answers a bounded response for a CREATE TABLE / INSERT against that cluster; CreateDBClusterSnapshot reaches Status 'available'; StartExportTask on the cluster snapshot ARN is accepted and DescribeExportTasks walks the task to a terminal Status 'COMPLETE'; and the exported table data then exists in S3 as compressed Parquet objects nested under <S3Prefix>/<ExportTaskIdentifier>/<database>/<table>/..., alongside an export_info_<id>.json manifest (AWS documents this layout in USER_ExportSnapshot.FileNames). Every one of those calls answers in bounded time against AWS; none of them is allowed to hang.

Emulator: each check reports only "Check timed out after 60.0s" — the whole budget is consumed inside the provisioning sequence and no step-level response is recorded. The shared sequence, read from the checks, is: s3.CreateBucket -> kms.CreateKey -> iam.CreateRole/PutRolePolicy (trust export.rds.amazonaws.com) -> rds.CreateDBSubnetGroup -> rds.CreateDBCluster (MasterUserPassword, EnableHttpEndpoint=true, ServerlessV2ScalingConfiguration) -> rds.CreateDBInstance (db.serverless) -> poll DescribeDBInstances for 'available' -> secretsmanager.CreateSecret -> rds-data.ExecuteStatement x2 to seed a table -> rds.CreateDBClusterSnapshot -> poll DescribeDBClusterSnapshots for 'available' -> rds.StartExportTask -> poll DescribeExportTasks for a terminal status -> list the S3 objects. Note that these checks do NOT use ManageMasterUserPassword, so the CreateDBCluster 501 refusal filed for the other groups is not what stops them.

Hypothesis: the most likely hang point is the rds-data ExecuteStatement seeding step, because an already-open issue records exactly that symptom — 20261101T205955Z-rds-data-executestatement-never-answers-on-the-live-cluster-round-trip: "the call hangs until the fixture budget is spent instead of returning a bounded...". A non-answering ExecuteStatement inside the check's 180 s seed poll would consume the entire 60 s harness cap and produce precisely this output. The alternative candidates, in the order they are reached, are the DescribeDBInstances 'available' poll and the DescribeDBClusterSnapshots 'available' poll never settling. The fix must identify which by instrumenting the sequence.

Why: no client can export an RDS snapshot to S3 through this emulator — the flow stalls instead of either completing or refusing. A caller gets no response and no error code to act on; a hang is strictly worse than a refusal because there is nothing to branch on. The documented export artefacts (Parquet objects under <prefix>/<export_id>/<database>/<table>/ and the export_info_<id>.json manifest) are consequently never observable, and the whole rds__s3 group is 0/4.

Fix: whichever handler in the sequence above fails to answer or fails to settle — start with the rds-data ExecuteStatement handler's live-cluster round trip (it must always return a bounded response or a modeled error, never block), then the RDS DB-instance and DB-cluster-snapshot state machines that must reach 'available', then the StartExportTask handler and the export-task state machine that must reach 'COMPLETE' and write the S3 objects.

Related open issues that do NOT carry these four replay keys: 20261101T205955Z-rds-data-executestatement-never-answers-on-the-live.md (the hang symptom, filed under rds-data), 20261102T071929Z-rds-the-snapshot-export-to-s3-workflow-never.md (non-convergence, filed under rds), 20261103T040901Z-rds-s3-rds-startexporttask-answers-501-notimplemented.md and 20261029T210641Z-rds-s3-3-net-new-root-causes-startexporttask.md (StartExportTask itself). The replay report marks none of these four checks as owned.

Traces: the four check ids in trace_ids.

## Evidence
From the behavioral replay report (suite simparity-behavioral-aws-v1), service rds__s3: 0/4 passed (0%), 4 failed, all [mismatch] with the identical detail:

- rds__s3.cli-rds-export-task-s3-object-listing-verify      -> Error: Check timed out after 60.0s
- rds__s3.rds-export-manifest-json-format-preserved          -> Error: Check timed out after 60.0s
- rds__s3.rds-export-task-creates-s3-objects-positive        -> Error: Check timed out after 60.0s
- rds__s3.rds-snapshot-export-task-manifest-file-in-s3       -> Error: Check timed out after 60.0s

No step-level response was recorded, so the diverging status/code/message cannot be quoted: the divergence IS the absence of any answer. Every call in the sequence is made against the emulator; no external dependency, no missing tool (the one check that needs the aws CLI declares require_tools('aws') and did not report it missing), and no account-specific value is involved before the timeout.

The shared sequence and its waits, quoted from the checks in the report:
  rds.create_db_cluster(Engine='aurora-mysql', MasterUserPassword=<generated>, EnableHttpEndpoint=True, ServerlessV2ScalingConfiguration={'MinCapacity':0.5,'MaxCapacity':2.0}, StorageEncrypted=True, DBSubnetGroupName=...)
  rds.create_db_instance(DBInstanceClass='db.serverless', DBClusterIdentifier=...)
  _poll_until(describe_db_instances -> DBInstanceStatus == 'available', timeout_s=1200, interval_s=20)
  rds_data.execute_statement(... 'CREATE TABLE IF NOT EXISTS export_rows (id INT PRIMARY KEY, val VARCHAR(50))') and an INSERT, wrapped in _poll_until(timeout_s=180, interval_s=10)
  rds.create_db_cluster_snapshot(...); _poll_until(Status == 'available', timeout_s=900, interval_s=15)
  rds.start_export_task(ExportTaskIdentifier=..., SourceArn=<DBClusterSnapshotArn>, S3BucketName=..., S3Prefix=..., IamRoleArn=..., KmsKeyId=...)
  _poll_until(describe_export_tasks -> Status in ('COMPLETE','FAILED','CANCELED'), timeout_s=1800, interval_s=30)
  then: assert Status == 'COMPLETE'; assert the S3 listing under <prefix>/ is non-empty; assert some key contains the export task identifier; assert some key ends with '.parquet'.

Expected (AWS): the sequence completes, DescribeExportTasks reports Status 'COMPLETE', and `aws s3 ls s3://<bucket>/<prefix>/ --recursive` lists Parquet objects nested under <prefix>/<export_id>/<database>/<table>/ plus export_info_<export_id>.json.
Actual (emulator): no step-level response; the check is terminated at the 60 s harness cap.

## Steps to Reproduce
Build and boot the emulator, then replay the four checks in trace_ids.

Manually, timing each call:
1. s3.create_bucket; kms.create_key; iam.create_role (trust export.rds.amazonaws.com) + iam.put_role_policy (s3:PutObject*/GetObject*/ListBucket + kms:CreateGrant/DescribeKey/Decrypt/GenerateDataKey*).
2. rds.create_db_subnet_group; rds.create_db_cluster(Engine='aurora-mysql', MasterUsername='sim2realadmin', MasterUserPassword=<pw>, DatabaseName='sim2realexp', EnableHttpEndpoint=True, ServerlessV2ScalingConfiguration={'MinCapacity':0.5,'MaxCapacity':2.0}, StorageEncrypted=True, BackupRetentionPeriod=1); rds.create_db_instance(DBInstanceClass='db.serverless').
3. Poll rds.describe_db_instances until DBInstanceStatus == 'available' -> record whether it ever settles and how long it takes.
4. secretsmanager.create_secret({'username','password'}); rds-data.execute_statement(resourceArn=<DBClusterArn>, secretArn=<secret ARN>, database='sim2realexp', sql='CREATE TABLE IF NOT EXISTS export_rows (id INT PRIMARY KEY, val VARCHAR(50))') -> this call MUST return a bounded response or a modeled error; if it blocks, this is the hang.
5. rds.create_db_cluster_snapshot; poll describe_db_cluster_snapshots until Status == 'available'.
6. rds.start_export_task(SourceArn=<DBClusterSnapshotArn>, S3BucketName=..., S3Prefix=..., IamRoleArn=..., KmsKeyId=...); poll describe_export_tasks until Status is terminal -> expect 'COMPLETE'.
7. aws s3 ls s3://<bucket>/<prefix>/ --recursive -> expect keys containing the export task identifier and at least one '.parquet' object, plus export_info_<id>.json.
Each of steps 3-6 should be timed individually to identify which one does not converge.

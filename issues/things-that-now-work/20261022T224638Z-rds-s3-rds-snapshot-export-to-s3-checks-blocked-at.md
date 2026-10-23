Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cli-rds-export-task-s3-object-listing-verify rds__s3.rds-export-manifest-json-format-preserved rds__s3.rds-export-task-creates-s3-objects-positive rds__s3.rds-snapshot-export-task-manifest-file-in-s3
Issue-Key: rds__s3|CreateDBCluster|NotImplemented
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# RDS snapshot-export-to-S3 checks blocked at setup: RDS CreateDBCluster answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
This issue carries ONE root cause.

Cause 1 of 1: RDS CreateDBCluster is an unimplemented scaffold stub, so no cluster exists to snapshot and export to S3.
AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model): CreateDBCluster requires DBClusterIdentifier and Engine, accepts MasterUsername/MasterUserPassword, DatabaseName, StorageEncrypted, Tags and the serverless/scaling members, and returns CreateDBClusterResult {DBCluster} on HTTP 200 with DBClusterArn populated. Its modeled error set is the awsQuery RDS fault family (DBClusterAlreadyExistsFault 400, DBSubnetGroupNotFoundFault 404, InvalidDBClusterStateFault 400, ...). NotImplemented is not a modeled error for it.
Emulator: the handler answers HTTP 501 with error code NotImplemented and message "operation CreateDBCluster is scaffolded but not implemented in this emulator" (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_cluster.rs carries the SCAFFOLDED marker).
Why: the whole RDS->S3 export path is unreachable. A client that provisions a cluster, snapshots it, and runs StartExportTask to an S3 bucket cannot get past step one, and receives an unmodeled error code with a 501 status (not retryable, not in the operation's error union) where AWS returns 200. Consequently nothing in the emulator ever asserts that an export writes Parquet objects and an export manifest into the target bucket.
Fix: the operation's handler plus DB-cluster state in the rds crate (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_cluster.rs and the cluster store in that crate's state), after which the snapshot/export-task and S3 object-writing path can be exercised.
Traces: rds__s3.cli-rds-export-task-s3-object-listing-verify, rds__s3.rds-export-manifest-json-format-preserved, rds__s3.rds-export-task-creates-s3-objects-positive, rds__s3.rds-snapshot-export-task-manifest-file-in-s3.
NOTE: the replay report auto-filed a CreateDBCluster stub issue against the single rds.* check it blocked; that issue does not carry these rds__s3 replay keys, so this cause is recorded here for the four export checks it blocks.

## Evidence
All four rds__s3 checks fail identically: "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDBCluster operation: operation CreateDBCluster is scaffolded but not implemented in this emulator", raised from the check's cluster-provisioning step before any snapshot, StartExportTask or S3 call.
Expected: HTTP 200 with DBCluster.DBClusterArn, per CreateDBClusterResult in the RDS Smithy model.
Per-service pass rate from the replay: rds__s3 0/4 (0%).

## Steps to Reproduce
1. s3.create_bucket(Bucket='export-target') -> 200 (works).
2. rds.create_db_cluster(DBClusterIdentifier='exp-src', Engine='aurora-mysql', MasterUsername='dbadmin', MasterUserPassword='<pw>', DatabaseName='appdb') -> expect HTTP 200 with DBCluster.DBClusterArn; observe HTTP 501 error code NotImplemented.
3. (unreachable) rds.create_db_cluster_snapshot, then rds.start_export_task(S3BucketName='export-target', ...) and s3.list_objects_v2 to see the exported objects and export_info/manifest.

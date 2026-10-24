Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-s3-export-kms-key-mismatch-negative rds__s3.cli-rds-start-export-task-workflow rds__s3.aurora-restore-from-s3-missing-manifest-rejected rds__s3.rds-restore-db-cluster-from-s3-engine-mismatch-negative rds__s3.cfn-rds-cluster-parameter-group-s3-role-arn-positive
Issue-Key: rds__s3.rds-s3-export-kms-key-mismatch-negative|run#13|AssertionError|assert error["Code"] == "KMSKeyNotAccessibleFault", (
Issue-Key: rds__s3.cli-rds-start-export-task-workflow|_wait_for#7|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.aurora-restore-from-s3-missing-manifest-rejected|run#7|AssertionError|assert code == "InvalidS3BucketFault", f"expected InvalidS3BucketFault, got {code!r}"
Issue-Key: rds__s3.rds-restore-db-cluster-from-s3-engine-mismatch-negative|run#10|AssertionError|assert code == "InvalidParameterCombination", f"expected InvalidParameterCombination, got {code}"
Issue-Key: rds__s3|DescribeDBClusterParameters|DBParameterGroupNotFound
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# RDS snapshot export to S3 and restore-from-S3 are unimplemented, and a CloudFormation-created DBClusterParameterGroup is absent from RDS

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
Three causes on the RDS<->S3 boundary.

Cause 1 of 3: StartExportTask is an unimplemented scaffold stub, so snapshot export to S3 never runs. AWS: Smithy AmazonRDSv19 models StartExportTask -> ExportTask {ExportTaskIdentifier, SourceArn, S3Bucket, S3Prefix, IamRoleArn, KmsKeyId, Status, PercentProgress, SourceType, FailureCause, ...} with errors including KMSKeyNotAccessibleFault (client, httpError 400, awsQueryError code KMSKeyNotAccessibleFault), InvalidS3BucketFault (400), IamRoleMissingPermissionsFault (400), IamRoleNotFoundFault (404) and DBSnapshotNotFoundFault (404). A CMK whose key policy denies the export role kms:CreateGrant/kms:DescribeKey must be refused with KMSKeyNotAccessibleFault at 400, and a well-formed request must report Status 'STARTING'/'IN_PROGRESS' and eventually write .parquet objects under the bucket. Emulator: StartExportTask answers {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation StartExportTask is scaffolded but not implemented in this emulator'}, so neither the KMS rejection nor any export ever happens; the CLI-driven workflow check never gets a terminal export task and the harness kills it at its 60s wall clock. Why: no client can export a snapshot to S3, and the KMS/IAM/bucket rejection paths an exporter branches on are all unreachable. Fix: the StartExportTask handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/start_export_task.rs) plus the export-task record in the RDS state module and the S3 objects it must produce. Traces: rds__s3.rds-s3-export-kms-key-mismatch-negative, rds__s3.cli-rds-start-export-task-workflow.

Cause 2 of 3: RestoreDBClusterFromS3 is an unimplemented scaffold stub. AWS: Smithy models RestoreDBClusterFromS3 -> DBCluster with required SourceEngine, SourceEngineVersion, S3BucketName and S3IngestionRoleArn, and errors including InvalidS3BucketFault (client, httpError 400, awsQueryError code InvalidS3BucketFault). A SourceEngine other than the single documented valid value is rejected during request validation with InvalidParameterCombination before the bucket is touched; a bucket/prefix with no backup manifest is rejected with InvalidS3BucketFault. Emulator: both probes answer code 'NotImplemented', so neither validation layer exists. Why: Aurora migration-from-S3 is unusable, and a client cannot distinguish 'your engine parameters are wrong' from 'your bucket has no manifest'. Fix: the RestoreDBClusterFromS3 handler and its request validation (SourceEngine value check ahead of the S3 lookup). Traces: rds__s3.aurora-restore-from-s3-missing-manifest-rejected, rds__s3.rds-restore-db-cluster-from-s3-engine-mismatch-negative.

Cause 3 of 3: an AWS::RDS::DBClusterParameterGroup that CloudFormation reports as created does not exist in RDS. AWS: once the stack reaches CREATE_COMPLETE, the resource's PhysicalResourceId names a cluster parameter group RDS itself resolves; DescribeDBClusterParameters on it returns the family's parameters. (Smithy: DBClusterParameterGroupNotFoundFault — client, httpError 404, awsQueryError code DBClusterParameterGroupNotFound — is returned only for a group that does not exist.) Emulator: DescribeDBClusterParameters on the CloudFormation-reported physical id answers 404 'DBClusterParameterGroup not found: <stack>-ClusterParamGroup-<suffix>'. Why: the stack claims a parameter group exists, so anything that then reads or modifies it through RDS — including the cluster that is supposed to reference it and the role-ARN association the check exercises — fails against a resource CloudFormation says it created. Fix: the CloudFormation resource provisioner for AWS::RDS::DBClusterParameterGroup — it must create the group through the RDS service state and record that name as the PhysicalResourceId. Traces: rds__s3.cfn-rds-cluster-parameter-group-s3-role-arn-positive.

## Evidence
Cause 1: rds__s3.rds-s3-export-kms-key-mismatch-negative — AssertionError: expected KMSKeyNotAccessibleFault for a key whose policy denies grant and describe, got {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation StartExportTask is scaffolded but not implemented in this emulator'}. rds__s3.cli-rds-start-export-task-workflow — 'Check timed out after 60.0s' driving the same `aws rds start-export-task` workflow to a terminal export task and .parquet objects in S3.
Cause 2: rds__s3.aurora-restore-from-s3-missing-manifest-rejected — AssertionError: expected InvalidS3BucketFault, got 'NotImplemented'. rds__s3.rds-restore-db-cluster-from-s3-engine-mismatch-negative — AssertionError: expected InvalidParameterCombination, got NotImplemented.
Cause 3: rds__s3.cfn-rds-cluster-parameter-group-s3-role-arn-positive — botocore.errorfactory.DBParameterGroupNotFoundFault: An error occurred (DBParameterGroupNotFound) when calling the DescribeDBClusterParameters operation: DBClusterParameterGroup not found: sim2real-xsvc-stk-f2deb20a8f5c-ClusterParamGroup-1D923BA982A5.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: create a DB snapshot, an export IAM role and a CMK, then call Action=StartExportTask (or `aws rds start-export-task`) — the response is error code NotImplemented instead of an ExportTask (and instead of KMSKeyNotAccessibleFault when the CMK policy denies the role kms:CreateGrant/kms:DescribeKey).
3. Cause 2: call Action=RestoreDBClusterFromS3 with SourceEngine set to a value other than the documented one, and again with a bucket/prefix holding no manifest — both answer NotImplemented instead of InvalidParameterCombination / InvalidS3BucketFault.
4. Cause 3: CreateStack with a template containing AWS::RDS::DBClusterParameterGroup, wait for CREATE_COMPLETE, take the resource's PhysicalResourceId and call Action=DescribeDBClusterParameters with it — 404 DBClusterParameterGroupNotFound.

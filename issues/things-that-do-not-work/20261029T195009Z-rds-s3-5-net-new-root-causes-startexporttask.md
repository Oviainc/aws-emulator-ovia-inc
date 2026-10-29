Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cfn-deploy-rds-s3-export-role-stack-positive rds__s3.cfn-stack-update-rds-add-s3-role-positive rds__s3.cli-rds-export-task-missing-role-negative rds__s3.rds-s3-export-kms-key-mismatch-negative rds__s3.cfn-deploy-rds-s3-export-bucket-missing-negative rds__s3.cfn-deploy-rds-missing-s3-export-role-negative rds__s3.cfn-deploy-rds-cluster-missing-iam-trust-negative rds__s3.cli-rds-add-role-to-cluster-then-verify-sdk rds__s3.cfn-deploy-rds-instance-with-s3-import-role-positive rds__s3.cfn-deploy-rds-role-trust-wrong-principal-negative rds__s3.cfn-deploy-rds-s3-role-missing-trust-policy-negative rds__s3.cfn-rds-instance-iam-role-association-arn-resolution rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative rds__s3.cfn-deploy-rds-cluster-s3-export-role-positive rds__s3.cfn-deploy-rds-s3-role-policy-wrong-bucket-arn-negative rds__s3.cfn-stack-update-rds-cluster-add-s3-role-positive
Issue-Key: rds__s3|StartExportTask|NotImplemented
Issue-Key: rds__s3.cli-rds-export-task-missing-role-negative|run#16|AssertionError|assert code == "KMSKeyNotAccessibleFault", (
Issue-Key: rds__s3.cfn-deploy-rds-s3-export-bucket-missing-negative|run#21|AssertionError|assert code == "InvalidS3BucketFault", (
Issue-Key: rds__s3.cfn-deploy-rds-missing-s3-export-role-negative|run#19|AssertionError|assert code == "IamRoleMissingPermissions", (
Issue-Key: rds__s3|AddRoleToDBCluster|NotImplemented
Issue-Key: rds__s3.cli-rds-add-role-to-cluster-then-verify-sdk|_run_aws_with_retry#20|AssertionError|raise AssertionError(
Issue-Key: rds__s3.cfn-deploy-rds-instance-with-s3-import-role-positive|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: rds__s3.cfn-deploy-rds-role-trust-wrong-principal-negative|_deploy_cfn#2|AssertionError|raise AssertionError(
Issue-Key: rds__s3.cfn-deploy-rds-s3-role-missing-trust-policy-negative|run#7|AssertionError|assert "AWS_ROLE_INTEGRATION" in db_failures[0]["reason"], (
Issue-Key: rds__s3.cfn-rds-instance-iam-role-association-arn-resolution|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: rds__s3|DescribeDBClusters|DBClusterNotFoundFault
Issue-Key: rds__s3|CreateDBClusterSnapshot|DBClusterNotFoundFault
Issue-Key: rds__s3.cfn-stack-update-rds-cluster-add-s3-role-positive|_pick_valid_feature#3|AssertionError|raise AssertionError(
Issue-Related: 20261024T030130Z-rds-s3-rds-snapshot-export-to-s3-and-restore-from.md
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# rds__s3: 5 net-new root causes — StartExportTask and AddRoleToDBCluster are 501 stubs, CloudFormation drops AWS::RDS::DBInstance AssociatedRoles and never registers AWS::RDS::DBCluster with RDS, and the default aurora-mysql version advertises withdrawn s3Import/s3Export features

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
Five distinct root causes behind the rds__s3 cross-service failures (132 of 134 checks in this group fail). Related open issues exist but do NOT cover these replay keys: 20261029T072224Z-rds-s3-startexporttask-is-an-unimplemented-scaffold-stub.md carries only the `aws rds start-export-task` CLI check, and 20261024T030130Z-rds-s3-rds-snapshot-export-to-s3-and-restore-from.md carries the restore-from-S3 checks; the checks below are unowned in the replay report.

Cause 1 of 5: StartExportTask is an unimplemented scaffold stub, so every snapshot-export flow and every export refusal class is unreachable.
AWS: Smithy: StartExportTask(ExportTaskIdentifier, SourceArn, S3BucketName, IamRoleArn, KmsKeyId all required) returns an ExportTask and errors with InvalidS3BucketFault (400) for a bucket that does not exist or that the role cannot write, IamRoleMissingPermissionsFault (400) / IamRoleNotFoundFault (404) for an export role without the needed S3/KMS policy, KMSKeyNotAccessibleFault (400) for a CMK whose policy denies grant/describe, DBSnapshotNotFound / DBClusterSnapshotNotFoundFault, InvalidExportOnlyFault, InvalidExportSourceStateFault, ExportTaskAlreadyExistsFault. These refusals are synchronous.
Emulator: every call answers code 'NotImplemented' with "operation StartExportTask is scaffolded but not implemented in this emulator" (HTTP 501), so the positive export flow never starts and each negative class is answered with the wrong code.
Why: snapshot export to S3 is wholly unavailable, and a client cannot distinguish a bad bucket, a role missing permissions and an inaccessible CMK — AWS gives three different codes.
Fix: implement the StartExportTask handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/start_export_task.rs) together with its bucket/role/KMS validation and export-task state.
Traces: rds__s3.cfn-deploy-rds-s3-export-role-stack-positive, rds__s3.cfn-stack-update-rds-add-s3-role-positive, rds__s3.cli-rds-export-task-missing-role-negative, rds__s3.rds-s3-export-kms-key-mismatch-negative, rds__s3.cfn-deploy-rds-s3-export-bucket-missing-negative, rds__s3.cfn-deploy-rds-missing-s3-export-role-negative

Cause 2 of 5: AddRoleToDBCluster is an unimplemented scaffold stub.
AWS: Smithy: AddRoleToDBCluster(DBClusterIdentifier, RoleArn required; FeatureName) returns an empty body and errors DBClusterNotFoundFault (404), DBClusterRoleAlreadyExistsFault, DBClusterRoleQuotaExceededFault, InvalidDBClusterStateFault; the association then shows up in DescribeDBClusters AssociatedRoles with Status ACTIVE.
Emulator: code 'NotImplemented' for both the SDK and the `aws rds add-role-to-db-cluster` CLI path, so no cluster role association can ever be made or read back.
Why: every Aurora-to-S3 import/export authorization flow is impossible, and the not-found/invalid-feature refusals are unobservable.
Fix: implement the AddRoleToDBCluster handler and the cluster AssociatedRoles state it writes.
Traces: rds__s3.cfn-deploy-rds-cluster-missing-iam-trust-negative, rds__s3.cli-rds-add-role-to-cluster-then-verify-sdk

Cause 3 of 5: CloudFormation's AWS::RDS::DBInstance provider does not forward the AssociatedRoles property, so any instance template that carries one fails to create.
AWS: a stack declaring AWS::RDS::DBInstance with AssociatedRoles creates the instance and the role association (feature AWS_ROLE_INTEGRATION / s3Import), reaching CREATE_COMPLETE; when the role's trust policy or policy is wrong, the DBInstance resource's CREATE_FAILED ResourceStatusReason names the role association that RDS refused (the check asserts the reason names the AWS_ROLE_INTEGRATION association), and the stack rolls back.
Emulator: the DBInstance resource fails with the provider's own message — 'Resource handler returned message: "this emulator does not send AWS::RDS::DBInstance\'s AssociatedRoles property to the service that owns the type, so the resource was not created" (... HandlerErrorCode: InvalidRequest)' — so positive stacks end in ROLLBACK_COMPLETE and negative stacks fail for the emulator's reason instead of RDS's.
Why: no CloudFormation-deployed RDS instance with an IAM role association can exist, and the negative cases cannot be attributed to the thing the template broke.
Fix: the CloudFormation AWS::RDS::DBInstance resource provider — it must pass AssociatedRoles through to RDS (and surface RDS's refusal as the resource's status reason).
Traces: rds__s3.cfn-deploy-rds-instance-with-s3-import-role-positive, rds__s3.cfn-deploy-rds-role-trust-wrong-principal-negative, rds__s3.cfn-deploy-rds-s3-role-missing-trust-policy-negative, rds__s3.cfn-rds-instance-iam-role-association-arn-resolution

Cause 4 of 5: a CloudFormation-deployed AWS::RDS::DBCluster is not registered with RDS — the stack finishes but RDS holds no cluster.
AWS: once a stack containing AWS::RDS::DBCluster reaches CREATE_COMPLETE, DescribeDBClusters(DBClusterIdentifier=<the template's id>) returns the cluster and CreateDBClusterSnapshot against it succeeds.
Emulator: after deployment both calls are refused with DBClusterNotFoundFault — DescribeDBClusters answers 'DBCluster <id> not found.' and CreateDBClusterSnapshot answers 'DBCluster not found: <id>' (the two templates are themselves inconsistent, which is worth fixing alongside).
Why: anything that deploys an Aurora cluster through CloudFormation and then addresses it through the RDS API breaks; the cluster exists only inside the stack.
Fix: the CloudFormation AWS::RDS::DBCluster resource provider — it must create the cluster through RDS so RDS state holds it.
Traces: rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative, rds__s3.cfn-deploy-rds-cluster-s3-export-role-positive, rds__s3.cfn-deploy-rds-s3-role-policy-wrong-bucket-arn-negative

Cause 5 of 5: the default aurora-mysql engine version advertises s3Import/s3Export in SupportedFeatureNames, which the recorded account no longer supports.
AWS: recorded behavior only — for the account/region the suite recorded, DescribeDBEngineVersions for the default aurora-mysql version reports SupportedFeatureNames as ONLY "Bedrock"; s3Import/s3Export have been withdrawn, so AddRoleToDBCluster with FeatureName='s3Export' is refused for that engine version.
Emulator: the engine-version catalog still lists s3Export as a valid feature for the default aurora-mysql version, so the check's feature-selection helper concludes s3Export is supported and the withdrawn-feature rejection can never be exercised.
Why: callers discover features through SupportedFeatureNames; advertising a withdrawn feature makes the emulator accept associations AWS refuses.
Fix: the engine-version catalog data behind DescribeDBEngineVersions (SupportedFeatureNames for aurora-mysql) and the FeatureName validation in AddRoleToDBCluster.
Traces: rds__s3.cfn-stack-update-rds-cluster-add-s3-role-positive

## Evidence
All quoted from the behavioral replay report (suite simparity-behavioral-aws-v1), service group rds__s3 (2/134 passed).

Cause 1 — StartExportTask 501:
- rds__s3.cfn-deploy-rds-s3-export-role-stack-positive [error]: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartExportTask operation: operation StartExportTask is scaffolded but not implemented in this emulator
- rds__s3.cfn-stack-update-rds-add-s3-role-positive [error]: same NotImplemented on StartExportTask
- rds__s3.cli-rds-export-task-missing-role-negative [mismatch]: AssertionError: expected KMSKeyNotAccessibleFault, got 'NotImplemented' (stderr: aws: [ERROR]: An error occurred (NotImplemented) when calling the StartExportTask operation ...)
- rds__s3.rds-s3-export-kms-key-mismatch-negative [mismatch]: AssertionError: expected KMSKeyNotAccessibleFault for a key whose policy denies grant and describe, got {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation StartExportTask is scaffolded but not implemented in this emulator'}
- rds__s3.cfn-deploy-rds-s3-export-bucket-missing-negative [mismatch]: AssertionError: export to a nonexistent bucket should be rejected synchronously with InvalidS3BucketFault, got 'NotImplemented'
- rds__s3.cfn-deploy-rds-missing-s3-export-role-negative [mismatch]: AssertionError: expected IamRoleMissingPermissions for a role with no attached S3/KMS policy, got 'NotImplemented'

Cause 2 — AddRoleToDBCluster 501:
- rds__s3.cfn-deploy-rds-cluster-missing-iam-trust-negative [error]: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddRoleToDBCluster operation: operation AddRoleToDBCluster is scaffolded but not implemented in this emulator
- rds__s3.cli-rds-add-role-to-cluster-then-verify-sdk [mismatch]: AssertionError: aws rds add-role-to-db-cluster --db-cluster-identifier ... --feature-name s3Import failed (rc=254): aws: [ERROR]: An error occurred (NotImplemented) when calling the AddRoleToDBCluster operation ...

Cause 3 — CFN DBInstance AssociatedRoles dropped:
- rds__s3.cfn-deploy-rds-role-trust-wrong-principal-negative [mismatch]: AssertionError: Stack ... ended in ROLLBACK_COMPLETE (reason: The following resource(s) failed to create: [DbInstance]. ); failed resources: [..., ('DbInstance', 'Resource handler returned message: "this emulator does not send AWS::RDS::DBInstance\'s AssociatedRoles property to the service that owns the type, so the resource was not created" (RequestToken: ..., HandlerErrorCode: InvalidRequest)')]
- rds__s3.cfn-deploy-rds-s3-role-missing-trust-policy-negative [mismatch]: AssertionError: expected the DBInstance CREATE_FAILED reason to name the AWS_ROLE_INTEGRATION association, got 'Resource handler returned message: "this emulator does not send AWS::RDS::DBInstance\'s AssociatedRoles property ..."'
- rds__s3.cfn-deploy-rds-instance-with-s3-import-role-positive [mismatch]: AssertionError: Stack sim2real-xsvc-stk-4036e25a87cd ended in ROLLBACK_COMPLETE
- rds__s3.cfn-rds-instance-iam-role-association-arn-resolution [mismatch]: AssertionError: Stack sim2real-xsvc-stk-6ad0d59545d0 ended in ROLLBACK_COMPLETE

Cause 4 — CFN DBCluster absent from RDS:
- rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative [error]: botocore.errorfactory.DBClusterNotFoundFault: An error occurred (DBClusterNotFoundFault) when calling the DescribeDBClusters operation: DBCluster sim2real-xsvc-cluster-0a116231b1c0 not found.
- rds__s3.cfn-deploy-rds-cluster-s3-export-role-positive [error]: DBClusterNotFoundFault when calling CreateDBClusterSnapshot: DBCluster not found: sim2real-xsvc-cluster-f2eacc2c7b91
- rds__s3.cfn-deploy-rds-s3-role-policy-wrong-bucket-arn-negative [error]: DBClusterNotFoundFault when calling CreateDBClusterSnapshot: DBCluster not found: sim2real-xsvc-cluster-3071e6d5936a

Cause 5 — withdrawn feature still advertised:
- rds__s3.cfn-stack-update-rds-cluster-add-s3-role-positive [mismatch]: AssertionError: s3Export is now a valid AddRoleToDBCluster feature for the default aurora-mysql engine version -- real AWS may have re-added support for it, and this check's withdrawal premise needs re-authoring (the check's docstring records that the recorded account reports SupportedFeatureNames as ONLY "Bedrock").

Smithy citations (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): StartExportTask errors InvalidS3BucketFault(400), IamRoleMissingPermissionsFault(400), IamRoleNotFoundFault(404), KMSKeyNotAccessibleFault(400), DBSnapshotNotFound(404), ExportTaskAlreadyExists(400), InvalidExportOnly(400), InvalidExportSourceState(400); AddRoleToDBCluster errors DBClusterNotFoundFault(404), DBClusterRoleAlreadyExists(400), InvalidDBClusterStateFault(400).

## Steps to Reproduce
Build and boot the emulator, then replay the rds__s3 checks listed in trace_ids.

Manually:
1. Create a DB snapshot, an S3 bucket, a KMS key and an export role, then rds.start_export_task(ExportTaskIdentifier=..., SourceArn=<snapshot arn>, S3BucketName=..., IamRoleArn=..., KmsKeyId=...) -> expect an ExportTask, not NotImplemented; repeat with a nonexistent bucket (expect InvalidS3BucketFault), with a role holding no S3/KMS policy (expect IamRoleMissingPermissions) and with a CMK whose policy denies grant/describe (expect KMSKeyNotAccessibleFault).
2. rds.add_role_to_db_cluster(DBClusterIdentifier=<aurora cluster>, RoleArn=<role>, FeatureName=<supported feature>) -> expect an empty 200 and the association visible in DescribeDBClusters AssociatedRoles with Status ACTIVE; also `aws rds add-role-to-db-cluster ...`.
3. Deploy a CloudFormation stack whose AWS::RDS::DBInstance carries AssociatedRoles -> expect CREATE_COMPLETE and the association on the instance; deploy the same template with a role whose trust policy names the wrong principal -> expect ROLLBACK_COMPLETE whose DBInstance CREATE_FAILED reason comes from RDS and names the role association.
4. Deploy a stack containing AWS::RDS::DBCluster; after CREATE_COMPLETE call rds.describe_db_clusters(DBClusterIdentifier=<template id>) and rds.create_db_cluster_snapshot on it -> expect both to succeed.
5. rds.describe_db_engine_versions(Engine='aurora-mysql', DefaultOnly=True) -> SupportedFeatureNames must not advertise s3Import/s3Export; add_role_to_db_cluster(FeatureName='s3Export') on that version must be refused.

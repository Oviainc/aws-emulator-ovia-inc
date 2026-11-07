Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-associate-role-missing-trust-policy-negative rds__s3.rds-associate-role-wrong-feature-name-negative rds__s3.iam-role-trust-s3-service-for-rds-integration-positive rds__s3.rds-remove-iam-role-association-stops-s3-access-negative rds__s3.rds-remove-role-unassociated-feature-name-not-found rds__s3.rds-s3-import-role-missing-trust-negative rds__s3.rds-s3-import-role-trust-missing-rds-principal-negative rds__s3.rds-instance-multiple-roles-different-features-positive rds__s3.cfn-rds-load-from-s3-role-missing-read-policy-negative rds__s3.rds-postgres-aws-s3-extension-missing-permission-negative rds__s3.rds-s3-import-feature-name-mismatch-negative rds__s3.rds-s3-import-missing-role-association-negative rds__s3.rds-mysql-load-data-from-s3-positive rds__s3.rds-postgres-aws-s3-extension-import-positive rds__s3.rds-s3-import-load-data-from-s3-positive rds__s3.rds-s3-import-role-removed-during-operation-negative rds__s3.rds-load-from-s3-role-association-positive rds__s3.cli-rds-import-from-s3-aurora-workflow rds__s3.cli-rds-remove-role-from-db-instance-then-import-fails-negative rds__s3.cli-rds-add-role-to-db-instance-then-verify-s3-access rds__s3.rds-s3-import-role-missing-s3-getobject-negative rds__s3.rds-s3-export-query-export-to-s3-positive rds__s3.deploy-cfn-rds-export-task-custom-resource-positive
Issue-Key: rds__s3.rds-associate-role-missing-trust-policy-negative|run#10|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.rds-associate-role-wrong-feature-name-negative|run#13|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.iam-role-trust-s3-service-for-rds-integration-positive|run#13|AssertionError|assert message == (
Issue-Key: rds__s3.rds-remove-iam-role-association-stops-s3-access-negative|run#14|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.rds-remove-role-unassociated-feature-name-not-found|run#12|AssertionError|assert message == (
Issue-Key: rds__s3.rds-s3-import-role-missing-trust-negative|run#9|AssertionError|assert message == (
Issue-Key: rds|AddRoleToDBInstance|InvalidParameterValue
Issue-Key: rds__s3.rds-instance-multiple-roles-different-features-positive|run#8|AssertionError|raise AssertionError(
Issue-Key: rds__s3.cfn-rds-load-from-s3-role-missing-read-policy-negative|run#23|AssertionError|assert "denied" in str(raised).lower() or "accessdenied" in str(raised).lower(), (
Issue-Key: rds__s3.rds-postgres-aws-s3-extension-missing-permission-negative|run#16|AssertionError|assert "s3import" in lowered and "credentials" in lowered and "accessed" in lowered, (
Issue-Key: rds__s3.rds-s3-import-feature-name-mismatch-negative|run#11|AssertionError|assert any(
Issue-Key: rds__s3.rds-s3-import-missing-role-association-negative|run#9|AssertionError|assert import_err is None, (
Issue-Key: rds__s3.rds-mysql-load-data-from-s3-positive|_mysql_load_from_s3#15|OperationalError|conn = pymysql.connect(host=host_dial, port=port_dial, user="sim2real", password=password, connect_timeout=30)
Issue-Key: rds__s3.rds-postgres-aws-s3-extension-import-positive|_postgres_import_from_s3#9|SyntaxError|cur.execute(
Issue-Key: rds__s3.rds-s3-import-load-data-from-s3-positive|_import_from_s3_with_retry#12|SyntaxError|cur.execute(
Issue-Key: rds__s3.rds-s3-import-role-removed-during-operation-negative|_import_ready#11|SyntaxError|cur.execute(
Issue-Key: rds__s3.rds-load-from-s3-role-association-positive|_load_from_s3_and_read_back#13|TimeoutError|raise TimeoutError(f"MySQL connection to {endpoint}:{port}: the run budget ended first; not graded")
Issue-Key: rds__s3.cli-rds-import-from-s3-aurora-workflow|_connect_with_retry#27|TimeoutError|raise TimeoutError(f"connection to Aurora endpoint {endpoint}: the run budget ended first; not graded") from last_exc
Issue-Key: rds__s3.cli-rds-remove-role-from-db-instance-then-import-fails-negative|_import_from_s3#21|SyntaxError|cur.execute(
Issue-Key: rds__s3.cli-rds-add-role-to-db-instance-then-verify-s3-access|_import_with_retry#22|SyntaxError|cur.execute(
Issue-Key: rds__s3.rds-s3-import-role-missing-s3-getobject-negative|_connect_with_retry#19|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-s3-export-query-export-to-s3-positive|_export_with_retry#9|SyntaxError|cur.execute(
Issue-Key: rds__s3.deploy-cfn-rds-export-task-custom-resource-positive|_deploy_cfn#15|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}: {reasons}")
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# RDS S3 integration: 3 causes — no aws_s3/LOAD DATA FROM S3 engine support, divergent Add/RemoveRoleFromDBInstance messages, and a stuck CFN custom resource

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBInstance

## Description
Three root causes, all grounded in failing checks this report does NOT mark ALREADY FILED or OWNED. Open issues 20261107T061500Z-rds-s3-6-remaining-causes-in-engine-s3-statements-and-cloudformation.md and 20261105T074335Z-rds-s3-a-provisioned-db-instance-s-reported.md own OTHER checks of this service and do not carry the replay keys below. Smithy: AddRoleToDBInstance / RemoveRoleFromDBInstance take DBInstanceIdentifier, RoleArn, FeatureName and model DBInstanceRoleAlreadyExistsFault (400) / DBInstanceRoleNotFoundFault (404) / InvalidDBInstanceStateFault (400).

Cause 1 of 3: the provisioned engine implements none of the S3 integration SQL, and the MySQL endpoint is not reachable at all.
AWS: on an S3-integrated instance, PostgreSQL's aws_s3 extension exposes aws_s3.table_import_from_s3(...)/aws_s3.query_export_to_s3(...) and MySQL/Aurora MySQL accepts LOAD DATA FROM S3 '<uri>' INTO TABLE ...; a successful call moves the rows, and a failing one raises the engine's own S3/role error (for PostgreSQL an aws_s3 error naming the inaccessible credentials and the s3Import feature; for MySQL an error naming the missing S3/role capability).
Emulator: the statements do not parse — PostgreSQL raises SyntaxError 'syntax error at or near "("' for every aws_s3 call, and MySQL cannot even be connected to: 'Can\'t connect to MySQL server on \'127.0.0.1\' ([Errno 111] Connection refused)'. One check also shows the import still refused 120 s after the role association reported ACTIVE, with the same SyntaxError, and one runs out the 180 s budget.
Why: S3 import/export through the engine — the whole point of the s3Import/s3Export features — cannot be performed, and every negative case reports a SQL parse error instead of the role/permission error a client must act on, so a permissions defect is indistinguishable from an unimplemented statement.
Fix: the RDS data-plane engine the emulator provisions: the aws_s3 extension functions on the PostgreSQL endpoint, LOAD DATA FROM S3 on the MySQL endpoint (and a listening MySQL endpoint at all), with errors derived from the instance's role associations and the S3 object's accessibility.
Traces: rds__s3.cfn-rds-load-from-s3-role-missing-read-policy-negative, .rds-postgres-aws-s3-extension-missing-permission-negative, .rds-s3-import-feature-name-mismatch-negative, .rds-s3-import-missing-role-association-negative, .rds-mysql-load-data-from-s3-positive, .rds-postgres-aws-s3-extension-import-positive, .rds-s3-import-load-data-from-s3-positive, .rds-s3-import-role-removed-during-operation-negative, .rds-load-from-s3-role-association-positive, .cli-rds-import-from-s3-aurora-workflow, .cli-rds-remove-role-from-db-instance-then-import-fails-negative, .cli-rds-add-role-to-db-instance-then-verify-s3-access, .rds-s3-import-role-missing-s3-getobject-negative, .rds-s3-export-query-export-to-s3-positive.

Cause 2 of 3: AddRoleToDBInstance / RemoveRoleFromDBInstance refuse with emulator-authored messages instead of AWS's.
AWS: a role whose trust policy does not allow rds.amazonaws.com is refused with "IAM role ARN value is invalid or does not include the required permissions for: AWS_ROLE_INTEGRATION" (fixed text; the feature token is the one being associated). An unsupported FeatureName is refused with "The feature name <FeatureName> is not valid for the engine Aurora (PostgreSQL)." — fixed text with the requested feature name and the engine's display name. RemoveRoleFromDBInstance for an association that does not exist is refused DBInstanceRoleNotFound with "The <role-arn> IAM role doesn't match with the <FeatureName> feature for the <db-instance-identifier> DB instance. Check and try again." — fixed text with the role ARN, feature name and instance id from the request.
Emulator: substitutes its own sentences: "The IAM role <arn> cannot be assumed by Amazon RDS. Add rds.amazonaws.com to the trust relationship of the role and try again."; "The feature name <x> is not supported for engine aurora-postgresql; supported feature names are Bedrock, Comprehend, Lambda, SageMaker, s3Export, s3Import."; "The IAM role <arn> is not associated with the DB instance <id> for the feature s3Import." It also accepts AddRoleToDBInstance(s3Export) for a role already associated under s3Import where the recording shows AWS refusing it.
Why: callers that branch on AWS's message (and CloudFormation reasons that quote it) diverge; the emulator's engine name and feature list are its own vocabulary, so a client cannot tell which condition AWS would have reported.
Fix: the AddRoleToDBInstance / RemoveRoleFromDBInstance validators' message construction and their duplicate-association check.
Traces: rds__s3.rds-associate-role-missing-trust-policy-negative, .rds-associate-role-wrong-feature-name-negative, .iam-role-trust-s3-service-for-rds-integration-positive, .rds-remove-iam-role-association-stops-s3-access-negative, .rds-remove-role-unassociated-feature-name-not-found, .rds-s3-import-role-missing-trust-negative, .rds-s3-import-role-trust-missing-rds-principal-negative, .rds-instance-multiple-roles-different-features-positive.

Cause 3 of 3: a CloudFormation custom resource that drives the export never leaves CREATE_IN_PROGRESS, and the rollback then fails.
AWS: a Custom:: resource is created by invoking its ServiceToken function, which signals SUCCESS/FAILED; the resource reaches CREATE_COMPLETE (or CREATE_FAILED) and the stack settles, and a rollback deletes it cleanly.
Emulator: the stack ends ROLLBACK_FAILED with reasons "Custom::RdsExportTrigger <logical-id> was still CREATE_IN_PROGRESS when the wait for it to be created ran out" plus "The following resource(s) failed to delete: [TriggerResource]" — the custom resource never settles and then cannot be deleted, leaving the stack unrecoverable.
Why: any custom-resource-driven workflow hangs and then leaves a stack stuck in ROLLBACK_FAILED, which a client cannot clear.
Fix: the CloudFormation custom-resource provisioner (invoke the ServiceToken function, consume its response signal, time out to CREATE_FAILED) and the delete path for a never-created custom resource.
Traces: rds__s3.deploy-cfn-rds-export-task-custom-resource-positive.

## Evidence
C1: expected rows moved by aws_s3.table_import_from_s3 / LOAD DATA FROM S3 (or the engine's S3/role error); got SyntaxError('syntax error at or near "("') on PostgreSQL and '(2003, "Can\'t connect to MySQL server on \'127.0.0.1\' ([Errno 111] Connection refused)")' on MySQL.
C2: expected 'IAM role ARN value is invalid or does not include the required permissions for: AWS_ROLE_INTEGRATION', 'The feature name <x> is not valid for the engine Aurora (PostgreSQL).' and 'The <arn> IAM role doesn\'t match with the s3Import feature for the <id> DB instance. Check and try again.'; got the emulator's own three sentences.
C3: expected CREATE_COMPLETE (or a clean CREATE_FAILED/ROLLBACK_COMPLETE); got ROLLBACK_FAILED with 'Custom::RdsExportTrigger ... was still CREATE_IN_PROGRESS when the wait for it to be created ran out'.

## Steps to Reproduce
1) C1: provision an S3-integrated PostgreSQL and a MySQL instance; run SELECT aws_s3.table_import_from_s3(...) and LOAD DATA FROM S3 '<uri>' INTO TABLE t, with and without the role association.
2) C2: AddRoleToDBInstance with a role not trusting rds.amazonaws.com; with FeatureName 'NotARealFeatureXYZ'; twice for the same role under s3Import then s3Export; RemoveRoleFromDBInstance for an unassociated (role, feature) pair.
3) C3: deploy a stack with a Custom:: resource that starts an export task and poll the stack to a terminal status.

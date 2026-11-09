Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cfn-deploy-rds-cluster-missing-iam-trust-negative rds__s3.cli-rds-export-task-missing-role-arn-negative rds__s3.rds-export-task-missing-iam-role-negative rds__s3.rds-export-task-wrong-bucket-negative rds__s3.deploy-rds-s3-export-stack-missing-role-negative rds__s3.rds-export-task-wrong-bucket-name-negative rds__s3.rds-iam-role-trust-wrong-principal-negative rds__s3.rds-export-snapshot-to-s3-positive rds__s3.rds-s3-export-manifest-content-positive rds__s3.cli-rds-import-from-s3-aurora-workflow rds__s3.rds-s3-import-role-missing-s3-getobject-negative rds__s3.rds-export-manifest-file-format-positive rds__s3.rds-s3-import-role-trust-missing-rds-principal-negative
Issue-Key: rds__s3|AddRoleToDBCluster|DBClusterNotFoundFault
Issue-Key: rds__s3.cli-rds-export-task-missing-role-arn-negative|_client_error#31|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: rds__s3.rds-export-task-missing-iam-role-negative|run#15|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.rds-export-task-wrong-bucket-negative|run#14|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.deploy-rds-s3-export-stack-missing-role-negative|run#22|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: rds__s3.rds-export-task-wrong-bucket-name-negative|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: rds__s3.rds-iam-role-trust-wrong-principal-negative|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: rds__s3.rds-export-snapshot-to-s3-positive|run#16|AssertionError|assert info_keys, f"no export_info summary object found among exported objects: {keys}"
Issue-Key: rds__s3.rds-s3-export-manifest-content-positive|run#19|AssertionError|assert matches, (
Issue-Key: rds__s3.cli-rds-import-from-s3-aurora-workflow|run#28|SyntaxError|cur.execute(
Issue-Key: rds__s3.rds-s3-import-role-missing-s3-getobject-negative|run#12|PublishedUrlRejected|endpoint_dial, port_dial = ctx.endpoint_host(endpoint, int(port))
Issue-Key: rds__s3.rds-export-manifest-file-format-positive|_poll_until#10|Timeout|return last
Issue-Key: rds__s3.rds-s3-import-role-trust-missing-rds-principal-negative|_pg_retry_import#13|Timeout|time.sleep(interval_s)
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# rds__s3: 9 net-new causes — export-task authorization messages name the wrong S3 action, a missing IamRoleArn is accepted, export objects omit the export_info summary, and a CFN-created cluster is invisible to AddRoleToDBCluster

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
9 net-new root causes in the RDS<->S3 export/import surface. Related open issues cover neighbouring causes but carry none of these replay keys: 20261107T185637Z-rds-s3-rds-snapshot-export-to-s3-7-causes-no.md, 20261107T185637Z-rds-s3-integration-3-causes-no-aws-s3.md, 20261107T061500Z-rds-s3-6-remaining-causes-in-engine-s3-statements-and-cloudformation.md.

Cause 1 of 9: AddRoleToDBCluster cannot see a cluster a CloudFormation stack just created.
AWS: once a stack reaches CREATE_COMPLETE for an AWS::RDS::DBCluster, DescribeDBClusters and AddRoleToDBCluster resolve that DBClusterIdentifier; DBClusterNotFoundFault means no such cluster exists. Smithy: AddRoleToDBCluster errors DBClusterNotFoundFault (404), DBClusterRoleAlreadyExists, DBClusterRoleQuotaExceeded, InvalidDBClusterStateFault.
Emulator: AddRoleToDBCluster on the stack's cluster is refused 404 DBClusterNotFoundFault 'DBCluster not found: <id>' - the CFN-provisioned cluster does not exist in RDS state.
Why: a client that provisions a cluster by template and then attaches a role (the documented S3-integration path) cannot; the stack reports success while the resource is absent.
Fix: the AWS::RDS::DBCluster provider in the CloudFormation service - it must create the cluster in RDS state under the identifier it reports.
Traces: rds__s3.cfn-deploy-rds-cluster-missing-iam-trust-negative.

Cause 2 of 9: StartExportTask is accepted with no usable IamRoleArn.
AWS: IamRoleArn is a required member of StartExportTaskMessage (Smithy: required), and a request naming a role that cannot be assumed/does not exist is refused - IamRoleNotFound (404) or IamRoleMissingPermissions (400) - before any task starts.
Emulator: the call answers 200 and returns an ExportTask in STARTING with the bogus role echoed back.
Why: a client's negative test cannot pass, and a misconfigured export appears to start and then has no failure to report, so the data never lands and nothing explains it.
Fix: the StartExportTask handler's role resolution/validation.
Traces: rds__s3.cli-rds-export-task-missing-role-arn-negative.

Cause 3 of 9: the export authorization refusal names s3:PutObject where AWS names s3:GetBucketLocation.
AWS: when the export role cannot reach the bucket, StartExportTask refuses with the message "The IAM role <role-arn> isn't authorized to call s3:GetBucketLocation on the S3 bucket <bucket>." - the fixed text with the role ARN and bucket name from the request; GetBucketLocation is the first call RDS makes, so it is the action named.
Emulator: the same sentence but with s3:PutObject in place of s3:GetBucketLocation.
Why: an operator fixes the wrong IAM statement - adding s3:PutObject will not make the export work, because the call that actually failed is GetBucketLocation.
Fix: the authorization probe order and message template in the StartExportTask handler.
Traces: rds__s3.rds-export-task-missing-iam-role-negative, rds__s3.rds-export-task-wrong-bucket-negative, rds__s3.deploy-rds-s3-export-stack-missing-role-negative.

Cause 4 of 9: the nonexistent-bucket refusal is worded as an authorization failure.
AWS: StartExportTask naming a bucket that does not exist in the region is refused InvalidS3BucketFault with "The S3 bucket <bucket> isn't valid. Either it isn't located in the current AWS Region or it doesn't exist." - fixed text with the requested bucket name substituted.
Emulator: 'The specified Amazon S3 bucket name <bucket> can't be found or Amazon RDS isn't authorized to access the specified Amazon S3 bucket.'
Why: the two causes AWS keeps apart - bucket absent/wrong region vs role not authorized - are merged, so a client cannot tell which to fix.
Fix: the bucket-existence check and its message in the StartExportTask handler.
Traces: rds__s3.rds-export-task-wrong-bucket-name-negative.

Cause 5 of 9: the wrong-trust-principal refusal is emulator-authored.
AWS: when the export role's trust policy does not allow export.rds.amazonaws.com, StartExportTask refuses with "The principal export.rds.amazonaws.com isn't allowed to assume the IAM role <role-arn> or the IAM role <role-arn> doesn't exist." - fixed text, with the service principal and the role ARN from the request.
Emulator: 'The IAM role <role-arn> is missing for exporting to an Amazon S3 bucket. The role must exist and its trust relationship must allow export.rds.amazonaws.com to assume it.'
Why: a client matching AWS's sentence (or extracting the principal from it) matches nothing.
Fix: the trust-policy check's message template in the StartExportTask handler.
Traces: rds__s3.rds-iam-role-trust-wrong-principal-negative.

Cause 6 of 9: a completed export writes no export_info summary object and its metadata does not reference the source snapshot ARN.
AWS: a snapshot export lays down, under <prefix>/<export-task-identifier>/, an export_info_<...> summary object alongside the per-table export_tables_info_<...> objects, and the exported JSON metadata references the source snapshot's ARN.
Emulator: the only object written is '<export-task>/export_tables_info_<export-task>_from_1_to_1.json', with no export_info summary, and none of the JSON metadata objects contains the source snapshot ARN.
Why: a client that locates an export's manifest by the export_info key, or that ties exported data back to the snapshot it came from, finds neither.
Fix: the export-task object writer in the RDS export implementation (its key layout and metadata content).
Traces: rds__s3.rds-export-snapshot-to-s3-positive, rds__s3.rds-s3-export-manifest-content-positive.

Cause 7 of 9: the aws_s3 PostgreSQL import statement is a syntax error.
AWS: on RDS/Aurora PostgreSQL, 'SELECT aws_s3.table_import_from_s3(<table>, <cols>, <options>, aws_commons.create_s3_uri(...))' is valid and loads the object into the table.
Emulator: the statement is refused with psycopg SyntaxError 'syntax error at or near "("'.
Why: the documented S3-import path for PostgreSQL does not exist, so no client can load data from S3 through the engine.
Fix: the aws_s3/aws_commons extension functions in the PostgreSQL dialect of the SQL engine.
Traces: rds__s3.cli-rds-import-from-s3-aurora-workflow.

Cause 8 of 9: the instance endpoint used by the import flow is published as an unreachable AWS hostname.
AWS: the Endpoint a client connects to must be an address that answers; an emulator must publish its own host.
Emulator: '<instance>.<hex>.<region>.rds.amazonaws.com:5432' is published and refused as not one of the emulator's own hosts, so the import negative case never reaches the engine.
Why: the connection string the API hands out cannot be dialled, so the S3-import permission boundary is unobservable.
Fix: the instance endpoint publisher in the RDS instance projection (same component as the reader/proxy endpoint publishing filed for service rds).
Traces: rds__s3.rds-s3-import-role-missing-s3-getobject-negative.

Cause 9 of 9: two export/import checks never settle inside their 60 s budget.
AWS: StartExportTask moves to COMPLETE (and its objects appear), and an import whose role trust is missing is refused, well inside a minute.
Emulator: both checks time out after 60.0 s with no terminal export status and no refusal (recorded behavior only).
Why: a client waiting on an export task's terminal Status, or on the import's refusal, waits forever and gets no error to act on.
Fix: the export-task state machine's progression to a terminal Status and the S3-import authorization path's refusal in the RDS service.
Traces: rds__s3.rds-export-manifest-file-format-positive, rds__s3.rds-s3-import-role-trust-missing-rds-principal-negative.

## Evidence
Cause 1: DBClusterNotFoundFault when calling AddRoleToDBCluster: DBCluster not found: <cluster-id> (after the stack reported the cluster created).
Cause 2: real AWS rejects this call, but it SUCCEEDED and returned an ExportTask with Status 'STARTING' (HTTP 200).
Cause 3: expected "...isn't authorized to call s3:GetBucketLocation on the S3 bucket <bucket>."; got "...isn't authorized to call s3:PutObject on the S3 bucket <bucket>." (three checks).
Cause 4: expected "The S3 bucket <bucket> isn't valid. Either it isn't located in the current AWS Region or it doesn't exist."; got "The specified Amazon S3 bucket name <bucket> can't be found or Amazon RDS isn't authorized to access the specified Amazon S3 bucket."
Cause 5: expected "The principal export.rds.amazonaws.com isn't allowed to assume the IAM role <arn> or the IAM role <arn> doesn't exist."; got 'The IAM role <arn> is missing for exporting to an Amazon S3 bucket...'.
Cause 6: no export_info summary object found among exported objects: ['<task>/export_tables_info_<task>_from_1_to_1.json']; none of the export's JSON metadata objects reference the source snapshot ARN.
Cause 7: psycopg.errors.SyntaxError: syntax error at or near "(".
Cause 8: PublishedUrlRejected: endpoint_host was given sim2real-instance-<hex>.<hex>.us-west-2.rds.amazonaws.com:5432 - not one of the emulator's own hosts.
Cause 9: Check timed out after 60.0s (twice).

## Steps to Reproduce
Cause 1: deploy a stack with an AWS::RDS::DBCluster, then call AddRoleToDBCluster with that cluster identifier.
Cause 2: StartExportTask with an IamRoleArn that does not exist.
Cause 3: StartExportTask with a role whose policy grants nothing on the bucket, and read the message.
Cause 4: StartExportTask naming a bucket that does not exist.
Cause 5: StartExportTask with a role whose trust policy omits export.rds.amazonaws.com.
Cause 6: run an export to completion and list the objects under the task prefix; read their JSON.
Cause 7: on an Aurora PostgreSQL session, call aws_s3.table_import_from_s3(...).
Cause 8: read DescribeDBInstances' Endpoint.Address for the import instance and connect to it.
Cause 9: run the manifest-format export check and the import-trust-missing check and observe neither settles in 60 s.

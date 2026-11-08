Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-s3-import-role-trust-missing-rds-principal-negative
Issue-Key: rds__s3.rds-s3-import-role-trust-missing-rds-principal-negative|_pg_retry_import#13|AssertionError|raise AssertionError(
Issue-Commit: 0a1e14ce9c5d04d4281ea317c037d5ece999a2b8

# rds__s3: the emulated PostgreSQL engine has no aws_s3 extension — aws_s3.table_import_from_s3(...) fails to parse, so the s3Import role-association positive control can never succeed (1 cause)

> _Note: this issue was identified against the code base at commit `0a1e14ce9c5d04d4281ea317c037d5ece999a2b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBInstance

## Description
Cause 1 of 1 — on an RDS for PostgreSQL instance reached over its native endpoint, calling the aws_s3 extension's import function is rejected by the SQL parser, so no S3 import can be performed even after the s3Import role association has reached ACTIVE.
AWS: once an IAM role is associated with the instance under FeatureName 's3Import' and the association's Status reaches ACTIVE, `CREATE EXTENSION aws_s3` makes the function aws_s3.table_import_from_s3(table, columns, options, bucket, key, region) available; the call executes using the associated role's credentials and returns a result row describing the rows imported. The control-plane half of the behaviour (AddRoleToDBInstance refusing a role whose trust policy omits rds.amazonaws.com with InvalidParameterValue and the message 'IAM role ARN value is invalid or does not include the required permissions for: AWS_ROLE_INTEGRATION', and never listing it in DescribeDBInstances AssociatedRoles) is the negative under test; the engine-side import is its positive control. Recorded behavior only for the engine's result text; the requirement here is that the function exists and executes rather than failing to parse.
Emulator: the statement does not parse — the engine answers `syntax error at or near "("` — repeatedly for 120s after the correctly-trusted role association reached ACTIVE, so the import never runs and the check can never reach its assertions.
Why: a client that uses Aurora/RDS PostgreSQL S3 integration (aws_s3.table_import_from_s3 / aws_s3.query_export_to_s3) cannot exercise it at all, and any test that proves 'a correctly trusted role makes the import work' is impossible to pass, so the whole s3Import authorization boundary is unverifiable.
Fix: the RDS native PostgreSQL data plane — register the aws_s3 extension's functions (table_import_from_s3 and its siblings) so the statement parses and executes against the associated s3Import role's credentials. Related open issue that does NOT carry this check: 20261107T185637Z-rds-s3-integration-3-causes-no-aws-s3.md.
Traces: rds__s3.rds-s3-import-role-trust-missing-rds-principal-negative.

## Evidence
Expected: after AddRoleToDBInstance(FeatureName='s3Import') with a role trusting rds.amazonaws.com converges to ACTIVE, `SELECT aws_s3.table_import_from_s3(...)` executes and returns its import summary row.
Actual: every attempt within the 120s window fails at the parser. Replay detail: `AssertionError: aws_s3.table_import_from_s3 did not succeed within 120.0s of the role association reaching ACTIVE; last error: syntax error at or near "("`.

## Steps to Reproduce
1. Create a bucket and upload a CSV (id,name rows) at imports/data.csv.
2. Create a PostgreSQL DB instance reachable on 5432 and an IAM role that trusts rds.amazonaws.com with s3:GetObject/s3:ListBucket on that bucket.
3. AddRoleToDBInstance(DBInstanceIdentifier, RoleArn, FeatureName='s3Import') and poll DescribeDBInstances until the AssociatedRoles entry is ACTIVE.
4. Connect with psycopg, run `CREATE EXTENSION IF NOT EXISTS aws_s3 CASCADE;` then `SELECT aws_s3.table_import_from_s3('<table>', '', '(format csv)', '<bucket>', 'imports/data.csv', '<region>');`.
5. Expect a result row and the rows imported; observe `syntax error at or near "("`.
6. Replay: rds__s3.rds-s3-import-role-trust-missing-rds-principal-negative.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-mysql-load-data-from-s3-missing-role-negative rds__s3.rds-s3-import-missing-role-association-negative rds__s3.rds-s3-import-role-missing-s3-getobject-negative rds__s3.cfn-rds-load-from-s3-role-missing-read-policy-negative rds__s3.deploy-rds-instance-missing-associated-role-negative
Issue-Key: rds__s3.rds-mysql-load-data-from-s3-missing-role-negative|run#17|AssertionError|assert "S3" in error_detail, f"expected the rejection to name the missing S3/role capability, got: {error_detail}"
Issue-Key: rds__s3.rds-s3-import-missing-role-association-negative|_wait_for_pg_connect#8|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-s3-import-role-missing-s3-getobject-negative|_add_role_with_retry#16|Timeout|time.sleep(delay_s)
Issue-Key: rds__s3.cfn-rds-load-from-s3-role-missing-read-policy-negative|_connect_with_retry#22|Timeout|time.sleep(5.0)
Issue-Key: rds__s3.deploy-rds-instance-missing-associated-role-negative|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Commit: 6cc7c0f4d58adfa1ddf47e8354a4acf1282f7003

# rds__s3: a provisioned DB instance's reported Endpoint serves no MySQL/PostgreSQL wire protocol, so every S3 import check never opens a session

> _Note: this issue was identified against the code base at commit `6cc7c0f4d58adfa1ddf47e8354a4acf1282f7003`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Cause 1 of 1: an RDS DB instance that the emulator reports as created (including one created by CloudFormation from an AWS::RDS::DBInstance) announces an `Endpoint` that no database engine answers on, so no SQL session can be opened and no S3-import behaviour can be exercised.
AWS: Smithy `Endpoint` = { `Address` "Specifies the DNS address of the DB instance", `Port` "Specifies the port that the database engine is listening on", `HostedZoneId` }, returned by CreateDBInstance/DescribeDBInstances. Once the instance is `available`, a client that connects to Address:Port with the engine's native protocol gets a session: MySQL on 3306, PostgreSQL on 5432. The S3-integration behaviour under test is then engine-side — `LOAD DATA FROM S3` on MySQL and `aws_s3.table_import_from_s3` (aws_s3 extension) on PostgreSQL — and with no IAM role associated for the s3Import feature the engine REFUSES the statement with an error that names the missing S3/role capability, leaving the target table empty, while the identical statement succeeds once the role/feature is associated.
Emulator: nothing listens on the announced endpoint. The MySQL check gets the client-side connect failure `(2003, "Can't connect to MySQL server on '127.0.0.1' ([Errno 111] Connection refused)")` instead of an engine error naming the missing S3/role capability; the PostgreSQL checks sit in their connect poll until `Error: Check timed out after 60.0s`. The address in that message is a per-run value — the requirement is that the address the emulator announces is one it answers the engine protocol on.
Why: the whole S3-import surface is ungradable and, worse, indistinguishable: a client cannot tell a correctly refused import (no role associated, or a role without s3:GetObject) from an unreachable database, because both present as a transport failure rather than an engine error. Anything that connects to the endpoint RDS gave it — an application, a migration, a smoke test — fails at connect against the emulator while succeeding against AWS.
Fix: the RDS DB-instance provisioning/state component must either serve the engine protocol on the Address:Port it reports, or report an endpoint that is reachable; the engine-side S3-import statement handling (`LOAD DATA FROM S3`, `aws_s3.table_import_from_s3`) and its role/feature authorization refusal belong behind that listener.
Related open issue that does NOT carry these checks' replay keys (so it does not cover them): 20261104T053605Z-rds-the-published-db-instance-cluster-endpoint-serves.md reports the same class for other checks under the `rds` heading; this issue is grounded in the five rds__s3 checks in trace_ids, which the replay report marked neither ALREADY FILED nor OWNED.
Traces: the 5 check ids in trace_ids.

## Evidence
Attributed instance — rds__s3.rds-mysql-load-data-from-s3-missing-role-negative
  Expected: the engine refuses LOAD DATA FROM S3 with an error naming the missing S3/role capability, and the target table stays empty.
  Actual:   AssertionError: expected the rejection to name the missing S3/role capability, got: (2003, "Can't connect to MySQL server on '127.0.0.1' ([Errno 111] Connection refused)")  (address/port shown are per-run values)

Unattributed instances — the other 4 checks in trace_ids (PostgreSQL / CFN-deployed instances)
  Expected: psycopg connects to the reported Endpoint.Address:Port once the instance is available, CREATE EXTENSION aws_s3 and CREATE TABLE succeed, and aws_s3.table_import_from_s3 is refused for an instance with no s3Import role association while the identical call succeeds after association.
  Actual:   Error: Check timed out after 60.0s — the connect poll (_wait_for_pg_connect) never succeeded.

## Steps to Reproduce
PostgreSQL: CreateDBInstance (engine postgres) in a subnet group with a security group open on 5432; wait for available; read Endpoint.Address/Port from DescribeDBInstances; connect with psycopg. Expect a session; the emulator never accepts the connection. Same via a CloudFormation AWS::RDS::DBInstance with no AssociatedRoles.

MySQL: CreateDBInstance (engine mysql), security group open on 3306; connect to the reported Endpoint and run LOAD DATA FROM S3 's3://<bucket>/<key>' with no s3Import role associated. Expect an engine error naming the missing S3/role capability; the emulator gives MySQL client error 2003 connection refused.

Replay the 5 checks in trace_ids to confirm.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-load-from-s3-missing-role-negative rds__s3.rds-load-from-s3-role-association-positive rds__s3.rds-mysql-load-data-from-s3-positive rds__s3.rds-mysql-load-data-from-s3-missing-role-negative
Issue-Key: rds__s3.rds-load-from-s3-missing-role-negative|run#21|AssertionError|raise AssertionError(
Issue-Key: rds__s3.rds-load-from-s3-role-association-positive|_load_from_s3_and_read_back#13|ProgrammingError|cur.execute(
Issue-Key: rds__s3.rds-mysql-load-data-from-s3-positive|_mysql_load_from_s3#15|ProgrammingError|cur.execute(
Issue-Key: rds__s3.rds-mysql-load-data-from-s3-missing-role-negative|_mysql_load_from_s3#22|ProgrammingError|cur.execute(
Issue-Commit: 8f7c6ce558c7d939146a7c42cc95df5134e5d406

# Aurora MySQL S3 integration: AddRoleToDBCluster accepts the retired s3Import feature name, and the engine rejects LOAD DATA FROM S3 as a syntax error

> _Note: this issue was identified against the code base at commit `8f7c6ce558c7d939146a7c42cc95df5134e5d406`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBCluster

## Description
Two causes.

Cause 1 of 2: AddRoleToDBCluster(FeatureName="s3Import") on an Aurora MySQL DB cluster is accepted, where AWS refuses it.
Failing check ids: rds__s3.rds-load-from-s3-missing-role-negative
AWS: Smithy (com-amazonaws-rds-amazonrdsv19) AddRoleToDBCluster takes DBClusterIdentifier (required), RoleArn (required, pattern ^arn:aws[a-z-]*:iam::[0-9]*:role/.*$) and FeatureName, and models DBClusterNotFoundFault (404), DBClusterRoleAlreadyExistsFault (400, awsQueryError DBClusterRoleAlreadyExists), DBClusterRoleQuotaExceededFault (400), InvalidDBClusterStateFault (400). Recorded behavior for an unsupported feature name: HTTP 400 with Error.Code "InvalidParameterValue" and Error.Message "The feature name s3Import is not valid for the engine Aurora MySQL." — fixed text "The feature name ", " is not valid for the engine ", ".", with the feature name taken from the request's FeatureName and the engine name from the cluster's engine. s3Import is not a valid feature name on any currently available aurora-mysql engine version; an association for a supported feature (FeatureName="Bedrock") on the same cluster still succeeds and reaches Status ACTIVE, which is the check's positive control, so the refusal is specific to the feature name and not a blanket denial.
Emulator: the call succeeds, associating the role. The replay asserts: "AddRoleToDBCluster(FeatureName='s3Import') unexpectedly succeeded".

Cause 2 of 2: the Aurora MySQL engine does not implement the Aurora extension statement LOAD DATA FROM S3 — it is rejected by the SQL parser as a syntax error, in every configuration.
Failing check ids: rds__s3.rds-load-from-s3-role-association-positive, rds__s3.rds-mysql-load-data-from-s3-positive, rds__s3.rds-mysql-load-data-from-s3-missing-role-negative
AWS: recorded behavior only (this is an engine-level statement, not an API operation). Recorded: on an Aurora MySQL cluster whose cluster parameter group names the import role (aws_default_s3_role / aurora_load_from_s3_role) and which has the role associated, `LOAD DATA FROM S3 's3://<bucket>/<key>' INTO TABLE <table> FIELDS TERMINATED BY ','` parses, executes, and loads exactly the rows of the S3 object (so the loaded rows equal the object's CSV rows). Without an associated import role the same statement still parses but fails at execution with an error naming the missing S3/role capability, and loads no rows — the failure is an authorization/credentials failure, not a parse failure.
Emulator: the statement never parses. The engine answers MySQL error 1064: "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near 'LOAD DATA FROM S3 's3://<bucket>/<key>' INTO ' at line 1" — identically on the authorized path and the unauthorized path. The bucket and key in the message are per-run values; the divergence is that a supported Aurora statement is reported as a syntax error.

Why: Cause 1 is overly permissive — a client wiring an Aurora S3 import gets a successful association for a feature name AWS rejects, so it believes the import capability is configured and only discovers the refusal against real AWS; the specific InvalidParameterValue message is also the signal that tells an operator to use a supported feature name. Cause 2 makes the whole Aurora S3-import data path unusable and, worse, collapses two different outcomes into one: because the statement fails to parse whether or not the role is associated, a client cannot distinguish "your cluster has no s3Import role" (an authorization problem with a specific remedy) from "this statement is not supported", and a least-privilege negative test that expects an S3/credentials error passes for the wrong reason while the positive control fails.

Fix: Cause 1 belongs in the AddRoleToDBCluster handler's input validation — it must reject FeatureName values the cluster's engine does not support with InvalidParameterValue and AWS's message template, while continuing to accept supported names such as Bedrock. Cause 2 belongs in the emulated Aurora MySQL engine's SQL surface: the LOAD DATA FROM S3 statement must be parsed and executed against the cluster's associated s3Import role (reading the named object through S3 and inserting its rows), and must fail at execution — naming the missing S3/role capability — when no import role is associated, rather than failing to parse.

Traces: Cause 1 — rds__s3.rds-load-from-s3-missing-role-negative. Cause 2 — rds__s3.rds-load-from-s3-role-association-positive, rds__s3.rds-mysql-load-data-from-s3-positive, rds__s3.rds-mysql-load-data-from-s3-missing-role-negative. The open issue 20261107T185637Z-rds-s3-integration-3-causes-no-aws-s3.md covers other rds__s3 integration causes and does not carry these replay keys.

## Evidence
Cause 1 — rds__s3.rds-load-from-s3-missing-role-negative
  Setup: Aurora MySQL cluster whose cluster parameter group does NOT set aws_default_s3_role/aurora_load_from_s3_role and with no role associated; a second, distinct rds.amazonaws.com-trusted role is first associated with FeatureName="Bedrock" and reaches Status ACTIVE (positive control, proving association works on this cluster).
  Expected (AWS): add_role_to_db_cluster(DBClusterIdentifier=<cluster>, RoleArn=<import-role>, FeatureName="s3Import") raises ClientError with Error.Code == "InvalidParameterValue" and Error.Message == "The feature name s3Import is not valid for the engine Aurora MySQL."
  Actual (emulator): the call returns successfully. AssertionError: AddRoleToDBCluster(FeatureName='s3Import') unexpectedly succeeded; AWS may have restored the feature -- if so this check needs re-auditing

Cause 2 — three checks, one engine-level symptom
  rds__s3.rds-load-from-s3-role-association-positive (role associated, parameter group names the role; expects rows == [(1,'alice'),(2,'bob'),(3,'carol')] from the loaded CSV):
    pymysql.err.ProgrammingError: (1064, "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near 'LOAD DATA FROM S3 's3://<bucket>/import-data/rows.csv' INTO ' at line 1")
  rds__s3.rds-mysql-load-data-from-s3-positive (role associated and applied after reboot; expects the object's rows loaded):
    pymysql.err.ProgrammingError: (1064, "... near 'LOAD DATA FROM S3 's3://<bucket>/imports/rows.csv' INTO ' at line 1")
  rds__s3.rds-mysql-load-data-from-s3-missing-role-negative (no role associated; expects a failure naming the missing S3/role capability and zero rows, then the same statement to work after associating the role and rebooting):
    pymysql.err.ProgrammingError: (1064, "... near 'LOAD DATA FROM S3 's3://<bucket>/imports/rows.csv' INTO ' at line 1")
  Expected (AWS): the statement parses in all three; it loads the object's rows when an s3Import role is associated, and fails at execution with an S3/role error (loading nothing) when one is not.

## Steps to Reproduce
Cause 1:
1. Create an Aurora MySQL cluster (subnet group, security group, cluster parameter group without aws_default_s3_role).
2. Create two distinct roles trusted by rds.amazonaws.com: an S3-import role and a bare probe role.
3. add_role_to_db_cluster(DBClusterIdentifier=<cluster>, RoleArn=<probe-role>, FeatureName="Bedrock") and wait for that association to reach Status ACTIVE — it succeeds (control).
4. add_role_to_db_cluster(DBClusterIdentifier=<cluster>, RoleArn=<import-role>, FeatureName="s3Import"): the emulator returns success; AWS raises InvalidParameterValue "The feature name s3Import is not valid for the engine Aurora MySQL."

Cause 2:
1. PutObject a CSV at s3://<bucket>/imports/rows.csv.
2. Create an Aurora MySQL cluster plus instance; create a cluster parameter group that sets the s3 import role and associate the role with the cluster; reboot the instance so the credentials reach the engine.
3. Connect over MySQL and run: LOAD DATA FROM S3 's3://<bucket>/imports/rows.csv' INTO TABLE <table> FIELDS TERMINATED BY ',';
4. The emulator answers MySQL error 1064 (syntax error near 'LOAD DATA FROM S3 ...'); AWS parses and loads the object's rows.
5. Repeat on a cluster with no associated import role: AWS parses the statement and fails at execution naming the missing S3/role capability with zero rows loaded, while the emulator again answers 1064, so the two cases are indistinguishable.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data.catalogue-reflects-created-table-and-absent-table-over-permissive redshift-data.execute-statement-invalid-target-combination-rejected
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data|ListDatabases|ResourceNotFoundException
Issue-Key: redshift-data.execute-statement-invalid-target-combination-rejected|run#10|AssertionError|assert msg == _ONE_TARGET_MESSAGE, msg
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# redshift-data serverless catalogue and target validation diverge: ListDatabases 404s on the namespace's own database, and a both-targets ExecuteStatement is refused with the wrong message (2 causes)

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDatabases

## Description
Two net-new root causes in the redshift-data surface over a Redshift Serverless workgroup. Related but NOT covering them: the open issues 20261022T180846Z-redshift-data-statement-execution-engine-is-not-redshift.md (statement-engine faces: absent database FINISHES, CREATE SCHEMA/LOCK TABLE/:: casts, sessions, STARTED) and 20261022T180846Z-redshift-data-data-api-surface-diverges-resultformat-echoed.md (ResultFormat echo, GetStatementResult, ListSessions). Neither names the ListDatabases refusal below nor the target-combination message, and the replay marked neither check ALREADY FILED nor OWNED. This bundle carries TWO causes; both must be closed before submitting.

=== Cause 1 of 2: ListDatabases refuses the namespace's own database with ResourceNotFoundException instead of listing it ===
Failing check: redshift-data.catalogue-reflects-created-table-and-absent-table-over-permissive

AWS: Smithy (com.amazonaws.redshiftdata) ListDatabases is @readonly and paginated (items Databases, inputToken/outputToken NextToken, pageSize MaxResults 0..1000); its input takes WorkgroupName (length 3..128) and an optional Database naming the database to connect through, and its output is a DatabaseList. Its modeled errors are DatabaseConnectionException(500), InternalServerException(500), QueryTimeoutException(400), ResourceNotFoundException(404) and ValidationException(400). Recorded behavior: on a Redshift Serverless workgroup whose namespace was created with dbName=devdb, ListDatabases(WorkgroupName=<wg>, Database='devdb') answers HTTP 200 with Databases containing that initial database (plus the server's built-in databases), because the namespace's dbName IS a real database of that namespace. ResourceNotFoundException is for a target that does not exist (a missing workgroup/cluster), not for the namespace's own database.

Emulator: the call is refused with ResourceNotFoundException (HTTP 404) and message 'Database does not exist: devdb' — fixed text 'Database does not exist: ' with the Database member echoed from the request. The database named is exactly the dbName passed to redshift-serverless CreateNamespace moments earlier, so the redshift-data catalogue does not see the database the namespace was created with. Hypothesis: redshift-data resolves Database against its own (empty) database registry rather than against the namespace record created by redshift-serverless.

Why: the first catalogue call a client makes against a freshly provisioned serverless workgroup fails with a 404, so the entire documented discovery path (ListDatabases -> ListSchemas -> ListTables -> DescribeTable) is unreachable, and an exception class reserved for missing resources is raised for a resource that exists. Clients that treat ResourceNotFoundException as 'the workgroup/database was deleted' take a terminal error branch on a healthy namespace.

Fix: the ListDatabases handler (and the shared target/database resolution it shares with ListSchemas/ListTables/DescribeTable and ExecuteStatement) plus the namespace/database state it reads — register the namespace's dbName as a database when redshift-serverless CreateNamespace succeeds, and resolve the request's Database against that state so the call returns 200 with the database listed.

Traces: redshift-data.catalogue-reflects-created-table-and-absent-table-over-permissive.

=== Cause 2 of 2: ExecuteStatement with BOTH ClusterIdentifier and WorkgroupName is refused with the cluster-lookup message instead of the mutual-exclusivity message ===
Failing check: redshift-data.execute-statement-invalid-target-combination-rejected

AWS: Smithy: ExecuteStatementInput carries ClusterIdentifier, WorkgroupName, SecretArn/DbUser and SessionId as independent optional members and ValidationException(400) among its errors; the model does not encode the exclusivity, so the wording is recorded behavior: exactly one target must be given, and both the no-target form and the both-targets form are refused synchronously with ValidationException / HTTP 400 and the message, verbatim and with no variable part, 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'. The refusal is a request-validation decision taken BEFORE any target is resolved, so a nonexistent ClusterIdentifier in the request does not change it, and no statement record is created.

Emulator: the no-target form is refused correctly, but ExecuteStatement(ClusterIdentifier=<nonexistent cluster>, WorkgroupName=<existing workgroup>, Database=..., Sql='SELECT 1') is refused with ValidationException whose message is 'Redshift endpoint doesn't exist in this region.' — the handler skips the exclusivity check and goes straight to resolving the supplied ClusterIdentifier, reporting the lookup failure instead. The validation precedence is inverted and the message is a different error class's text.

Why: clients and SDK test suites match on this message to tell 'you wired two targets' from 'your cluster is gone'. A caller that accidentally passes both a cluster and a workgroup is told the cluster does not exist in the region and goes looking for a provisioning or region problem, and a caller whose cluster really is missing cannot be distinguished from one with a malformed target set.

Fix: ExecuteStatement's request validation (and the same precedence in BatchExecuteStatement and the catalogue operations that accept the same target members) — reject the zero-target and multi-target combinations first, with ValidationException/400 and the exact one-target message, before any cluster/workgroup/session resolution runs, and leave no statement record behind.

Traces: redshift-data.execute-statement-invalid-target-combination-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1 — redshift-data 12/24 (50%), 12 failed (10 of them OWNED by existing issues and skipped).

Cause 1 — check_redshift_data_catalogue_reflects_created_table_and_absent_table_over_permissive.py (run, first catalogue call) after redshift-serverless CreateNamespace(dbName='devdb') + CreateWorkgroup(status AVAILABLE)
  expected: list_databases(WorkgroupName=<wg>, Database='devdb') -> HTTP 200, Databases containing 'devdb'
  actual:   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListDatabases operation: Database does not exist: devdb   [HTTP 404; fixed text 'Database does not exist: ' + the request's Database]

Cause 2 — check_redshift_data_execute_statement_invalid_target_combination_rejected.py:43
  expected: execute_statement(ClusterIdentifier='sim2real-bogus-cluster', WorkgroupName=<wg>, Database=<db>, Sql='SELECT 1') -> ValidationException / HTTP 400, Message exactly 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'
  actual:   AssertionError: Redshift endpoint doesn't exist in this region.   (code ValidationException, but the cluster-lookup message)
  (the preceding no-target leg passes, so only the precedence/message for the both-targets form diverges)

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): ListDatabases @readonly, paginated(items=Databases, MaxResults 0..1000), errors include ResourceNotFoundException(404)/ValidationException(400); ExecuteStatementInput.ClusterIdentifier (pattern ^[a-z][a-z0-9]*(-[a-z0-9]+)*$, 1..63) and .WorkgroupName (3..128) are independent optional members, errors include ValidationException(400).

## Steps to Reproduce
Cause 1: 1) Boot the emulator. 2) redshift-serverless create_namespace(namespaceName=<ns>, adminUsername='adminuser', adminUserPassword=<pw>, dbName='devdb'); create_workgroup(workgroupName=<wg>, namespaceName=<ns>, baseCapacity=8, subnetIds=[...]) and poll get_workgroup until status='AVAILABLE'. 3) redshift-data list_databases(WorkgroupName=<wg>, Database='devdb') -> expect 200 with 'devdb' in Databases; observe ResourceNotFoundException/404 'Database does not exist: devdb'. 4) Re-replay redshift-data.catalogue-reflects-created-table-and-absent-table-over-permissive.
Cause 2: 5) With the same workgroup, redshift-data execute_statement(Database='devdb', Sql='SELECT 1') with no target -> ValidationException/400 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId' (passes today). 6) execute_statement(ClusterIdentifier='sim2real-bogus-cluster', WorkgroupName=<wg>, Database='devdb', Sql='SELECT 1') -> expect the same ValidationException/400 message; observe ValidationException with "Redshift endpoint doesn't exist in this region." 7) list_statements(WorkgroupName=<wg>) before/after to confirm no statement record is created by either rejection. 8) Re-replay redshift-data.execute-statement-invalid-target-combination-rejected.

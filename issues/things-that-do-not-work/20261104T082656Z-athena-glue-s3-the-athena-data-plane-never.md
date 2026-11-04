Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.athena-counts-cloudfront-rows-skipping-two-header-lines-in-every-file athena__glue__s3.athena-glue-catalog-cross-account-missing-permission-negative athena__glue__s3.athena-glue-catalog-missing-permission-negative athena__glue__s3.athena-partition-projection-reads-glue-partitions-positive athena__glue__s3.athena-query-large-result-multipart-s3-positive athena__glue__s3.athena-query-nonexistent-glue-table-negative athena__glue__s3.athena-query-on-a-missing-table-or-schema-fails-not-found athena__glue__s3.athena-query-reads-csv-data-from-s3-positive athena__glue__s3.athena-query-uses-glue-catalog-table-positive athena__glue__s3.athena-reads-gzip-and-nested-objects-under-the-table-location athena__glue__s3.athena-status-histogram-matches-the-seeded-log-lines athena__glue__s3.cli-athena-query-missing-glue-table-error-negative athena__glue__s3.cli-glue-create-table-athena-query-s3-roundtrip athena__glue__s3.dash-fields-in-numeric-columns-read-as-null-and-typed-columns-aggregate athena__glue__s3.glue-accepts-a-location-in-an-absent-bucket-that-athena-rejects-at-query-time athena__glue__s3.glue-accepts-an-invalid-column-type-that-athena-rejects-at-query-time athena__glue__s3.glue-catalog-table-location-points-to-s3-positive athena__glue__s3.root-located-table-reads-athena-result-objects-back-as-rows athena__glue__s3.athena-ddl-create-table-registers-in-glue-catalog-positive athena__glue__s3.athena-ddl-table-differs-from-the-glue-api-table-in-owner-partitions-and-location athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl athena__glue__s3.e2e-create-athena-tables-from-s3-lifecycle athena__glue__s3.athena-create-database-comment-lands-as-the-glue-description athena__glue__s3.athena-ddl-surfaces-external-table-in-the-glue-data-catalog-and-reads-it athena__glue__s3.athena-list-databases-reflects-glue-database-positive athena__glue__s3.cli-athena-start-query-execution-poll-results-s3 athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected athena__glue__s3.athena-query-to-an-output-bucket-that-does-not-exist-is-rejected-until-it-is-created athena__glue__s3.athena-missing-s3-output-bucket-permission-negative athena__glue__s3.athena-s3-bucket-policy-blocks-query-results-write-negative athena__glue__s3.athena-query-output-format-preserved-in-s3-positive athena__glue__s3.deploy-cfn-athena-workgroup-enforce-configuration-positive athena__glue__s3.deploy-cfn-athena-workgroup-output-location-s3-positive
Issue-Key: athena__glue__s3.athena-counts-cloudfront-rows-skipping-two-header-lines-in-every-file|_select#10|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.athena-glue-catalog-cross-account-missing-permission-negative|run#13|AssertionError|assert reason_bad == expected_reason, reason_bad
Issue-Key: athena__glue__s3.athena-glue-catalog-missing-permission-negative|run#16|AssertionError|assert reason_bad == expected_reason, reason_bad
Issue-Key: athena__glue__s3.athena-partition-projection-reads-glue-partitions-positive|run#19|AssertionError|assert state == "SUCCEEDED", f"query failed: state={state} reason={reason}"
Issue-Key: athena__glue__s3.athena-query-large-result-multipart-s3-positive|run#13|AssertionError|assert state == "SUCCEEDED", f"query did not succeed: {exec_resp['Status']}"
Issue-Key: athena__glue__s3.athena-query-nonexistent-glue-table-negative|run#13|AssertionError|assert state_ok == "SUCCEEDED", f"control query failed: state={state_ok} reason={reason_ok}"
Issue-Key: athena__glue__s3.athena-query-on-a-missing-table-or-schema-fails-not-found|_select#7|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.athena-query-reads-csv-data-from-s3-positive|run#10|AssertionError|assert state == "SUCCEEDED", f"query did not succeed: {exec_resp['Status']}"
Issue-Key: athena__glue__s3.athena-query-uses-glue-catalog-table-positive|run#12|AssertionError|assert state == "SUCCEEDED", f"query failed: state={state} reason={reason}"
Issue-Key: athena__glue__s3.athena-reads-gzip-and-nested-objects-under-the-table-location|_select#10|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.athena-status-histogram-matches-the-seeded-log-lines|_select#13|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.cli-athena-query-missing-glue-table-error-negative|run#14|AssertionError|assert reason == (
Issue-Key: athena__glue__s3.cli-glue-create-table-athena-query-s3-roundtrip|run#14|AssertionError|assert execution["Status"]["State"] == "SUCCEEDED", execution["Status"]
Issue-Key: athena__glue__s3.dash-fields-in-numeric-columns-read-as-null-and-typed-columns-aggregate|_select#15|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.glue-accepts-a-location-in-an-absent-bucket-that-athena-rejects-at-query-time|_select#11|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.glue-accepts-an-invalid-column-type-that-athena-rejects-at-query-time|_select#12|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.glue-catalog-table-location-points-to-s3-positive|run#21|AssertionError|assert state == "SUCCEEDED", f"Athena query against the catalog table failed: state={state!r} reason={reason!r}"
Issue-Key: athena__glue__s3.root-located-table-reads-athena-result-objects-back-as-rows|run#10|AssertionError|assert first["Status"]["State"] == "SUCCEEDED", f"query 1 ended {first['Status']!r}"
Issue-Key: athena__glue__s3.athena-ddl-create-table-registers-in-glue-catalog-positive|run#10|AssertionError|assert state == "SUCCEEDED", f"DDL failed: state={state} reason={reason}"
Issue-Key: athena__glue__s3.athena-ddl-table-differs-from-the-glue-api-table-in-owner-partitions-and-location|run#11|AssertionError|assert ex["Status"]["State"] == "SUCCEEDED", f"the DDL ended {ex['Status']!r}"
Issue-Key: athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl|run#9|AssertionError|assert ddl["Status"]["State"] == "SUCCEEDED", f"the DDL ended {ddl['Status']!r}"
Issue-Key: athena__glue__s3.e2e-create-athena-tables-from-s3-lifecycle|phase#13|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: athena__glue__s3.athena-create-database-comment-lands-as-the-glue-description|run#7|AssertionError|assert ex["Status"]["State"] == "SUCCEEDED", f"CREATE DATABASE ended {ex['Status']!r}"
Issue-Key: athena__glue__s3.athena-ddl-surfaces-external-table-in-the-glue-data-catalog-and-reads-it|_run_query#12|AssertionError|assert state == "SUCCEEDED", (
Issue-Key: athena__glue__s3|ListDatabases|NotImplemented
Issue-Key: athena__glue__s3|GetObject|NoSuchKey
Issue-Key: athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected|run#7|AssertionError|assert ok["ResultConfiguration"]["OutputLocation"] == want, (
Issue-Key: athena__glue__s3.athena-query-to-an-output-bucket-that-does-not-exist-is-rejected-until-it-is-created|_error#7|AssertionError|raise AssertionError(f"real AWS refuses this call, but it succeeded and returned {result!r}")
Issue-Key: athena__glue__s3.athena-missing-s3-output-bucket-permission-negative|run#8|AssertionError|assert state == "FAILED", f"expected FAILED, got {state}: {exec_resp['Status']}"
Issue-Key: athena__glue__s3.athena-s3-bucket-policy-blocks-query-results-write-negative|run#9|AssertionError|assert state == "FAILED", f"expected FAILED, got {state}: {exec_resp['Status']}"
Issue-Key: athena__glue__s3.athena-query-output-format-preserved-in-s3-positive|run#13|AssertionError|assert types_by_name["bool_col"] == "boolean", f"bool_col: expected type 'boolean', got {types_by_name['bool_col']!r}"
Issue-Key: athena|StartQueryExecution|InvalidRequestException
Issue-Commit: ed67363a200a02a71118cd5e7b7585c85db4efeb

# athena__glue__s3: the Athena data plane never reads the Glue catalog, Hive DDL is refused as SQLite syntax errors, result objects are not written and output-location authorization is ignored (10 causes)

> _Note: this issue was identified against the code base at commit `ed67363a200a02a71118cd5e7b7585c85db4efeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
The emulator's Athena implementation answers on the wire but its query engine is a local SQLite instance that neither resolves Glue Data Catalog tables nor understands Hive DDL, its result-writing path does not produce the per-query result object, and it performs no authorization or bucket validation on the output location. 10 causes.

Cause 1 of 10: a SELECT over a table that exists in the Glue Data Catalog fails with 'no such table'.
AWS: Athena resolves <catalog>.<database>.<table> through the data catalog (awsdatacatalog -> Glue) and scans the objects under the table's StorageDescriptor.Location (honouring skip.header.line.count, gzip objects, nested prefixes and partition projection), so the query reaches QueryExecutionStatus.State SUCCEEDED (Smithy: QueryExecutionState enum) and GetQueryResults returns the rows. When the table genuinely does not exist the query fails with AthenaError.ErrorCategory 2 / ErrorType 1301 and a message of the fixed form "TABLE_NOT_FOUND: line <l>:<c>: Table 'awsdatacatalog.<db>.<table>' does not exist" (SCHEMA_NOT_FOUND ... Schema '<db>' does not exist for a missing database); only the identifiers and the line/column come from the request.
Emulator: State FAILED with StateChangeReason == AthenaError.ErrorMessage == 'no such table: <db>.<table>' and ErrorCategory 2 with no ErrorType, for tables the Glue API served moments earlier in the same check. Negative checks also fail because the emulator's not-found text and missing ErrorType do not match AWS's.
Why: nothing can be read through Athena — every analytic read, aggregate, partition-projection read and permission-boundary negative sees the same fabricated 'no such table', and a client cannot distinguish a missing table from a missing database or an access denial.
Fix: the athena crate's query-execution engine — catalog/table resolution for DML and the AthenaError classification of not-found failures.
Traces: athena__glue__s3.athena-counts-cloudfront-rows-skipping-two-header-lines-in-every-file, athena__glue__s3.athena-glue-catalog-cross-account-missing-permission-negative, athena__glue__s3.athena-glue-catalog-missing-permission-negative, athena__glue__s3.athena-partition-projection-reads-glue-partitions-positive, athena__glue__s3.athena-query-large-result-multipart-s3-positive, athena__glue__s3.athena-query-nonexistent-glue-table-negative, athena__glue__s3.athena-query-on-a-missing-table-or-schema-fails-not-found, athena__glue__s3.athena-query-reads-csv-data-from-s3-positive, athena__glue__s3.athena-query-uses-glue-catalog-table-positive, athena__glue__s3.athena-reads-gzip-and-nested-objects-under-the-table-location, athena__glue__s3.athena-status-histogram-matches-the-seeded-log-lines, athena__glue__s3.cli-athena-query-missing-glue-table-error-negative, athena__glue__s3.cli-glue-create-table-athena-query-s3-roundtrip, athena__glue__s3.dash-fields-in-numeric-columns-read-as-null-and-typed-columns-aggregate, athena__glue__s3.glue-accepts-a-location-in-an-absent-bucket-that-athena-rejects-at-query-time, athena__glue__s3.glue-accepts-an-invalid-column-type-that-athena-rejects-at-query-time, athena__glue__s3.glue-catalog-table-location-points-to-s3-positive, athena__glue__s3.root-located-table-reads-athena-result-objects-back-as-rows.

Cause 2 of 10: CREATE EXTERNAL TABLE is refused as a syntax error.
AWS: `CREATE EXTERNAL TABLE [IF NOT EXISTS] <db>.<tbl> (<columns>) [ROW FORMAT DELIMITED FIELDS TERMINATED BY '<c>'] LOCATION 's3://<bucket>/<prefix>/' [TBLPROPERTIES (...)]` reaches State SUCCEEDED with StatementType DDL and registers the table in Glue (TableType EXTERNAL_TABLE, Owner hadoop, Parameters EXTERNAL=TRUE plus the declared TBLPROPERTIES, location with the trailing slash dropped).
Emulator: State FAILED with StateChangeReason == AthenaError.ErrorMessage == 'near "EXTERNAL": syntax error in <the submitted statement> at offset 7' (ErrorCategory 2, no ErrorType) — the fixed text 'near "EXTERNAL": syntax error in' / 'at offset 7' is the local SQLite parser's and appears in no AWS response.
Why: tables cannot be created through Athena DDL, so the whole DDL-authored catalog path (and the CTAS-style lifecycle flows that depend on it) is unavailable.
Fix: the athena crate's query-execution engine — Hive DDL parsing/execution for CREATE EXTERNAL TABLE and its write-through to the Glue catalog.
Traces: athena__glue__s3.athena-ddl-create-table-registers-in-glue-catalog-positive, athena__glue__s3.athena-ddl-table-differs-from-the-glue-api-table-in-owner-partitions-and-location, athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl, athena__glue__s3.e2e-create-athena-tables-from-s3-lifecycle.

Cause 3 of 10: CREATE DATABASE is refused as a syntax error.
AWS: `CREATE DATABASE [IF NOT EXISTS] <name> [COMMENT '<text>']` reaches State SUCCEEDED and creates the Glue database, with COMMENT landing as the database's Description.
Emulator: State FAILED with StateChangeReason == AthenaError.ErrorMessage == 'near "DATABASE": syntax error in <the submitted statement> at offset 7', ErrorCategory 2, no ErrorType.
Why: a client cannot create a schema through Athena, and the COMMENT -> Glue Description mapping never happens.
Fix: the athena crate's query-execution engine — CREATE DATABASE handling and its write-through to the Glue catalog.
Traces: athena__glue__s3.athena-create-database-comment-lands-as-the-glue-description, athena__glue__s3.athena-ddl-surfaces-external-table-in-the-glue-data-catalog-and-reads-it, athena__glue__s3.e2e-create-athena-tables-from-s3-lifecycle.

Cause 4 of 10: ListDatabases is an unimplemented scaffold stub.
AWS: Smithy: ListDatabases input CatalogName (required, @length 1..256), output DatabaseList + NextToken, paginated (MaxResults @range 1..50); errors InternalServerException/InvalidRequestException/MetadataException. For CatalogName 'AwsDataCatalog' it lists the Glue databases.
Emulator: HTTP 501 ClientError (NotImplemented) 'operation ListDatabases is scaffolded but not implemented in this emulator'.
Why: a client cannot enumerate databases through Athena's metadata surface at all; the call is not even a modeled error.
Fix: the athena crate's ListDatabases handler, backed by the Glue database store.
Traces: athena__glue__s3.athena-list-databases-reflects-glue-database-positive.

Cause 5 of 10: a SUCCEEDED query leaves no result object at the output location.
AWS: on success Athena writes the result CSV to <OutputLocation>/<QueryExecutionId>.csv (plus the <...>.csv.metadata companion), so a GetObject of that key returns the body.
Emulator: GetObject of the result key raises NoSuchKey ('The specified key does not exist.') — nothing was written.
Why: every client that polls to SUCCEEDED and then reads the result object from S3 (the documented CLI workflow) gets a 404 for a query the service reported as successful.
Fix: the athena crate's query-execution result-writing path (result object + .csv.metadata companion) into the resolved output location.
Traces: athena__glue__s3.cli-athena-start-query-execution-poll-results-s3.

Cause 6 of 10: ResultConfiguration.OutputLocation is reported as the directory prefix instead of the result object key.
AWS: Smithy: ResultConfiguration.OutputLocation. The submitted or workgroup-default location is a prefix ('s3://<bucket>/<prefix>/'), and the query execution's ResultConfiguration.OutputLocation reads back as the object AWS wrote: 's3://<bucket>/<prefix>/<QueryExecutionId>.csv'. Only the bucket, prefix and query-execution id are request/state derived.
Emulator: GetQueryExecution echoes the bare prefix 's3://<bucket>/<prefix>/' with no '<QueryExecutionId>.csv' suffix.
Why: a client that derives the result key from the execution record reads (or heads) a directory prefix and never finds its results.
Fix: the athena crate's GetQueryExecution serializer / result-location resolution.
Traces: athena__glue__s3.athena-workgroup-output-location-removal-makes-a-locationless-query-rejected.

Cause 7 of 10: StartQueryExecution accepts an output location in a bucket the account can neither verify nor create.
AWS: Smithy: StartQueryExecution errors include InvalidRequestException (HTTP 400). An OutputLocation naming a bucket owned by another account is refused synchronously with InvalidRequestException whose Message is of the fixed form 'Unable to verify/create output bucket <bucket>' (<bucket> taken from the request's OutputLocation), and no query execution is recorded in the workgroup.
Emulator: HTTP 200 with a QueryExecutionId, and the execution is recorded.
Why: a client relying on the synchronous refusal to catch a mistyped or foreign result bucket proceeds as if the query were running, and the workgroup's execution list gains an entry AWS never creates.
Fix: the athena crate's StartQueryExecution validation of the resolved output location (bucket ownership/creatability check).
Traces: athena__glue__s3.athena-query-to-an-output-bucket-that-does-not-exist-is-rejected-until-it-is-created.

Cause 8 of 10: S3 write authorization on the output location is not enforced — a query that cannot write its result reports SUCCEEDED.
AWS: when the caller's identity is denied s3:PutObject on the output location (by an IAM deny or by a bucket policy), the execution terminates State FAILED with StateChangeReason of the fixed form 'Access denied when writing to location: s3://<bucket>/<prefix><QueryExecutionId>.csv' and no result object is left behind (HeadObject 404 'Not Found'); the same role writing to an allowed bucket SUCCEEDS.
Emulator: State SUCCEEDED with no StateChangeReason in both the IAM-deny and the bucket-policy case.
Why: a permission misconfiguration that AWS surfaces as a failed query is reported as success, so a client never learns its results were not persisted.
Fix: the athena crate's query-execution result-writing path — it must perform the write as the caller and map an S3 authorization failure onto a FAILED execution with that StateChangeReason.
Traces: athena__glue__s3.athena-missing-s3-output-bucket-permission-negative, athena__glue__s3.athena-s3-bucket-policy-blocks-query-results-write-negative.

Cause 9 of 10: GetQueryResults reports a BOOLEAN column as type 'integer'.
AWS: Smithy: ResultSet.ResultSetMetadata.ColumnInfo with required members Name and Type. For `SELECT CAST(42 AS INTEGER) AS int_col, CAST(3.14 AS DOUBLE) AS dbl_col, CAST(true AS BOOLEAN) AS bool_col, '<s>' AS str_col` the ColumnInfo types are integer, double, boolean, varchar respectively, and the same type names appear in the .csv.metadata companion object.
Emulator: bool_col is reported with Type 'integer' — the local SQL engine's affinity leaks through instead of Athena's type name.
Why: a client that maps ColumnInfo.Type to a native type decodes booleans as numbers; typed-result handling diverges for every boolean column.
Fix: the athena crate's result-set metadata construction (engine type -> Athena type-name mapping) in the GetQueryResults path.
Traces: athena__glue__s3.athena-query-output-format-preserved-in-s3-positive.

Cause 10 of 10: a CloudFormation-deployed AWS::Athena::WorkGroup does not exist in Athena.
AWS: a stack declaring AWS::Athena::WorkGroup with Name/WorkGroupConfiguration reaches CREATE_COMPLETE and the workgroup is then usable: StartQueryExecution with WorkGroup=<name> is accepted, and with EnforceWorkGroupConfiguration=true the workgroup's ResultConfiguration overrides the client-supplied one.
Emulator: the stack deploys, but StartQueryExecution answers InvalidRequestException 'WorkGroup <name> is not found.' — the CFN resource created no Athena workgroup.
Why: every infrastructure-as-code path that provisions a workgroup and then queries through it is broken, and the enforcement flag can never be exercised.
Fix: the cloudformation crate's AWS::Athena::WorkGroup resource provisioner (it must create the workgroup in the athena crate's workgroup store), not the Athena handler itself.
Traces: athena__glue__s3.deploy-cfn-athena-workgroup-enforce-configuration-positive, athena__glue__s3.deploy-cfn-athena-workgroup-output-location-s3-positive.

Related open issues that do NOT cover these causes: 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md, 20261104T063259Z-athena-getqueryresults-refuses-completed-queries-and-query-reads.md and 20261104T063259Z-glue-catalog-objects-created-through-athena-ddl-are.md are wire-corpus issues filed under the athena/glue service tokens; 20261102T114851Z-athena-glue-s3-a-glue-job-run-reached.md owns only the Glue-job-run checks of this group, and 20261102T114851Z-athena-createnamedquery-stub.md only CreateNamedQuery.

## Evidence
Replay of the simparity-behavioral-aws-v1 behavioral suite against the emulator built from this checkout (service heading athena__glue__s3, 3/56 passed).
Cause 1 — athena-query-uses-glue-catalog-table-positive: expected SUCCEEDED; actual 'query failed: state=FAILED reason=no such table: <db>.<tbl>' (18 checks, same text shape).
Cause 2 — athena-ddl-create-table-registers-in-glue-catalog-positive: expected SUCCEEDED + a Glue table; actual FAILED, reason 'near "EXTERNAL": syntax error in CREATE EXTERNAL TABLE <db>.<tbl> (id INT, val STRING) ROW FORMAT DELIMITED FIELDS TERMINATED BY \',\' LOCATION \'s3://<bucket>/ddl-data/\' at offset 7'.
Cause 3 — athena-create-database-comment-lands-as-the-glue-description: expected SUCCEEDED + Glue Description 'CloudFront distribution logs'; actual FAILED, 'near "DATABASE": syntax error in CREATE DATABASE <db> COMMENT \'CloudFront distribution logs\' at offset 7'.
Cause 4 — athena-list-databases-reflects-glue-database-positive: botocore.exceptions.ClientError (NotImplemented) 'operation ListDatabases is scaffolded but not implemented in this emulator'.
Cause 5 — cli-athena-start-query-execution-poll-results-s3: botocore.errorfactory.NoSuchKey on GetObject of the result key: 'The specified key does not exist.'
Cause 6 — athena-workgroup-output-location-removal-...: expected OutputLocation 's3://<bucket>/athena-results/<qid>.csv'; actual {'OutputLocation': 's3://<bucket>/athena-results/'}.
Cause 7 — athena-query-to-an-output-bucket-that-does-not-exist-...: expected InvalidRequestException 'Unable to verify/create output bucket amazon-reviews-pds'; actual HTTP 200 {'QueryExecutionId': '<id>'}.
Cause 8 — athena-missing-s3-output-bucket-permission-negative: expected FAILED with StateChangeReason 'Access denied when writing to location: s3://<deny-bucket>/results/<qid>.csv'; actual {'State': 'SUCCEEDED'} (also athena-s3-bucket-policy-blocks-query-results-write-negative).
Cause 9 — athena-query-output-format-preserved-in-s3-positive: "bool_col: expected type 'boolean', got 'integer'".
Cause 10 — deploy-cfn-athena-workgroup-enforce-configuration-positive: botocore.errorfactory.InvalidRequestException on StartQueryExecution: 'WorkGroup <name> is not found.' after the stack deployed the AWS::Athena::WorkGroup (also deploy-cfn-athena-workgroup-output-location-s3-positive).

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the simparity-behavioral-aws-v1 suite and inspect the checks listed in trace_ids under the athena__glue__s3 heading.
3. Cause 1: glue.create_database + glue.create_table pointing at an s3:// prefix with seeded objects, then athena.start_query_execution('SELECT count(*) FROM <db>.<tbl>') — expect SUCCEEDED, observe FAILED 'no such table: <db>.<tbl>'.
4. Causes 2-3: start_query_execution("CREATE DATABASE <d>") and ("CREATE EXTERNAL TABLE <d>.<t> (id INT) LOCATION 's3://<b>/p/'") — expect SUCCEEDED and Glue entries, observe FAILED with 'near "DATABASE"/"EXTERNAL": syntax error ... at offset 7'.
5. Cause 4: athena.list_databases(CatalogName='AwsDataCatalog') — expect the Glue databases, observe NotImplemented.
6. Causes 5-6: run 'SELECT 1' with ResultConfiguration.OutputLocation='s3://<b>/r/', poll to SUCCEEDED, read QueryExecution.ResultConfiguration.OutputLocation (expect 's3://<b>/r/<qid>.csv') and s3.get_object that key (expect the CSV).
7. Cause 7: start_query_execution with OutputLocation in a bucket owned elsewhere (e.g. s3://amazon-reviews-pds/athena-results/) — expect InvalidRequestException 'Unable to verify/create output bucket <bucket>'.
8. Cause 8: assume a role denied s3:PutObject on the output bucket and run a query — expect FAILED with the access-denied StateChangeReason and no result object.
9. Cause 9: run "SELECT CAST(true AS BOOLEAN) AS bool_col" and read ResultSet.ResultSetMetadata.ColumnInfo — expect Type 'boolean'.
10. Cause 10: deploy a stack with AWS::Athena::WorkGroup and then StartQueryExecution with that WorkGroup — expect acceptance, observe 'WorkGroup <name> is not found.'

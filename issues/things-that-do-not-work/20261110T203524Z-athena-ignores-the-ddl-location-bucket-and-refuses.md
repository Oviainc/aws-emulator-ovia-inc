Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-table-location-mistakes
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-table-location-mistakes
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetQueryExecution|2xx->2xx|$.QueryExecution.Status.AthenaError
Issue-Key: athena|GetQueryResults|2xx->4xx|$.ResultSet
Issue-Key: athena|GetQueryResults|2xx->4xx|$.UpdateCount
Issue-Key: athena|GetQueryResults|2xx->4xx|status
Issue-Related: 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md
Issue-Commit: dd8fce8e7ad2da3c1b7c1776d58c99e163a1e8cd

# Athena ignores the DDL LOCATION bucket and refuses results for a SELECT that must succeed (2 causes)

> _Note: this issue was identified against the code base at commit `dd8fce8e7ad2da3c1b7c1776d58c99e163a1e8cd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
Two net-new root causes in the Athena query-execution / query-results path, both exercised by the table-location fixture. Both must be closed before this issue is submitted.

Cause 1 of 2 — a CREATE EXTERNAL TABLE whose LOCATION names a bucket that does not exist is reported SUCCEEDED (GetQueryExecution, fixture athena-cats-traj-table-location-mistakes step task#10).
AWS: Athena validates the DDL LOCATION against S3 before the statement commits. When the bucket in the LOCATION clause does not exist the execution settles in a terminal failure: QueryExecution.Status.State = 'FAILED', QueryExecution.Status.StateChangeReason = 'The bucket specified in the ddl statement does not exist', and QueryExecution.Status.AthenaError is populated with ErrorCategory 2 (user error), ErrorType 1306 and Retryable false. That whole message is fixed text — it interpolates nothing from the request, so it is reproduced verbatim. Smithy: GetQueryExecution output QueryExecution; QueryExecutionStatus members State (enum QueryExecutionState, 'FAILED indicates that the query experienced an error and did not complete processing'), StateChangeReason, AthenaError; AthenaError members ErrorCategory (@range 1-3, 2 = user), ErrorType (@range 0-9999), Retryable (default false), ErrorMessage. The category/type/message values themselves are recorded behavior.
Emulator: the execution settles State = 'SUCCEEDED' with no StateChangeReason and no AthenaError member at all, and the polling assertion gives up because 'SUCCEEDED' is terminal. The table is also committed into the catalog with the unreachable Location (visible as the Glue StorageDescriptor.Location still pointing at the absent bucket on a later read).
Why: a client that probes a mistyped or not-yet-created result/data bucket by submitting the DDL and polling GetQueryExecution is told the table was created successfully. It then proceeds to query a table whose storage cannot be read, and never takes the remediation branch (create the bucket, fix the LOCATION) that AWS's FAILED + ErrorType 1306 drives. Error-category-based retry logic (Retryable false, category 2 = caller must fix the request) never fires.
Fix: the StartQueryExecution DDL execution path for CREATE [EXTERNAL] TABLE — the LOCATION target must be resolved against S3 bucket state before the catalog write, and on absence the execution must be settled FAILED with the StateChangeReason and the AthenaError triple, in the query-execution state that GetQueryExecution serializes.
Traces: athena-cats-traj-table-location-mistakes#task#10 (GetQueryExecution; expected State FAILED + StateChangeReason + AthenaError{ErrorCategory 2, ErrorType 1306, Retryable false}, got State SUCCEEDED with no AthenaError and no StateChangeReason).

Cause 2 of 2 — GetQueryResults answers HTTP 400 for a SELECT whose execution must have succeeded, instead of 200 with the result set (fixture athena-cats-traj-table-location-mistakes step task#17).
AWS: GetQueryResults on a query execution that completed successfully answers HTTP 200 with ResultSet.ResultSetMetadata.ColumnInfo describing each projected column (here one bigint column, the SELECT's alias as both Name and Label, CatalogName 'hive', empty SchemaName/TableName, Nullable 'UNKNOWN', CaseSensitive false), ResultSet.Rows carrying the header row followed by one row per result row (each cell as VarCharValue), and UpdateCount 0 for a read-only statement. Smithy: GetQueryResults output members ResultSet (ResultSet -> Rows: RowList, ResultSetMetadata) and UpdateCount (Long); its only client errors are InvalidRequestException and TooManyRequestsException, neither of which applies to a successfully completed SELECT. Column labels, types and row values come from the submitted statement and the table's data, never from a fixed list.
Emulator: the call is refused with HTTP 400 and no body members — no ResultSet, no UpdateCount. Hypothesis: the upstream SELECT is parked in FAILED by the emulator's execution engine, so GetQueryResults takes its not-successfully-finished refusal branch; the requirement violated here is nonetheless that a completed SELECT's results are readable at 200 with the modeled ResultSet and UpdateCount.
Why: a client that submits a SELECT, polls to a terminal state and then reads the rows gets a 400 where AWS gives it data. SDK paginators over GetQueryResults raise instead of yielding pages, so any read-after-query workflow (count checks, ETL validation, dashboards) aborts rather than returning rows.
Fix: the GetQueryResults handler and the query-execution result state it reads — a completed SELECT must have its result rows and column metadata materialized and served at 200 with UpdateCount, rather than the request being refused.
Traces: athena-cats-traj-table-location-mistakes#task#17 (GetQueryResults; expected 200 with ResultSet.ResultSetMetadata.ColumnInfo + Rows and UpdateCount 0, got 400 with ResultSet and UpdateCount absent).

Related open issues that do NOT cover these two causes: '20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md' (queries that must succeed settling FAILED — the inverse of cause 1, which is a query that must FAIL settling SUCCEEDED), '20261108T041854Z-athena-getqueryresults-omits-columninfo-catalog-schema-table-updatecount.md' (members missing from a 200 GetQueryResults, not a 400 refusal) and '20261104T163600Z-athena-statements-are-executed-by-a-sqlite-engine.md'. The replay report attributes neither divergence to any open issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture athena-cats-traj-table-location-mistakes, step task#10, operation GetQueryExecution, status 200 -> 200:
  $.QueryExecution.Status.State: expected 'FAILED', got 'SUCCEEDED' (poll: 'SUCCEEDED' is in fail_on after 1 attempt)
  $.QueryExecution.Status.StateChangeReason: expected 'The bucket specified in the ddl statement does not exist', got None
  $.QueryExecution.Status.AthenaError: expected {ErrorCategory: 2, ErrorType: 1306, Retryable: false, ErrorMessage: 'The bucket specified in the ddl statement does not exist'}, got None
  Statement under execution: CREATE EXTERNAL TABLE <db>.items (...) ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' LOCATION 's3://<absent-bucket>/products/' TBLPROPERTIES ('skip.header.line.count'='1')
  Fixed text of the message: the entire string 'The bucket specified in the ddl statement does not exist' (no request- or state-derived parts). Fixed values: ErrorCategory 2, ErrorType 1306, Retryable false.

Cause 2 — fixture athena-cats-traj-table-location-mistakes, step task#17, operation GetQueryResults, status expected 200, got 400:
  $.ResultSet: expected {ResultSetMetadata: {ColumnInfo: [{Name/Label = the SELECT's column alias, Type 'bigint', CatalogName 'hive', SchemaName '', TableName '', Nullable 'UNKNOWN', CaseSensitive false, Precision 19, Scale 0}]}, Rows: [header row, one row per result row]}, got None
  $.UpdateCount: expected 0, got None
  Preceding step task#16 (GetQueryExecution for 'SELECT count(*) AS n FROM <db>.items') expected State 'SUCCEEDED' and got 'FAILED'.
  Template parts: the column name/label/type and the row values come from the submitted statement and the table's rows; UpdateCount 0 and the ColumnInfo defaults (CatalogName 'hive', empty SchemaName/TableName, Nullable 'UNKNOWN') are fixed for a read-only SELECT.

## Steps to Reproduce
1. git clone the project and boot the emulator built from the checkout.
2. Replay the wire corpus: replay_wire_corpus() over trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture athena-cats-traj-table-location-mistakes up to step task#10 — create a workgroup and database, then StartQueryExecution with CREATE EXTERNAL TABLE <db>.items (item_id string, name string, price int) ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' LOCATION 's3://<a-bucket-that-does-not-exist>/products/' TBLPROPERTIES ('skip.header.line.count'='1'), then poll GetQueryExecution on the returned QueryExecutionId. Observe Status.State 'SUCCEEDED' with no StateChangeReason and no AthenaError, where AWS settles FAILED with StateChangeReason 'The bucket specified in the ddl statement does not exist' and AthenaError{ErrorCategory 2, ErrorType 1306, Retryable false}.
4. Cause 2: continue the same fixture to step task#17 — after the table is created against an existing location and its data objects are present, StartQueryExecution with 'SELECT count(*) AS n FROM <db>.items', poll to a terminal state, then call GetQueryResults with that QueryExecutionId. Observe HTTP 400 with no ResultSet/UpdateCount, where AWS answers 200 with ResultSet.ResultSetMetadata.ColumnInfo, ResultSet.Rows (header + data rows) and UpdateCount 0.

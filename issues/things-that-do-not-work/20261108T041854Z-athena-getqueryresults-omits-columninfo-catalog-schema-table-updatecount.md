Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-query-execution-polling-reads
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-query-execution-polling-reads athena-cats-traj-table-location-mistakes athena-kfi-traj-athena-reads-the-iceberg-table athena-kfi-traj-athena-query-context-mistakes athena-cats-traj-output-location-mistakes athena-cats-traj-table-before-database-and-ddl-retries
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetQueryResults|2xx->2xx|$.Output
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.ColumnInfos
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.ResultRows
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.ResultSetMetadata.ColumnInfo[*]
Issue-Key: athena|GetQueryResults|2xx->2xx|$.UpdateCount
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.ResultSetMetadata.ColumnInfo[*].CatalogName
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.ResultSetMetadata.ColumnInfo[*].Precision
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.ResultSetMetadata.ColumnInfo[*].SchemaName
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.ResultSetMetadata.ColumnInfo[*].TableName
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.ResultSetMetadata.ColumnInfo[*].Type
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.Rows[*].Data[*].VarCharValue
Issue-Key: athena|GetQueryResults|2xx->2xx|$.ResultSet.ResultSetMetadata.ColumnInfo[*].CaseSensitive
Issue-Commit: 8c5329059e752b9fa65691305eadc8c02e100ead

# Athena GetQueryResults omits ColumnInfo catalog/schema/table, UpdateCount and maps Hive column types wrongly (6 causes)

> _Note: this issue was identified against the code base at commit `8c5329059e752b9fa65691305eadc8c02e100ead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResults

## Description
SIX net-new causes, all in Athena's GetQueryResults result-set serializer: the emulator builds ResultSetMetadata.ColumnInfo from its own SQLite column descriptor instead of from the Hive/Trino type the catalog and engine report, omits the ColumnInfo members that identify where a column came from, omits UpdateCount, and emits no result-output view at all. All six share one component and one fault, so they are bundled.

Cause 1 of 6 — ColumnInfo identity members and UpdateCount are omitted (GetQueryResults, fixture athena-cats-traj-query-execution-polling-reads step task#5, a SHOW DATABASES-style read).
AWS: Smithy com.amazonaws.athena#ColumnInfo models CatalogName ('The catalog to which the query results belong'), SchemaName ('The schema name (database name) to which the query results belong') and TableName ('The table name for the query results'); GetQueryResultsOutput models UpdateCount (target Long). AWS populates CatalogName with the catalog that served the query ('hive' for the Hive/AwsDataCatalog engine), SchemaName and TableName with the empty string for a column that is not projected from a single table, and UpdateCount with the DML row count (0 for a DDL/SHOW statement). The recorded response also carries the derived views $.Output, $.ResultSet.ColumnInfos and $.ResultSet.ResultRows — these are not in the published Smithy model, so recorded behavior only: they are the textual result output and the flattened column/row projection of the same result set.
Emulator: ColumnInfo[0] is returned with CatalogName, SchemaName and TableName absent (None), UpdateCount absent, and $.Output / $.ResultSet.ColumnInfos / $.ResultSet.ResultRows all absent.
Why: a client that reads ColumnInfo.CatalogName/SchemaName/TableName to qualify a result column (JDBC/ODBC metadata, BI tools, any code that renders a result grid) gets null where AWS always sends a string, and a client that reads UpdateCount to report rows affected by a DML statement sees no value instead of a number.
Fix: the GetQueryResults handler's result-set serializer — populate the ColumnInfo identity members and UpdateCount from the execution's catalog/engine context rather than leaving them unset.
Traces: athena-cats-traj-query-execution-polling-reads#task#5.

Cause 2 of 6 — a COUNT(*) column is typed 'integer' with Precision 10 instead of 'bigint' with Precision 19 (GetQueryResults, fixture athena-cats-traj-table-location-mistakes step task#17).
AWS: ColumnInfo.Type carries the Athena/Trino type name. For an aggregate such as count(*) AS n, Athena reports Type 'bigint' and Precision 19 (the decimal digit width of a 64-bit integer), Scale 0, Nullable 'UNKNOWN', CaseSensitive false, CatalogName 'hive', SchemaName '' and TableName ''.
Emulator: Type 'integer' and Precision 10, with CatalogName/SchemaName/TableName absent, and the derived ColumnInfos/ResultRows views absent. The row value also diverges (recorded '4', emulator '2'); that value difference is a downstream effect of the table-LOCATION cause filed separately for this same fixture, not of the serializer.
Why: a client keys its value decoding and its column widths off ColumnInfo.Type, so 'integer' makes a 64-bit aggregate decode as a 32-bit column and overflow or truncate for counts above 2^31.
Fix: the GetQueryResults column-type mapping — map engine integer aggregates to Athena's bigint/19 rather than reporting the storage engine's own narrower type.
Traces: athena-cats-traj-table-location-mistakes#task#17.

Cause 3 of 6 — the same bigint/Precision divergence on an Iceberg-table count read (GetQueryResults, fixture athena-kfi-traj-athena-reads-the-iceberg-table step task#11).
AWS: ColumnInfo for count(*) AS n is Type 'bigint', Precision 19, Scale 0, CatalogName 'hive', SchemaName '', TableName '', Nullable 'UNKNOWN', CaseSensitive false.
Emulator: Type 'integer', Precision 10, CatalogName/SchemaName/TableName absent, derived ColumnInfos/ResultRows views absent.
Why: same as cause 2 — the column metadata a client decodes against is the storage engine's, not Athena's.
Fix: the GetQueryResults column-type mapping.
Traces: athena-kfi-traj-athena-reads-the-iceberg-table#task#11.

Cause 4 of 6 — a SHOW TABLES string column is typed 'varchar' with Precision 2147483647 and CaseSensitive true instead of 'string' / Precision 0 / CaseSensitive false (GetQueryResults, fixture athena-kfi-traj-athena-query-context-mistakes step task#12).
AWS: for the tab_name column of a SHOW TABLES result Athena reports Type 'string', Precision 0, Scale 0, CaseSensitive false, Nullable 'UNKNOWN', CatalogName 'hive', SchemaName '', TableName ''.
Emulator: Type 'varchar', Precision 2147483647, CaseSensitive true, with CatalogName/SchemaName/TableName absent and the derived $.Output / ColumnInfos / ResultRows views absent.
Why: Precision 2147483647 is the engine's unbounded-varchar sentinel, not an Athena value; a client that allocates or formats by Precision sees a 2-billion-wide column, and CaseSensitive true changes comparison semantics a client may apply client-side.
Fix: the GetQueryResults column-type mapping, for the Hive 'string' type specifically.
Traces: athena-kfi-traj-athena-query-context-mistakes#task#12.

Cause 5 of 6 — an empty result set is returned with no result-set body at all (GetQueryResults, fixture athena-cats-traj-output-location-mistakes step task#8).
AWS: recorded behavior only for the derived views — for a statement that produced no rows AWS still returns the result set, so $.Output is the empty string and the flattened ColumnInfos and ResultRows views are empty lists; the members are present and empty, not absent.
Emulator: $.Output, $.ResultSet.ColumnInfos and $.ResultSet.ResultRows are all absent (None).
Why: a client cannot distinguish 'query returned nothing' from 'no result set was served'; code that iterates the rows list without a null check faults instead of iterating zero rows.
Fix: the GetQueryResults handler — always serialize the result set and its (possibly empty) metadata and row list.
Traces: athena-cats-traj-output-location-mistakes#task#8.

Cause 6 of 6 — a FAILED statement's result output text is not served (GetQueryResults, fixture athena-cats-traj-table-before-database-and-ddl-retries step task#6).
AWS: recorded behavior only — for a DDL statement that failed because its target database does not exist, the query's result output carries the engine's failure text. Its fixed form is 'FAILED: SemanticException [Error 10072]: Database does not exist: <databaseName>'; the fixed text is 'FAILED: SemanticException [Error 10072]: Database does not exist: ' and the only variable part is the database name taken from the request's query/QueryExecutionContext (the recording's sim2real_probe_cats_0392935c is one run's value, not part of the requirement).
Emulator: $.Output is absent (None) — no result output is served for the failed statement.
Why: a client that surfaces the engine's diagnostic to a user (or parses the Hive error number out of it) gets nothing and cannot tell the user why the DDL failed. Note the separately open issues 20261104T163600Z-athena-writes-no-query-result-object-to-s3.md and 20261106T181113Z-athena-never-writes-the-query-result-object-to.md describe the S3 result object; neither owns this cluster's replay key, so this cause is filed here and they are named for the fixer's benefit.
Fix: the GetQueryResults handler / the query-execution result plumbing — serve the failure diagnostic as the failed statement's result output.
Traces: athena-cats-traj-table-before-database-and-ddl-retries#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (36 fixtures, 384 steps, score 35.9%).

Cause 1 — athena-cats-traj-query-execution-polling-reads#task#5, GetQueryResults, status 200 vs 200:
  $.ResultSet.ResultSetMetadata.ColumnInfo[0]: missing (expected {'CaseSensitive': False, 'CatalogName': 'hive', 'Label': 'database_name', 'Name': 'database_name', 'Nullable': 'UNKNOWN', 'Precision': 0, 'Scale': 0, 'SchemaName': '', 'TableName': '', 'Type': 'string'}, got None)
  $.UpdateCount: missing (expected 0, got None)
  $.Output: missing (expected '', got None)
  $.ResultSet.ColumnInfos: missing (expected [...], got None)
  $.ResultSet.ResultRows: missing (expected [], got None)

Cause 2 — athena-cats-traj-table-location-mistakes#task#17, GetQueryResults, status 200 vs 200:
  $.ResultSet.ResultSetMetadata.ColumnInfo[0].Type: value_mismatch (expected 'bigint', got 'integer')
  $.ResultSet.ResultSetMetadata.ColumnInfo[0].Precision: value_mismatch (expected 19, got 10)
  $.ResultSet.ResultSetMetadata.ColumnInfo[0].CatalogName: missing (expected 'hive', got None)
  $.ResultSet.ResultSetMetadata.ColumnInfo[0].SchemaName: missing (expected '', got None)
  $.ResultSet.ResultSetMetadata.ColumnInfo[0].TableName: missing (expected '', got None)
  $.ResultSet.ColumnInfos / $.ResultSet.ResultRows: missing
  $.ResultSet.Rows[1].Data[0].VarCharValue: value_mismatch (expected '4', got '2')  [downstream of the table-LOCATION cause]

Cause 3 — athena-kfi-traj-athena-reads-the-iceberg-table#task#11, GetQueryResults, status 200 vs 200:
  ColumnInfo[0].Type expected 'bigint', got 'integer'; Precision expected 19, got 10; CatalogName expected 'hive', got None; SchemaName expected '', got None; TableName expected '', got None; $.ResultSet.ColumnInfos and $.ResultSet.ResultRows missing.

Cause 4 — athena-kfi-traj-athena-query-context-mistakes#task#12, GetQueryResults, status 200 vs 200:
  ColumnInfo[0].Type: expected 'string', got 'varchar'
  ColumnInfo[0].Precision: expected 0, got 2147483647
  ColumnInfo[0].CaseSensitive: expected False, got True
  ColumnInfo[0].CatalogName expected 'hive', got None; SchemaName expected '', got None; TableName expected '', got None
  $.Output, $.ResultSet.ColumnInfos, $.ResultSet.ResultRows: missing

Cause 5 — athena-cats-traj-output-location-mistakes#task#8, GetQueryResults, status 200 vs 200:
  $.Output: missing (expected '', got None)
  $.ResultSet.ColumnInfos: missing (expected [], got None)
  $.ResultSet.ResultRows: missing (expected [], got None)

Cause 6 — athena-cats-traj-table-before-database-and-ddl-retries#task#6, GetQueryResults, status 200 vs 200:
  $.Output: missing (expected 'FAILED: SemanticException [Error 10072]: Database does not exist: <databaseName>', got None)

Smithy cited: com.amazonaws.athena#ColumnInfo members CatalogName/SchemaName/TableName/Type/Precision/Scale/Nullable/CaseSensitive (Precision and Scale default 0, CaseSensitive default false); GetQueryResultsOutput members UpdateCount (Long), ResultSet, NextToken; com.amazonaws.athena#ResultSet members Rows + ResultSetMetadata. $.Output, $.ResultSet.ColumnInfos and $.ResultSet.ResultRows are not modeled — recorded behavior only.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: fixture athena-cats-traj-query-execution-polling-reads, step task#5 — run a SHOW DATABASES statement through StartQueryExecution, poll GetQueryExecution to a terminal state, then call GetQueryResults and inspect ResultSet.ResultSetMetadata.ColumnInfo[0] and UpdateCount.
4. Cause 2: fixture athena-cats-traj-table-location-mistakes, step task#17 — run SELECT count(*) AS n FROM <db>.<table> and inspect ColumnInfo[0].Type and .Precision.
5. Cause 3: fixture athena-kfi-traj-athena-reads-the-iceberg-table, step task#11 — same count(*) read, inspect ColumnInfo[0].Type/.Precision.
6. Cause 4: fixture athena-kfi-traj-athena-query-context-mistakes, step task#12 — run SHOW TABLES and inspect ColumnInfo[0].Type, .Precision and .CaseSensitive.
7. Cause 5: fixture athena-cats-traj-output-location-mistakes, step task#8 — call GetQueryResults for a statement that produced no rows and observe the absent result set / output.
8. Cause 6: fixture athena-cats-traj-table-before-database-and-ddl-retries, step task#6 — submit DDL against a nonexistent database, let it fail, then read the query's result output.

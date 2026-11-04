Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-output-location-mistakes
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-output-location-mistakes athena-cats-traj-query-execution-polling-reads athena-kfi-traj-athena-query-context-mistakes athena-cats-traj-table-before-database-and-ddl-retries athena-cats-traj-table-location-mistakes athena-kfi-traj-athena-reads-the-iceberg-table athena-query-execution-inherits-workgroup-output-location
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetQueryResults|2xx->4xx|$.Output
Issue-Key: athena|GetQueryResults|2xx->4xx|$.ResultSet
Issue-Key: athena|GetQueryResults|2xx->4xx|status
Issue-Key: athena|GetQueryResults|2xx->4xx|$.UpdateCount
Issue-Related: 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md
Issue-Commit: fb394b2a45eb301a39bf46320599094d7846a70d

# Athena GetQueryResults refuses completed queries and query reads report only the result directory prefix (5 causes)

> _Note: this issue was identified against the code base at commit `fb394b2a45eb301a39bf46320599094d7846a70d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResults

## Description
Reading the results of a query that AWS completed is impossible on the emulator: GetQueryResults refuses with HTTP 400 in every recorded shape, and the query-execution record never names the result object AWS wrote. 5 causes.

Cause 1 of 5: GetQueryResults refuses a completed DDL/UTILITY statement instead of returning an empty result set.
AWS: Smithy: GetQueryResults input QueryExecutionId (required), output UpdateCount/ResultSet/NextToken; modeled errors InvalidRequestException, TooManyRequestsException, InternalServerException. For a statement that produced no rows AWS answers HTTP 200 with Output '' and ResultSet carrying empty ColumnInfos, ResultRows, ResultSetMetadata.ColumnInfo and Rows collections (the empty collections are present, not omitted).
Emulator: HTTP 400; Output and ResultSet absent.
Why: a client that polls to SUCCEEDED and then reads results gets a client error for a query that worked, and cannot distinguish 'no rows' from 'bad request'.
Fix: the athena crate's GetQueryResults handler and its result-set serializer.
Traces: athena-cats-traj-output-location-mistakes#task#8.

Cause 2 of 5: GetQueryResults refuses a completed metadata query whose result set has columns.
AWS: HTTP 200 with UpdateCount 0 and ResultSet whose ResultSetMetadata.ColumnInfo describes each projected column (CatalogName, Label, Name, Nullable, Precision, Scale, SchemaName, TableName, Type, CaseSensitive) alongside the Rows.
Emulator: HTTP 400; Output, ResultSet and UpdateCount all absent.
Why: a client cannot read column metadata or rows for a query AWS completed.
Fix: the athena crate's GetQueryResults handler and its ColumnInfo/ResultSetMetadata serializer.
Traces: athena-cats-traj-query-execution-polling-reads#task#5, athena-kfi-traj-athena-query-context-mistakes#task#12.

Cause 3 of 5: GetQueryResults refuses a FAILED statement whose textual Output AWS still serves.
AWS: for a failed DDL statement AWS answers HTTP 200 and returns the engine's textual output in Output, of the form "FAILED: SemanticException [Error 10072]: Database does not exist: <database>" (<database> from the statement).
Emulator: HTTP 400; Output absent.
Why: the diagnostic the client reads to explain a failed DDL is unavailable.
Fix: the athena crate's GetQueryResults handler (failed-statement output path).
Traces: athena-cats-traj-table-before-database-and-ddl-retries#task#6.

Cause 4 of 5: GetQueryResults refuses a completed SELECT whose rows AWS returns.
AWS: HTTP 200 with UpdateCount 0, ResultSet.Rows carrying the header row and the data rows as Data[].VarCharValue, and ResultSetMetadata.ColumnInfo describing the projected column (e.g. a bigint column with Precision 19, Scale 0).
Emulator: HTTP 400; ResultSet and UpdateCount absent.
Why: query results — the point of the service — can never be fetched.
Fix: the athena crate's GetQueryResults handler and its row serializer.
Traces: athena-cats-traj-table-location-mistakes#task#17, athena-kfi-traj-athena-reads-the-iceberg-table#task#11.

Cause 5 of 5: the query-execution record names the result directory instead of the result object, and omits reuse/statistics members.
AWS: Smithy: QueryExecution.ResultConfiguration.OutputLocation, QueryExecution.ResultReuseConfiguration, QueryExecution.SubstatementType, QueryExecutionStatistics (ServicePreProcessingTimeInMillis, ServiceProcessingTimeInMillis, ResultReuseInformation, DataScannedInBytes). After a query runs, AWS replaces the configured directory with the resolved result object: "<configured output location>/<QueryExecutionId>.csv" for result-producing statements and "<...>/<QueryExecutionId>.txt" for DDL/UTILITY statements. It also sets ResultReuseConfiguration.ResultReuseByAgeConfiguration.Enabled=false, Statistics.ResultReuseInformation.ReusedPreviousResult=false, SubstatementType, and the pre-processing/processing times; DataScannedInBytes is 0 for a query that scans no data (recorded behavior only: the recording additionally carries an undocumented QueryExecutionDetail mirror of the execution, which the public Smithy model does not declare).
Emulator: OutputLocation is echoed as the configured directory prefix with a trailing slash and no object key; ResultReuseConfiguration, Statistics.ResultReuseInformation, ServicePreProcessingTimeInMillis, ServiceProcessingTimeInMillis, SubstatementType and QueryExecutionDetail are absent, and DataScannedInBytes is non-zero (8) where AWS reports 0 for 'SELECT 1'.
Why: a client that reads OutputLocation to fetch the result file from S3 is handed a directory and cannot locate the object; result-reuse and statement-kind branching have nothing to read.
Fix: the athena crate's query-execution state (result-location resolution at completion) and the GetQueryExecution serializer.
Traces: athena-query-execution-inherits-workgroup-output-location#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.
Cause 1 — athena-cats-traj-output-location-mistakes#task#8, GetQueryResults: expected 200 with $.Output='' and $.ResultSet={'ColumnInfos': [], 'ResultRows': [], 'ResultSetMetadata': {'ColumnInfo': []}, 'Rows': []}; actual 400 with both members missing.
Cause 2 — athena-cats-traj-query-execution-polling-reads#task#5, GetQueryResults: expected 200 with $.UpdateCount=0 and a ResultSet whose ColumnInfo describes the 'database_name' string column; actual 400, $.Output/$.ResultSet/$.UpdateCount missing (2 steps).
Cause 3 — athena-cats-traj-table-before-database-and-ddl-retries#task#6, GetQueryResults: expected 200 with $.Output='FAILED: SemanticException [Error 10072]: Database does not exist: <db>'; actual 400, $.Output missing.
Cause 4 — athena-cats-traj-table-location-mistakes#task#17, GetQueryResults: expected 200 with ResultSet.Rows=[{'Data': [{'VarCharValue': 'n'}]}, {'Data': [{'VarCharValue': '4'}]}] and $.UpdateCount=0; actual 400, both missing (2 steps).
Cause 5 — athena-query-execution-inherits-workgroup-output-location#task#4, GetQueryExecution: expected $.QueryExecution.ResultConfiguration.OutputLocation='s3://<bucket>/results/<QueryExecutionId>.csv', ResultReuseConfiguration={'ResultReuseByAgeConfiguration': {'Enabled': False}}, Statistics.ResultReuseInformation={'ReusedPreviousResult': False}, Statistics.ServicePreProcessingTimeInMillis and ServiceProcessingTimeInMillis present, SubstatementType='SELECT', Statistics.DataScannedInBytes=0; actual OutputLocation='s3://<bucket>/results/' (directory only), the reuse/statistics/SubstatementType members absent and DataScannedInBytes=8.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Inspect the listed fixture#step pairs, representative athena-cats-traj-output-location-mistakes#task#8.
4. Minimal manual repro: submit any statement with StartQueryExecution, wait for a terminal state, then call GetQueryResults with that QueryExecutionId (expect 200 with ResultSet and UpdateCount, empty collections present) and GetQueryExecution (expect ResultConfiguration.OutputLocation to end in '<QueryExecutionId>.csv' or '.txt').

Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.create-database-ddl-populates-glue-catalog-location athena.database-ddl-lifecycle athena.ctas-creates-queryable-table athena.statement-type-reflects-query-kind athena.get-table-metadata-reflects-ctas-table athena.query-execution-missing-table-fails athena.query-execution-missing-table-fails-with-athena-error athena.bytes-scanned-cutoff-fails-the-oversized-query athena.cancelled-scan-writes-no-result-object-and-rejects-results athena.columnar-projection-reads-a-fraction-of-the-row-format-bytes athena.concurrent-identical-queries-result-reuse-invariant athena.csv-serde-quoted-field-and-skipped-header-yield-exact-rows athena.ctas-partition-key-not-last-column-rejected athena.ctas-partitioned-by-writes-one-s3-prefix-per-partition athena.disabled-workgroup-preserves-running-query-and-rejects-new-start-rejected athena.execute-prepared-statement-from-other-workgroup-rejected athena.expected-bucket-owner-mismatch-fails-the-query-and-writes-nothing athena.external-table-query-returns-exact-ordered-rows athena.failed-query-error-classification-distinguishes-causes athena.group-by-aggregation-matches-independently-computed-values athena.iceberg-time-travel-returns-the-snapshot-before-the-row-level-update athena.insert-into-appends-rows-visible-to-subsequent-select athena.json-serde-nested-array-struct-and-map-project-exact-values athena.partition-projection-bounds-the-scan-with-no-metastore-partitions athena.partition-pruning-scans-only-the-selected-partition athena.prepared-statement-execute-with-parameters athena.query-execution-external-table-s3-roundtrip athena.query-results-header-row-only-for-select athena.query-results-pagination-next-token-exactness athena.query-results-typed-column-metadata athena.query-statistics-reflect-actual-scan athena.result-reuse-respects-query-identity athena.result-reuse-serves-stale-rows-after-the-source-object-changes athena.runtime-cast-of-non-numeric-text-rejected-with-invalid-cast-argument athena.schemaevo-add-column-reads-null-from-older-parquet-files athena.schemaevo-cross-family-type-change-fails-instead-of-reinterpreting-bytes athena.schemaevo-dropped-column-is-recovered-by-re-adding-it athena.schemaevo-index-access-reorder-fails-while-name-based-reorder-resolves athena.schemaevo-narrowing-past-a-stored-value-fails-instead-of-truncating athena.schemaevo-widen-int-to-bigint-keeps-int32-files-readable athena.unload-parquet-round-trips-through-a-new-external-table athena.view-select-returns-underlying-rows-and-is-not-a-table athena.window-functions-rank-and-partition-total-per-row athena.query-execution-result-reuse-ttl-window athena.query-execution-values-multirow-results-exact athena.query-result-reuse-cache-hit-on-rerun athena.athena-query-execution-syntax-error-failed-state athena.query-execution-failed-query-then-results-unavailable athena.start-query-execution-malformed-sql-rejected athena.query-execution-syntax-error-rejected-synchronously athena.query-execution-syntax-error-fails-with-athena-error
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.create-database-ddl-populates-glue-catalog-location|run#9|AssertionError|assert created["Status"]["State"] == "SUCCEEDED", created["Status"]
Issue-Key: athena.database-ddl-lifecycle|run#7|AssertionError|assert created["Status"]["State"] == "SUCCEEDED", created["Status"]
Issue-Key: athena.ctas-creates-queryable-table|run#8|AssertionError|assert created_db["Status"]["State"] == "SUCCEEDED", created_db["Status"]
Issue-Key: athena.statement-type-reflects-query-kind|run#7|AssertionError|assert ddl_exec["Status"]["State"] == "SUCCEEDED", ddl_exec["Status"]
Issue-Key: athena.get-table-metadata-reflects-ctas-table|run#8|AssertionError|assert created_db["Status"]["State"] == "SUCCEEDED", created_db["Status"]
Issue-Key: athena.query-execution-missing-table-fails|run#7|AssertionError|assert created["Status"]["State"] == "SUCCEEDED", created["Status"]
Issue-Key: athena.query-execution-missing-table-fails-with-athena-error|run#7|AssertionError|assert created["Status"]["State"] == "SUCCEEDED", created["Status"]
Issue-Key: athena.bytes-scanned-cutoff-fails-the-oversized-query|_run_sql#15|AssertionError|raise AssertionError(
Issue-Key: athena.cancelled-scan-writes-no-result-object-and-rejects-results|_run_sql#12|AssertionError|raise AssertionError(
Issue-Key: athena.columnar-projection-reads-a-fraction-of-the-row-format-bytes|_run_sql#13|AssertionError|raise AssertionError(
Issue-Key: athena.concurrent-identical-queries-result-reuse-invariant|_run_sql#11|AssertionError|raise AssertionError(
Issue-Key: athena.csv-serde-quoted-field-and-skipped-header-yield-exact-rows|_run_sql#13|AssertionError|raise AssertionError(
Issue-Key: athena.ctas-partition-key-not-last-column-rejected|_run_sql#13|AssertionError|raise AssertionError(
Issue-Key: athena.ctas-partitioned-by-writes-one-s3-prefix-per-partition|_run_sql#13|AssertionError|raise AssertionError(
Issue-Key: athena.disabled-workgroup-preserves-running-query-and-rejects-new-start-rejected|_run_sql#13|AssertionError|raise AssertionError(
Issue-Key: athena.execute-prepared-statement-from-other-workgroup-rejected|_run_sql#10|AssertionError|raise AssertionError(
Issue-Key: athena.expected-bucket-owner-mismatch-fails-the-query-and-writes-nothing|_run_sql#22|AssertionError|raise AssertionError(
Issue-Key: athena.external-table-query-returns-exact-ordered-rows|_run_sql#3|AssertionError|raise AssertionError(
Issue-Key: athena.failed-query-error-classification-distinguishes-causes|_run_sql#3|AssertionError|raise AssertionError(
Issue-Key: athena.group-by-aggregation-matches-independently-computed-values|_run_sql#11|AssertionError|raise AssertionError(
Issue-Key: athena.iceberg-time-travel-returns-the-snapshot-before-the-row-level-update|_run_sql#9|AssertionError|raise AssertionError(
Issue-Key: athena.insert-into-appends-rows-visible-to-subsequent-select|_run_sql#11|AssertionError|raise AssertionError(
Issue-Key: athena.json-serde-nested-array-struct-and-map-project-exact-values|_run_sql#11|AssertionError|raise AssertionError(
Issue-Key: athena.partition-projection-bounds-the-scan-with-no-metastore-partitions|_run_sql#12|AssertionError|raise AssertionError(
Issue-Key: athena.partition-pruning-scans-only-the-selected-partition|_run_sql#13|AssertionError|raise AssertionError(
Issue-Key: athena.prepared-statement-execute-with-parameters|_run_sql#3|AssertionError|raise AssertionError(
Issue-Key: athena.query-execution-external-table-s3-roundtrip|run#12|AssertionError|assert db_exec["Status"]["State"] == "SUCCEEDED", (
Issue-Key: athena.query-results-header-row-only-for-select|_run_sql#3|AssertionError|raise AssertionError(
Issue-Key: athena.query-results-pagination-next-token-exactness|_run_sql#3|AssertionError|raise AssertionError(
Issue-Key: athena.query-results-typed-column-metadata|_run_sql#3|AssertionError|raise AssertionError(
Issue-Key: athena.query-statistics-reflect-actual-scan|_run_sql#3|AssertionError|raise AssertionError(
Issue-Key: athena.result-reuse-respects-query-identity|_run_sql#3|AssertionError|raise AssertionError(
Issue-Key: athena.result-reuse-serves-stale-rows-after-the-source-object-changes|_run_sql#14|AssertionError|raise AssertionError(
Issue-Key: athena.runtime-cast-of-non-numeric-text-rejected-with-invalid-cast-argument|_run_sql#13|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-add-column-reads-null-from-older-parquet-files|_run_sql#12|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-cross-family-type-change-fails-instead-of-reinterpreting-bytes|_run_sql#12|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-dropped-column-is-recovered-by-re-adding-it|_run_sql#12|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-index-access-reorder-fails-while-name-based-reorder-resolves|_run_sql#12|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-narrowing-past-a-stored-value-fails-instead-of-truncating|_run_sql#14|AssertionError|raise AssertionError(
Issue-Key: athena.schemaevo-widen-int-to-bigint-keeps-int32-files-readable|_run_sql#12|AssertionError|raise AssertionError(
Issue-Key: athena.unload-parquet-round-trips-through-a-new-external-table|_run_sql#13|AssertionError|raise AssertionError(
Issue-Key: athena.view-select-returns-underlying-rows-and-is-not-a-table|_run_sql#11|AssertionError|raise AssertionError(
Issue-Key: athena.window-functions-rank-and-partition-total-per-row|_run_sql#14|AssertionError|raise AssertionError(
Issue-Key: athena.query-execution-result-reuse-ttl-window|_run_ddl#4|AssertionError|raise AssertionError(f"DDL {ddl!r} did not succeed, state={state}, reason={reason}")
Issue-Key: athena.query-execution-values-multirow-results-exact|run#5|AssertionError|assert execution["Status"]["State"] == "SUCCEEDED", execution["Status"]
Issue-Key: athena.query-result-reuse-cache-hit-on-rerun|run#9|AssertionError|assert exec1["Status"]["State"] == "SUCCEEDED", exec1["Status"]
Issue-Key: athena.athena-query-execution-syntax-error-failed-state|run#14|AssertionError|assert athena_error["ErrorMessage"] == expected_reason, (
Issue-Key: athena.query-execution-failed-query-then-results-unavailable|run#16|AssertionError|assert reason == expected_reason, f"unexpected StateChangeReason: {reason!r}"
Issue-Key: athena.start-query-execution-malformed-sql-rejected|run#8|AssertionError|raise AssertionError("expected StartQueryExecution to reject malformed SQL, but it accepted it")
Issue-Key: athena.query-execution-syntax-error-rejected-synchronously|run#8|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.query-execution-syntax-error-fails-with-athena-error|run#5|AssertionError|raise AssertionError("expected StartQueryExecution to reject the malformed query synchronously")
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Athena statements are executed by a SQLite engine: Hive/Trino DDL and DML are refused as syntax errors, missing-table failures carry SQLite diagnostics, and malformed SQL is accepted (5 causes)

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
The Athena data plane does not implement Athena's SQL dialect. Statements submitted through StartQueryExecution are handed to a SQLite parser/engine, so Hive DDL and Trino DML that AWS executes settle FAILED with a SQLite diagnostic, failures are attributed to the wrong object, and statements AWS refuses at submission are accepted. 5 causes. This is the dominant cause in the athena behavioral suite (50 of the failing checks below). Related open issues describe the same engine from the wire corpus but do not carry these behavioral check keys: 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md and 20261104T080454Z-athena-events-sqs-athena-ddl-never-succeeds-a-create-database-submitted.md.

Cause 1 of 5: Hive `CREATE DATABASE` is refused as a syntax error instead of succeeding.
AWS: Smithy: StartQueryExecution input QueryString (required, @length 1..262144); QueryExecutionStatus.State enum QUEUED/RUNNING/SUCCEEDED/FAILED/CANCELLED. A `CREATE DATABASE <name>` (with or without a `LOCATION '<s3-uri>'` clause, with or without backquoted identifiers) is valid Athena DDL: the execution reaches State SUCCEEDED, StatementType DDL, and the database appears in the catalog.
Emulator: every such statement reaches State FAILED with StateChangeReason and Status.AthenaError.ErrorMessage both set to a SQLite parser diagnostic of the fixed form `near "DATABASE": syntax error in <the submitted QueryString> at offset 7` — fixed text `near "DATABASE": syntax error in `, then the QueryString echoed verbatim, then ` at offset 7`. AthenaError.ErrorCategory is 2 and AthenaError.ErrorType is absent. The offset 7 and the token `DATABASE` show SQLite rejecting `CREATE DATABASE` outright (SQLite only knows CREATE TABLE/INDEX/VIEW/TRIGGER).
Why: no client can create a database through Athena, so every downstream table, CTAS, partition, SerDe, schema-evolution and result-reuse workflow in the suite dies on its first setup statement. A client waiting for SUCCEEDED gets a permanent failure for DDL AWS performs.
Fix: the athena crate's query-execution engine — the statement parser/executor behind StartQueryExecution must implement Athena's (Hive/Trino) DDL grammar rather than delegating to SQLite, and its AthenaError population must stop surfacing engine-internal parser text.
Traces: athena.create-database-ddl-populates-glue-catalog-location, athena.database-ddl-lifecycle, athena.ctas-creates-queryable-table, athena.statement-type-reflects-query-kind, athena.get-table-metadata-reflects-ctas-table, athena.query-execution-missing-table-fails, athena.query-execution-missing-table-fails-with-athena-error, athena.bytes-scanned-cutoff-fails-the-oversized-query, athena.cancelled-scan-writes-no-result-object-and-rejects-results, athena.columnar-projection-reads-a-fraction-of-the-row-format-bytes, athena.concurrent-identical-queries-result-reuse-invariant, athena.csv-serde-quoted-field-and-skipped-header-yield-exact-rows, athena.ctas-partition-key-not-last-column-rejected, athena.ctas-partitioned-by-writes-one-s3-prefix-per-partition, athena.disabled-workgroup-preserves-running-query-and-rejects-new-start-rejected, athena.execute-prepared-statement-from-other-workgroup-rejected, athena.expected-bucket-owner-mismatch-fails-the-query-and-writes-nothing, athena.external-table-query-returns-exact-ordered-rows, athena.failed-query-error-classification-distinguishes-causes, athena.group-by-aggregation-matches-independently-computed-values, athena.iceberg-time-travel-returns-the-snapshot-before-the-row-level-update, athena.insert-into-appends-rows-visible-to-subsequent-select, athena.json-serde-nested-array-struct-and-map-project-exact-values, athena.partition-projection-bounds-the-scan-with-no-metastore-partitions, athena.partition-pruning-scans-only-the-selected-partition, athena.prepared-statement-execute-with-parameters, athena.query-execution-external-table-s3-roundtrip, athena.query-results-header-row-only-for-select, athena.query-results-pagination-next-token-exactness, athena.query-results-typed-column-metadata, athena.query-statistics-reflect-actual-scan, athena.result-reuse-respects-query-identity, athena.result-reuse-serves-stale-rows-after-the-source-object-changes, athena.runtime-cast-of-non-numeric-text-rejected-with-invalid-cast-argument, athena.schemaevo-add-column-reads-null-from-older-parquet-files, athena.schemaevo-cross-family-type-change-fails-instead-of-reinterpreting-bytes, athena.schemaevo-dropped-column-is-recovered-by-re-adding-it, athena.schemaevo-index-access-reorder-fails-while-name-based-reorder-resolves, athena.schemaevo-narrowing-past-a-stored-value-fails-instead-of-truncating, athena.schemaevo-widen-int-to-bigint-keeps-int32-files-readable, athena.unload-parquet-round-trips-through-a-new-external-table, athena.view-select-returns-underlying-rows-and-is-not-a-table, athena.window-functions-rank-and-partition-total-per-row.

Cause 2 of 5: Hive `CREATE EXTERNAL TABLE` is refused as a syntax error.
AWS: `CREATE EXTERNAL TABLE <name> (<cols>) ROW FORMAT DELIMITED FIELDS TERMINATED BY '<c>' LOCATION '<s3-uri>'` is valid Athena DDL and reaches State SUCCEEDED, registering an external table.
Emulator: State FAILED with StateChangeReason of the fixed form `near "EXTERNAL": syntax error in <the submitted QueryString> at offset 7` — the `EXTERNAL` keyword is unknown to the engine.
Why: external tables over S3 — the primary Athena use case — can never be defined, so no SELECT has anything to read.
Fix: the athena crate's query-execution engine (Hive CREATE EXTERNAL TABLE / SerDe / ROW FORMAT grammar and its catalog registration).
Traces: athena.query-execution-result-reuse-ttl-window.

Cause 3 of 5: Trino inline-table `VALUES` in a SELECT is refused as a syntax error.
AWS: `SELECT * FROM (VALUES (1,'alpha'),(2,'beta')) AS t(id, name) ORDER BY id` is valid Trino/Athena SQL and reaches State SUCCEEDED, returning the literal rows.
Emulator: State FAILED with StateChangeReason of the fixed form `near "(": syntax error in <the submitted QueryString> at offset <the character offset of the VALUES list in the QueryString>` — the engine cannot parse the parenthesised inline table or the `AS t(col, col)` column alias list.
Why: a client cannot run a self-contained query at all; any query without catalog dependencies — the usual connectivity/smoke probe — fails.
Fix: the athena crate's query-execution engine (Trino VALUES / relation-alias grammar).
Traces: athena.query-execution-values-multirow-results-exact, athena.query-result-reuse-cache-hit-on-rerun.

Cause 4 of 5: a reference to a table or schema that does not exist fails with the engine's own 'no such table' text instead of Athena's classified diagnostic.
AWS: when the statement references an absent table, the execution fails with Status.AthenaError populated (Smithy: AthenaError { ErrorCategory, ErrorType @range 0..9999, Retryable, ErrorMessage }) and StateChangeReason carrying Athena's diagnostic. For a missing table AWS reports a TABLE_NOT_FOUND-class message of the fixed form `line <line>:<column>: Table '<catalog>.<database>.<table>' does not exist`; for a missing schema, `SCHEMA_NOT_FOUND: line <line>:<column>: Schema '<database>' does not exist`. The line/column and the qualified name come from the submitted QueryString and the QueryExecutionContext; the rest is AWS's fixed text, and AthenaError.ErrorType identifies the class.
Emulator: StateChangeReason and AthenaError.ErrorMessage are both the SQLite message `no such table: <database>.<table>`, AthenaError.ErrorType is absent, and a missing *schema* is reported as a missing *table*.
Why: a client cannot distinguish a missing database from a missing table, cannot branch on ErrorType, and sees an error string no Athena client parses.
Fix: the athena crate's query-execution engine (catalog/schema resolution) and its AthenaError population.
Traces: athena.athena-query-execution-syntax-error-failed-state, athena.query-execution-failed-query-then-results-unavailable.

Cause 5 of 5: StartQueryExecution accepts unparseable SQL instead of refusing it synchronously.
AWS: Athena parses the statement at submission. Unparseable SQL is refused with HTTP 400 InvalidRequestException whose AthenaErrorCode and ErrorCode are both MALFORMED_QUERY and whose Message is the parser diagnostic of the fixed form `line <line>:<column>: mismatched input '<offending token>'. Expecting: <comma-separated list of accepted tokens>` — line, column, token and expectation list derived from the submitted QueryString. No QueryExecutionId is issued.
Emulator: HTTP 200 with a QueryExecutionId; the statement is queued and only later settles FAILED (or, in one check, is accepted outright with no refusal at all).
Why: a client that relies on submission-time rejection to surface a SQL syntax error instead receives an execution id, must poll, and then has to mis-diagnose a generic failure.
Fix: the athena crate's StartQueryExecution handler — statement parsing/validation before an id is issued — and the Athena error serializer (AthenaErrorCode/ErrorCode = MALFORMED_QUERY).
Traces: athena.start-query-execution-malformed-sql-rejected, athena.query-execution-syntax-error-rejected-synchronously, athena.query-execution-syntax-error-fails-with-athena-error.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Service pass rate: athena 30/311 (10%).

Cause 1 — athena.database-ddl-lifecycle, GetQueryExecution after StartQueryExecution of `CREATE DATABASE sim2real_athena_lcdb_<id> LOCATION 's3://<bucket>/db/'`: expected Status.State='SUCCEEDED'; actual {'State': 'FAILED', 'StateChangeReason': 'near "DATABASE": syntax error in CREATE DATABASE sim2real_athena_lcdb_<id> LOCATION \'s3://<bucket>/db/\' at offset 7', 'AthenaError': {'ErrorCategory': 2, 'Retryable': False, 'ErrorMessage': <same text>}} — ErrorType absent. Identical shape on 43 further checks, including the backquoted form `CREATE DATABASE \`sim2real_behav_db_<id>\`` → 'near "DATABASE": syntax error ... at offset 7'.
Cause 2 — athena.query-execution-result-reuse-ttl-window: DDL "CREATE EXTERNAL TABLE sim2real_behav_reuse_tbl_<id> (id int, val string) ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' LOCATION 's3://<bucket>/reuse-data/'" expected SUCCEEDED; actual state=FAILED, reason='near "EXTERNAL": syntax error in <that statement> at offset 7'.
Cause 3 — athena.query-execution-values-multirow-results-exact: expected SUCCEEDED for `SELECT * FROM (VALUES (1, 'alpha'), (2, 'beta'), (3, 'gamma')) AS t(id, name) ORDER BY id`; actual FAILED with 'near "(": syntax error in <that statement> at offset 67'. athena.query-result-reuse-cache-hit-on-rerun: same for `SELECT '<marker>' AS marker, count(*) AS c FROM (VALUES (1), (2), (3)) AS t(x)`, 'near "(": ... at offset 86'.
Cause 4 — athena.athena-query-execution-syntax-error-failed-state: unexpected AthenaError.ErrorMessage 'no such table: sim2real_nonexistent_db_<id>.sim2real_nonexistent_table_<id>'. athena.query-execution-failed-query-then-results-unavailable: unexpected StateChangeReason 'no such table: sim2real_behav_no_such_db_xyz.no_such_table_xyz'.
Cause 5 — athena.start-query-execution-malformed-sql-rejected: 'expected StartQueryExecution to reject malformed SQL, but it accepted it'. athena.query-execution-syntax-error-rejected-synchronously: 'expected unparseable SQL to be rejected at submit time, got None'. athena.query-execution-syntax-error-fails-with-athena-error: 'expected StartQueryExecution to reject the malformed query synchronously'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect the athena checks listed in trace_ids.
3. Minimal manual repro:
   a. StartQueryExecution with QueryString="CREATE DATABASE probe_db" and a ResultConfiguration.OutputLocation, then poll GetQueryExecution — expect Status.State=SUCCEEDED, StatementType=DDL; observe FAILED with 'near "DATABASE": syntax error ... at offset 7'.
   b. StartQueryExecution with "CREATE EXTERNAL TABLE probe_t (id int) ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' LOCATION 's3://<bucket>/d/'" — expect SUCCEEDED; observe FAILED with 'near "EXTERNAL": ...'.
   c. StartQueryExecution with "SELECT * FROM (VALUES (1,'a'),(2,'b')) AS t(id,name)" — expect SUCCEEDED with two rows; observe FAILED with 'near "(": ...'.
   d. StartQueryExecution with a SELECT over a nonexistent database.table — expect AthenaError.ErrorType set and StateChangeReason naming the absent Table/Schema in Athena's wording; observe 'no such table: <db>.<table>' with ErrorType absent.
   e. StartQueryExecution with QueryString="SELCT 1" — expect HTTP 400 InvalidRequestException, AthenaErrorCode=ErrorCode=MALFORMED_QUERY and a 'line L:C: mismatched input ...' Message, and no QueryExecutionId; observe HTTP 200 with a QueryExecutionId.

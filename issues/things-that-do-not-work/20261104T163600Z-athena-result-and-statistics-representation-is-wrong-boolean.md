Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.query-results-metadata-and-rows athena.query-results-decimal-literal-precision-preserved athena.query-results-numeric-precision-preserved athena.query-results-unicode-and-numeric-literal-preserved athena.athena-query-execution-success-lifecycle athena.op-contract-batch-get-query-execution athena.list-query-executions-reflects-workgroup-scope
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.query-results-metadata-and-rows|run#12|AssertionError|assert by_name["flag"] == "boolean", by_name
Issue-Key: athena.query-results-decimal-literal-precision-preserved|run#19|AssertionError|assert data_value == decimal_literal, (
Issue-Key: athena.query-results-numeric-precision-preserved|run#15|AssertionError|assert decimal_value == decimal_literal, (
Issue-Key: athena.query-results-unicode-and-numeric-literal-preserved|run#16|AssertionError|assert data_row[1]["VarCharValue"] == _NUMERIC_LITERAL, (
Issue-Key: athena.athena-query-execution-success-lifecycle|run#6|AssertionError|assert qe["Statistics"]["DataScannedInBytes"] == 0, qe["Statistics"]
Issue-Key: athena.op-contract-batch-get-query-execution|run#10|AssertionError|assert scanned == 0, f"literal-only query should scan 0 bytes, got {scanned}"
Issue-Key: athena.list-query-executions-reflects-workgroup-scope|run#13|KeyError|ids_in_b = athena.list_query_executions(WorkGroup=wg_b)["QueryExecutionIds"]
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Athena result and statistics representation is wrong: BOOLEAN typed as integer, DECIMAL values returned as floats, synthetic query statistics, and ListQueryExecutions returns no ids (5 causes)

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResults

## Description
For the few statements the emulator does execute, the values and metadata it serves do not match Athena's. Column types are mis-derived, exact decimal/numeric values are passed through a floating-point representation and lose digits, query statistics are fixed synthetic constants rather than measurements, and ListQueryExecutions serializes no ids. 5 causes.

Cause 1 of 5: a BOOLEAN column is reported with Type 'integer'.
AWS: Smithy: GetQueryResults output ResultSet.ResultSetMetadata.ColumnInfo[*].Type (required). For `SELECT CAST(42 AS INTEGER) AS num, 'hello' AS txt, true AS flag`, Athena reports Type 'integer', 'varchar' and 'boolean' respectively, and the data row's VarCharValue for the boolean is 'true'.
Emulator: ColumnInfo types are {'num': 'integer', 'txt': 'varchar', 'flag': 'integer'} — the boolean column is typed 'integer', the type the underlying SQLite engine stores booleans as.
Why: a client that maps ColumnInfo.Type to a native type binds the column as a number, so `true`/`false` become 1/0 and any schema the client derives from Athena metadata is wrong.
Fix: the athena crate's GetQueryResults ResultSetMetadata serializer and the engine-type → Athena-type mapping behind it.
Traces: athena.query-results-metadata-and-rows.

Cause 2 of 5: a DECIMAL value is returned in floating-point notation with digits lost.
AWS: `SELECT CAST('<literal>' AS DECIMAL(38,9))` round-trips the exact decimal string through GetQueryResults: the data cell's VarCharValue is the literal's exact decimal representation, with no rounding, no trailing-digit loss and no exponent notation (33 significant digits fit inside DECIMAL(38,9)).
Emulator: the cell carries a float rendering in scientific notation — the exact decimal is replaced by a ~17-significant-digit double in `<mantissa>e<exponent>` form.
Why: monetary and high-precision analytical values are silently corrupted; a client parsing the cell as a decimal gets a different number than Athena returned and cannot even detect the loss.
Fix: the athena crate's result serializer and the engine's DECIMAL handling — decimal values must be carried as exact strings, not as doubles.
Traces: athena.query-results-decimal-literal-precision-preserved, athena.query-results-numeric-precision-preserved.

Cause 3 of 5: a high-precision numeric literal is truncated/rounded in the result cell.
AWS: a numeric literal selected and read back through GetQueryResults preserves its exact digits.
Emulator: the cell carries a rounded value with fewer fractional digits than the literal.
Why: as cause 2, on the literal path — the result a client reads is not the value it asked for.
Fix: same result serializer / numeric representation path as cause 2.
Traces: athena.query-results-unicode-and-numeric-literal-preserved.

Cause 4 of 5: QueryExecution.Statistics are fixed synthetic constants, not measurements.
AWS: Smithy: QueryExecutionStatistics { EngineExecutionTimeInMillis, DataScannedInBytes, TotalExecutionTimeInMillis, QueryQueueTimeInMillis, QueryPlanningTimeInMillis, ServicePreProcessingTimeInMillis, ServiceProcessingTimeInMillis }. A literal-only statement with no FROM clause (e.g. `SELECT 1 AS col`) scans no data, so DataScannedInBytes is 0; the timing members reflect the actual execution.
Emulator: DataScannedInBytes is 8 for `SELECT 1 AS col`, and the timings are the same constants on every execution — TotalExecutionTimeInMillis 3000 and QueryQueueTimeInMillis 1500 while EngineExecutionTimeInMillis is 0 (an internally inconsistent set: queue+engine does not relate to total). BatchGetQueryExecution reports the same non-zero scan for a literal-only query.
Why: cost and performance controls read these numbers. A client that bills, alarms or applies a bytes-scanned budget on DataScannedInBytes charges for a scan that did not happen, and one that reports latency shows a fabricated 3 s for every query.
Fix: the athena crate's query-execution statistics population (measure the execution; report 0 scanned bytes when no data source is read) and the GetQueryExecution / BatchGetQueryExecution serializers.
Traces: athena.athena-query-execution-success-lifecycle, athena.op-contract-batch-get-query-execution.

Cause 5 of 5: ListQueryExecutions omits QueryExecutionIds entirely.
AWS: Smithy: ListQueryExecutions output QueryExecutionIds (QueryExecutionIdList, @length 1..50) plus NextToken; after queries have been submitted in a workgroup the response lists their ids, most recent first, scoped to the WorkGroup given in the request.
Emulator: the 200 response has no QueryExecutionIds member at all — a client dereferencing it raises KeyError 'QueryExecutionIds'.
Why: query history cannot be enumerated or paged, and a workgroup-scoped audit of what ran is impossible.
Fix: the athena crate's ListQueryExecutions handler, its workgroup scoping, and the query-execution state store it reads.
Traces: athena.list-query-executions-reflects-workgroup-scope.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout.

Cause 1 — athena.query-results-metadata-and-rows, GetQueryResults for `SELECT CAST(42 AS INTEGER) AS num, 'hello' AS txt, true AS flag`: assertion by_name['flag'] == 'boolean' fails; actual {'num': 'integer', 'txt': 'varchar', 'flag': 'integer'}.
Cause 2 — athena.query-results-decimal-literal-precision-preserved, `SELECT CAST('123456789012345678901234.123456789' AS DECIMAL(38,9)) AS dec_val`: expected='123456789012345678901234.123456789', actual='1.2345678901234569e23'. athena.query-results-numeric-precision-preserved: 'decimal precision not preserved: expected 1234567890123456789012345678.123456789, got 1.2345678901234569e27'.
Cause 3 — athena.query-results-unicode-and-numeric-literal-preserved: 'numeric literal mismatch: 123456789012345.13 != 123456789012345.123456789'.
Cause 4 — athena.athena-query-execution-success-lifecycle, GetQueryExecution for `SELECT 1 AS col`: assertion Statistics.DataScannedInBytes == 0 fails; actual {'EngineExecutionTimeInMillis': 0, 'DataScannedInBytes': 8, 'TotalExecutionTimeInMillis': 3000, 'QueryQueueTimeInMillis': 1500}. athena.op-contract-batch-get-query-execution: 'literal-only query should scan 0 bytes, got 8'.
Cause 5 — athena.list-query-executions-reflects-workgroup-scope: [error] KeyError: 'QueryExecutionIds' on the ListQueryExecutions response.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect the athena checks listed in trace_ids.
3. Minimal manual repro:
   a. StartQueryExecution("SELECT CAST(42 AS INTEGER) AS num, 'hello' AS txt, true AS flag") with an OutputLocation; after SUCCEEDED call GetQueryResults — ColumnInfo for 'flag' must be Type 'boolean' and the cell VarCharValue 'true'.
   b. StartQueryExecution("SELECT CAST('123456789012345678901234.123456789' AS DECIMAL(38,9)) AS dec_val") — the cell must be the exact decimal string, not an exponent-notation double.
   c. StartQueryExecution('SELECT 1 AS col') — GetQueryExecution must report Statistics.DataScannedInBytes 0 and measured (not constant 3000/1500) timings; BatchGetQueryExecution on the same id must agree.
   d. Submit two queries in workgroup W, then ListQueryExecutions(WorkGroup=W) — the response must carry QueryExecutionIds with those ids.

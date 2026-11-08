Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds-data.execute-statement-generated-fields-autoincrement rds-data.batch-execute-statement-generated-fields-per-row rds-data.execute-statement-database-parameter-switches-schema rds-data.aggregate-join-and-window-results-are-computed-by-the-engine rds-data.formatted-json-records-match-column-records rds-data.typed-value-round-trip rds-data.rds-data-column-metadata-members-and-result-set-options-under-json rds-data.rds-data-type-hint-is-required-for-engine-typed-parameters rds-data.rds-data-batch-update-delete-and-returning-report-empty-update-results rds-data.rds-data-concurrent-transactions-independent-and-dead-transaction-ids-refuse-apart
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data|ExecuteStatement|BadRequestException
Issue-Key: rds-data.aggregate-join-and-window-results-are-computed-by-the-engine|_arm#5|AssertionError|assert arm in field, (
Issue-Key: rds-data.formatted-json-records-match-column-records|_grade#4|AssertionError|assert obj["amount"] == "123.4567", (
Issue-Key: rds-data.typed-value-round-trip|_grade#4|AssertionError|assert field[arm] == expected, (
Issue-Key: rds-data.rds-data-column-metadata-members-and-result-set-options-under-json|_grade#4|AssertionError|assert reported == measured, (
Issue-Key: rds-data.rds-data-type-hint-is-required-for-engine-typed-parameters|_grade#4|AssertionError|assert _envelope(round_trip) == {"records": expected, "numberOfRecordsUpdated": 0}, (
Issue-Key: rds-data.rds-data-batch-update-delete-and-returning-report-empty-update-results|graded_batch#4|AssertionError|assert _envelope(resp) == {"updateResults": [_EMPTY] * entries}, (
Issue-Key: rds-data.rds-data-concurrent-transactions-independent-and-dead-transaction-ids-refuse-apart|_grade#4|AssertionError|assert in_b_after_a == b_sees, (
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# rds-data: 9 net-new root causes — MySQL DDL and CREATE DATABASE are still refused by the backing SQL parser, and successful results diverge (numeric arm, formatted numerics, blobs, timestamptz metadata, typed parameters, empty batches, transaction isolation)

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
REOPENS 20261023T045849Z-rds-data-result-fidelity-postgresql-statements-rejected-column.md: its fix left the call refused. That issue's causes 1-3 were 'statements AWS executes are rejected with HTTP 400' and its stated fix was the SQL execution layer behind the ExecuteStatement handler, which must accept the statement forms the API advertises; two statement forms exercised here (a MySQL AUTO_INCREMENT column definition and CREATE DATABASE) are still answered 400 BadRequestException by the backing parser.

Nine net-new causes, all in rds-data's statement execution and result serialisation. A fixer must close all nine before submitting.

Cause 1 of 9: a MySQL AUTO_INCREMENT column definition is a parse error.
AWS: recorded behavior only for the dialect (Smithy: ExecuteStatement$sql targets SqlStatement, @length max 65536, and the operation has no modelled error for valid engine SQL). On an aurora-mysql cluster, `CREATE TABLE <t> (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32))` is accepted (200), and a following INSERT reports the server-assigned key in generatedFields (first row of a fresh table = 1), with BatchExecuteStatement returning one distinct, increasing generatedFields value per parameterSet.
Emulator: 400 BadRequestException "sql parser error: Expected: ',' or ')' after column definition, found: PRIMARY at Line: 1, Column: <n>" — the parser does not accept the AUTO_INCREMENT column attribute, so the table is never created and neither generated-fields behaviour can be reached.
Why: the canonical MySQL identity-column DDL is unusable, so no client workflow that creates its own schema through the Data API can run, and the generatedFields contract is unobservable.
Fix: the SQL dialect/parse layer behind the ExecuteStatement/BatchExecuteStatement handlers for the MySQL engine, plus the generated-key propagation into generatedFields/updateResults[].generatedFields.
Traces: execute-statement-generated-fields-autoincrement, batch-execute-statement-generated-fields-per-row.

Cause 2 of 9: CREATE DATABASE is a syntax error, so the `database` request member cannot be exercised.
AWS: recorded behavior only (Smithy: ExecuteStatement$database targets DbName, @length max 64, 'the name of the database'). AWS accepts `CREATE DATABASE <name>` through the Data API and a following request naming that database in `database` runs against it.
Emulator: 400 BadRequestException 'near "DATABASE": syntax error in CREATE DATABASE <name> at offset 7' — the backing engine's own tokenizer wording, for a statement AWS executes.
Why: schema/database provisioning through the Data API is impossible and the `database`/`schema` routing members cannot be verified; the message also leaks a backing engine that is not Aurora.
Fix: the same SQL dialect layer, plus the database namespace the `database` member selects.
Traces: execute-statement-database-parameter-switches-schema.

Cause 3 of 9: exact-numeric results are returned in the doubleValue arm of the Field union instead of stringValue.
AWS: Smithy: Field is a union of isNull/booleanValue/longValue/doubleValue/stringValue/blobValue/arrayValue — exactly one arm is set per value. For Aurora PostgreSQL, NUMERIC/DECIMAL columns and aggregates over them (SUM over NUMERIC, SUM over bigint, which PostgreSQL promotes to numeric) come back as text in stringValue, preserving the operands' scale exactly.
Emulator: returns {'doubleValue': 30.75} where AWS returns {'stringValue': '30.75'}.
Why: the arm IS the type contract: every caller deserialises the wrong union member, scale is lost, and an exact total above 2**53 is silently rounded to a neighbouring double — a money column read through the emulator and through AWS differ in type and in value.
Fix: the Field-arm selection in the ExecuteStatement result serialiser (exact-numeric types -> stringValue, with the engine's text and scale), and exact (non-float) accumulation of aggregates.
Traces: aggregate-join-and-window-results-are-computed-by-the-engine.

Cause 4 of 9: in formatRecordsAs=JSON output, a numeric column is rendered as a JSON number instead of a JSON string.
AWS: Smithy: ExecuteStatementResponse.formattedRecords is the JSON rendering of the result set (RecordsFormatType NONE|JSON). Consistently with cause 3, an exact-numeric column appears there as a JSON STRING, so both forms refuse to turn it into a float.
Emulator: emits the bare number (e.g. 123.4567) in formattedRecords.
Why: a client parsing formattedRecords gets a float where AWS gives it an exact decimal string, so values are rounded in the JSON path even after the records path is fixed.
Fix: the value-to-JSON mapping in the formattedRecords renderer.
Traces: formatted-json-records-match-column-records.

Cause 5 of 9: a binary column comes back as the backing engine's hex-escape text instead of its bytes.
AWS: Smithy: Field$blobValue targets Blob (base64 on the REST-JSON wire), so a bytea column round-trips to exactly the bytes that were written.
Emulator: blobValue decodes to the ASCII text of the engine's hex literal (the "\\x" prefix followed by lowercase hex digits) rather than the stored bytes.
Why: binary round-trips corrupt silently — the client receives a longer byte string that is the printable spelling of its data, with no error raised.
Fix: the blob value mapping in the ExecuteStatement records serialiser (decode the engine's binary representation into Blob bytes).
Traces: typed-value-round-trip. (The open issue 20261103T222520Z-rds-data-executestatement-result-fidelity-postgresql-typed-values.md records the same hex-escape leak in formattedRecords; this is the records/blobValue path and that issue carries no behavioral check.)

Cause 6 of 9: columnMetadata for a timestamptz column reports the wrong JDBC type code and loses the fractional-seconds scale.
AWS: Smithy: ColumnMetadata{name,label,type,typeName,precision,scale,nullable,isSigned,isCaseSensitive,isAutoIncrement,isCurrency,arrayBaseColumnType,schemaName,tableName}. For a PostgreSQL timestamptz column AWS reports type 1111 (OTHER) with typeName 'timestamptz' and scale 6, nullable 1, precision 35.
Emulator: reports type 93 (TIMESTAMP) and scale 0, with the rest of the entry matching.
Why: a client that maps results by JDBC type code binds timestamptz to a plain timestamp and reads it with zero sub-second scale, so microsecond precision and the time-zone-aware mapping are lost.
Fix: the columnMetadata builder's type-code/scale table in the ExecuteStatement result serialiser.
Traces: rds-data-column-metadata-members-and-result-set-options-under-json.

Cause 7 of 9: typeHint parameters do not reach the column as that column's own type, so the value comes back re-rendered.
AWS: Smithy: SqlParameter$typeHint (JSON|UUID|TIMESTAMP|DATE|TIME|DECIMAL) sends the String parameter as an object of that type to the database — TIME accepts HH:MM:SS[.FFF]. The round trip therefore returns the engine's own text for that type: a JSON value keeps the member order the request supplied, and a TIME value keeps its three-digit fractional part (e.g. the submitted '18:30:00.500' comes back as '18:30:00.500').
Emulator: returns the JSON object with its members re-ordered and the TIME value with its fractional part re-formatted ('18:30:00.5'), i.e. the parameter was stored as a generic string/number and re-serialised by the emulator rather than typed by the engine.
Why: a client that compares, hashes or diffs what it wrote against what it reads back sees a different document and a different time literal, for data it never changed.
Fix: the typed-parameter binding path in the ExecuteStatement handler (apply typeHint as the column's type at bind time) and the text the result serialiser returns for those types.
Traces: rds-data-type-hint-is-required-for-engine-typed-parameters.

Cause 8 of 9: BatchExecuteStatement with parameterSets omitted returns no updateResults at all.
AWS: Smithy: BatchExecuteStatement$parameterSets is optional; the response carries only updateResults (list of UpdateResult{generatedFields}). With parameterSets omitted, AWS runs the statement once and answers exactly one UpdateResult, whose generatedFields is empty for a statement that generates nothing — and no numberOfRecordsUpdated, records or formattedRecords member (those belong to ExecuteStatement).
Emulator: answers {'updateResults': []} — the statement is not run once, and the single result entry is missing.
Why: a client that batches a parameterless statement gets neither the effect nor the one result entry it indexes, so a code path that works on AWS silently does nothing here.
Fix: the parameterSets handling in the BatchExecuteStatement handler (an absent/empty parameterSets means one execution with one UpdateResult).
Traces: rds-data-batch-update-delete-and-returning-report-empty-update-results.

Cause 9 of 9: concurrent transactions do not read committed data — a transaction sees only its own writes.
AWS: recorded behavior only (Smithy: BeginTransaction/CommitTransaction/RollbackTransaction with ExecuteStatement$transactionId). The engine's default isolation is READ COMMITTED, so a transaction B that is still open reads every row transaction A committed after B began, alongside B's own uncommitted row.
Emulator: B reads only the pre-existing baseline row and its own write; rows A committed while B was open are invisible to B.
Why: the emulator behaves as if each transaction had a private copy of the database, so any application logic or test that depends on reading another transaction's committed effect (queue draining, idempotency checks, read-modify-write) behaves differently here than on Aurora — and concurrency bugs it would catch are hidden.
Fix: the transaction/visibility model in rds-data state — transactions must share one store with committed-read visibility, not a per-transaction snapshot/overlay.
Traces: rds-data-concurrent-transactions-independent-and-dead-transaction-ids-refuse-apart.

## Evidence
Cause 1 — execute-statement-generated-fields-autoincrement and batch-execute-statement-generated-fields-per-row: botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the ExecuteStatement operation: sql parser error: Expected: ',' or ')' after column definition, found: PRIMARY at Line: 1, Column: 56 (resp. Column: 61) — for `CREATE TABLE <t> (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32))`. Expected 200, then generatedFields[0].longValue == 1 on the first insert / n distinct increasing ids across n parameterSets.
Cause 2 — execute-statement-database-parameter-switches-schema: BadRequestException: near "DATABASE": syntax error in CREATE DATABASE <name> at offset 7. Expected 200 and the new database usable via the `database` member.
Cause 3 — aggregate-join-and-window-results-are-computed-by-the-engine: AssertionError: SUM(amount) for {'stringValue': 'alice'}: the result must be returned in the 'stringValue' arm of the Field union; got {'doubleValue': 30.75}. The check also requires SUM over NUMERIC(12,s) to preserve scale s in the returned text and an exact SUM(bigint) above 2**53.
Cause 4 — formatted-json-records-match-column-records: AssertionError: numeric must ALSO be a JSON string in the formatted form — both forms refuse to turn it into a float; got 123.4567.
Cause 5 — typed-value-round-trip: AssertionError: c_bytea (bytea) must round-trip to b'\x01\x02\xff' in 'blobValue'; got b'\\x0102ff' (the three bytes are this run's value; the requirement is byte-identical round trip).
Cause 6 — rds-data-column-metadata-members-and-result-set-options-under-json: the metadata for the timestamptz column must report type 1111 and scale 6 (typeName 'timestamptz', nullable 1, precision 35, isSigned/isCaseSensitive/isAutoIncrement/isCurrency false, arrayBaseColumnType 0); it reports type 93 and scale 0.
Cause 7 — rds-data-type-hint-is-required-for-engine-typed-parameters: expected [[{'stringValue': '{"a":[2,3],"b":1}'}, …, {'stringValue': '18:30:00.500'}, …]]; got {'records': [[{'stringValue': '{"b":1,"a":[2,3]}'}, …, {'stringValue': '18:30:00.5'}, …]]} — member order and the TIME fractional part differ; the UUID/TIMESTAMP/DATE/DECIMAL members match.
Cause 8 — rds-data-batch-update-delete-and-returning-report-empty-update-results: AssertionError: a batched INSERT of a statement that binds nothing, with parameterSets omitted entirely must answer exactly 1 updateResults entries, every one of them {'generatedFields': []}, and no numberOfRecordsUpdated, records or formattedRecords key at all; got {'updateResults': []}.
Cause 9 — rds-data-concurrent-transactions-independent-and-dead-transaction-ids-refuse-apart: AssertionError: transaction B is still open and reads at READ COMMITTED, so after A commits B must see every row A committed alongside its own uncommitted row; expected [(1, 'baseline'), (10, 'txn-a-10'), (11, 'txn-a-11'), (20, 'txn-b-20')], got [(1, 'baseline'), (20, 'txn-b-20')].

## Steps to Reproduce
1. Build and boot the emulator from this checkout; replay the behavioral suite group rds-data (or the checks in trace_ids).
2. Cause 1: on an aurora-mysql Data API cluster, POST /Execute `CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32))` -> expect 200; observe 400 BadRequestException 'sql parser error: … found: PRIMARY'.
3. Cause 2: POST /Execute `CREATE DATABASE <name>` -> expect 200 and the database usable as the `database` member; observe 400 'near "DATABASE": syntax error'.
4. Cause 3: insert rows with a NUMERIC(12,2) amount and bigint qty, then POST /Execute `SELECT cust, SUM(amount) … GROUP BY … HAVING SUM(amount) > …` and `SELECT SUM(qty) …` -> expect each numeric in stringValue with scale preserved and an exact total; observe doubleValue.
5. Cause 4: the same SELECT with formatRecordsAs=JSON -> expect the numeric as a JSON string; observe a JSON number.
6. Cause 5: write three bytes into a bytea column and read it back -> expect blobValue to decode to those bytes; observe the ASCII of the engine's "\\x…" literal.
7. Cause 6: SELECT a timestamptz column with includeResultMetadata -> expect type 1111 / scale 6; observe 93 / 0.
8. Cause 7: POST /Execute with parameters carrying typeHint JSON and TIME ('HH:MM:SS.FFF') into typed columns, then read them back -> expect the JSON member order and the three-digit fractional seconds the request supplied; observe reordered JSON and a shortened time.
9. Cause 8: POST /BatchExecute with a statement that binds nothing and parameterSets omitted -> expect exactly one UpdateResult with empty generatedFields; observe updateResults [].
10. Cause 9: BeginTransaction A and B; insert and commit in A; read in B -> expect B to see A's committed rows plus its own uncommitted row; observe only the baseline and B's own row.

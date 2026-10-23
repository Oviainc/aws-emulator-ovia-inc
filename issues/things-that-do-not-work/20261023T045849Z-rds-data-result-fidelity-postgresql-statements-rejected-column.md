Issue-Source: trace-sweep-divergence
Issue-Trace: rdsdata-execute-statement-field-union-and-error-envelopes
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rdsdata-execute-statement-field-union-and-error-envelopes rdsdata-execute-statement-live-cluster-round-trip rdsdata-execute-statement-resource-and-sql-validation
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data|ExecuteStatement|2xx->4xx|$.generatedFields
Issue-Key: rds-data|ExecuteStatement|2xx->4xx|$.numberOfRecordsUpdated
Issue-Key: rds-data|ExecuteStatement|2xx->4xx|status
Issue-Key: rds-data|ExecuteStatement|2xx->4xx|$.columnMetadata
Issue-Key: rds-data|ExecuteStatement|2xx->4xx|$.records
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].isAutoIncrement
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].isCaseSensitive
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].isSigned
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].nullable
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].precision
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].scale
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].schemaName
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].type
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.columnMetadata[*].typeName
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.records[*]
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.records[*][*].arrayValue
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.records[*][*].blobValue
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.records[*][*].booleanValue
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.records[*][*].doubleValue
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.records[*][*].longValue
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.records[*][*].stringValue
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.generatedFields[*]
Issue-Key: rds-data|BatchExecuteStatement|2xx->2xx|$.updateResults[*].generatedFields[*]
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.formattedRecords
Issue-Key: rds-data|RollbackTransaction|2xx->2xx|$.transactionStatus
Issue-Related: 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions.md
Issue-Commit: 0f903c65ef17614b948ed7843da2bc7f944d5d1a

# rds-data result fidelity: PostgreSQL statements rejected, column metadata, generated fields and identity values wrong

> _Note: this issue was identified against the code base at commit `0f903c65ef17614b948ed7843da2bc7f944d5d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
The emulator answers rds-data statements from a SQL engine that is not Aurora PostgreSQL and serialises its results with a generic type mapping, so statements AWS executes are rejected, and the ones that do run come back with the wrong column metadata, the wrong generated-field semantics and missing identity-column values. Ten causes, all in rds-data, all in statement execution and result serialisation. No open issue covers rds-data.

Cause 1 of 10: a DML statement AWS executes is rejected with HTTP 400.
AWS: Smithy: ExecuteStatementResponse carries numberOfRecordsUpdated (long) and generatedFields (list). AWS runs the statement and answers HTTP 200 with numberOfRecordsUpdated set and generatedFields empty for Aurora PostgreSQL.
Emulator: answers HTTP 400 and no result body - the statement never executes.
Why: a client doing normal DML through the Data API gets a hard client error for a statement the real service accepts, so any workflow built on it cannot run against the emulator at all.
Fix: the SQL execution layer behind the ExecuteStatement handler - it must accept the Aurora PostgreSQL statement forms the API advertises.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#task#15.

Cause 2 of 10: a typed SELECT AWS executes is rejected with HTTP 400.
AWS: Smithy: ExecuteStatement answers 200 with records and columnMetadata. The recorded response returns six columns (int4, text, bool, numeric, float8, text) and one record, with the numeric column serialised as a stringValue.
Emulator: answers HTTP 400 with no records/columnMetadata/numberOfRecordsUpdated.
Why: the single most common Data API call - a SELECT of typed expressions - fails, so the field-union serialisation the API is built around is never exercised.
Fix: same SQL execution layer, plus the Field union mapping for numeric/float8.
Traces: rdsdata-execute-statement-live-cluster-round-trip#task#8.

Cause 3 of 10: PostgreSQL cast syntax is a parse error instead of an unsupported-result refusal.
AWS: Smithy: ExecuteStatement models UnsupportedResultException (client, httpError 400, doc 'It contained an unsupported data type'). AWS parses the statement, runs it, and refuses only the RESULT, at HTTP 400 (recorded message, as an example: 'The result contains the unsupported data type INTERVAL.').
Emulator: answers HTTP 400 with x-amzn-errortype BadRequestException and a tokenizer error (recorded example: 'unrecognized token: ":" in SELECT ''1 day''::interval AS iv at offset 14'), i.e. it cannot even parse the PostgreSQL '::' cast.
Why: the error code differs and the cause is misreported: the client is told its SQL is syntactically invalid when AWS accepts the SQL and objects to the result type, so it 'fixes' valid PostgreSQL instead of projecting the column differently.
Fix: the SQL dialect/parse layer, and the UnsupportedResultException path in the ExecuteStatement result serialiser.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#task#19 (cluster of 6, also field-union task#20, rdsdata-execute-statement-live-cluster-round-trip#task#21 and rdsdata-execute-statement-resource-and-sql-validation#task#2, #3, #6).

Cause 4 of 10: columnMetadata is filled from a generic type map instead of PostgreSQL catalog data.
AWS: Smithy: ColumnMetadata carries type (JDBC type code), typeName (the database-specific type name), precision, scale, nullable, schemaName, isSigned, isCaseSensitive, isAutoIncrement. Recorded Aurora PostgreSQL values are JDBC codes (4 int4, 12 text, -7 bool, 8 float8, 2 numeric), lower-case PostgreSQL type names ('serial', 'text', 'int8', 'float8', 'bool'), real precisions (10, 19, 17, 2147483647, 1), nullable 0 for a NOT NULL identity column, and schemaName present as ''.
Emulator: emits type 1111/-1/16 placeholders, upper-case names ('SERIAL', 'TEXT', 'BIGINT', 'FLOAT8', 'BOOLEAN'), precision/scale 0, nullable 1 everywhere, isSigned/isCaseSensitive/isAutoIncrement false, and omits schemaName entirely.
Why: every client that maps results by JDBC type code, or that renders/validates against typeName, sees 'unknown type' (1111) and a name that does not exist in PostgreSQL; nullability and auto-increment are reported wrongly for identity columns.
Fix: the columnMetadata builder in the ExecuteStatement result serialiser.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#task#17 (11 columns affected).

Cause 5 of 10: the same columnMetadata gap on the live round-trip fixture.
AWS: as cause 4 - recorded metadata for the identity and text columns is type 4/'serial' with precision 10 and nullable 0, and type 12/'text' with precision 2147483647 and isCaseSensitive true.
Emulator: type 1111/'SERIAL' and -1/'TEXT', precision 0, nullable 1, isCaseSensitive/isAutoIncrement/isSigned false, schemaName absent; the identity column's record values are missing too.
Why: as cause 4, and it shows the defect is in the shared serialiser rather than in one statement.
Fix: same columnMetadata builder.
Traces: rdsdata-execute-statement-live-cluster-round-trip#task#18.

Cause 6 of 10: ExecuteStatement populates generatedFields, which Aurora PostgreSQL does not.
AWS: Smithy: ExecuteStatementResponse.generatedFields doc - 'The generatedFields data isn't supported by Aurora PostgreSQL. To get the values of generated fields, use the RETURNING clause.' The recorded response for an insert is HTTP 200 with generatedFields [].
Emulator: returns generatedFields with an entry (recorded example: {'longValue': 2}).
Why: a client developed against the emulator reads the inserted key out of generatedFields and works; against Aurora PostgreSQL the list is empty, so the code breaks in production on exactly the path the emulator blessed.
Fix: the generatedFields population in the ExecuteStatement result serialiser - leave it empty for the PostgreSQL engine.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#task#16.

Cause 7 of 10: BatchExecuteStatement populates updateResults[].generatedFields the same way.
AWS: Smithy: UpdateResult.generatedFields; the same Aurora PostgreSQL restriction applies and the recorded response has no generatedFields entries for either batch row.
Emulator: returns updateResults[0].generatedFields[0] and updateResults[1].generatedFields[0] (recorded examples {'longValue': 3} and {'longValue': 4}).
Why: as cause 6, on the batch path.
Fix: the same generated-fields logic as used by the BatchExecuteStatement handler.
Traces: rdsdata-execute-statement-live-cluster-round-trip#task#19.

Cause 8 of 10: values of identity/SERIAL columns are missing from records.
AWS: Smithy: ExecuteStatementResponse.records is a list of Field unions; the recorded responses carry the identity column as a longValue (e.g. longValue 1, longValue 3).
Emulator: that position is absent from the record - no longValue and no isNull.
Why: the primary key of every selected row is silently missing, so a client that reads row ids from a SELECT (to then update, delete or join) gets nothing where AWS gives it the key, and the field union it receives is not even one of the modeled variants.
Fix: the record/field serialisation for auto-increment columns in the ExecuteStatement result path.
Traces: rdsdata-execute-statement-live-cluster-round-trip#task#10 (cluster of 4, also task#12, task#14, task#16 of the same fixture).

Cause 9 of 10: formattedRecords (formatRecordsAs JSON) loses a whole row and nulls the identity column.
AWS: Smithy: ExecuteStatementResponse.formattedRecords - the JSON rendering of the result set. The recorded value contains all three rows with their real ids ('id':1, 'id':2, 'id':3).
Emulator: returns only two rows and both with 'id':null.
Why: the JSON-formatted result set is not the same result set: one row is dropped entirely and the key column is nulled, so a client using formatRecordsAs=JSON reads an incomplete table and cannot key its rows.
Fix: the formattedRecords renderer in the ExecuteStatement result serialiser (same identity-column defect as cause 8, plus a dropped first row).
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#task#18 (cluster of 2, also rdsdata-execute-statement-live-cluster-round-trip#task#20).

Cause 10 of 10: RollbackTransaction reports a transactionStatus string AWS does not use.
AWS: Smithy: RollbackTransactionResponse.transactionStatus (TransactionStatus, @length max 128), 'The status of the rollback operation'. The recorded AWS value is 'Rollback complete'.
Emulator: returns 'Transaction Rolled Back' at HTTP 200.
Why: transactionStatus is a modeled response value, not an error message; a client that asserts or logs the rollback status sees a string the service never emits.
Fix: the status literal in the RollbackTransaction handler's response builder.
Traces: rdsdata-execute-statement-live-cluster-round-trip#task#13.

## Evidence
Cause 1 (rdsdata-execute-statement-field-union-and-error-envelopes#task#15): status expected 200, got 400; $.numberOfRecordsUpdated expected 1, got None; $.generatedFields expected [], got None. Cause 2 (rdsdata-execute-statement-live-cluster-round-trip#task#8): status expected 200, got 400; $.columnMetadata (6 entries incl. type 4/'int4', 12/'text', -7/'bool', 2/'numeric', 8/'float8') missing; $.records expected [[{'longValue':1},{'stringValue':'abc'},{'booleanValue':True},{'stringValue':'2.5'},{'doubleValue':1.5},{'isNull':True}]], got None. Cause 3 (field-union#task#19, 400->400, 6 steps): $.message expected 'The result contains the unsupported data type INTERVAL.', got 'unrecognized token: ":" in SELECT \'1 day\'::interval AS iv at offset 14'; x-amzn-errortype got 'BadRequestException'. Cause 4 (field-union#task#17, 200->200): columnMetadata[0].type expected 4 got 1111, typeName expected 'serial' got 'SERIAL', precision expected 10 got 0, nullable expected 0 got 1, isAutoIncrement/isSigned expected True got False, schemaName expected '' missing; [1].type expected 12 got -1, typeName 'text' vs 'TEXT', precision 2147483647 vs 0; [2].typeName 'int8' vs 'BIGINT'; [4].type -7 vs 16, typeName 'bool' vs 'BOOLEAN'; same pattern through [10]. Cause 5 (live-cluster-round-trip#task#18): same metadata deltas on columns 0-1 plus $.records[0][0].longValue expected 1 got None and $.records[1][0].longValue expected 3 got None. Cause 6 (field-union#task#16, 200->200): $.generatedFields[0] extra (expected None, got {'longValue': 2}). Cause 7 (live-cluster-round-trip#task#19): $.updateResults[0].generatedFields[0] extra ({'longValue': 3}), $.updateResults[1].generatedFields[0] extra ({'longValue': 4}). Cause 8 (live-cluster-round-trip#task#10, 4 steps): $.records[0][0].longValue expected 1, got None. Cause 9 (field-union#task#18, 2 steps): $.formattedRecords expected 3 rows with 'id':1/2/3, got 2 rows both with 'id':null. Cause 10 (live-cluster-round-trip#task#13): $.transactionStatus expected 'Rollback complete', got 'Transaction Rolled Back'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. rdsdata-execute-statement-field-union-and-error-envelopes#task#15: POST /Execute with the recorded DML -> 400 instead of 200 with numberOfRecordsUpdated 1.
3. rdsdata-execute-statement-live-cluster-round-trip#task#8: POST /Execute with the recorded typed SELECT -> 400 instead of 200 with 6 columns and 1 record.
4. rdsdata-execute-statement-field-union-and-error-envelopes#task#19: POST /Execute with SELECT '1 day'::interval AS iv -> 400/BadRequestException tokenizer error instead of 400 'The result contains the unsupported data type INTERVAL.'.
5. field-union#task#17 and live-cluster-round-trip#task#18: inspect columnMetadata - JDBC type codes are 1111/-1/16, typeName is upper-cased, precision/scale 0, nullable always 1, schemaName absent.
6. field-union#task#16 and live-cluster-round-trip#task#19: insert rows and observe generatedFields / updateResults[].generatedFields populated where Aurora PostgreSQL returns them empty.
7. live-cluster-round-trip#task#10 and field-union#task#18: SELECT rows with a SERIAL primary key - records omit the identity value and formattedRecords drops a row and nulls 'id'.
8. live-cluster-round-trip#task#13: POST /RollbackTransaction -> transactionStatus 'Transaction Rolled Back' instead of 'Rollback complete'.

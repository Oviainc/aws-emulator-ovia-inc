Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds-data.batch-execute-statement-generated-fields-per-row rds-data.execute-statement-generated-fields-autoincrement rds-data.execute-statement-database-parameter-switches-schema rds-data.batch-execute-statement-atomic-rollback-on-constraint-violation rds-data.rds-data-unsupported-result-refusal-arrives-after-the-row-is-committed rds-data.rejects-bad-resource-secret-and-database-with-distinct-codes
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data|ExecuteStatement|DatabaseErrorException
Issue-Key: rds-data.batch-execute-statement-atomic-rollback-on-constraint-violation|run#10|AssertionError|assert error.get("Message") == (
Issue-Key: rds-data.rds-data-unsupported-result-refusal-arrives-after-the-row-is-committed|_refusal#4|AssertionError|raise AssertionError(
Issue-Key: rds-data.rejects-bad-resource-secret-and-database-with-distinct-codes|_refusal#5|AssertionError|raise AssertionError(
Issue-Commit: 8b0f35809934c567f1bf7b94b8ce8845151fefb8

# rds-data: 5 net-new root causes — an aurora-mysql cluster is served by a PostgreSQL dialect (AUTO_INCREMENT DDL and CREATE DATABASE refused, duplicate-key diagnostic in Postgres wording), an unrepresentable NaN result is not refused, and the `schema` member is honoured instead of refused

> _Note: this issue was identified against the code base at commit `8b0f35809934c567f1bf7b94b8ce8845151fefb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
REOPENS 20261023T045849Z-rds-data-result-fidelity-postgresql-statements-rejected-column.md: its fix left the call refused. That issue's "PostgreSQL statements rejected" causes were answered by making the SQL layer behind ExecuteStatement accept the statement forms the API advertises; two statement forms exercised here (a MySQL AUTO_INCREMENT column definition and CREATE DATABASE) are still answered 400 DatabaseErrorException by the backing parser, and the duplicate-key diagnostic that 20261106T233903Z-rds-data-8-net-new-root-causes-executestatement.md (cause 6, fixed) said would be emitted "in the engine's wording" is emitted in PostgreSQL's wording on an aurora-mysql cluster.

Five net-new causes, all in rds-data's statement execution, result shaping and request validation. A fixer must close all five before submitting. The open issue 20261106T233903Z-rds-data-9-net-new-root-causes-mysql.md describes the same two dialect symptoms (its causes 1-2) against the earlier sqlparser-rs wording; the replay no longer produces that wording and does not treat these checks as covered by it, so causes 1-2 below record the current refusals.

Cause 1 of 5: a MySQL AUTO_INCREMENT column definition is a parse error, so identity-column DDL and the whole generatedFields contract are unreachable.
AWS: Smithy: ExecuteStatement$sql targets SqlStatement (@length max 65536) and the operation models no error for valid engine SQL; ExecuteStatementResponse$generatedFields is a FieldList and BatchExecuteStatementResponse$updateResults is a list of UpdateResult{generatedFields}. Recorded behavior: on an aurora-mysql Data API cluster `CREATE TABLE <t> (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32))` returns 200, an INSERT then reports the server-assigned key in generatedFields (the first row of a fresh table is 1), and BatchExecuteStatement returns one distinct, increasing generatedFields value per parameterSet.
Emulator: 400 DatabaseErrorException "ERROR: syntax error at or near \"PRIMARY\"; Position: <n>; SQLState: 42601" — a PostgreSQL diagnostic (SQLState 42601, "Position:") for MySQL DDL on a MySQL-engine cluster. The table is never created, so neither generated-fields behaviour can be observed.
Why: the canonical MySQL identity-column DDL is unusable, so no client that creates its own schema through the Data API can run on a MySQL cluster, and the message advertises a PostgreSQL backend where the cluster's Engine is aurora-mysql.
Fix: the SQL dialect selection in the execution layer behind the ExecuteStatement/BatchExecuteStatement handlers — pick the dialect from the resolved cluster's Engine — plus propagation of the generated key into generatedFields / updateResults[].generatedFields.
Traces: rds-data.execute-statement-generated-fields-autoincrement, rds-data.batch-execute-statement-generated-fields-per-row.

Cause 2 of 5: CREATE DATABASE is a syntax error, so the `database` request member cannot select a schema.
AWS: Smithy: ExecuteStatement$database targets DbName (@length max 64, "the name of the database"). Recorded behavior: AWS executes `CREATE DATABASE <name>` through the Data API (200), and a later request naming that database in `database` runs against it, so two same-named tables in two databases stay isolated.
Emulator: 400 DatabaseErrorException 'near "DATABASE": syntax error in CREATE DATABASE <name> at offset 7' — a third tokenizer's wording (neither MySQL's nor PostgreSQL's), for a statement AWS executes.
Why: database provisioning through the Data API is impossible and the `database` routing member cannot be exercised at all; the message also leaks a backing engine that is not Aurora.
Fix: the same dialect/execution layer, plus the database namespace that the `database` member selects.
Traces: rds-data.execute-statement-database-parameter-switches-schema.

Cause 3 of 5: a primary-key violation on an aurora-mysql cluster is refused with PostgreSQL's duplicate-key diagnostic instead of MySQL's.
AWS: Smithy: BatchExecuteStatement models DatabaseErrorException (client, @httpError 400, doc "There was an error in processing the SQL statement") — the code is right today. Recorded behavior: the message is the engine's own duplicate-key diagnostic, which on MySQL/Aurora MySQL is "Duplicate entry '<the colliding key value>' for key '<table>.PRIMARY'; Error code: 1062; SQLState: 23000". Fixed text: "Duplicate entry ", " for key ", "; Error code: 1062; SQLState: 23000"; variable: the colliding value from the request's parameterSet and the table/index name from the statement and state.
Emulator: 400 DatabaseErrorException 'ERROR: duplicate key value violates unique constraint "<table>_pkey"; SQLState: 23505' — PostgreSQL's text, PostgreSQL's SQLState, PostgreSQL's implicit index name, and no MySQL error code 1062.
Why: clients branch and log on the vendor error code and SQLState (1062/23000 for MySQL); tooling that parses the duplicate-key message to recover the colliding key gets a Postgres sentence that names an index the MySQL schema does not have, so retry/upsert logic written against Aurora MySQL misfires.
Fix: the engine-error-to-message mapping shared by the ExecuteStatement/BatchExecuteStatement handlers, driven by the resolved cluster's Engine (same dialect selection as causes 1-2).
Traces: rds-data.batch-execute-statement-atomic-rollback-on-constraint-violation.

Cause 4 of 5: a NaN double in the result is neither refused nor returned — it is silently replaced by 0.0.
AWS: Smithy: ExecuteStatement models UnsupportedResultException as errorKind client @httpError(400) (its doc lists an unsupported data type / a multidimensional array / an oversized result). Recorded behavior: `INSERT ... RETURNING <a float8 holding NaN>` runs and COMMITS the row, then the RESULT is refused with 400 UnsupportedResultException and the fixed message "NaN and Infinity values are not supported"; asking for formatRecordsAs=JSON rescues exactly that case, answering {"numberOfRecordsUpdated": 0, "formattedRecords": the JSON rendering with the value as the string "NaN"}.
Emulator: the call is ACCEPTED with HTTP 200 and returns {'records': [[{'doubleValue': 0.0}]], 'numberOfRecordsUpdated': 0} — no refusal, and the value is not the one stored.
Why: a caller is handed 0.0 where the database holds NaN, with no error raised anywhere — arithmetic, comparisons and totals computed from that cell are wrong, and the UnsupportedResultException class the client is written to handle never appears.
Fix: the Field/result-shaping path of the ExecuteStatement result serializer — detect non-finite doubles after the statement has been applied and refuse with UnsupportedResultException(400, "NaN and Infinity values are not supported"), keeping the committed row, and render them as the JSON string form under formatRecordsAs=JSON.
Traces: rds-data.rds-data-unsupported-result-refusal-arrives-after-the-row-is-committed.

Cause 5 of 5: the modelled `schema` member is honoured instead of refused, so a request AWS rejects outright succeeds.
AWS: Smithy: ExecuteStatement$schema targets DbName (@length max 64) and carries no deprecation, yet the service refuses every request that carries it — recorded behavior: 400 ValidationException with the fixed message "The schema parameter isn't supported." (ValidationException is a framework/common error, not in the operation's own error list), and the statement does not run.
Emulator: the call is ACCEPTED and returns {'records': [[{'longValue': 1}]], 'numberOfRecordsUpdated': 0}, i.e. the statement executed.
Why: a model-driven client that populates `schema` (it is modelled and undeprecated) is told by the emulator that its call is fine, and the same code path fails against AWS; the refusal taxonomy this check grades (distinct codes per single fault) loses one of its members.
Fix: the request-validation leg of the ExecuteStatement/BatchExecuteStatement handlers — refuse any request carrying `schema` with ValidationException before resource resolution and execution. (The fixed issue 20261106T233903Z-rds-data-8-net-new-root-causes-executestatement.md covers other faults of this same check's taxonomy; the `schema` leg is net-new — it is reached only after those earlier legs stopped being ACCEPTED.)
Traces: rds-data.rejects-bad-resource-secret-and-database-with-distinct-codes.

## Evidence
Cause 1 — rds-data.execute-statement-generated-fields-autoincrement and rds-data.batch-execute-statement-generated-fields-per-row:
botocore.errorfactory.DatabaseErrorException: An error occurred (DatabaseErrorException) when calling the ExecuteStatement operation: ERROR: syntax error at or near "PRIMARY"; Position: 56 (resp. Position: 61); SQLState: 42601
for `CREATE TABLE <t> (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32))` on an aurora-mysql Data API cluster. Expected 200, then generatedFields[0].longValue == 1 on the first INSERT, and n distinct increasing generatedFields across n parameterSets.

Cause 2 — rds-data.execute-statement-database-parameter-switches-schema:
botocore.errorfactory.DatabaseErrorException: ... near "DATABASE": syntax error in CREATE DATABASE <name> at offset 7
Expected 200 for `CREATE DATABASE <name>`, and the new database then usable through the `database` member with two same-named tables staying isolated.

Cause 3 — rds-data.batch-execute-statement-atomic-rollback-on-constraint-violation:
AssertionError: unexpected refusal message: 'ERROR: duplicate key value violates unique constraint "<table>_pkey"; SQLState: 23505'
Expected Code DatabaseErrorException (matches) with Message "Duplicate entry '<colliding id>' for key '<table>.PRIMARY'; Error code: 1062; SQLState: 23000" — the id and table name come from the request, the rest is fixed text.

Cause 4 — rds-data.rds-data-unsupported-result-refusal-arrives-after-the-row-is-committed:
AssertionError: the call was ACCEPTED and returned {'records': [[{'doubleValue': 0.0}]], 'numberOfRecordsUpdated': 0}
for `INSERT INTO <t> (id, dbl) VALUES (<n>, 'NaN'::float8) RETURNING dbl`. Expected Code UnsupportedResultException, HTTP 400, Message "NaN and Infinity values are not supported", with the inserted row still present afterwards; the same statement with formatRecordsAs=JSON must instead answer numberOfRecordsUpdated 0 plus formattedRecords rendering the column as the JSON string "NaN".

Cause 5 — rds-data.rejects-bad-resource-secret-and-database-with-distinct-codes:
AssertionError: the call was ACCEPTED and returned {'records': [[{'longValue': 1}]], 'numberOfRecordsUpdated': 0}
for an otherwise-valid ExecuteStatement carrying schema="public". Expected Code ValidationException, HTTP 400, Message "The schema parameter isn't supported." and no execution.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; replay the behavioral suite group rds-data (or exactly the checks in trace_ids).
2. Provision an Aurora Serverless v2 cluster with the Data API enabled (rds CreateDBCluster Engine=aurora-mysql for causes 1-3, EnableHttpEndpoint=true, plus a db.serverless CreateDBInstance; wait for 'available').
3. Cause 1: POST /Execute `CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(32))` -> expect 200, then INSERT and read generatedFields (expect longValue 1); observe 400 DatabaseErrorException 'ERROR: syntax error at or near "PRIMARY" ... SQLState: 42601'. Repeat through POST /BatchExecute with three parameterSets and expect three distinct increasing generatedFields.
4. Cause 2: POST /Execute `CREATE DATABASE <name>` -> expect 200 and the name usable as `database`; observe 400 'near "DATABASE": syntax error ... at offset 7'.
5. Cause 3: create `t (id INT PRIMARY KEY, name VARCHAR(32))`, then POST /BatchExecute an INSERT whose second parameterSet reuses the first id -> expect 400 DatabaseErrorException "Duplicate entry '<id>' for key 't.PRIMARY'; Error code: 1062; SQLState: 23000" and 0 rows persisted; observe the PostgreSQL 23505 'duplicate key value violates unique constraint "t_pkey"' wording.
6. Cause 4: on an aurora-postgresql cluster create `t (id int PRIMARY KEY, dbl float8)` and POST /Execute `INSERT INTO t (id, dbl) VALUES (4, 'NaN'::float8) RETURNING dbl` -> expect 400 UnsupportedResultException "NaN and Infinity values are not supported" with row 4 present afterwards; observe 200 and doubleValue 0.0. Re-send with formatRecordsAs=JSON and expect the NaN rendered as a JSON string.
7. Cause 5: POST /Execute a valid `SELECT 1` with schema="public" -> expect 400 ValidationException "The schema parameter isn't supported."; observe 200 with records.

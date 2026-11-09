Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data.date-type-hint-rejects-unparseable-date-format rds__rds-data.ddl-inside-a-data-api-transaction-implicitly-commits-past-a-rollback rds__rds-data.self-join-duplicate-column-names-refused-under-json-but-aliasing-one-side-succeeds
Issue-Key: rds__rds-data.date-type-hint-rejects-unparseable-date-format|_refused#4|AssertionError|assert _message(exc) == message, (
Issue-Key: rds__rds-data.ddl-inside-a-data-api-transaction-implicitly-commits-past-a-rollback|_grade#4|AssertionError|assert names == ["sim2real"], (
Issue-Key: rds__rds-data.self-join-duplicate-column-names-refused-under-json-but-aliasing-one-side-succeeds|expect_error#4|AssertionError|raise AssertionError(
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# rds-data: 3 net-new causes — DATE type-hint parse message, DDL inside a transaction does not implicitly commit, and duplicate column names are not refused under JSON

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
3 net-new root causes in the Data API's statement surface.

Cause 1 of 3: the DATE type-hint parse failure carries an emulator-authored message.
AWS: ExecuteStatement with a parameter carrying typeHint DATE whose stringValue is not in the accepted date form is refused with the fixed message 'Parse Error for Date: input contains invalid characters' - the text is fixed for this class and does not quote the offending value.
Emulator: the refusal message is 'Cannot parse DATE parameter: "<value>"' - a different sentence that additionally echoes the request value.
Why: a client matching on the message to classify a bad date parameter (the message is the only discriminator inside BadRequestException) matches nothing, and a message that interpolates user input is not what AWS returns.
Fix: the DATE type-hint parsing/refusal message in the rds-data ExecuteStatement parameter coercion.
Traces: rds__rds-data.date-type-hint-rejects-unparseable-date-format.

Cause 2 of 3: DDL inside a Data API transaction does not implicitly commit the work before it.
AWS: on the MySQL-family engines a DDL statement (CREATE TABLE) issued inside a transaction implicitly commits the transaction, so an INSERT that preceded it in the same transaction survives a later RollbackTransaction.
Emulator: after the rollback the preceding INSERT is gone - the row set is empty where the committed row must remain.
Why: a client that mixes DDL and DML in one transaction gets the opposite durability outcome from AWS: rows it believes were committed by the DDL disappear, silently, with no error anywhere.
Fix: the transaction/DDL interaction in the rds-data execution path (implicit commit on DDL for the MySQL-family engines).
Traces: rds__rds-data.ddl-inside-a-data-api-transaction-implicitly-commits-past-a-rollback.

Cause 3 of 3: a result set with duplicate column names is served instead of refused under formatRecordsAs=JSON.
AWS: when formatRecordsAs is JSON and the result set has two columns with the same name (a self-join selecting both sides' id), the call is refused with UnsupportedResultException, because the JSON object cannot represent the duplicate key; aliasing one side makes the same query succeed.
Emulator: the call answers 200 with formattedRecords '[{"id":1,"id":1},{"id":2,"id":2}]' - a JSON document with duplicate object keys.
Why: a client gets a document whose parse result depends on its JSON library (last key wins, or an error), silently losing a column, instead of the modeled refusal that tells it to alias.
Fix: the JSON result formatter in the rds-data ExecuteStatement response path (detect duplicate column labels and refuse with UnsupportedResultException).
Traces: rds__rds-data.self-join-duplicate-column-names-refused-under-json-but-aliasing-one-side-succeeds.

## Evidence
Cause 1: expected message 'Parse Error for Date: input contains invalid characters'; got 'Cannot parse DATE parameter: "22/10/2026"'.
Cause 2: the INSERT that preceded the CREATE TABLE in the same transaction must survive the later RollbackTransaction; expected ['sim2real'], got [].
Cause 3: expected the call to be refused with 'UnsupportedResultException', but it succeeded: {'numberOfRecordsUpdated': 0, 'formattedRecords': '[{"id":1,"id":1},{"id":2,"id":2}]'} (HTTP 200).

## Steps to Reproduce
Cause 1: ExecuteStatement with parameters=[{name:'d', value:{stringValue:'22/10/2026'}, typeHint:'DATE'}] and read the error message.
Cause 2: BeginTransaction, INSERT a row, CREATE TABLE in the same transaction, RollbackTransaction, then read the first table.
Cause 3: ExecuteStatement a self-join selecting both sides' id with formatRecordsAs='JSON'.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__firehose__glue__s3.firehose-raw-json-delivery-queryable-via-athena-recursive-prefix athena__firehose__glue__s3.non-numeric-scalar-coercion-rejected-while-numeric-string-sibling-converts
Issue-Key: athena__firehose__glue__s3.firehose-raw-json-delivery-queryable-via-athena-recursive-prefix|run#19|AssertionError|assert state == "SUCCEEDED", f"Athena query {qid} ended {state!r}: {reason!r}"
Issue-Key: athena__firehose__glue__s3.non-numeric-scalar-coercion-rejected-while-numeric-string-sibling-converts|_query_rows#15|AssertionError|assert state == "SUCCEEDED", f"Athena query {qid} ended {state!r}: {reason!r} for sql {sql!r}"
Issue-Commit: ed67363a200a02a71118cd5e7b7585c85db4efeb

# athena__firehose__glue__s3: Athena cannot read the Glue table Firehose delivered to — the SELECT fails with a SQLite 'no such table'

> _Note: this issue was identified against the code base at commit `ed67363a200a02a71118cd5e7b7585c85db4efeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
1 cause.

Cause 1 of 1: a SELECT over a Glue Data Catalog table fed by a Firehose delivery stream fails with 'no such table'.
AWS: Athena resolves <catalog>.<database>.<table> through the data catalog (awsdatacatalog -> Glue) and scans every object under the table's StorageDescriptor.Location, including the recursive date-partitioned prefixes Firehose writes, so the query reaches QueryExecutionStatus.State SUCCEEDED (Smithy: QueryExecutionState enum) and returns the delivered records as rows (a record whose scalar cannot be coerced to the column type is rejected to the error prefix while a numeric-string sibling converts). A genuinely absent table fails with AthenaError.ErrorCategory 2 / ErrorType 1301 and a message of the fixed form "TABLE_NOT_FOUND: line <l>:<c>: Table 'awsdatacatalog.<db>.<table>' does not exist"; only the identifiers and line/column are request-derived.
Emulator: the query terminates State FAILED with StateChangeReason == AthenaError.ErrorMessage == 'no such table: <db>.<table>' (ErrorCategory 2, no ErrorType) for a table the Glue API created in the same check.
Why: the read-back leg of a Firehose -> S3 -> Glue -> Athena pipeline reports a fabricated 'no such table', so delivery and type-coercion outcomes can never be confirmed through Athena.
Fix: the athena crate's query-execution engine — catalog/table resolution for DML and the AthenaError classification of not-found failures.
Traces: athena__firehose__glue__s3.firehose-raw-json-delivery-queryable-via-athena-recursive-prefix, athena__firehose__glue__s3.non-numeric-scalar-coercion-rejected-while-numeric-string-sibling-converts.

Related open issue that does NOT cover this cause: 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md is a wire-corpus issue filed under the athena service token.

## Evidence
Replay of the simparity-behavioral-aws-v1 behavioral suite against the emulator built from this checkout (service heading athena__firehose__glue__s3, 0/2 passed).
firehose-raw-json-delivery-queryable-via-athena-recursive-prefix: "Athena query <id> ended 'FAILED': 'no such table: <db>.<tbl>'" — expected SUCCEEDED with the delivered rows.
non-numeric-scalar-coercion-rejected-while-numeric-string-sibling-converts: same failure for sql 'SELECT ticker, price FROM "<db>"."<tbl>"'.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the simparity-behavioral-aws-v1 suite and inspect the two checks under the athena__firehose__glue__s3 heading.
3. Minimal manual repro: create a Glue database/table whose location is the Firehose S3 destination prefix, PutRecord through the delivery stream, then athena.start_query_execution('SELECT ticker, price FROM "<db>"."<table>"') and poll GetQueryExecution — expect SUCCEEDED, observe FAILED with 'no such table: <db>.<table>'.

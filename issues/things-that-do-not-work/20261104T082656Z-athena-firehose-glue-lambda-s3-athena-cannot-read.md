Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__firehose__glue__lambda__s3.fhtransform-dropped-records-never-become-athena-rows-until-the-verdict-is-repaired athena__firehose__glue__lambda__s3.fhtransform-lambda-rewritten-records-are-read-back-as-parquet-rows-by-athena athena__firehose__glue__lambda__s3.fhtransform-processing-failed-is-attributed-to-the-transform-not-the-parquet-conversion
Issue-Key: athena__firehose__glue__lambda__s3.fhtransform-dropped-records-never-become-athena-rows-until-the-verdict-is-repaired|_query_rows#15|AssertionError|assert state == "SUCCEEDED", f"Athena query {qid} ended {state!r}: {reason!r} for sql {sql!r}"
Issue-Key: athena__firehose__glue__lambda__s3.fhtransform-lambda-rewritten-records-are-read-back-as-parquet-rows-by-athena|_query_rows#14|AssertionError|assert state == "SUCCEEDED", f"Athena query {qid} ended {state!r}: {reason!r} for sql {sql!r}"
Issue-Key: athena__firehose__glue__lambda__s3.fhtransform-processing-failed-is-attributed-to-the-transform-not-the-parquet-conversion|_query_rows#17|AssertionError|assert state == "SUCCEEDED", f"Athena query {qid} ended {state!r}: {reason!r} for sql {sql!r}"
Issue-Commit: ed67363a200a02a71118cd5e7b7585c85db4efeb

# athena__firehose__glue__lambda__s3: Athena cannot read the Glue table a Firehose+Lambda transform delivered to — the SELECT fails with a SQLite 'no such table'

> _Note: this issue was identified against the code base at commit `ed67363a200a02a71118cd5e7b7585c85db4efeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
1 cause.

Cause 1 of 1: a SELECT over a Glue Data Catalog table that Firehose delivered into fails with 'no such table'.
AWS: Athena resolves <catalog>.<database>.<table> through the data catalog (awsdatacatalog -> Glue) and scans the objects under the table's StorageDescriptor.Location (here the Parquet/JSON objects Firehose wrote after the Lambda transform), so the query reaches QueryExecutionStatus.State SUCCEEDED (Smithy: QueryExecutionState enum) and GetQueryResults returns one row per delivered record — Ok records become rows, Dropped records never do, and a ProcessingFailed verdict is attributed to the transform. When a table genuinely does not exist AWS reports AthenaError.ErrorCategory 2 / ErrorType 1301 with a message of the fixed form "TABLE_NOT_FOUND: line <l>:<c>: Table 'awsdatacatalog.<db>.<table>' does not exist"; only the identifiers and line/column are request-derived.
Emulator: the query terminates State FAILED with StateChangeReason == AthenaError.ErrorMessage == 'no such table: <db>.<table>' (ErrorCategory 2, no ErrorType) for the table created through the Glue API in the same check, so the delivery can never be verified through Athena.
Why: the read-back leg of every Firehose -> Lambda transform -> S3 -> Glue -> Athena pipeline reports a fabricated 'no such table', and a client cannot distinguish 'the table is not registered' from 'the records were not delivered'.
Fix: the athena crate's query-execution engine — catalog/table resolution for DML and the AthenaError classification of not-found failures.
Traces: athena__firehose__glue__lambda__s3.fhtransform-dropped-records-never-become-athena-rows-until-the-verdict-is-repaired, athena__firehose__glue__lambda__s3.fhtransform-lambda-rewritten-records-are-read-back-as-parquet-rows-by-athena, athena__firehose__glue__lambda__s3.fhtransform-processing-failed-is-attributed-to-the-transform-not-the-parquet-conversion.

Related open issue that does NOT cover this cause: 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md is a wire-corpus issue filed under the athena service token.

## Evidence
Replay of the simparity-behavioral-aws-v1 behavioral suite against the emulator built from this checkout (service heading athena__firehose__glue__lambda__s3, 0/3 passed).
All three checks fail in the same probe: "Athena query <id> ended 'FAILED': 'no such table: <db>.<tbl>' for sql 'SELECT marker, n, kind, fn FROM \"<db>\".\"<tbl>\" ORDER BY n'" — expected State SUCCEEDED with the delivered rows.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the simparity-behavioral-aws-v1 suite and inspect the three checks under the athena__firehose__glue__lambda__s3 heading.
3. Minimal manual repro: create a Glue database/table over the Firehose destination prefix, PutRecord through a delivery stream with a Lambda transform, then athena.start_query_execution('SELECT marker, n FROM "<db>"."<table>" ORDER BY n') and poll GetQueryExecution — expect SUCCEEDED, observe FAILED with 'no such table: <db>.<table>'.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3.ctas-partitioned-output-layout-matches-show-partitions athena__s3.data-manifest-lists-exactly-the-objects-unload-wrote athena__s3.unload-into-non-empty-s3-prefix-rejected athena__s3.result-csv-escapes-delimiters-and-distinguishes-null-from-empty-string athena__s3.athena-query-result-object-written-to-s3-output-location
Issue-Key: athena__s3.ctas-partitioned-output-layout-matches-show-partitions|_run_sql#12|AssertionError|raise AssertionError(
Issue-Key: athena__s3.data-manifest-lists-exactly-the-objects-unload-wrote|_run_sql#12|AssertionError|raise AssertionError(
Issue-Key: athena__s3.unload-into-non-empty-s3-prefix-rejected|_run_sql#10|AssertionError|raise AssertionError(
Issue-Key: athena__s3.result-csv-escapes-delimiters-and-distinguishes-null-from-empty-string|run#14|AssertionError|assert state == "SUCCEEDED", (
Issue-Key: athena__s3.athena-query-result-object-written-to-s3-output-location|run#16|AssertionError|assert reported == f"{output_location}{qid}.csv", (
Issue-Commit: 4158638b0e6fd8cb210c88acf50a6ef4ca666704

# athena__s3: Athena refuses CREATE DATABASE and aliased/typed SELECT statements as syntax errors, and reports the result directory instead of the result object (3 causes)

> _Note: this issue was identified against the code base at commit `4158638b0e6fd8cb210c88acf50a6ef4ca666704`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
The Athena statement engine's SQL dialect is not Athena's, and the execution record does not name the object it wrote. 3 net-new root causes. (The open wire issue 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md reports neighbouring DDL symptoms on the wire corpus but carries none of the behavioral check ids below.)

Cause 1 of 3: CREATE DATABASE is refused by the statement parser, so CTAS/UNLOAD workflows cannot start.
AWS: Athena accepts 'CREATE DATABASE `<name>`'; the execution reaches State SUCCEEDED (SubstatementType CREATE_DATABASE) and the database is created in the Glue Data Catalog. Backquoted identifiers are legal.
Emulator: the execution settles FAILED with StateChangeReason/AthenaError.ErrorMessage of the form 'near "DATABASE": syntax error in <the submitted statement> at offset 7' — the fixed text 'near "DATABASE": syntax error in ' plus the statement echoed from the request plus ' at offset 7'.
Why: every workflow that provisions its own database before CTAS or UNLOAD (partitioned CTAS layout, UNLOAD manifests, UNLOAD into a non-empty prefix) stops at the first statement, so none of the CTAS/UNLOAD surface is reachable.
Fix: the athena crate's query-execution engine — DDL statement parsing/execution for CREATE DATABASE and the Glue catalog write it performs.
Traces: athena__s3.ctas-partitioned-output-layout-matches-show-partitions, athena__s3.data-manifest-lists-exactly-the-objects-unload-wrote, athena__s3.unload-into-non-empty-s3-prefix-rejected.

Cause 2 of 3: a SELECT of literals with AS aliases, CAST and typed literals is refused by the statement parser.
AWS: Athena (Trino) accepts a literal SELECT with column aliases and typed literals, e.g. SELECT 'x' AS plain, CAST(NULL AS varchar) AS null_str, '' AS empty_str, TIMESTAMP '2024-03-05 06:07:08.123' AS ts, DATE '2024-03-05' AS d, CAST(1.05 AS DECIMAL(10,4)) AS dec_val; the execution reaches SUCCEEDED and the CSV result quotes embedded delimiters/quotes and distinguishes SQL NULL (empty, unquoted) from the empty string (quoted).
Emulator: FAILED with StateChangeReason 'near "AS": syntax error in <the submitted statement> at offset <n>' — the AS alias is not understood, so neither the statement nor the CSV rendering it would produce can be exercised.
Why: aliases, CAST and typed literals are everyday Athena SQL; a client issuing ordinary analytic SQL gets a syntax error for a statement AWS runs, and the result-CSV quoting/NULL contract is unobservable.
Fix: the athena crate's query-execution engine — the statement parser/SQL dialect (column aliases, CAST, DATE/TIMESTAMP/DECIMAL literals) and the CSV result writer it feeds.
Traces: athena__s3.result-csv-escapes-delimiters-and-distinguishes-null-from-empty-string.

Cause 3 of 3: GetQueryExecution reports the configured output directory as the result location instead of the result object.
AWS: a SUCCEEDED query writes '<ResultConfiguration.OutputLocation><QueryExecutionId>.csv' and QueryExecution.ResultConfiguration.OutputLocation in GetQueryExecution is that full object URI.
Emulator: OutputLocation is echoed as the trailing-slash directory that was submitted.
Why: a client that reads the result object at the URI the service reports addresses a prefix, not an object, so result retrieval fails even when the query reports success.
Fix: the athena crate's GetQueryExecution serializer / the query-execution record's result-location field.
Traces: athena__s3.athena-query-result-object-written-to-s3-output-location.

## Evidence
Replay of the simparity-behavioral-aws-v1 suite against the emulator built from this checkout (service heading '### athena__s3').
Cause 1 — athena__s3.ctas-partitioned-output-layout-matches-show-partitions: AssertionError: statement reached FAILED, expected SUCCEEDED: 'CREATE DATABASE `sim2real_xsvc_db_c2b22457b3ab`' -- near "DATABASE": syntax error in CREATE DATABASE `sim2real_xsvc_db_c2b22457b3ab` at offset 7. Identical first-statement failure in data-manifest-lists-exactly-the-objects-unload-wrote and unload-into-non-empty-s3-prefix-rejected.
Cause 2 — athena__s3.result-csv-escapes-delimiters-and-distinguishes-null-from-empty-string: AssertionError: the literal SELECT must succeed before its rendering is judged; got 'FAILED' / 'near "AS": syntax error in SELECT \'plain-...\' AS plain, \'has,comma\' AS comma_val, \'has"quote\' AS quote_val, CAST(NULL AS varchar) AS null_str, \'\' AS empty_str, TIMESTAMP \'2024-03-05 06:07:08.123\' AS ts, DATE \'2024-03-05\' AS d, CAST(1.05 AS DECIMAL(10,4)) AS dec_val at offset 178'.
Cause 3 — athena__s3.athena-query-result-object-written-to-s3-output-location: AssertionError: expected the result location to be s3://<bucket>/results/<QueryExecutionId>.csv, got 's3://<bucket>/results/'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite and look at the '### athena__s3' section.
3. Re-replay the checks listed in trace_ids.
4. Minimal manual repro: (a) StartQueryExecution 'CREATE DATABASE `d`' and poll — expect SUCCEEDED, observe FAILED with 'near "DATABASE": syntax error ... at offset 7'. (b) StartQueryExecution "SELECT 'a' AS plain, CAST(NULL AS varchar) AS null_str, TIMESTAMP '2024-03-05 06:07:08.123' AS ts" — expect SUCCEEDED, observe 'near "AS": syntax error'. (c) run any succeeding query with ResultConfiguration.OutputLocation='s3://b/results/' and read GetQueryExecution — expect OutputLocation 's3://b/results/<QueryExecutionId>.csv'.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__lakeformation__s3.lf-column-grant-projects-granted-columns-and-excluded-column-denied athena__glue__lakeformation__s3.lf-revoke-select-stops-athena-read-and-rehides-table athena__glue__lakeformation__s3.lf-row-filter-restricts-athena-rows-to-matching-predicate athena__glue__lakeformation__s3.lf-table-select-grant-readable-through-athena-and-ungranted-table-denied
Issue-Key: athena__glue__lakeformation__s3.lf-column-grant-projects-granted-columns-and-excluded-column-denied|run#16|AssertionError|assert state == "SUCCEEDED", f"column-granted table never became readable: {state} / {reason!r}"
Issue-Key: athena__glue__lakeformation__s3.lf-revoke-select-stops-athena-read-and-rehides-table|run#18|AssertionError|assert state == "SUCCEEDED", f"granted table never became readable: {state} / {reason!r}"
Issue-Key: athena__glue__lakeformation__s3.lf-row-filter-restricts-athena-rows-to-matching-predicate|run#20|AssertionError|assert state == "SUCCEEDED", f"filtered table never became readable: {state} / {reason!r}"
Issue-Key: athena__glue__lakeformation__s3.lf-table-select-grant-readable-through-athena-and-ungranted-table-denied|run#22|AssertionError|assert state == "SUCCEEDED", f"granted table never became readable: {state} / {reason!r}"
Issue-Commit: ed67363a200a02a71118cd5e7b7585c85db4efeb

# athena__glue__lakeformation__s3: an Athena SELECT over a Lake Formation-granted Glue table fails with a SQLite 'no such table', so no grant, revoke, column projection or row filter is ever observable

> _Note: this issue was identified against the code base at commit `ed67363a200a02a71118cd5e7b7585c85db4efeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
1 cause.

Cause 1 of 1: Athena cannot resolve a Glue Data Catalog table, so every Lake Formation enforcement path is unreachable.
AWS: Athena resolves <catalog>.<database>.<table> through the data catalog (awsdatacatalog -> Glue) and reads the objects under the table's StorageDescriptor.Location; with a Lake Formation SELECT grant in place the query reaches QueryExecutionStatus.State SUCCEEDED (Smithy: QueryExecutionState enum QUEUED/RUNNING/SUCCEEDED/FAILED/CANCELLED) and returns only the granted columns / the rows matching the row filter, while an ungranted table fails with an authorization failure, not a parse or lookup error. When a table genuinely does not exist AWS reports AthenaError.ErrorCategory 2 / ErrorType 1301 with a message of the fixed form "TABLE_NOT_FOUND: line <l>:<c>: Table 'awsdatacatalog.<db>.<table>' does not exist" (only the identifiers and line/column are request-derived).
Emulator: the query terminates State FAILED with StateChangeReason == AthenaError.ErrorMessage == 'no such table: <db>.<table>' (ErrorCategory 2, no ErrorType) for the table Glue just created and Lake Formation just granted, so the granted table never becomes readable and the checks cannot reach the revoke / column-exclusion / row-filter assertions at all.
Why: a client that governs a Glue table with Lake Formation and reads it through Athena gets a fabricated 'no such table' for a table it was granted, cannot tell that from a real authorization denial, and never sees column or row filtering applied.
Fix: the athena crate's query-execution engine — catalog/table resolution for DML (and the Lake Formation-filtered read path it must consult), plus the AthenaError classification of not-found failures.
Traces: athena__glue__lakeformation__s3.lf-column-grant-projects-granted-columns-and-excluded-column-denied, athena__glue__lakeformation__s3.lf-revoke-select-stops-athena-read-and-rehides-table, athena__glue__lakeformation__s3.lf-row-filter-restricts-athena-rows-to-matching-predicate, athena__glue__lakeformation__s3.lf-table-select-grant-readable-through-athena-and-ungranted-table-denied.

Related open issue that does NOT cover this cause: 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md is a wire-corpus issue filed under the athena service token.

## Evidence
Replay of the simparity-behavioral-aws-v1 behavioral suite against the emulator built from this checkout (service heading athena__glue__lakeformation__s3, 0/4 passed).
lf-table-select-grant-readable-through-athena-and-ungranted-table-denied: expected the granted table to become readable (State SUCCEEDED); actual 'granted table never became readable: FAILED / no such table: <db>.t_granted'.
lf-column-grant-...: 'column-granted table never became readable: FAILED / no such table: <db>.t_cols'.
lf-row-filter-...: 'filtered table never became readable: FAILED / no such table: <db>.t_rows'.
lf-revoke-select-...: 'granted table never became readable: FAILED / no such table: <db>.t_rev'.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the simparity-behavioral-aws-v1 suite and inspect the four checks under the athena__glue__lakeformation__s3 heading.
3. Minimal manual repro: create a Glue database and table over an s3:// prefix with seeded objects, grant SELECT on it through lakeformation.grant_permissions, then athena.start_query_execution('SELECT * FROM "<db>"."<table>"') and poll GetQueryExecution — expect State SUCCEEDED with the granted columns/rows, observe State FAILED with AthenaError.ErrorMessage 'no such table: <db>.<table>'.

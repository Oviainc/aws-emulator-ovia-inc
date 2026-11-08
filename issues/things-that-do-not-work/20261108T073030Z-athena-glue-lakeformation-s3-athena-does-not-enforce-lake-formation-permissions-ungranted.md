Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__lakeformation__s3.lf-table-select-grant-readable-through-athena-and-ungranted-table-denied athena__glue__lakeformation__s3.lf-revoke-select-stops-athena-read-and-rehides-table athena__glue__lakeformation__s3.lf-column-grant-projects-granted-columns-and-excluded-column-denied athena__glue__lakeformation__s3.lf-row-filter-restricts-athena-rows-to-matching-predicate
Issue-Key: athena__glue__lakeformation__s3.lf-table-select-grant-readable-through-athena-and-ungranted-table-denied|run#25|AssertionError|assert state_no == "FAILED", (
Issue-Key: athena__glue__lakeformation__s3.lf-revoke-select-stops-athena-read-and-rehides-table|run#24|AssertionError|assert state_after == "FAILED", (
Issue-Key: athena__glue__lakeformation__s3.lf-column-grant-projects-granted-columns-and-excluded-column-denied|run#17|AssertionError|assert rows == [["1", "alpha"], ["2", "beta"], ["3", "gamma"], ["4", "delta"]], (
Issue-Key: athena__glue__lakeformation__s3.lf-row-filter-restricts-athena-rows-to-matching-predicate|run#21|AssertionError|assert rows == [["1", "alpha", "west"], ["3", "gamma", "west"]], (
Issue-Commit: 2d22b922898c4268bec96b0923119d5f2b2856eb

# Athena does not enforce Lake Formation permissions: ungranted/revoked tables stay readable and column- and row-level grants are not applied (2 causes)

> _Note: this issue was identified against the code base at commit `2d22b922898c4268bec96b0923119d5f2b2856eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
TWO net-new causes. The Athena query engine now resolves a Lake Formation-governed Glue table and returns its rows (the positive controls in all four checks pass), but it never consults Lake Formation when deciding WHETHER the reader may read the table, or WHAT subset of it the reader may see. Every query is answered with the full, unfiltered table.

Related open issue that no longer covers these: 20261104T082656Z-athena-glue-lakeformation-s3-an-athena-select-over.md was filed when the granted read failed with a SQLite 'no such table' and its replay keys are the "granted table never became readable" assertions. Those assertions now pass; the four checks fail further down, at the enforcement assertions described below, so that issue does not carry this cluster.

Cause 1 of 2: a table the reader holds no Lake Formation SELECT permission on is fully readable through Athena.
AWS: when Lake Formation is the authority on a table (IAM_ALLOWED_PRINCIPALS removed from its permissions), a principal with no SELECT grant does not see the table at all: Athena settles the query State 'FAILED' (Smithy com.amazonaws.athena#QueryExecutionState enum QUEUED/RUNNING/SUCCEEDED/FAILED/CANCELLED) and reports StateChangeReason / AthenaError.ErrorMessage with the fixed form "TABLE_NOT_FOUND: line <line>:<col>: Table 'awsdatacatalog.<database>.<table>' does not exist" — the prefix 'TABLE_NOT_FOUND: ' and "does not exist" are fixed text; the line/column come from the position of the table reference in the submitted SQL and the catalog/database/table names come from the request. A revoked grant converges on exactly the same refusal, i.e. the table is re-hidden, not refused with an authorization error. Correspondingly the reader's glue:GetTable on it is refused AccessDeniedException with the message "Insufficient Lake Formation permission(s): Required Describe on <table>" ('Insufficient Lake Formation permission(s): Required Describe on ' fixed, the table name from the request) while the data-lake admin still sees both tables. The specific wording is recorded behavior; Smithy models the members carrying it (AthenaError.ErrorMessage, QueryExecutionStatus.StateChangeReason).
Emulator: the ungranted table's SELECT reaches State 'SUCCEEDED' and returns its rows ('ungranted table must not be readable, but the query reached SUCCEEDED'), and after lakeformation.revoke_permissions removes the reader's SELECT the identical query keeps reaching 'SUCCEEDED' ('after revoking SELECT the read must stop working, but it reached SUCCEEDED'), so no refusal, no TABLE_NOT_FOUND reason and no AthenaError are ever produced.
Why: this is a confidentiality failure, not a cosmetic one. A client that governs a lake with Lake Formation and verifies the boundary by reading as a non-granted principal is told the data is protected while the emulator hands over every row; a revoke appears to take effect in Lake Formation's own records yet changes nothing about what the reader can read. Code that distinguishes "hidden" (TABLE_NOT_FOUND) from "denied" also has no signal to branch on.
Traces: athena__glue__lakeformation__s3.lf-table-select-grant-readable-through-athena-and-ungranted-table-denied, athena__glue__lakeformation__s3.lf-revoke-select-stops-athena-read-and-rehides-table.

Cause 2 of 2: Lake Formation cell-level grants (column subsets and row filters) are not applied to the result set.
AWS: a SELECT * over a table on which the principal holds SELECT with a column include-list returns all rows PROJECTED to the granted columns only, and the excluded column is not addressable by that principal (naming it fails as an unknown column rather than returning data). A SELECT * over a table whose grant carries a row filter (a PartiationFilter/RowFilter expression such as "zone" = 'west') returns only the rows satisfying that predicate. Lake Formation performs this filtering for the reader's credentials; the unfiltered table is only visible to a principal granted all columns / ALL rows.
Emulator: both checks get the entire table back. lf-column-grant: SELECT * returned [['1','alpha','west'],['2','beta','east'],['3','gamma','west'],['4','delta','east']] — three columns — where only the two granted columns may appear, so the excluded column's values are disclosed. lf-row-filter: SELECT * returned all four rows where only the two rows matching the granted predicate may appear.
Why: column- and row-level grants are the mechanism clients use to expose a table to an audience without exposing the sensitive column or the other tenants' rows. An emulator that returns the full table makes a test of that boundary pass while the real deployment would have redacted it, and the excluded column/rows are leaked verbatim to the reader's query result (and to the result object written under the output location).
Fix for both causes: the Athena query-execution path's authorization and data-filtering step — before planning a DML statement, resolve the caller's Lake Formation permissions for each referenced catalog/database/table (and the column include-list and row filter attached to the grant), settle the execution FAILED with the TABLE_NOT_FOUND reason above when no SELECT permission resolves, and project/filter the result set to the granted columns and rows when it does.
Traces: athena__glue__lakeformation__s3.lf-column-grant-projects-granted-columns-and-excluded-column-denied, athena__glue__lakeformation__s3.lf-row-filter-restricts-athena-rows-to-matching-predicate.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout, service heading 'athena__glue__lakeformation__s3' (0/4 passed). In all four checks the positive control — the granted read reaching SUCCEEDED with the expected rows — PASSED, so the failures below are enforcement, not resolution.

Cause 1:
  lf-table-select-grant-readable-through-athena-and-ungranted-table-denied [mismatch], at 'assert state_no == "FAILED"':
    AssertionError: ungranted table must not be readable, but the query reached 'SUCCEEDED'
    (expected State FAILED with StateChangeReason "TABLE_NOT_FOUND: line 1:28: Table 'awsdatacatalog.<db>.t_ungranted' does not exist")
  lf-revoke-select-stops-athena-read-and-rehides-table [mismatch], at 'assert state_after == "FAILED"':
    AssertionError: after revoking SELECT the read must stop working, but it reached 'SUCCEEDED'
    (expected State FAILED with StateChangeReason "TABLE_NOT_FOUND: line 1:28: Table 'awsdatacatalog.<db>.t_rev' does not exist")

Cause 2:
  lf-column-grant-projects-granted-columns-and-excluded-column-denied [mismatch]:
    AssertionError: SELECT * must return all four rows projected to the two granted columns, got [['1', 'alpha', 'west'], ['2', 'beta', 'east'], ['3', 'gamma', 'west'], ['4', 'delta', 'east']]
  lf-row-filter-restricts-athena-rows-to-matching-predicate [mismatch]:
    AssertionError: the reader must see only the rows matching '"zone" = \'west\'', got [['1', 'alpha', 'west'], ['2', 'beta', 'east'], ['3', 'gamma', 'west'], ['4', 'delta', 'east']]

Smithy cited: com.amazonaws.athena#QueryExecutionState (enum), com.amazonaws.athena#QueryExecutionStatus.StateChangeReason, com.amazonaws.athena#AthenaError (ErrorCategory range 1-3, ErrorType range 0-9999, ErrorMessage). The TABLE_NOT_FOUND and 'Insufficient Lake Formation permission(s)' message texts are recorded behavior.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run simparity-behavioral-aws-v1 and inspect the four checks under the 'athena__glue__lakeformation__s3' heading.
3. Manual repro of cause 1: create a Glue database with two tables over seeded S3 prefixes, remove IAM_ALLOWED_PRINCIPALS from both, lakeformation.grant_permissions(SELECT) to a reader role on ONE of them, then as that role run athena.start_query_execution('SELECT id, name, zone FROM "<db>"."<ungranted>"') and poll GetQueryExecution — expect State FAILED with StateChangeReason "TABLE_NOT_FOUND: line 1:28: Table 'awsdatacatalog.<db>.<ungranted>' does not exist", observe State SUCCEEDED with the rows. Then revoke SELECT on the granted table and re-run the same query: expect it to converge on the same TABLE_NOT_FOUND failure, observe SUCCEEDED.
4. Manual repro of cause 2: grant the reader SELECT with a column include-list of two of three columns and run 'SELECT * FROM "<db>"."<table>"' as the reader — expect rows projected to the two granted columns, observe all three. Grant SELECT with a row filter ("zone" = 'west') on another table and run SELECT * — expect only the matching rows, observe all rows.

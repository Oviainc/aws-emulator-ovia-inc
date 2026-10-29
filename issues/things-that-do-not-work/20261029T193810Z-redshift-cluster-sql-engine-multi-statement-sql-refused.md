Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session redshift.rdapi-failed-statement-reports-error-and-result-read-rejected redshift.get-cluster-credentials-autocreate-creates-db-user redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog redshift.redshift-distribution-and-sort-keys-are-materialised-in-the-cluster-catalog
Issue-Key: redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session|_in_session#19|AssertionError|assert described["Status"] == expect, (
Issue-Key: redshift.rdapi-failed-statement-reports-error-and-result-read-rejected|run#7|AssertionError|assert failed.get("Error") == _EXPECTED_ERROR, (
Issue-Key: redshift.get-cluster-credentials-autocreate-creates-db-user|_select#8|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected|run#7|AssertionError|assert running["Status"] == "STARTED", (
Issue-Key: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count|run#18|AssertionError|assert copied["Status"] == "FINISHED", (
Issue-Key: redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty|run#26|AssertionError|assert denied.get("Error") == (
Issue-Key: redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors|run#25|AssertionError|assert copy_status["Status"] == "FINISHED", (
Issue-Key: redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog|_run_sql#11|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: redshift.redshift-distribution-and-sort-keys-are-materialised-in-the-cluster-catalog|_run_sql#11|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift cluster SQL engine: multi-statement SQL refused, SQLite-shaped error text, no pg_user/stv_blocklist catalog, COPY/ENCODE/INTERVAL unparsed (6 causes)

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Replay of simparity-behavioral-aws-v1 against the emulated Redshift cluster data plane (ExecuteStatement/BatchExecuteStatement reaching a provisioned cluster through redshift-data) produced these failures. This bundle carries SIX causes, all in the cluster SQL engine behind ExecuteStatement; all six must be closed before submitting. Related but NOT covering them: the open issue 20261024T024248Z-redshift-cluster-sql-engine-rejects-core-redshift-statements.md (COPY/UNLOAD/CREATE EXTERNAL SCHEMA/SET/SHOW) and 20261022T180846Z-redshift-data-statement-execution-engine-is-not-redshift.md (CREATE SCHEMA / LOCK TABLE / :: casts). None of the six faces below is named in either issue, and the suite marked none of these checks OWNED.

=== Cause 1 of 6: a semicolon-separated multi-statement Sql is refused with 'Multiple statements provided' instead of being run ===
AWS: Smithy (com.amazonaws.redshiftdata): ExecuteStatementInput.Sql is a @required StatementString and the model constrains nothing about statement count; DescribeStatementResponse.SubStatements exists precisely because 'The SQL statements from a multiple statement run' are reported. Recorded behavior (the check header marks itself BLESSED against real AWS us-west-2): ExecuteStatement with Sql = "BEGIN; INSERT INTO <table> VALUES (1)" submitted on a kept-alive session is accepted and reaches Status FINISHED, leaving the transaction OPEN for the next statement on that SessionId; the same session later runs "COMMIT" and "ROLLBACK" as their own statements.
Emulator: the statement goes terminal Status='FAILED' with Error='Multiple statements provided' — the submit is accepted but the engine refuses anything containing more than one statement, so no transaction is ever opened.
Why: BEGIN/COMMIT/ROLLBACK bracketing is the only way a Data API caller gets a transaction, and migration/bootstrap scripts routinely send several statements in one Sql. Every such client is told its statement failed, and the session-isolation contract (a session's uncommitted write is invisible to a second session until COMMIT) can never be observed.
Fix: the cluster SQL engine's statement splitter/executor behind ExecuteStatement — run each statement of a multi-statement Sql in order on the session's own connection and report the run as one statement record, instead of rejecting the text.
Traces: redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session.

=== Cause 2 of 6: a failed statement's Error is SQLite's text, not Redshift's ===
AWS: Smithy: DescribeStatementResponse.Error is 'The error message from the cluster if the SQL statement encountered an error while running' — the model fixes no text, so this is recorded behavior: a statement naming a table that does not exist lands FAILED with Error exactly 'ERROR: relation "<table>" does not exist' (fixed text 'ERROR: relation "' + the table name from the request + '" does not exist').
Emulator: Error='no such table: <table>' — SQLite's wording, with no 'ERROR:' prefix and no 'relation ... does not exist' phrasing. The same shape appears wherever the embedded engine raises (see Cause 3).
Why: Redshift clients match on the engine's message to classify failures (missing relation vs permission vs load failure); a differently worded message defeats every such matcher and every error-path assertion, while the rest of the check's contract (ResourceNotFoundException with 'Query does not have result. Please check query status with DescribeStatement.' on reading a FAILED statement) is unreachable behind it.
Fix: the cluster SQL engine's error translation layer — map the embedded engine's diagnostics onto Redshift/PostgreSQL message text (variable parts: the relation/column/type names taken from the submitted SQL) before storing them in the statement record's Error.
Traces: redshift.rdapi-failed-statement-reports-error-and-result-read-rejected.

=== Cause 3 of 6: Redshift's system catalog relations are absent, so any statement reading pg_user or stv_blocklist fails ===
AWS: recorded behavior only (the Smithy model describes no SQL surface). A Redshift database exposes the PostgreSQL catalog views and the Redshift system tables: SELECT usename FROM pg_user WHERE usename LIKE '%<user>' returns the autocreated DB user after GetClusterCredentials with AutoCreate, and a self-join over stv_blocklist is a legitimate long-running query that stays in Status STARTED long enough to be cancelled.
Emulator: the pg_user query goes terminal Status='FAILED' with Error='no such table: pg_user'. The stv_blocklist self-join (INSERT ... SELECT a.n FROM (SELECT 1 n FROM stv_blocklist LIMIT 1000) a, ... ) is observed FAILED at the first poll where STARTED was expected. Hypothesis: it fails for the same missing relation, since the statement's own Error was not surfaced by the check; a second contributing factor would be statements executing synchronously at submit so STARTED is never observable (the same face already recorded for redshift-data in 20261022T180846Z-redshift-data-statement-execution-engine-is-not-redshift.md, Cause 6).
Why: the catalog views are how clients discover users, tables, keys and load errors; every such introspection query fails outright. It also makes the whole cancel path untestable, because there is no query that can be observed running.
Fix: the cluster SQL engine's catalog component — back pg_user (and the pg_*/stv_*/stl_* relations the checks read) with the emulator's own cluster/user/table state, and keep statement execution asynchronous so a long query is observable as STARTED and cancellable.
Traces: redshift.get-cluster-credentials-autocreate-creates-db-user, redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected.

=== Cause 4 of 6: COPY ... FROM 's3://...' IAM_ROLE ... CSV [MAXERROR n] is a parser error, on the cluster data plane too ===
AWS: recorded behavior only. A Redshift cluster parses and executes COPY from S3 with IAM_ROLE/CSV/MAXERROR: a COPY of well-formed rows FINISHES and the rows become visible (the session's own record reports the loaded count); a COPY with one bad row under MAXERROR 2 FINISHES and records the rejected row in stl_load_errors; a COPY whose bad rows exceed MAXERROR fails with Redshift's own load error naming the table and stl_load_errors — recorded text: "ERROR: Load into table '<table>' failed.  Check 'stl_load_errors' system table for details." (fixed text with the table name taken from the statement) — and leaves the table empty.
Emulator: every one of these lands FAILED with a parser error of the form 'near "COPY": syntax error in <the whole statement> at offset 0' — the keyword is never recognised, so nothing is loaded, stl_load_errors is never populated and the MAXERROR threshold has no effect.
Why: S3 ingest is the primary way data enters Redshift; every loader fails with a syntax error real Redshift never returns, and a client that implements the documented load-failure path (catch the failure, read stl_load_errors) gets an unrelated parse error.
Fix: the cluster SQL engine's statement parser and its COPY executor plus the state it touches (table rows, stl_load_errors, S3 object reads through the S3 service). The open issue 20261024T024248Z-redshift-cluster-sql-engine-rejects-core-redshift-statements.md names COPY for OTHER checks; these three checks were not marked OWNED by it and must be re-replayed here.
Traces: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count, redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty, redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors.

=== Cause 5 of 6: a column-level ENCODE clause in CREATE TABLE is a parser error ===
AWS: recorded behavior only. Redshift accepts the compression-encoding column option, e.g. CREATE TABLE t (v VARCHAR(64) ENCODE RAW), the statement FINISHES, and the chosen encoding is then visible in the cluster catalog (so ANALYZE COMPRESSION can be shown to pick a non-RAW encoding).
Emulator: Status='FAILED', Error='near "ENCODE": syntax error in CREATE TABLE <table> (v VARCHAR(64) ENCODE RAW) at offset 51' (the offset is the position of the keyword in the submitted text).
Why: ENCODE is pervasive in Redshift DDL — every schema script that pins encodings fails to create its tables, and no compression behaviour (ANALYZE COMPRESSION, catalog-visible encodings) can be exercised.
Fix: the cluster SQL engine's CREATE TABLE parser and its catalog state — accept the ENCODE <encoding> column option (and the table-level encoding forms), record the encoding, and report it through the catalog relations.
Traces: redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog.

=== Cause 6 of 6: TIMESTAMP literal + INTERVAL arithmetic is a parser error ===
AWS: recorded behavior only. Redshift accepts typed literals and interval arithmetic in a SELECT list, e.g. TIMESTAMP '2026-01-01 00:00:00' + (<expr> * INTERVAL '1 second'), and the INSERT ... SELECT that uses it FINISHES, producing the row volume the distribution/sort-key check needs.
Emulator: Status='FAILED', Error='near "+": syntax error in INSERT INTO <table> SELECT ... TIMESTAMP \'2026-01-01 00:00:00\' + ((...) * INTERVAL \'1 second\') ... at offset 121' — the typed literal/INTERVAL operand is not tokenised, so the expression fails at the operator.
Why: date arithmetic with INTERVAL is ordinary Redshift SQL; every ETL expression built on it fails, and downstream contracts (DISTKEY/SORTKEY materialised in the cluster catalog) are unreachable because the source table can never be populated.
Fix: the cluster SQL engine's expression parser/evaluator — support typed literals (TIMESTAMP/DATE '...') and INTERVAL literals with the arithmetic operators defined over them.
Traces: redshift.redshift-distribution-and-sort-keys-are-materialised-in-the-cluster-catalog.

## Evidence
Suite: simparity-behavioral-aws-v1 — redshift 5/28 (18%), 23 failed (6 of them OWNED by existing issues and skipped).

Cause 1 — check_redshift_data_api_session_transaction_hides_uncommitted_rows_from_a_second_session.py:489/_in_session:314
  expected: ExecuteStatement(Sql="BEGIN; INSERT INTO <table> VALUES (1)", SessionId=<session A>) -> Status FINISHED, transaction left open
  actual:   AssertionError: ... must reach 'FINISHED', got status='FAILED' error='Multiple statements provided'

Cause 2 — check_redshift_rdapi_failed_statement_reports_error_and_result_read_rejected.py:51
  expected: Error == 'ERROR: relation "<table>" does not exist'
  actual:   Error == 'no such table: <table>'

Cause 3 — check_redshift_get_cluster_credentials_autocreate_creates_db_user.py:195
  expected: SELECT usename FROM pg_user WHERE usename LIKE '%<user>' -> FINISHED with the autocreated user
  actual:   status='FAILED' error='no such table: pg_user'
  check_redshift_rdapi_cancelled_statement_aborts_and_second_cancel_rejected.py:58
  expected: the stv_blocklist self-join INSERT observed as Status 'STARTED' so the cancel can land
  actual:   AssertionError: the long INSERT must still be running when the cancel lands, got 'FAILED' -- it reached a terminal state first

Cause 4 — rdapi-copy-from-s3-loads-object-rows-and-session-reports-count:
  actual: FAILED, error='near "COPY": syntax error in COPY <table> (id, name, amt) FROM \'s3://<bucket>/<key>.csv\' IAM_ROLE \'arn:aws:iam::<acct>:role/<role>\' CSV at offset 0' (expected FINISHED, three rows loaded)
  redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors:
  actual: FAILED, error='near "COPY": syntax error in COPY <table> (id, name, qty) FROM \'s3://<bucket>/load.csv\' IAM_ROLE \'<arn>\' CSV MAXERROR 2 at offset 0' (expected FINISHED with the bad row in stl_load_errors)
  redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty:
  actual: same parser error for '... CSV MAXERROR 2' (expected the load-failure error naming the table and stl_load_errors)

Cause 5 — check_redshift_redshift_analyze_compression_picks_a_non_raw_encoding_visible_in_the_catalog.py:293
  expected: CREATE TABLE <t> (v VARCHAR(64) ENCODE RAW) -> FINISHED
  actual:   status='FAILED' error='near "ENCODE": syntax error in CREATE TABLE <t> (v VARCHAR(64) ENCODE RAW) at offset 51'

Cause 6 — check_redshift_redshift_distribution_and_sort_keys_are_materialised_in_the_cluster_catalog.py:307
  expected: INSERT INTO <t> SELECT ..., TIMESTAMP '2026-01-01 00:00:00' + ((...) * INTERVAL '1 second'), ... -> FINISHED
  actual:   status='FAILED' error='near "+": syntax error in INSERT INTO <t> SELECT ... at offset 121'

(bucket names, role ARNs, cluster ids, table names and offsets are per-run examples; the requirement is that each statement parses, executes, and reports Redshift's own text when it fails.)

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): ExecuteStatementInput.Sql @required StatementString, no statement-count constraint; DescribeStatementResponse.Error = 'The error message from the cluster...'; DescribeStatementResponse.SubStatements = 'The SQL statements from a multiple statement run'; StatementStatusString = [SUBMITTED, PICKED, STARTED, FINISHED, ABORTED, FAILED].

## Steps to Reproduce
1. Clone the repo, boot the emulator, CreateCluster a single-node Redshift cluster, wait for 'available', and point a botocore redshift-data client at the emulator with ClusterIdentifier + DbUser.
2. Cause 1: execute_statement(Sql="CREATE TABLE t (n INT)") then, with SessionKeepAliveSeconds set, execute_statement(Sql="BEGIN; INSERT INTO t VALUES (1)", SessionId=<session>) -> expect FINISHED; observe FAILED / 'Multiple statements provided'.
3. Cause 2: execute_statement(Sql="SELECT 1 FROM absent_table") -> expect FAILED with Error 'ERROR: relation "absent_table" does not exist'; observe 'no such table: absent_table'.
4. Cause 3: execute_statement(Sql="SELECT usename FROM pg_user") -> expect FINISHED; observe 'no such table: pg_user'. Then submit an INSERT ... SELECT self-join over stv_blocklist and poll describe_statement -> expect STARTED; observe FAILED.
5. Cause 4: put a CSV in S3, create a table, and execute_statement(Sql="COPY t (a,b,c) FROM 's3://<bucket>/<key>.csv' IAM_ROLE '<arn>' CSV MAXERROR 2") -> expect FINISHED (rows loaded, bad rows in stl_load_errors); observe 'near "COPY": syntax error'.
6. Cause 5: execute_statement(Sql="CREATE TABLE enc (v VARCHAR(64) ENCODE RAW)") -> expect FINISHED; observe 'near "ENCODE": syntax error'.
7. Cause 6: execute_statement(Sql="SELECT TIMESTAMP '2026-01-01 00:00:00' + (1 * INTERVAL '1 second')") -> expect FINISHED; observe 'near "+": syntax error'.
8. Re-replay every check id in trace_ids to confirm the fix.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors redshift.rdapi-copy-malformed-row-rejected-and-recorded-in-stl-load-errors redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty redshift.redshift-copy-with-a-role-lacking-s3-read-fails-and-leaves-the-table-empty redshift.redshift-unload-writes-the-table-to-s3-and-the-manifest-parts-reassemble-it redshift.redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog redshift.redshift-distribution-and-sort-keys-are-materialised-in-the-cluster-catalog redshift.get-cluster-credentials-autocreate-creates-db-user redshift.rdapi-failed-statement-reports-error-and-result-read-rejected redshift.data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected
Issue-Key: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count|run#18|AssertionError|assert copied["Status"] == "FINISHED", (
Issue-Key: redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors|run#25|AssertionError|assert copy_status["Status"] == "FINISHED", (
Issue-Key: redshift.rdapi-copy-malformed-row-rejected-and-recorded-in-stl-load-errors|run#17|AssertionError|assert failed.get("Error") == _EXPECTED_ERROR, (
Issue-Key: redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty|run#26|AssertionError|assert denied.get("Error") == (
Issue-Key: redshift.redshift-copy-with-a-role-lacking-s3-read-fails-and-leaves-the-table-empty|run#30|AssertionError|assert "S3ServiceException" in error, (
Issue-Key: redshift.redshift-unload-writes-the-table-to-s3-and-the-manifest-parts-reassemble-it|run#23|AssertionError|assert unload_status["Status"] == "FINISHED", (
Issue-Key: redshift.redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them|_run_sql#26|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog|_run_sql#11|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: redshift.redshift-distribution-and-sort-keys-are-materialised-in-the-cluster-catalog|_run_sql#11|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: redshift.get-cluster-credentials-autocreate-creates-db-user|_select#8|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: redshift.rdapi-failed-statement-reports-error-and-result-read-rejected|run#7|AssertionError|assert failed.get("Error") == _EXPECTED_ERROR, (
Issue-Key: redshift.data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll|_run_sql#23|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected|run#7|AssertionError|assert running["Status"] == "STARTED", (
Issue-Commit: 8087bf5e6100a6c9af7468e05f3ad1b7d1320fd6

# Redshift provisioned-cluster SQL engine is not Redshift-compatible: COPY, UNLOAD, Spectrum, ENCODE, interval arithmetic and the pg_* catalog are syntax errors, and long queries end FAILED 'interrupted' (8 causes)

> _Note: this issue was identified against the code base at commit `8087bf5e6100a6c9af7468e05f3ad1b7d1320fd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Replay of simparity-behavioral-aws-v1 against the redshift emulator scored 2/29 (7%). Thirteen of the surviving failures share one fault area: the SQL engine behind a provisioned Redshift cluster (reached through the Redshift Data API: ExecuteStatement + DescribeStatement + GetStatementResult) is a generic SQLite-style engine, not Redshift. Every Redshift-specific statement form is rejected as a parser syntax error, the Redshift/PostgreSQL catalog is absent, the engine's error text is SQLite's rather than Redshift's, and any statement with real work to do ends FAILED with 'interrupted'. This bundle carries EIGHT causes; all eight must be closed before submitting.

Related open issue: 20261022T180846Z-redshift-data-statement-execution-engine-is-not-redshift.md records the same class of fault for the redshift-data/serverless entry point. It does NOT cover these checks (different service, and the suite did not mark any of them OWNED), but a fixer should expect one shared engine to be the single place both are fixed.

=== Cause 1 of 8: COPY ... FROM 's3://...' IAM_ROLE ... is a parser syntax error ===
AWS: a Redshift cluster's bulk loader is the COPY command: `COPY <table> (<cols>) FROM 's3://<bucket>/<key>' IAM_ROLE '<role-arn>' CSV [MAXERROR n]`. Smithy says nothing about SQL text (ExecuteStatementInput.Sql is a StatementString with no grammar), so this is recorded behavior only: real Redshift parses and executes COPY, the statement reaches DescribeStatement Status=FINISHED, the well-formed rows land in the table, rows rejected for being malformed are counted against MAXERROR, and the loader records each rejected row in the stl_load_errors system table. When the rejected-row count exceeds MAXERROR, or the IAM role cannot read the object, the statement goes to Status=FAILED and the table is left empty.
Emulator: every COPY statement is refused by the parser. DescribeStatement reports Status='FAILED' with Error of the form 'near "COPY": syntax error in COPY <table> (...) FROM '...' IAM_ROLE '...' CSV at offset 0'. No rows are ever loaded, MAXERROR is never evaluated, and no load-error rows exist to read.
Why: a client cannot load data into the emulated cluster at all. The primary ingest path of Redshift is unavailable, the success path and both documented failure paths (too many bad rows, unreadable S3 object) are indistinguishable from each other because all three produce the same parser error, and a caller cannot tell a permission problem from a data problem.
Fix: the cluster data-plane SQL engine's statement parser/executor — add the COPY statement form (column list, FROM s3 URI, IAM_ROLE, CSV and MAXERROR options) and wire it to the emulator's S3 state plus a load-error record, in the engine the Data API handlers execute against.
Traces: redshift.rdapi-copy-from-s3-loads-object-rows-and-session-reports-count, redshift.redshift-copy-from-s3-loads-rows-and-records-the-one-bad-row-in-stl-load-errors, redshift.rdapi-copy-malformed-row-rejected-and-recorded-in-stl-load-errors, redshift.redshift-copy-exceeding-maxerror-fails-and-leaves-the-table-empty, redshift.redshift-copy-with-a-role-lacking-s3-read-fails-and-leaves-the-table-empty.

=== Cause 2 of 8: UNLOAD ... TO 's3://...' is a parser syntax error ===
AWS: recorded behavior only (no model coverage of SQL text): `UNLOAD ('<select>') TO 's3://<bucket>/<prefix>' IAM_ROLE '<arn>' MANIFEST DELIMITER '|' ALLOWOVERWRITE` runs to Status=FINISHED and writes the selected rows to S3 as one or more part objects plus a manifest object listing them, so the parts reassemble into the source table.
Emulator: Status='FAILED', Error 'near "UNLOAD": syntax error in UNLOAD (...) TO '...' IAM_ROLE '...' MANIFEST DELIMITER '|' ALLOWOVERWRITE at offset 0'. Nothing is written to S3.
Why: the export half of the warehouse is unusable; any client or pipeline that unloads a table to S3 (and anything that then reads the manifest) fails with a parser error instead of producing objects.
Fix: the same cluster SQL engine's parser/executor — add UNLOAD (query, TO prefix, IAM_ROLE, MANIFEST, DELIMITER, ALLOWOVERWRITE) and have it write parts plus a manifest into the emulator's S3 state.
Traces: redshift.redshift-unload-writes-the-table-to-s3-and-the-manifest-parts-reassemble-it.

=== Cause 3 of 8: CREATE EXTERNAL SCHEMA ... FROM DATA CATALOG (Spectrum) is a parser syntax error ===
AWS: recorded behavior only: `CREATE EXTERNAL SCHEMA <name> FROM DATA CATALOG DATABASE '<glue-db>' IAM_ROLE '<arn>' CREATE EXTERNAL DATABASE IF NOT EXISTS` succeeds, after which an external table in that schema returns rows read directly from S3 without loading them into the cluster.
Emulator: Status='FAILED', Error 'near "EXTERNAL": syntax error in CREATE EXTERNAL SCHEMA ... at offset 7'.
Why: Spectrum is entirely absent — a client cannot register or query external data, and the whole query-in-place pattern returns a parser error.
Fix: the cluster SQL engine's DDL parser plus an external-schema/external-table binding onto the emulator's Glue catalog and S3 state.
Traces: redshift.redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them.

=== Cause 4 of 8: the ENCODE column attribute in CREATE TABLE is a parser syntax error ===
AWS: recorded behavior only: Redshift accepts a per-column compression encoding, `CREATE TABLE t (v VARCHAR(64) ENCODE RAW)`, and the chosen encoding is afterwards visible in the cluster catalog; ANALYZE COMPRESSION then recommends a non-RAW encoding for such a column.
Emulator: Status='FAILED', Error 'near "ENCODE": syntax error in CREATE TABLE sim2real_behav_enc_raw (v VARCHAR(64) ENCODE RAW) at offset 51'.
Why: ordinary Redshift DDL that any migration or schema tool emits is rejected, so tables cannot even be created with their production definitions, let alone have their encodings inspected.
Fix: the cluster SQL engine's CREATE TABLE parser — accept (and retain, for catalog queries) the ENCODE column attribute. The same parser needs the DISTKEY/SORTKEY attributes exercised by Cause 5's check.
Traces: redshift.redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog.

=== Cause 5 of 8: Redshift expression dialect unsupported — TIMESTAMP literal + INTERVAL arithmetic and :: casts in INSERT ... SELECT ===
AWS: recorded behavior only: Redshift (PostgreSQL-derived) evaluates `TIMESTAMP '2026-01-01 00:00:00' + (<int expr> * INTERVAL '1 second')`, modulo arithmetic, and `<expr>::VARCHAR` casts inside an INSERT ... SELECT over cross-joined seed tables, reaching Status=FINISHED and materialising the generated rows.
Emulator: Status='FAILED', Error 'near "+": syntax error in INSERT INTO ... TIMESTAMP '2026-01-01 00:00:00' + ((...) * INTERVAL '1 second') ... at offset 121'. (The redshift-data suite shows the same engine rejecting `::VARCHAR` with 'unrecognized token: ":"'.)
Why: the generator idiom used to populate a table — and more generally any Redshift date/interval arithmetic or cast — fails at parse time, so a client cannot run everyday analytic SQL and no downstream assertion about the data is reachable.
Fix: the cluster SQL engine's expression parser/evaluator — support TIMESTAMP/DATE literals with INTERVAL arithmetic and the PostgreSQL `::` cast operator.
Traces: redshift.redshift-distribution-and-sort-keys-are-materialised-in-the-cluster-catalog.

=== Cause 6 of 8: the Redshift/PostgreSQL system catalog is absent (pg_user, stl_load_errors) ===
AWS: recorded behavior only: a Redshift cluster exposes PostgreSQL-compatible catalog views and Redshift system tables over the normal query path — `SELECT usename FROM pg_user` lists database users (so GetClusterCredentials with AutoCreate=true is observable as a newly created db user), and `stl_load_errors` holds one row per row a COPY rejected.
Emulator: Status='FAILED', Error 'no such table: pg_user'. There is no catalog at all; the same gap means the load-error table named in Cause 1 can never be read.
Why: anything that introspects the cluster — a credentials/auto-create flow confirming the user exists, a loader checking why rows were rejected, a BI tool enumerating schemas and columns — gets a hard 'no such table' failure instead of catalog rows, and a control-plane side effect (GetClusterCredentials AutoCreate) becomes unverifiable.
Fix: the cluster SQL engine's catalog layer — back pg_user (and the pg_* views / stl_* system tables the data plane needs, including stl_load_errors) with the emulator's own cluster/user/load state so they are queryable through the Data API.
Traces: redshift.get-cluster-credentials-autocreate-creates-db-user.

=== Cause 7 of 8: statement failure text is the embedded engine's, not Redshift's ===
AWS: recorded behavior only, but the rule is known because the text is built from the request: a failed statement's DescribeStatement.Error carries the engine's PostgreSQL-style diagnostic naming the offending object — selecting from a missing table yields `ERROR: relation "<table>" does not exist`, and a COPY that exceeds MAXERROR yields `ERROR: Load into table '<table>' failed.  Check 'stl_load_errors' system table for details.` (Smithy: DescribeStatementResponse.Error is a plain String, so the model fixes nothing here; these are the recorded strings as examples of the two rules.)
Emulator: the raw SQLite diagnostic is passed through verbatim — 'no such table: sim2real_rdapi_absent_table' for the missing relation, and the COPY parser error for the load failure.
Why: Status=FAILED is reported, so the severity is limited to diagnosis — but Error is the only channel a Data API client has for why a statement failed. A client that classifies failures (missing object vs. load failure vs. permission) on this text, and every tool that tells a user where to look (the stl_load_errors pointer), is handed a message from a different database engine.
Fix: the cluster SQL engine's error-reporting layer — map engine errors onto Redshift's PostgreSQL-style diagnostics before they are stored on the statement record and returned by DescribeStatement, rather than passing the backing engine's text through.
Traces: redshift.rdapi-failed-statement-reports-error-and-result-read-rejected.

=== Cause 8 of 8: a statement with real work to do ends FAILED with error 'interrupted', so no statement is ever observably running ===
AWS: recorded behavior only: a deliberately expensive query (a self cross-join over a filler table) runs for seconds-to-minutes on a real cluster. It passes through the non-terminal statuses and then reaches Status=FINISHED with its result; while it is in flight DescribeStatement reports a running status, which is what makes WaitTimeSeconds return a terminal status without a separate poll, and what makes CancelStatement land on an in-flight statement and drive it to ABORTED (a second CancelStatement on the now-terminal statement is then refused with ValidationException — Smithy: CancelStatement models ValidationException @httpError 400).
Emulator: the expensive statement reaches a terminal state almost immediately with Status='FAILED' and Error='interrupted'; in the cancel check the long INSERT is already 'FAILED' before the cancel is issued. No statement is ever observably in flight.
Why: two contracts become unobservable and are silently wrong. A client cannot cancel work (there is never anything running to cancel, and the terminal-cancel rejection is never reached), and a client using WaitTimeSeconds to block for completion is handed a failure for a query AWS completes. Worse, a legitimate query is reported as FAILED, so a caller sees data loss where AWS returns results.
Fix: the cluster data-plane statement executor/scheduler — execute statements asynchronously with real non-terminal statuses instead of interrupting them, and remove whatever cost/time guard aborts a large query with 'interrupted'; CancelStatement must then transition an in-flight statement to ABORTED and refuse a second cancel on a terminal statement with the 400 ValidationException.
Traces: redshift.data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll, redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1. redshift 2/29 (7%), 27 failed (4 of those are already-filed scaffold stubs or OWNED and are excluded from this issue).

Cause 1 — rdapi-copy-from-s3-loads-object-rows-and-session-reports-count
  expected: COPY of three well-formed rows -> DescribeStatement Status FINISHED, rows present in the table
  actual:   Status 'FAILED', Error 'near "COPY": syntax error in COPY sim2real_rdapi_copy (id, name, amt) FROM \'s3://.../good.csv\' IAM_ROLE \'arn:aws:iam::...:role/...\' CSV at offset 0'
  same error shape for redshift-copy-from-s3-... (CSV MAXERROR 2), rdapi-copy-malformed-row-..., redshift-copy-exceeding-maxerror-... and redshift-copy-with-a-role-lacking-s3-read-... (the last expected the failure to be S3's, raised inside the cluster's own request)

Cause 2 — redshift-unload-writes-the-table-to-s3-and-the-manifest-parts-reassemble-it
  expected: UNLOAD must FINISH and write parts + manifest to S3
  actual:   Status 'FAILED', Error 'near "UNLOAD": syntax error in UNLOAD (\'SELECT id, v FROM sim2real_behav_unload\') TO \'s3://.../out/\' IAM_ROLE \'...\' MANIFEST DELIMITER \'|\' ALLOWOVERWRITE at offset 0'

Cause 3 — redshift-spectrum-external-table-returns-rows-from-s3-without-loading-them
  expected: CREATE EXTERNAL SCHEMA ... FROM DATA CATALOG ... -> FINISHED
  actual:   Status 'FAILED', Error 'near "EXTERNAL": syntax error in CREATE EXTERNAL SCHEMA ... at offset 7'

Cause 4 — redshift-analyze-compression-picks-a-non-raw-encoding-visible-in-the-catalog
  expected: CREATE TABLE sim2real_behav_enc_raw (v VARCHAR(64) ENCODE RAW) -> FINISHED
  actual:   Status 'FAILED', Error 'near "ENCODE": syntax error ... at offset 51'

Cause 5 — redshift-distribution-and-sort-keys-are-materialised-in-the-cluster-catalog
  expected: INSERT ... SELECT with TIMESTAMP '...' + (expr * INTERVAL '1 second') -> FINISHED
  actual:   Status 'FAILED', Error 'near "+": syntax error in INSERT INTO sim2real_behav_dist_src SELECT ... at offset 121'

Cause 6 — get-cluster-credentials-autocreate-creates-db-user
  expected: SELECT usename FROM pg_user WHERE usename LIKE '%sim2real_behav_tempuser' -> FINISHED with catalog rows
  actual:   Status 'FAILED', Error 'no such table: pg_user'

Cause 7 — rdapi-failed-statement-reports-error-and-result-read-rejected
  expected: DescribeStatement.Error == 'ERROR: relation "sim2real_rdapi_absent_table" does not exist'
  actual:   'no such table: sim2real_rdapi_absent_table'
  (companion, rdapi-copy-malformed-row-...: expected "ERROR: Load into table 'sim2real_rdapi_copy_bad' failed.  Check 'stl_load_errors' system table for details.")

Cause 8 — data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll
  expected: SELECT count(*) FROM filler a, filler b WHERE a.n + b.n = -1 runs, then FINISHES
  actual:   Status 'FAILED', Error 'interrupted'
  rdapi-cancelled-statement-aborts-and-second-cancel-rejected: expected the long INSERT to still be running when the cancel lands; actual 'FAILED' — it reached a terminal state first, so the cancel and the terminal-cancel ValidationException are never exercised.

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json, awsJson1_1): ExecuteStatementInput.Sql is a StatementString (no grammar modelled, so SQL dialect fidelity is recorded behavior only); DescribeStatementResponse.Error is a plain String; CancelStatement errors include ValidationException (client, httpError 400) and ResourceNotFoundException (404).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. CreateCluster a single-node cluster, wait for ClusterStatus 'available', and point a botocore redshift-data client at the emulator with ClusterIdentifier/Database/DbUser.
3. Cause 1: put a CSV object in an emulator S3 bucket, then execute_statement(Sql="COPY t (id, name, amt) FROM 's3://<bucket>/<key>' IAM_ROLE '<role-arn>' CSV") and poll describe_statement. Expect FINISHED with the rows loaded; observe FAILED with 'near "COPY": syntax error'. Repeat with CSV MAXERROR 2 over a file containing one bad row (expect FINISHED, bad row recorded), with MAXERROR exceeded (expect FAILED, table empty), and with a role lacking s3:GetObject (expect an S3 permission failure).
4. Cause 2: execute_statement(Sql="UNLOAD ('SELECT id, v FROM t') TO 's3://<bucket>/out/' IAM_ROLE '<arn>' MANIFEST DELIMITER '|' ALLOWOVERWRITE"). Expect FINISHED plus part+manifest objects; observe FAILED with 'near "UNLOAD": syntax error'.
5. Cause 3: execute_statement(Sql="CREATE EXTERNAL SCHEMA s FROM DATA CATALOG DATABASE 'db' IAM_ROLE '<arn>' CREATE EXTERNAL DATABASE IF NOT EXISTS"). Expect FINISHED; observe FAILED with 'near "EXTERNAL": syntax error'.
6. Cause 4: execute_statement(Sql='CREATE TABLE enc_raw (v VARCHAR(64) ENCODE RAW)'). Expect FINISHED; observe FAILED with 'near "ENCODE": syntax error'.
7. Cause 5: execute_statement with INSERT ... SELECT using TIMESTAMP '2026-01-01 00:00:00' + (n * INTERVAL '1 second') over cross-joined seed tables. Expect FINISHED; observe FAILED with 'near "+": syntax error'. Also try <expr>::VARCHAR.
8. Cause 6: execute_statement(Sql="SELECT usename FROM pg_user"). Expect FINISHED with rows; observe FAILED with 'no such table: pg_user'.
9. Cause 7: execute_statement(Sql='SELECT 1 FROM no_such_table'), poll to FAILED and read DescribeStatement.Error. Expect 'ERROR: relation "no_such_table" does not exist'; observe 'no such table: no_such_table'.
10. Cause 8: build a ~1000-row filler table, then execute_statement(Sql='SELECT count(*) FROM filler a, filler b WHERE a.n + b.n = -1'). Expect it to be non-terminal for a while and then FINISH; observe an immediate FAILED with Error 'interrupted'. Then submit a long INSERT and cancel_statement(Id) while it should still be running: expect ABORTED and a ValidationException on a second cancel; observe the statement already FAILED.
11. Re-replay every check id in trace_ids to confirm a fix.

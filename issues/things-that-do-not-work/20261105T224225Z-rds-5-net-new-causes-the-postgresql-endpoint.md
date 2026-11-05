Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deferred-class-modification-stays-pending-while-instance-serves-sql rds.failover-force-failover-rejected-on-a-single-az-instance-moves-no-write-role rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed rds.read-replica-replication-query-and-promote rds.serializable-write-skew-rejected-repeatable-read-permits rds.static-parameter-pending-reboot-query-effect rds.iam-db-auth-token-minted-for-another-user-is-refused rds.iam-db-auth-token-connects-and-maps-to-the-granted-role rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte rds.copy-from-stdin-bulk-load-rejects-a-malformed-row rds.repeatable-read-transaction-sees-a-stable-snapshot
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deferred-class-modification-stays-pending-while-instance-serves-sql|_rows#10|SyntaxError|cur = conn.execute(sql, params)
Issue-Key: rds.failover-force-failover-rejected-on-a-single-az-instance-moves-no-write-role|_sql_rows#20|SyntaxError|cur.execute(statement)
Issue-Key: rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed|_rows#8|SyntaxError|cur = conn.execute(sql, params)
Issue-Key: rds.read-replica-replication-query-and-promote|_sql_rows#15|SyntaxError|cur.execute(statement)
Issue-Key: rds.serializable-write-skew-rejected-repeatable-read-permits|_rows#11|SyntaxError|cur.execute(statement)
Issue-Key: rds.static-parameter-pending-reboot-query-effect|_sql_rows#18|SyntaxError|cur.execute(statement)
Issue-Key: rds.iam-db-auth-token-minted-for-another-user-is-refused|_sql_rows#20|SyntaxError|cur.execute(statement, params)
Issue-Key: rds.iam-db-auth-token-connects-and-maps-to-the-granted-role|_sql_rows#16|SyntaxError|cur.execute(statement)
Issue-Key: rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte|_rows#7|SyntaxError|cur = conn.execute(sql, params)
Issue-Key: rds.copy-from-stdin-bulk-load-rejects-a-malformed-row|_copy#12|SyntaxError|with cur.copy(statement) as copy:
Issue-Key: rds.repeatable-read-transaction-sees-a-stable-snapshot|run#18|AssertionError|assert level == [("repeatable read",)], (
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# rds: 5 net-new causes — the PostgreSQL endpoint's SQL dialect rejects server functions, role DDL, :: casts and COPY, and ignores the session's isolation level

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
This issue bundles 5 net-new root causes in the rds service, all grounded in failing behavioural checks that the suite marked neither ALREADY FILED nor OWNED. A psycopg session now DOES open on the published postgres endpoint and the engine now answers with PostgreSQL-shaped errors (psycopg.errors.SyntaxError, 'syntax error at or near "X"'), so these are NOT the replay keys carried by the open issue 20261105T131136Z-rds-10-net-new-causes-the-engine-data.md (which recorded the previous SQLite-shaped 'no such function/no such table' signatures) nor by 20261104T053605Z-rds-the-published-db-instance-cluster-endpoint-serves.md (no wire protocol at all) — the session opens, and the statement parser is what diverges. A fixer must close all FIVE causes below before submitting.

=== Cause 1 of 5: server-function calls in a SELECT are refused as a syntax error ===
AWS: recorded behavior only (the engine surface is a PostgreSQL contract, not a Smithy one). A PostgreSQL DB instance evaluates pg_postmaster_start_time() (how a reboot/restart is observed), pg_is_in_recovery() (how a read replica is told from a writer) and current_setting('<parameter>') (how an applied DB-parameter-group value is read back), returning one row with the value.
Emulator: each of those statements is refused at parse time. The server sends a syntax error whose position is the function name itself, surfaced to the client as psycopg.errors.SyntaxError: 'syntax error at or near "pg_postmaster_start_time"', 'syntax error at or near "pg_is_in_recovery"', 'syntax error at or near "current_setting"'. The fixed part of such a message is PostgreSQL's 'syntax error at or near "…"'; the quoted token is the offending token from the submitted statement — but AWS emits no error here at all, it returns a result row.
Why: reboot, failover, replica-role and parameter-group effects are unobservable from the engine, and any client or monitoring tool that calls a PostgreSQL system function gets a syntax error from an endpoint advertised as PostgreSQL.
Fix: the engine data plane's PostgreSQL function surface and its statement parser (function-call expressions in a select list), backed by the instance lifecycle/parameter state the control plane already keeps.
Traces: rds.deferred-class-modification-stays-pending-while-instance-serves-sql, rds.failover-force-failover-rejected-on-a-single-az-instance-moves-no-write-role, rds.reboot-severs-open-session-rolls-back-uncommitted-keeps-committed, rds.read-replica-replication-query-and-promote, rds.serializable-write-skew-rejected-repeatable-read-permits, rds.static-parameter-pending-reboot-query-effect.

=== Cause 2 of 5: role DDL (CREATE USER/CREATE ROLE … WITH LOGIN, GRANT … TO <role>) is refused as a syntax error ===
AWS: recorded behavior only. A PostgreSQL DB instance accepts CREATE USER <name> WITH LOGIN / CREATE ROLE "<name>" WITH LOGIN, GRANT rds_iam TO "<name>" and GRANT CONNECT ON DATABASE "<db>" TO "<name>" from the master user, and the new role is then visible in pg_roles/pg_auth_members. Granting rds_iam is the engine-side half of IAM database authentication, so without it no GenerateDBAuthToken login can be exercised.
Emulator: the parser refuses the role DDL — psycopg.errors.SyntaxError: 'syntax error at or near "ROLE"' for CREATE ROLE "<name>" WITH LOGIN, and 'syntax error at or near "'<quoted-literal>'"' for the companion statement that quotes a role/login name as a string literal. Nothing is created, so the whole IAM-auth leg is unreachable.
Why: a client cannot create any database role, cannot grant rds_iam, and therefore cannot use IAM database authentication at all; every privilege-scoped behaviour keyed on a non-master role is unreachable.
Fix: the engine data plane's PostgreSQL statement parser/executor for role DDL (CREATE ROLE/USER with options, GRANT role TO role, GRANT CONNECT ON DATABASE) plus the pg_roles/pg_auth_members catalogs it must update.
Traces: rds.iam-db-auth-token-minted-for-another-user-is-refused, rds.iam-db-auth-token-connects-and-maps-to-the-granted-role.

=== Cause 3 of 5: the PostgreSQL type/expression dialect is not parsed — the :: cast operator is a syntax error ===
AWS: recorded behavior only. PostgreSQL accepts the `::` cast operator and the types the dialect is built on — `SELECT 0.1::numeric + 0.2::numeric`, `(1::numeric / 3)::numeric(10,4)`, `'2024-01-31 00:00:00+00'::timestamptz + interval '1 month'`, `'{"a":1}'::jsonb @> …`, array constructors, INSERT … RETURNING / ON CONFLICT and WITH RECURSIVE — and refuses only the DATA errors (numeric field overflow with SQLSTATE 22003, invalid input syntax for type json with SQLSTATE 22P02).
Emulator: the statement is refused at the cast operator: psycopg.errors.SyntaxError: 'syntax error at or near ":"'. Not one of the dialect's value-level behaviours (nor its documented data-error SQLSTATEs) can be reached.
Why: ordinary PostgreSQL application SQL fails with a syntax error, and a client cannot distinguish 'the endpoint does not speak this dialect' from 'my data was rejected' because the error class is wrong (42601 syntax error instead of 22003/22P02).
Fix: the engine data plane's PostgreSQL expression parser and type system (cast operator, numeric/timestamptz/interval/jsonb/array types, RETURNING / ON CONFLICT, WITH RECURSIVE).
Traces: rds.pg-dialect-numeric-timestamptz-jsonb-arrays-returning-recursive-cte.

=== Cause 4 of 5: COPY … FROM STDIN / TO STDOUT is refused as a syntax error, so bulk load and its data-error path do not exist ===
AWS: recorded behavior only. A PostgreSQL DB instance serves `COPY <table> FROM STDIN WITH (FORMAT text)` and `COPY (<query>) TO STDOUT WITH (FORMAT binary)` over the same connection; the binary stream begins with the signature PGCOPY\n\xff\r\n\x00. A malformed datum inside a text COPY aborts the single statement with SQLSTATE 22P02 (invalid_text_representation), an ErrorResponse whose context names the offending line and column (fixed form 'COPY <table>, line <n>, column <name>: "<value>"', with the table, line number, column name and value taken from the stream), and leaves zero rows — a COPY is one statement, so the valid prefix is never committed.
Emulator: the COPY statement itself is refused at parse time — psycopg.errors.SyntaxError: 'syntax error at or near "COPY"' — so no bulk load happens, no binary stream is produced, and the 22P02 data-error path with its line/column context is unreachable.
Why: every bulk-loading client (psql \copy, psycopg copy, ETL tooling) fails outright, and the all-or-nothing guarantee a loader depends on cannot be verified — a partial load would be indistinguishable from success.
Fix: the engine data plane's PostgreSQL COPY implementation (text and binary, FROM STDIN and TO STDOUT) and its per-row datum validation/error reporting.
Traces: rds.copy-from-stdin-bulk-load-rejects-a-malformed-row.

=== Cause 5 of 5: the isolation level a session asks for is not applied — a REPEATABLE READ session runs at READ COMMITTED ===
AWS: recorded behavior only. A PostgreSQL session that opens its transaction at REPEATABLE READ (psycopg isolation_level="REPEATABLE READ", i.e. BEGIN ISOLATION LEVEL REPEATABLE READ) reports `SHOW transaction_isolation` = 'repeatable read' and keeps reading its snapshot's value after another session commits an update to the same row, seeing the new value only after its own transaction ends.
Emulator: `SHOW transaction_isolation` answers 'read committed' in a session opened at REPEATABLE READ, so the requested isolation level is silently dropped (the statement is answered, the setting is not honoured).
Why: a client that relies on snapshot stability reads another transaction's committed write mid-transaction; read-modify-write logic, reporting queries and the whole MVCC contract behave as READ COMMITTED while the client believes it is at REPEATABLE READ — a silent correctness change, not an error.
Fix: the engine data plane's transaction manager — honour the per-transaction isolation level requested at BEGIN and report it through transaction_isolation.
Traces: rds.repeatable-read-transaction-sees-a-stable-snapshot.

## Evidence
Cause 1: psycopg.errors.SyntaxError: syntax error at or near "pg_postmaster_start_time" (SELECT pg_postmaster_start_time()); syntax error at or near "pg_is_in_recovery" (SELECT pg_is_in_recovery()); syntax error at or near "current_setting" (SELECT current_setting('max_connections')). AWS returns one row with the value and no error.
Cause 2: psycopg.errors.SyntaxError: syntax error at or near "ROLE" for CREATE ROLE "sim2real_iam_a" WITH LOGIN; syntax error at or near "'sim2real'" for the companion role statement that quotes the login name as a literal. AWS creates the role and the subsequent GRANT rds_iam succeeds.
Cause 3: psycopg.errors.SyntaxError: syntax error at or near ":" for SELECT 0.1::numeric + 0.2::numeric, (1::numeric / 3)::numeric(10,4). AWS returns (0.3, 0.3333); the only refusals in this dialect leg are SQLSTATE 22003 'numeric field overflow' and 22P02 'invalid input syntax for type json'.
Cause 4: psycopg.errors.SyntaxError: syntax error at or near "COPY" for COPY <table> FROM STDIN WITH (FORMAT text) (raised out of psycopg cursor.copy()). AWS loads the rows, emits a PGCOPY\n\xff\r\n\x00 binary stream for COPY … TO STDOUT WITH (FORMAT binary), and refuses a malformed numeric datum with SQLSTATE 22P02 and diag.context 'COPY <table>, line <n>, column amount: "not-a-number"', leaving 0 rows.
Cause 5: SHOW transaction_isolation returned [('read committed',)] in a session opened with isolation_level="REPEATABLE READ"; [('repeatable read',)] is required.

## Steps to Reproduce
Causes 1-4: CreateDBInstance(Engine=postgres, MasterUsername=<u>, MasterUserPassword=<p>, DBName=<db>), poll to available, open a psycopg session on DescribeDBInstances' Endpoint.Address/Port, then run each statement and read the error: (1) SELECT pg_postmaster_start_time(); SELECT pg_is_in_recovery(); SELECT current_setting('max_connections'); (2) CREATE USER r1 WITH LOGIN; CREATE ROLE "r2" WITH LOGIN; GRANT rds_iam TO "r2"; GRANT CONNECT ON DATABASE "<db>" TO "r2"; (3) SELECT 0.1::numeric + 0.2::numeric; SELECT '2024-01-31 00:00:00+00'::timestamptz + interval '1 month'; SELECT '{"a":1}'::jsonb @> '{"a":1}'; WITH RECURSIVE t(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM t WHERE n<10) SELECT sum(n) FROM t; (4) CREATE TABLE t(id int, amount numeric(12,4)); then cursor.copy("COPY t FROM STDIN WITH (FORMAT text)") feeding tab-separated rows, and COPY (SELECT * FROM t) TO STDOUT WITH (FORMAT binary).
Cause 5: on the same instance open a second session with autocommit=False and isolation_level="REPEATABLE READ", then run SHOW transaction_isolation; it answers 'read committed'. Commit an update to a row from a third session and re-read it inside the first transaction — the value changes.

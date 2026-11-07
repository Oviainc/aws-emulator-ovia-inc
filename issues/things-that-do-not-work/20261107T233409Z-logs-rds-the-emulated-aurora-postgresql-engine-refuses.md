Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__rds.obs-rds-3-aurora-postgresql-log-export-error-metric-filter-counts-only-error-lines
Issue-Key: logs__rds.obs-rds-3-aurora-postgresql-log-export-error-metric-filter-counts-only-error-lines|_raise_log#1|SyntaxError|cur.execute(f"DO $$ BEGIN RAISE LOG '{marker}'; END $$")
Issue-Commit: 8f24345936df6262f616e6a72be898a454a52720

# logs__rds: the emulated Aurora PostgreSQL engine refuses a PL/pgSQL anonymous DO block with SQLSTATE 42601, so no engine log line can be produced for the log-export metric filter

> _Note: this issue was identified against the code base at commit `8f24345936df6262f616e6a72be898a454a52720`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
ONE root cause (Cause 1 of 1).

This failing check is NOT marked ALREADY FILED or OWNED in the replay report. The OPEN issue 20261104T053605Z-logs-rds-the-aurora-postgresql-log-export-metric.md carries the same check id but for a different, earlier failure (nothing listened on the published endpoint, so the check timed out). The endpoint now accepts a psycopg session and SQL; the flow fails further on, at the first statement, which is a new root cause.

Cause 1 of 1 — the emulated Aurora PostgreSQL engine does not implement PL/pgSQL anonymous blocks (DO), so a statement that exists only to write an engine log line is rejected as a syntax error.
AWS: recorded behavior only (the SQL dialect is not part of the Smithy model; Smithy only gives the control plane: CreateDBCluster$EnableCloudwatchLogsExports with LogTypeList ['postgresql'] streams the engine log to /aws/rds/cluster/<cluster id>/postgresql). An Aurora PostgreSQL engine executes `DO $$ BEGIN RAISE LOG '<marker>'; END $$` successfully — the statement returns no rows, and the RAISE LOG writes one LOG line carrying the marker to the postgresql log, which is then exported to the CloudWatch Logs group where a metric filter counts only the ERROR lines. A statement the engine genuinely cannot parse is answered with SQLSTATE 42601 (syntax_error); that is NOT what a DO block is.
Emulator: the first such statement is refused — psycopg reports `psycopg.errors.SyntaxError: syntax error at or near "DO"` (SQLSTATE 42601) — so the session can be opened but the engine's SQL front end has no DO/PL/pgSQL support. The fixed text is PostgreSQL's "syntax error at or near "<token>""; the variable part is the offending token, here the leading DO keyword of a statement the real engine accepts. Because the statement never runs, no engine log line is produced, nothing is exported to the log group, and the metric-filter comparison (ERROR lines only) can never be made.
Why: a client that drives an RDS/Aurora PostgreSQL instance with ordinary PL/pgSQL — anonymous DO blocks, RAISE, procedural setup/migration scripts — gets a syntax error for valid SQL. Anything built on engine-produced log lines (log exports to CloudWatch Logs, log-based metric filters and the alarms over them) stays empty, which reads as "no errors occurred" instead of "no statement ever ran".
Fix: the emulated PostgreSQL engine's SQL front end in the rds crate's engine data plane — support PL/pgSQL anonymous DO blocks (and RAISE at its documented severities) and route RAISE LOG/ERROR output into the engine log that EnableCloudwatchLogsExports=['postgresql'] delivers.
Traces: logs__rds.obs-rds-3-aurora-postgresql-log-export-error-metric-filter-counts-only-error-lines.

## Evidence
Expected: `DO $$ BEGIN RAISE LOG '<marker>'; END $$` executes on the Aurora PostgreSQL endpoint without error and emits one LOG line containing the marker into the engine's postgresql log; that line is exported to /aws/rds/cluster/<cluster id>/postgresql, and a metric filter with the pattern ERROR counts only the ERROR lines (not the LOG line).
Actual: `psycopg.errors.SyntaxError: syntax error at or near "DO"` raised at the cur.execute() of the DO block, so the engine never writes any log line and the export/metric-filter assertions are never reached.

## Steps to Reproduce
1. rds.create_db_cluster(Engine='aurora-postgresql', EnableCloudwatchLogsExports=['postgresql']) plus a writer instance; wait for 'available'.
2. Connect to the published cluster endpoint with psycopg (this now succeeds).
3. cur.execute("DO $$ BEGIN RAISE LOG 'marker'; END $$") — the engine answers SQLSTATE 42601, syntax error at or near "DO".
4. logs.filter_log_events on /aws/rds/cluster/<id>/postgresql — nothing is delivered, so the ERROR metric filter never produces a datapoint.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed
Issue-Key: athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed|_run_sql#19|AssertionError|raise AssertionError(
Issue-Commit: 685d7dc1b36320cc963844050fe238af8df0451a

# Athena DDL never succeeds: a CREATE DATABASE submitted through StartQueryExecution settles FAILED with a SQLite parser diagnostic, so no Athena Query State Change scenario can run

> _Note: this issue was identified against the code base at commit `685d7dc1b36320cc963844050fe238af8df0451a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
Cause 1 of 1: a DDL statement (CREATE DATABASE) accepted by StartQueryExecution terminates in state FAILED, and the failure text is produced by the emulator's local SQL engine rather than by Athena.

AWS: Smithy (com.amazonaws.athena#AmazonAthena): StartQueryExecution takes QueryString (required, @length 1..262144) and returns QueryExecutionId; GetQueryExecution returns QueryExecution.Status.State from the QueryExecutionStatus.State enum (QUEUED | RUNNING | SUCCEEDED | FAILED | CANCELLED), plus QueryExecution.SubstatementType and, on failure only, QueryExecution.Status.AthenaError (ErrorCategory, ErrorType @range 0..9999, Retryable, ErrorMessage) and StateChangeReason. Athena executes DDL against the Glue data catalog: `CREATE DATABASE `<name>`` is a valid Athena statement, reaches State SUCCEEDED with SubstatementType CREATE_DATABASE, and carries NO AthenaError member. The statement's identifier (<name>) is the only request-derived part; everything else is fixed.

Emulator: every poll of GetQueryExecution for that submission reports State FAILED, and the failure text is a SQLite parser diagnostic of the fixed form 'near "DATABASE": syntax error in <submitted statement> at offset <n>' (observed: 'near "DATABASE": syntax error in CREATE DATABASE `sim2real_xsvc_db_9fe17be47505` at offset 7' — only the statement and the offset vary, the rest is the emulator's own SQL engine wording, which Athena never emits). The check asserts on reaching SUCCEEDED and fails with: AssertionError: statement reached FAILED, expected SUCCEEDED.

Why: a client that drives Athena with DDL (create database, create table, then query) never gets past the first statement — the catalog object AWS would have created does not exist, and the client is handed a syntax-error diagnosis for a statement that is valid Athena SQL. Downstream behaviour that depends on a succeeded query is unreachable: in this check, Athena publishes a 'Athena Query State Change' event (source aws.athena) whose detail must carry athenaError ONLY when the query failed, and that distinction can never be exercised because no query can succeed.

Fix: the athena crate's query-execution engine (DDL statement handling — CREATE DATABASE must be executed against the catalog rather than handed to the local SQL engine) and the GetQueryExecution serializer (State/SubstatementType, and AthenaError populated from Athena's taxonomy, absent on success).

Note: the open wire issue 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md describes the same root cause as its 'Cause 3 of 10' from the wire corpus, but it does not carry this behavioral check's replay key, so this check is not covered by it and must be re-replayed to confirm the fix.

Traces: athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed.

## Evidence
Behavioral suite replay against the emulator built from this checkout.

Check athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed [mismatch]:
  expected: StartQueryExecution of 'CREATE DATABASE `<db>`' polled via GetQueryExecution reaches QueryExecution.Status.State = SUCCEEDED (SubstatementType CREATE_DATABASE, no Status.AthenaError).
  actual:   State = FAILED, failure text 'near "DATABASE": syntax error in CREATE DATABASE `sim2real_xsvc_db_9fe17be47505` at offset 7'.
  runner output: AssertionError: statement reached FAILED, expected SUCCEEDED: 'CREATE DATABASE `sim2real_xsvc_db_9fe17be47505`' -- near "DATABASE": syntax error in CREATE DATABASE `sim2real_xsvc_db_9fe17be47505` at offset 7 (check_athena__events__sqs_query_state_change_event_carries_athenaerror_only_when_the_query_failed.py, line 75 in _run_sql, called from line 196).

The database name and the byte offset are per-run values; the requirement is that the statement SUCCEEDS and that no Athena-emitted failure text is a SQLite parser diagnostic.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from this checkout.
2. Replay check athena__events__sqs.query-state-change-event-carries-athenaerror-only-when-the-query-failed.
3. Manual repro: create an S3 bucket for results, then StartQueryExecution with QueryString 'CREATE DATABASE `probe_db`' and ResultConfiguration.OutputLocation set to s3://<bucket>/, and poll GetQueryExecution with the returned QueryExecutionId. Expect State to reach SUCCEEDED with SubstatementType CREATE_DATABASE and no Status.AthenaError; observe State FAILED with a 'near "DATABASE": syntax error ...' message instead.

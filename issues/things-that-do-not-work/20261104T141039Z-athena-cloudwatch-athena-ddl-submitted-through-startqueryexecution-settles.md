Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__cloudwatch.publish-cloudwatch-metrics-enabled-delivers-processedbytes-for-the-workgroup
Issue-Key: athena__cloudwatch.publish-cloudwatch-metrics-enabled-delivers-processedbytes-for-the-workgroup|_run_sql#12|AssertionError|raise AssertionError(
Issue-Commit: 732d86be3641565fdf04b49c6a1d1231e9cf6723

# athena__cloudwatch: Athena DDL submitted through StartQueryExecution settles FAILED with a SQLite parser diagnostic, so no AWS/Athena workgroup metric scenario can run

> _Note: this issue was identified against the code base at commit `732d86be3641565fdf04b49c6a1d1231e9cf6723`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
Cause 1 of 1: a DDL statement (CREATE DATABASE) accepted by StartQueryExecution never reaches SUCCEEDED — it terminates in state FAILED, and the failure text is produced by the emulator's local SQL engine rather than by Athena.

AWS: Smithy (com.amazonaws.athena#AmazonAthena): StartQueryExecution takes QueryString (required, @length 1..262144) plus optional QueryExecutionContext/ResultConfiguration/WorkGroup and returns QueryExecutionId (@length 1..128, @pattern ^\S+$); GetQueryExecution returns QueryExecution.Status.State drawn from the QueryExecutionStatus.State enum (QUEUED | RUNNING | SUCCEEDED | FAILED | CANCELLED), and populates Status.StateChangeReason / Status.AthenaError only for a failed or cancelled query. Athena runs DDL against the Glue data catalog through the Hive DDL dialect, where `CREATE DATABASE `<name>`` is a valid statement: it settles in State SUCCEEDED with SubstatementType CREATE_DATABASE, no StateChangeReason and no Status.AthenaError. The database identifier is the only request-derived part of the statement; the acceptance of the statement form is fixed.

Emulator: every poll of GetQueryExecution for that submission reports State FAILED with a StateChangeReason of the fixed form 'near "DATABASE": syntax error in <submitted statement> at offset <n>' — a SQLite parser diagnostic. Only the statement text and the byte offset vary in it; the rest is the emulator's own SQL engine wording, which Athena never emits (observed: 'near "DATABASE": syntax error in CREATE DATABASE `sim2real_xsvc_db_4127a8e4cfe6` at offset 7'). The check asserts the statement reaches SUCCEEDED and fails with: AssertionError: statement reached FAILED, expected SUCCEEDED.

Why: a client that drives Athena the way AWS documents it — CREATE DATABASE, then CREATE TABLE, then a SELECT — never gets past the first statement. The catalog object AWS would have created does not exist, and the client is handed a syntax-error diagnosis for a statement that is valid Athena SQL. Everything that depends on a succeeded query is unreachable; in this check that is the AWS/Athena workgroup telemetry (ProcessedBytes and EngineExecutionTime under the WorkGroup dimension, and the QueryState/QueryType/WorkGroup dimension set) that a workgroup with PublishCloudWatchMetricsEnabled must deliver to CloudWatch, which can never be exercised because no query can succeed.

Fix: the athena crate's query-execution engine (DDL statement handling — CREATE DATABASE must be executed against the catalog instead of being handed to the local SQL engine) and the GetQueryExecution serializer (State/SubstatementType, StateChangeReason and AthenaError populated from Athena's taxonomy and absent on success).

Note: the open issues 20261104T080454Z-athena-events-sqs-athena-ddl-never-succeeds-a-create-database-submitted.md, 20261104T083348Z-athena-s3-athena-refuses-create-database-and-aliased.md and 20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md describe this same root cause from other corpora/checks, but none of them carries this check's replay key, so this check is not covered by them and must be re-replayed to confirm the fix.

Traces: athena__cloudwatch.publish-cloudwatch-metrics-enabled-delivers-processedbytes-for-the-workgroup.

## Evidence
Behavioral suite replay against the emulator built from this checkout (521/924 passed).

Check athena__cloudwatch.publish-cloudwatch-metrics-enabled-delivers-processedbytes-for-the-workgroup [mismatch]:
  expected: StartQueryExecution of 'CREATE DATABASE `<db>`' polled through GetQueryExecution reaches QueryExecution.Status.State = SUCCEEDED, with no Status.StateChangeReason and no Status.AthenaError; the subsequent CREATE TABLE / SELECT then publish AWS/Athena ProcessedBytes and EngineExecutionTime for the workgroup.
  actual:   State = FAILED with StateChangeReason 'near "DATABASE": syntax error in CREATE DATABASE `sim2real_xsvc_db_4127a8e4cfe6` at offset 7'.
  runner output: AssertionError: statement reached FAILED, expected SUCCEEDED: 'CREATE DATABASE `sim2real_xsvc_db_4127a8e4cfe6`' -- near "DATABASE": syntax error in CREATE DATABASE `sim2real_xsvc_db_4127a8e4cfe6` at offset 7 (check_athena__cloudwatch_publish_cloudwatch_metrics_enabled_delivers_processedbytes_for_the_workgroup.py, line 78 in _run_sql, called from line 183).

The database name and the byte offset are per-run values; the requirement is that a Hive DDL CREATE DATABASE statement SUCCEEDS and that no Athena-emitted failure text is a SQLite parser diagnostic.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from this checkout.
2. Replay check athena__cloudwatch.publish-cloudwatch-metrics-enabled-delivers-processedbytes-for-the-workgroup.
3. Manual repro: create an S3 bucket for results and a workgroup with PublishCloudWatchMetricsEnabled=true, then StartQueryExecution with QueryString 'CREATE DATABASE `probe_db`', ResultConfiguration.OutputLocation s3://<bucket>/results/ and WorkGroup=<workgroup>; poll GetQueryExecution with the returned QueryExecutionId. Expect State to reach SUCCEEDED with no Status.StateChangeReason/Status.AthenaError; observe State FAILED with a 'near "DATABASE": syntax error ... at offset 7' message instead.

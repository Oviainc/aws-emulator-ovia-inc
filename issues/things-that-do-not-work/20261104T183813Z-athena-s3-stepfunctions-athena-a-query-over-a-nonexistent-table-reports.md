Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-missing-table-query-fails-as-task-failed
Issue-Key: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-missing-table-query-fails-as-task-failed|run#21|AssertionError|assert re.match(r"TABLE_NOT_FOUND\b", message) and table in message, (
Issue-Commit: 41f2e9525c7487ad1fe04cb3242a50ad61bb0d09

# athena: a query over a nonexistent table reports AthenaError.ErrorMessage 'no such table: <table>' instead of Athena's 'TABLE_NOT_FOUND: ... Table ... does not exist'

> _Note: this issue was identified against the code base at commit `41f2e9525c7487ad1fe04cb3242a50ad61bb0d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
1 net-new root cause. (The open issue 20261104T083348Z-athena-s3-stepfunctions-an-athena-startqueryexecution-sync-task.md lists this same check id, but its recorded replay key for it is a JSONDecodeError on json.loads of the States.TaskFailed Cause — the Cause is now valid JSON and the check gets past that point, so that key no longer matches and that issue does not cover the divergence below.)

Cause 1 of 1: the AthenaError text for a failed query naming a table that does not exist is the backing SQL engine's own message, not Athena's.
AWS: a DML statement that Athena accepts but that fails at run time because it names a nonexistent table ends with QueryExecution.Status.State = FAILED and QueryExecution.Status.AthenaError populated, where ErrorMessage is the engine's standardized error, which begins with the error code 'TABLE_NOT_FOUND' and names the table that could not be resolved — fixed text 'TABLE_NOT_FOUND: ' ... 'Table ' ... ' does not exist'; the variable parts are the statement's line:column position and the fully-qualified table identifier (catalog.database.table) taken verbatim from the request's QueryString. ErrorCategory is the USER category (2) for this class. Smithy (com.amazonaws.athena): QueryExecutionStatus.AthenaError -> AthenaError{ErrorCategory @range 1..3 ('1 - System', user/other for 2/3), ErrorType @range 0..9999, Retryable, ErrorMessage 'Contains a short description of the error that occurred'}; the model does not enumerate the message text, so the TABLE_NOT_FOUND code and the 'Table <name> does not exist' wording are AWS's documented/recorded behavior for this error class and are what the recording asserts.
Emulator: the same statement does fail (State FAILED) and does carry an AthenaError, but ErrorMessage is the raw message of the local SQL backend: 'no such table: <table>'. It has no 'TABLE_NOT_FOUND' error-code prefix and uses the backend's bare-identifier wording rather than Athena's. The identical value is also what the Step Functions athena:startQueryExecution.sync task puts in its States.TaskFailed Cause (cause.QueryExecution.Status.AthenaError.ErrorMessage), so both the Athena API and the workflow surface the wrong text.
Why: Athena's error codes are the documented contract clients branch on — a workflow Catch/Choice, a retry policy, or an operator dashboard that matches the leading 'TABLE_NOT_FOUND' code (as AWS's own error-reference table prescribes) never matches here and classifies a plain missing-table user error as an unknown/system failure. Clients also parse the table name out of Athena's wording; the emulator's message is a different shape.
Fix: the athena crate's query-execution engine — the layer that maps a failure from the SQL backend into QueryExecutionStatus.AthenaError must translate the backend error into Athena's error code + message (and set ErrorCategory/ErrorType for the class) rather than passing the backend string through. The sfn crate's optimized Athena integration then needs no change: it copies whatever Athena reports into the TaskFailed Cause.
Traces: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-missing-table-query-fails-as-task-failed.

## Evidence
Replay of the simparity-behavioral-aws-v1 suite against the emulator built from this checkout, service heading '### athena__s3__stepfunctions'.

[mismatch] athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-missing-table-query-fails-as-task-failed
  File "check_athena__s3__stepfunctions_xsvc_sfn_1_athena_sync_missing_table_query_fails_as_task_failed.py", line 67, in run
    assert re.match(r"TABLE_NOT_FOUND\b", message) and table in message, (
  AssertionError: the Cause's Athena error was 'no such table: sim2real_no_such_table_42495206b3b8'; expected TABLE_NOT_FOUND naming 'sim2real_no_such_table_42495206b3b8'

where message = json.loads(execution['cause'])['QueryExecution']['Status']['AthenaError']['ErrorMessage'] and the check next asserts that athena.get_query_execution(QueryExecutionId=qid)['QueryExecution']['Status']['AthenaError']['ErrorMessage'] equals that same value.

Expected (AWS): State 'FAILED' and an ErrorMessage matching ^TABLE_NOT_FOUND\b that names the unresolved table, e.g. "TABLE_NOT_FOUND: line 1:15: Table 'awsdatacatalog.<database>.<table>' does not exist" — fixed text 'TABLE_NOT_FOUND: ', 'Table ', ' does not exist'; variable parts the line:column and the table identifier from the submitted QueryString (the generated table name in this run, sim2real_no_such_table_42495206b3b8, is a per-run value, not part of the requirement).
Actual (emulator): "no such table: <table>" — no error code, different wording, served identically by GetQueryExecution and by the Step Functions States.TaskFailed Cause.

Everything else the check asserts up to that point held: the positive control succeeded, the bad run ended FAILED with error States.TaskFailed, and the Cause parsed as JSON with QueryExecution.Status.State == 'FAILED'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite and look at the '### athena__s3__stepfunctions' section; re-replay athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-missing-table-query-fails-as-task-failed.
3. Minimal manual repro: create a bucket and a workgroup whose ResultConfiguration.OutputLocation points into it; call athena StartQueryExecution with QueryString 'SELECT * FROM <a_table_that_does_not_exist>' in that workgroup; poll GetQueryExecution until Status.State is terminal.
4. Expect Status.State 'FAILED' with Status.AthenaError.ErrorMessage beginning 'TABLE_NOT_FOUND' and naming the table (and ErrorCategory 2, the USER category). Observe instead ErrorMessage 'no such table: <table>'.
5. Same divergence through Step Functions: run a state machine with a Task state Resource 'arn:aws:states:::athena:startQueryExecution.sync' over that statement and json-parse the execution's States.TaskFailed Cause — cause.QueryExecution.Status.AthenaError.ErrorMessage carries the same non-Athena text.
